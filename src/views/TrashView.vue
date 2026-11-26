<script setup lang="ts">
import { computed, onMounted, onUnmounted, reactive, ref } from 'vue'
import { useI18n } from 'vue-i18n'
import { core, events } from '../api/core'
import type { BulkReport, ProfileRow } from '../api/types'
import ConfirmDialog from '../components/ConfirmDialog.vue'
import UiButton from '../components/UiButton.vue'
import { type BulkKind, showBulkReport } from '../lib/bulkReport'
import { notify } from '../lib/notify'
import { createRefresher } from '../lib/refresher'
import { useConfirm } from '../lib/useConfirm'
import { useGroupsStore } from '../stores/groups'
import { type PageSize, PAGE_SIZES } from '../stores/profiles'
import ProfilesPager from './profiles/ProfilesPager.vue'
import TrashTable from './trash/TrashTable.vue'

type PendingPurge = { kind: 'one'; row: ProfileRow } | { kind: 'many'; ids: string[] } | { kind: 'all' }

const { t } = useI18n()
const groups = useGroupsStore()
const rows = ref<ProfileRow[]>([])
const loaded = ref(false)
const working = ref(false)
const selected = reactive(new Set<string>())
const page = ref(0)
const pageSize = ref<PageSize>(PAGE_SIZES[0])

const pageCount = computed(() => Math.max(1, Math.ceil(rows.value.length / pageSize.value)))
const pageRows = computed(() => rows.value.slice(page.value * pageSize.value, (page.value + 1) * pageSize.value))
const rangeFrom = computed(() => (pageRows.value.length === 0 ? 0 : page.value * pageSize.value + 1))
const rangeTo = computed(() => page.value * pageSize.value + pageRows.value.length)

/** Выделение и страница не переживают перечитывание за пределы списка. */
function apply(list: ProfileRow[]) {
  rows.value = list
  loaded.value = true
  const present = new Set(list.map((row) => row.id))
  for (const id of [...selected]) if (!present.has(id)) selected.delete(id)
  if (page.value >= pageCount.value) page.value = pageCount.value - 1
}

const refresher = createRefresher(() => core.listTrash(), apply)

/** Имена берутся из списка до операции: после перечитывания удалённых строк уже нет. */
async function run(kind: BulkKind, ids: string[], action: () => Promise<BulkReport>) {
  const names = new Map(rows.value.map((row) => [row.id, row.name]))
  working.value = true
  try {
    showBulkReport(kind, await action(), (id) => names.get(id) ?? null)
    for (const id of ids) selected.delete(id)
  } catch (e) {
    notify.error(e)
  } finally {
    working.value = false
  }
  await refresher.refresh().catch(notify.error)
  // Счётчики групп и фильтр по тегам «Профилей» читаются только при первом открытии.
  await groups.refresh().catch(notify.error)
}

const restore = (ids: string[]) => run('restore', ids, () => core.restoreProfiles(ids))

const purge = useConfirm(async (target: PendingPurge) => {
  if (target.kind === 'one') await run('purge', [target.row.id], () => core.purgeProfiles([target.row.id]))
  else if (target.kind === 'many') await run('purge', target.ids, () => core.purgeProfiles(target.ids))
  else await run('purge', rows.value.map((row) => row.id), () => core.emptyTrash())
})
const purgeTitle = computed(() => {
  const target = purge.shown.value
  if (!target) return ''
  if (target.kind === 'one') return t('trash.purgeTitle', { name: target.row.name })
  if (target.kind === 'many') return t('trash.bulkPurgeTitle', target.ids.length)
  return t('trash.emptyTitle')
})
const purgeLabel = computed(() => (purge.shown.value?.kind === 'all' ? t('trash.emptyTrash') : t('trash.purge')))

const selectedIds = () => rows.value.filter((row) => selected.has(row.id)).map((row) => row.id)

function togglePage(value: boolean) {
  for (const row of pageRows.value) {
    if (value) selected.add(row.id)
    else selected.delete(row.id)
  }
}

function toggle(id: string) {
  if (selected.has(id)) selected.delete(id)
  else selected.add(id)
}

function setPageSize(size: PageSize) {
  pageSize.value = size
  page.value = 0
}

let unlisten: (() => void) | null = null
let gone = false
onMounted(async () => {
  refresher.refresh().catch(notify.error).finally(() => (loaded.value = true))
  // Local API меняет корзину, пока экран открыт.
  try {
    const stop = await events.onProfilesChanged(() => refresher.schedule())
    if (gone) stop()
    else unlisten = stop
  } catch (e) {
    notify.error(e)
  }
})
onUnmounted(() => {
  gone = true
  unlisten?.()
  refresher.cancel()
})
</script>

<template>
  <main class="page list-page trash-page">
    <header class="head">
      <div class="title-group">
        <h1 class="page-title">{{ t('trash.title') }}</h1>
        <div class="hint">{{ t('trash.count', rows.length) }}</div>
      </div>
      <div class="head-actions">
        <UiButton variant="danger" icon="trash" :disabled="rows.length === 0 || working" @click="purge.ask({ kind: 'all' })">
          {{ t('trash.emptyTrash') }}
        </UiButton>
      </div>
    </header>

    <section class="table card" :aria-label="t('trash.title')">
      <div v-if="selected.size > 0" class="bulk" role="toolbar" :aria-label="t('trash.selected', { n: selected.size })">
        <span class="selected-count">{{ t('trash.selected', { n: selected.size }) }}</span>
        <UiButton size="sm" :busy="working" @click="restore(selectedIds())">{{ t('trash.restore') }}</UiButton>
        <span class="spacer" />
        <UiButton size="sm" variant="ghost" @click="selected.clear()">{{ t('trash.clear') }}</UiButton>
        <UiButton size="sm" variant="danger" :busy="working" @click="purge.ask({ kind: 'many', ids: selectedIds() })">
          {{ t('trash.purge') }}
        </UiButton>
      </div>
      <TrashTable
        :rows="pageRows"
        :selected="selected"
        :busy="working"
        @toggle="toggle"
        @toggle-page="togglePage"
        @restore="(row) => restore([row.id])"
        @purge="(row) => purge.ask({ kind: 'one', row })"
      >
        <template #empty>
          <template v-if="loaded">
            <div class="section-title">{{ t('trash.empty') }}</div>
            <div class="hint">{{ t('trash.emptyHint') }}</div>
          </template>
        </template>
      </TrashTable>
      <ProfilesPager
        v-if="rows.length > 0"
        :page="page"
        :page-count="pageCount"
        :from="rangeFrom"
        :to="rangeTo"
        :total="rows.length"
        :page-size="pageSize"
        @page="page = $event"
        @page-size="setPageSize"
      />
    </section>

    <ConfirmDialog
      v-model:open="purge.open.value"
      :title="purgeTitle"
      :description="t('trash.purgeText')"
      :confirm-label="purgeLabel"
      :busy="purge.busy.value"
      danger
      @confirm="purge.confirm"
    />
  </main>
</template>

<style scoped>
.trash-page {
  overflow: hidden;
}

.head-actions {
  display: flex;
  gap: 10px;
}

.table {
  display: flex;
  flex-direction: column;
  min-height: 0;
  max-height: 100%;
  overflow: hidden;
}

.bulk {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 8px 16px;
  border-bottom: 1px solid var(--row-border);
}

.selected-count {
  font-weight: 600;
}

.spacer {
  flex-grow: 1;
}
</style>
