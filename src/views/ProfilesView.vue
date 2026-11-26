<script setup lang="ts">
import { DropdownMenuContent, DropdownMenuItem, DropdownMenuPortal, DropdownMenuRoot, DropdownMenuTrigger } from 'reka-ui'
import { computed, onBeforeUnmount, onMounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'
import { useRouter } from 'vue-router'
import { core } from '../api/core'
import { toCoreError } from '../api/errors'
import type { BulkReport, ProfileRow } from '../api/types'
import ConfirmDialog from '../components/ConfirmDialog.vue'
import SyncStartDialog from '../components/SyncStartDialog.vue'
import UiButton from '../components/UiButton.vue'
import AppBanners from '../components/AppBanners.vue'
import { type BulkKind, showBulkReport } from '../lib/bulkReport'
import { isLive, loadHidden } from '../lib/columns'
import { notify } from '../lib/notify'
import { useConfirm } from '../lib/useConfirm'
import { useExtensionsStore } from '../stores/extensions'
import { useGroupsStore } from '../stores/groups'
import { useProfilesStore } from '../stores/profiles'
import { useSyncStore } from '../stores/sync'
import BulkBar from './profiles/BulkBar.vue'
import ProfileGroupsSidebar from './profiles/ProfileGroupsSidebar.vue'
import ProfilesPager from './profiles/ProfilesPager.vue'
import ProfilesTable from './profiles/ProfilesTable.vue'
import ProfilesToolbar from './profiles/ProfilesToolbar.vue'
import ProfileTagFilters from './profiles/ProfileTagFilters.vue'
import BulkCreateDialog from './profiles/BulkCreateDialog.vue'
import BulkEditDialog from './profiles/BulkEditDialog.vue'
import TaskDialog from './automation/TaskDialog.vue'
import ArchiveExportDialog from './profiles/ArchiveExportDialog.vue'
import CookieExportDialog from './profiles/CookieExportDialog.vue'
import HeaderMenu from './profiles/HeaderMenu.vue'
import ImportMenu from './profiles/ImportMenu.vue'

type PendingDelete = { kind: 'one'; row: ProfileRow } | { kind: 'many'; ids: string[] }

const { t } = useI18n()
const router = useRouter()
const profiles = useProfilesStore()
const groups = useGroupsStore()
const extensions = useExtensionsStore()
const sync = useSyncStore()

function editProfile(row: ProfileRow) {
  void router.push(`/profiles/${row.id}`)
}
const activeGroup = computed(() => typeof profiles.filters.group === 'number' ? groups.byId(profiles.filters.group)?.name : null)
const hidden = loadHidden()

/** Идёт массовая операция: кнопки панели неактивны. */
const working = ref(false)
const filtered = computed(() => {
  const filters = profiles.filters
  return (
    filters.search.trim() !== '' ||
    filters.label !== 'any' ||
    filters.group !== 'all' ||
    profiles.hasTagFilters ||
    profiles.only !== null
  )
})
const emptyGroup = computed(() => profiles.filters.group !== 'all' && profiles.filters.search.trim() === ''
  && profiles.filters.label === 'any' && !profiles.hasTagFilters && profiles.only === null)

/** Выбранные id в порядке списка (`selectedIds` хранилища) — в том же порядке идут строки отчёта. */
function selectedIds(): string[] {
  return profiles.selectedIds
}

/** Ошибка обновления — отдельным уведомлением: операция, после которой обновляем, уже закончилась. */
async function reload() {
  try {
    await Promise.all([profiles.refresh(), groups.refresh()])
  } catch (e) {
    notify.error(e)
  }
}

onMounted(async () => {
  // Экран открыт: события супервизора и `profiles-changed` снова перечитывают список.
  profiles.setActive(true)
  extensions.refresh().catch((e: unknown) => notify.error(e))
  try {
    await Promise.all([profiles.refresh(), groups.loaded ? undefined : groups.refresh()])
  } catch (e) {
    notify.error(e)
  }
})
// Экран закрыт: события не перечитывают список, запланированное обновление и поиск отменяются.
onBeforeUnmount(() => profiles.setActive(false))

async function runBulk(kind: BulkKind, ids: string[], run: (ids: string[]) => Promise<BulkReport>) {
  if (ids.length === 0) return
  working.value = true
  try {
    showBulkReport(kind, await run(ids))
  } catch (e) {
    notify.error(e)
  } finally {
    working.value = false
  }
  await reload()
}

const startSelected = () => runBulk('start', selectedIds(), (ids) => core.startProfiles(ids))
const stopSelected = () => runBulk('stop', selectedIds(), (ids) => core.stopProfiles(ids))
const cloneProfiles = (ids: string[]) => runBulk('clone', ids, (list) => core.cloneProfiles(list))
const setExtCategory = (ids: string[], category: number) =>
  runBulk('extSet', ids, (list) => core.setProfilesExtCategory(list, category))
const moveProfiles = (ids: string[], group: number | null) =>
  runBulk('move', ids, (list) => core.moveProfiles(list, group))

const syncOpen = ref(false)
const syncProfiles = ref<ProfileRow[]>([])
function syncSelected() {
  if (sync.state.active) {
    void router.push('/sync')
    return
  }
  syncProfiles.value = profiles.visibleRows.filter((row) => profiles.selected.has(row.id))
  if (syncProfiles.value.length >= 2) syncOpen.value = true
}

async function toggleRun(row: ProfileRow) {
  try {
    if (isLive(row) || row.status.state === 'waiting') await profiles.stop(row.id)
    else await profiles.start(row.id)
  } catch (e) {
    // «Стоп» во время запуска отменяет его: ожидающий start отвечает NotRunning, это не ошибка.
    if (toCoreError(e).code !== 'NotRunning') notify.error(e)
  }
}

async function clearCache(row: ProfileRow) {
  try {
    await core.clearCache(row.id)
    notify.info(t('profiles.cacheCleared', { name: row.name }))
  } catch (e) {
    notify.error(e)
  }
}

/** Снять временный фильтр «эти профили» из отчёта. */
function clearOnly() {
  profiles.only = null
  profiles.setPage(0)
}

function resetFilters() {
  profiles.openGroup('all')
  clearOnly()
}

const createOpen = ref(false)
const editOpen = ref(false)
const editIds = ref<string[]>([])
const taskOpen = ref(false)
const taskIds = ref<string[]>([])
const exportOpen = ref(false)
const exportIds = ref<string[]>([])
// Строку архива не обнуляем при закрытии — заголовок не меняется во время анимации.
const archiveOpen = ref(false)
const archiveRow = ref<ProfileRow | null>(null)
const exportNames = computed(() => {
  const ids = new Set(exportIds.value)
  return Object.fromEntries(profiles.rows.filter((row) => ids.has(row.id)).map((row) => [row.id, row.name]))
})

function openEdit(ids: string[]) {
  if (ids.length === 0) return
  editIds.value = ids
  editOpen.value = true
}

function openTask(ids: string[]) {
  if (ids.length === 0) return
  taskIds.value = ids
  taskOpen.value = true
}

function openExport(ids: string[]) {
  if (ids.length === 0) return
  exportIds.value = ids
  exportOpen.value = true
}

/** Диалог закрывается после удаления в любом случае; обновление списка — отдельно. */
const del = useConfirm(async (target: PendingDelete) => {
  if (target.kind === 'one') {
    await core.deleteProfile(target.row.id)
    notify.info(t('profiles.deleted'))
  } else {
    showBulkReport('delete', await core.deleteProfiles(target.ids))
  }
})
async function confirmDelete() {
  await del.confirm()
  await reload()
}
const deleteTitle = computed(() => {
  const target = del.pending.value
  if (!target) return ''
  return target.kind === 'one'
    ? t('profiles.deleteTitle', { name: target.row.name })
    : t('profiles.bulkDeleteTitle', target.ids.length)
})
const deleteText = computed(() =>
  del.pending.value?.kind === 'many' ? t('profiles.bulkDeleteText') : t('profiles.deleteText'),
)

</script>

<template>
  <div class="profiles-workspace">
    <ProfileGroupsSidebar />
    <main class="page list-page profiles">
      <header class="head">
        <div class="title-group">
          <h1 class="page-title">{{ t('profiles.title') }} <span class="total-count">{{ profiles.total }}</span></h1>
          <div class="hint">{{ t('profiles.running', { n: profiles.running }) }}</div>
        </div>
        <div class="head-actions">
          <ProfilesToolbar :hidden="hidden" />
          <ImportMenu @imported="reload" />
          <HeaderMenu />
          <div class="split">
            <UiButton variant="primary" icon="plus" @click="router.push('/profiles/new')">{{ t('profiles.new') }}</UiButton>
            <DropdownMenuRoot>
              <DropdownMenuTrigger as-child>
                <UiButton
                  variant="primary"
                  size="icon"
                  icon="chevron"
                  class="split-arrow"
                  :aria-label="t('bulk.create.more')"
                  :title="t('bulk.create.more')"
                />
              </DropdownMenuTrigger>
              <DropdownMenuPortal>
                <DropdownMenuContent align="end" :side-offset="4" class="popover">
                  <DropdownMenuItem class="menu-item" @select="createOpen = true">{{ t('bulk.create.menuItem') }}</DropdownMenuItem>
                </DropdownMenuContent>
              </DropdownMenuPortal>
            </DropdownMenuRoot>
          </div>
        </div>
      </header>

      <AppBanners security-notice />
      <ProfileTagFilters />
      <div class="filter-summary">
        <span class="scope-title">{{ activeGroup ?? (profiles.filters.group === 'ungrouped' ? t('common.ungrouped') : t('profiles.groupSidebar.all')) }}</span>
        <span class="hint">{{ t('profiles.count', profiles.visibleRows.length) }}</span>
      </div>

      <div v-if="profiles.only" class="only">
        <span>{{ t('profiles.only.text', { n: profiles.visibleRows.length }) }}</span>
        <button type="button" class="link" @click="clearOnly">{{ t('profiles.only.clear') }}</button>
      </div>

      <section class="table card">
        <BulkBar
          :count="profiles.selectedCount"
          :groups="groups.groups"
          :sets="extensions.categories"
          :risky-sets="extensions.riskyIds"
          :busy="working"
          @clear="profiles.clearSelection()"
          @start="startSelected"
          @stop="stopSelected"
          @sync="syncSelected"
          @run-flow="openTask(selectedIds())"
          @move="moveProfiles(selectedIds(), $event)"
          @ext-set="setExtCategory(selectedIds(), $event)"
          @clone="cloneProfiles(selectedIds())"
          @export="openExport(selectedIds())"
          @edit="openEdit(selectedIds())"
          @remove="del.ask({ kind: 'many', ids: selectedIds() })"
        />
        <ProfilesTable
          :rows="profiles.pageRows"
          :hidden="hidden"
          :groups="groups.groups"
          :sync-main="sync.state.active ? sync.state.main : null"
          @toggle-run="toggleRun"
          @edit="editProfile"
          @clone="(row) => cloneProfiles([row.id])"
          @move="(row, group) => moveProfiles([row.id], group)"
          @clear-cache="clearCache"
          @export-cookies="(row) => openExport([row.id])"
          @export-archive="(row) => ((archiveRow = row), (archiveOpen = true))"
          @remove="(row) => del.ask({ kind: 'one', row })"
        >
          <template #empty>
            <template v-if="!profiles.loading">
              <template v-if="emptyGroup">
                <div class="section-title">{{ t('profiles.groupSidebar.emptyProfiles') }}</div>
                <div class="hint">{{ t('profiles.groupSidebar.emptyProfilesHint') }}</div>
                <UiButton variant="primary" icon="plus" @click="router.push('/profiles/new')">{{ t('profiles.new') }}</UiButton>
              </template>
              <template v-else-if="filtered">
                <div class="section-title">{{ t('profiles.nothingFound') }}</div>
                <div class="hint">{{ t('profiles.nothingFoundHint') }}</div>
                <UiButton @click="resetFilters">{{ t('profiles.resetFilters') }}</UiButton>
              </template>
              <template v-else>
                <div class="section-title">{{ t('profiles.empty') }}</div>
                <div class="hint">{{ t('profiles.emptyHint') }}</div>
                <UiButton variant="primary" icon="plus" @click="router.push('/profiles/new')">{{ t('profiles.new') }}</UiButton>
              </template>
            </template>
          </template>
        </ProfilesTable>
        <ProfilesPager
          v-if="profiles.visibleRows.length > 0"
          :page="profiles.page"
          :page-count="profiles.pageCount"
          :from="profiles.rangeFrom"
          :to="profiles.rangeTo"
          :total="profiles.visibleRows.length"
          :page-size="profiles.pageSize"
          @page="profiles.setPage($event)"
          @page-size="profiles.setPageSize($event)"
        />
      </section>

      <SyncStartDialog v-model:open="syncOpen" :profiles="syncProfiles" @started="router.push('/sync')" />
      <BulkCreateDialog v-model:open="createOpen" />
      <BulkEditDialog v-model:open="editOpen" :ids="editIds" />
      <TaskDialog v-model:open="taskOpen" :flow-id="null" :profile-ids="taskIds" />
      <ArchiveExportDialog v-model:open="archiveOpen" :row="archiveRow" />
      <CookieExportDialog v-model:open="exportOpen" :ids="exportIds" :names="exportNames" />
      <ConfirmDialog
        v-model:open="del.open.value"
        :title="deleteTitle"
        :description="deleteText"
        :confirm-label="t('common.delete')"
        :busy="del.busy.value"
        danger
        @confirm="confirmDelete"
      />
    </main>
  </div>
</template>

<style scoped>
.profiles-workspace {
  position: relative;
  display: flex;
  flex: 1;
  min-width: 0;
  min-height: 0;
}

.profiles {
  overflow: hidden;
  padding-bottom: 0;
  gap: 16px;
}

.profiles > .head {
  flex-shrink: 0;
  align-items: flex-start;
  gap: 16px;
  flex-wrap: wrap;
}

.profiles > .head .title-group {
  flex-direction: column;
  gap: 4px;
}

.total-count {
  margin-left: 8px;
  color: var(--text-2);
  font-size: 20px;
  font-weight: 400;
}

.filter-summary {
  flex-shrink: 0;
}

.filter-summary {
  display: flex;
  align-items: center;
  gap: 16px;
}

.scope-title { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-size: 13px; font-weight: 500; }

.head-actions {
  flex-wrap: wrap;
}

.head-actions {
  display: flex;
  align-items: center;
  gap: 12px;
  min-width: 0;
  max-width: 100%;
  margin-left: auto;
}

.split {
  display: flex;
}

.split > :first-child {
  border-top-right-radius: 0;
  border-bottom-right-radius: 0;
}

.split .split-arrow {
  width: 40px;
  height: var(--control-h);
  border-left-color: color-mix(in srgb, var(--text) 25%, var(--accent));
  border-top-left-radius: 0;
  border-bottom-left-radius: 0;
}

.only {
  display: flex;
  align-items: center;
  gap: 12px;
  padding: 10px 16px;
  border-radius: var(--radius-control);
  background: var(--accent-soft);
  font-size: 13px;
}

.table {
  display: flex;
  flex-direction: column;
  flex-grow: 1;
  min-height: 0;
  overflow: hidden;
  margin-inline: -28px;
  border: 0;
  border-top: 1px solid var(--border);
  border-radius: 0;
}

.table {
  --row-pad: 28px;
}
</style>
