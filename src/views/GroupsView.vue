<script setup lang="ts">
import { DropdownMenuContent, DropdownMenuItem, DropdownMenuPortal, DropdownMenuRoot, DropdownMenuTrigger } from 'reka-ui'
import { computed, onMounted, reactive, ref } from 'vue'
import { useI18n } from 'vue-i18n'
import { useRouter } from 'vue-router'
import { core } from '../api/core'
import type { GroupRecord, ProfileRow } from '../api/types'
import AppIcon from '../components/AppIcon.vue'
import ConfirmDialog from '../components/ConfirmDialog.vue'
import UiButton from '../components/UiButton.vue'
import { showBulkReport } from '../lib/bulkReport'
import { countStatuses } from '../lib/groups'
import { notify } from '../lib/notify'
import { useConfirm } from '../lib/useConfirm'
import { useExtensionsStore } from '../stores/extensions'
import { useGroupsStore } from '../stores/groups'
import { type GroupFilter, useProfilesStore } from '../stores/profiles'
import GroupExtSetDialog from './groups/GroupExtSetDialog.vue'
import GroupPanel from './groups/GroupPanel.vue'
import StateSummary from './groups/StateSummary.vue'

/** Ключ строки «Без группы» в наборе запускаемых групп. */
const UNGROUPED = 'ungrouped'

const { t } = useI18n()
const router = useRouter()
const groups = useGroupsStore()
const profiles = useProfilesStore()

/** Открытая боковая панель: группа по id или новая группа. */
const panel = ref<{ id: number } | 'new' | null>(null)
const panelGroup = computed(() =>
  panel.value !== null && panel.value !== 'new' ? (groups.byId(panel.value.id) ?? null) : null,
)
const panelOpen = computed(() => panel.value === 'new' || panelGroup.value !== null)
const panelKey = computed(() => (panelGroup.value ? `group-${panelGroup.value.id}` : 'new'))

/** Профили по группам за один проход по полному списку; `null` — «Без группы». */
const rowsByGroup = computed(() => {
  const map = new Map<number | null, ProfileRow[]>()
  for (const row of groups.rows) {
    const key = row.group?.id ?? null
    const list = map.get(key)
    if (list) list.push(row)
    else map.set(key, [row])
  }
  return map
})
const rowsOf = (group: number | null) => rowsByGroup.value.get(group) ?? []
const countsOf = (group: number | null) => countStatuses(rowsOf(group))

const starting = reactive(new Set<number | typeof UNGROUPED>())

/** «Запустить все»: ядро берёт только неактивные профили группы; итог — общий отчёт. */
async function startAll(group: number | null) {
  const key = group ?? UNGROUPED
  starting.add(key)
  try {
    showBulkReport('start', await core.startGroup(group))
  } catch (e) {
    notify.error(e)
  } finally {
    starting.delete(key)
  }
}

async function toggleQuickAccess(group: GroupRecord) {
  try {
    await groups.update(group.id, {
      name: group.name,
      color: group.color,
      note: group.note,
      quick_access: !group.quick_access,
    })
  } catch (e) {
    notify.error(e)
  }
}

function openInList(group: GroupFilter) {
  profiles.openGroup(group)
  void router.push('/profiles')
}

/** Диалог набора расширений: группа или `null` — «Без группы». */
const extOpen = ref(false)
const extGroup = ref<GroupRecord | null>(null)
const extCount = computed(() => (extGroup.value ? extGroup.value.profile_count : groups.ungrouped))
function openExtSet(group: GroupRecord | null) {
  extGroup.value = group
  extOpen.value = true
}

function onSaved(group: GroupRecord) {
  panel.value = { id: group.id }
}

const del = useConfirm(async (group: GroupRecord) => {
  await groups.remove(group.id)
  notify.info(t('groups.deleted'))
  if (panelGroup.value?.id === group.id) panel.value = null
})

onMounted(() => {
  groups.refresh().catch((e) => notify.error(e))
  // Раздел «Расширения» в панели: наборы и их состав меняются на экране «Расширения».
  useExtensionsStore().refresh().catch(notify.error)
})
</script>

<template>
  <main class="page list-page groups-page">
    <header class="head">
      <div class="title-group">
        <h1 class="page-title">{{ t('groups.title') }}</h1>
        <div class="hint">
          {{ t('groups.groupsCount', groups.groups.length) }} · {{ t('groups.profilesCount', groups.rows.length) }}
        </div>
      </div>
      <UiButton variant="primary" icon="plus" @click="panel = 'new'">{{ t('groups.new') }}</UiButton>
    </header>
    <p class="note"><AppIcon name="info" />{{ t('groups.info') }}</p>

    <div class="layout">
      <section class="table card grid-table" :aria-label="t('groups.title')">
        <div class="row header">
          <div>{{ t('groups.columns.color') }}</div>
          <div>{{ t('groups.columns.name') }}</div>
          <div>{{ t('groups.columns.profiles') }}</div>
          <div>{{ t('groups.columns.state') }}</div>
          <div>{{ t('groups.columns.quickAccess') }}</div>
          <div class="right">{{ t('groups.columns.action') }}</div>
        </div>

        <div class="body">
          <div
            v-for="group in groups.groups"
            :key="group.id"
            :class="['row', 'item', { selected: panelGroup?.id === group.id }]"
            @click="panel = { id: group.id }"
          >
            <div><span class="dot" :style="{ background: group.color }" /></div>
            <div class="cell">
              <button type="button" class="name" @click.stop="panel = { id: group.id }">{{ group.name }}</button>
              <div v-if="group.note" class="sub">{{ group.note }}</div>
            </div>
            <div class="mono count">{{ group.profile_count }}</div>
            <div class="state"><StateSummary :counts="countsOf(group.id)" /></div>
            <div @click.stop>
              <button
                type="button"
                :class="['star', { on: group.quick_access }]"
                :aria-pressed="group.quick_access"
                :aria-label="t('groups.quickAccessFor', { name: group.name })"
                :title="t('groups.columns.quickAccess')"
                @click="toggleQuickAccess(group)"
              >
                <AppIcon :name="group.quick_access ? 'starFilled' : 'star'" :size="20" />
              </button>
            </div>
            <div class="actions" @click.stop>
              <UiButton size="sm" variant="soft" class="start-all" :loading="starting.has(group.id)" @click="startAll(group.id)">
                {{ t('groups.startAll') }}
              </UiButton>
              <DropdownMenuRoot>
                <DropdownMenuTrigger as-child>
                  <UiButton size="icon" variant="ghost" icon="more" :aria-label="t('common.moreActions')" />
                </DropdownMenuTrigger>
                <DropdownMenuPortal>
                  <DropdownMenuContent align="end" :side-offset="4" class="popover">
                    <DropdownMenuItem class="menu-item" @select="panel = { id: group.id }">
                      {{ t('common.edit') }}
                    </DropdownMenuItem>
                    <DropdownMenuItem class="menu-item" @select="openInList(group.id)">
                      {{ t('groups.openInList') }}
                    </DropdownMenuItem>
                    <DropdownMenuItem class="menu-item" @select="openExtSet(group)">
                      {{ t('groups.extSet') }}
                    </DropdownMenuItem>
                    <DropdownMenuItem class="menu-item danger" @select="del.ask(group)">
                      {{ t('groups.delete') }}
                    </DropdownMenuItem>
                  </DropdownMenuContent>
                </DropdownMenuPortal>
              </DropdownMenuRoot>
            </div>
          </div>

          <div class="row item system">
            <div><span class="dot none" /></div>
            <div class="cell">
              <div class="name static">{{ t('common.ungrouped') }}</div>
              <div class="sub">{{ t('groups.systemNote') }}</div>
            </div>
            <div class="mono count">{{ groups.ungrouped }}</div>
            <div class="state"><StateSummary :counts="countsOf(null)" /></div>
            <div />
            <div class="actions">
              <UiButton size="sm" variant="soft" class="start-all" :loading="starting.has(UNGROUPED)" @click="startAll(null)">
                {{ t('groups.startAll') }}
              </UiButton>
              <DropdownMenuRoot>
                <DropdownMenuTrigger as-child>
                  <UiButton size="icon" variant="ghost" icon="more" :aria-label="t('common.moreActions')" />
                </DropdownMenuTrigger>
                <DropdownMenuPortal>
                  <DropdownMenuContent align="end" :side-offset="4" class="popover">
                    <DropdownMenuItem class="menu-item" @select="openInList('ungrouped')">
                      {{ t('groups.openInList') }}
                    </DropdownMenuItem>
                    <DropdownMenuItem class="menu-item" @select="openExtSet(null)">
                      {{ t('groups.extSet') }}
                    </DropdownMenuItem>
                  </DropdownMenuContent>
                </DropdownMenuPortal>
              </DropdownMenuRoot>
            </div>
          </div>
        </div>
      </section>

      <GroupPanel
        v-if="panelOpen"
        :key="panelKey"
        :group="panelGroup"
        :rows="panelGroup ? rowsOf(panelGroup.id) : []"
        @close="panel = null"
        @saved="onSaved"
        @request-delete="del.ask($event)"
      />
    </div>

    <GroupExtSetDialog v-model:open="extOpen" :group="extGroup" :count="extCount" />
    <ConfirmDialog
      v-model:open="del.open.value"
      :title="t('groups.deleteTitle', { name: del.shown.value?.name ?? '' })"
      :description="t('groups.deleteText')"
      :confirm-label="t('groups.delete')"
      :busy="del.busy.value"
      danger
      @confirm="del.confirm"
    />
  </main>
</template>

<style scoped>
.groups-page {
  overflow: hidden;
}

.layout {
  display: flex;
  gap: 16px;
  flex-grow: 1;
  min-height: 0;
}

.table {
  display: flex;
  flex-direction: column;
  flex-grow: 1;
  align-self: flex-start;
  min-width: 0;
  max-height: 100%;
  overflow: hidden;
}

.row {
  grid-template-columns: 32px minmax(0, 1fr) 72px 150px 110px 164px;
}

.body {
  overflow: auto;
}

.row.item {
  min-height: 64px;
  border-bottom: 1px solid var(--row-border);
  cursor: pointer;
}

.row.item:last-child {
  border-bottom: 0;
}

.row.selected {
  background: var(--accent-soft);
}

.row.system {
  cursor: default;
}

.dot {
  display: block;
  width: 16px;
  height: 16px;
  border-radius: 50%;
}

/* «Без группы»: кольцо пунктиром вместо цвета. */
.dot.none {
  border: 1.5px dashed var(--switch-off);
}

.name {
  padding: 0;
  border: 0;
  background: none;
  color: var(--text);
  font-size: 14px;
  font-weight: 500;
  text-align: left;
  cursor: pointer;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.name:focus-visible {
  border-radius: 4px;
  outline: 2px solid var(--focus);
  outline-offset: 2px;
}

.row.selected .name {
  font-weight: 600;
}

.name.static {
  color: var(--text-2);
  cursor: default;
}

.count,
.state {
  font-size: 13px;
}

.state {
  min-width: 0;
}

.star {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 36px;
  height: 36px;
  padding: 0;
  border: 0;
  border-radius: 8px;
  background: none;
  color: var(--switch-off);
  cursor: pointer;
}

/* Включена: заливка акцентом, контур `accentInk` (холст «Группы»). */
.star.on {
  color: var(--accent-ink);
}

.star.on .icon.filled {
  fill: var(--accent);
  stroke: currentColor;
  stroke-width: 1.6;
}

.star:hover {
  background: var(--muted-bg);
}

.star:focus-visible {
  outline: 2px solid var(--focus);
  outline-offset: 2px;
}

.actions {
  display: flex;
  justify-content: flex-end;
  gap: 6px;
}

.row .start-all {
  width: 122px;
  padding: 0;
  font-weight: 600;
}

/* В выбранной строке кнопка (`accentSoft`) слилась бы с фоном: белая, с рамкой. */
.row.selected .start-all {
  border-color: var(--bulk-border);
  background: var(--surface);
}
</style>
