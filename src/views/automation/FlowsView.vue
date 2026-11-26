<script setup lang="ts">
import { DropdownMenuContent, DropdownMenuItem, DropdownMenuPortal, DropdownMenuRoot, DropdownMenuTrigger } from 'reka-ui'
import { computed, onMounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'
import { useRouter } from 'vue-router'
import { pickFlowFile, pickSaveFlow } from '../../api/dialogs'
import type { FlowRow } from '../../api/types'
import AppIcon from '../../components/AppIcon.vue'
import ConfirmDialog from '../../components/ConfirmDialog.vue'
import UiButton from '../../components/UiButton.vue'
import { formatDateTime } from '../../lib/columns'
import { notify } from '../../lib/notify'
import { useConfirm } from '../../lib/useConfirm'
import { useAutomationStore } from '../../stores/automation'
import AutomationHeader from './AutomationHeader.vue'
import FlowDefaultsDialog from './FlowDefaultsDialog.vue'
import TaskDialog from './TaskDialog.vue'

const { t, locale } = useI18n()
const router = useRouter()
const automation = useAutomationStore()
const search = ref('')

const rows = computed(() => {
  const q = search.value.trim().toLowerCase()
  return q ? automation.flows.filter((f) => `${f.name}\n${f.group}`.toLowerCase().includes(q)) : automation.flows
})

const runOpen = ref(false)
const runFlowId = ref<number | null>(null)
const defaultsOpen = ref(false)
const defaultsFlow = ref<FlowRow | null>(null)
const open = (row: FlowRow) => router.push(`/automation/flows/${row.id}`)
function run(row: FlowRow) {
  runFlowId.value = row.id
  runOpen.value = true
}
function defaults(row: FlowRow) {
  defaultsFlow.value = row
  defaultsOpen.value = true
}

async function importFile() {
  const path = await pickFlowFile()
  if (!path) return
  const flow = await automation.importFlow(path)
  notify.info(t('automation.flows.imported', { name: flow.name }))
}
async function exportFlow(row: FlowRow) {
  const path = await pickSaveFlow(`${row.name}.json`)
  if (!path) return
  await automation.exportFlow(row.id, path)
  notify.info(t('automation.flows.exported'))
}
async function copy(row: FlowRow) {
  await automation.copyFlow(row.id)
  notify.info(t('automation.flows.copied'))
}
const del = useConfirm(async (row: FlowRow) => {
  await automation.deleteFlow(row.id)
  notify.info(t('automation.flows.deleted'))
})
/** Ошибка действия меню — уведомлением. */
const guard = (action: () => Promise<void>) => () => action().catch((e: unknown) => notify.error(e))

onMounted(() => {
  automation.refreshFlows().catch((e: unknown) => notify.error(e))
})
</script>

<template>
  <main class="page list-page automation-flows">
    <AutomationHeader :subtitle="t('automation.flows.count', automation.flows.length)">
      <template #actions>
        <UiButton @click="router.push('/automation/library')">{{ t('automation.library.title') }}</UiButton>
        <UiButton icon="download" @click="guard(importFile)()">{{ t('automation.flows.import') }}</UiButton>
        <UiButton variant="primary" icon="plus" @click="router.push('/automation/flows/new')">{{ t('automation.flows.create') }}</UiButton>
      </template>
    </AutomationHeader>
    <label class="search input">
      <AppIcon name="search" />
      <span class="sr-only">{{ t('automation.flows.searchLabel') }}</span>
      <input v-model="search" type="search" :placeholder="t('automation.flows.search')" />
    </label>
    <section class="card table" :aria-label="t('automation.tabFlows')">
      <div class="grid-table">
        <div class="row header">
          <div>{{ t('automation.flows.columns.name') }}</div>
          <div>{{ t('automation.flows.columns.group') }}</div>
          <div>{{ t('automation.flows.columns.variables') }}</div>
          <div>{{ t('automation.flows.columns.updated') }}</div>
          <div class="right">{{ t('automation.flows.columns.actions') }}</div>
        </div>
        <div v-if="rows.length === 0" class="empty">
          <template v-if="automation.flowsLoaded">
            <div class="section-title">{{ t('automation.flows.empty') }}</div>
            <div class="hint">{{ t('automation.flows.emptyHint') }}</div>
          </template>
        </div>
        <div v-else class="body">
          <div v-for="flow in rows" :key="flow.id" class="row" @dblclick="open(flow)">
            <div class="cell">
              <div class="name-line">
                <div class="main" :title="flow.name">{{ flow.name }}</div>
                <span v-if="flow.has_errors" class="badge bad">{{ t('automation.flows.hasErrors') }}</span>
              </div>
              <div v-if="flow.remark" class="sub" :title="flow.remark">{{ flow.remark }}</div>
            </div>
            <div :class="['cell', { muted: !flow.group }]">{{ flow.group || '—' }}</div>
            <div>{{ flow.variables.length }}</div>
            <div class="muted">{{ formatDateTime(flow.updated_at, locale) }}</div>
            <!-- Действия строки и их меню не передают двойной щелчок строке. -->
            <div class="actions" @dblclick.stop>
              <UiButton size="sm" variant="ghost" icon="play" class="quick-action quick-run" :disabled="flow.has_errors"
                :title="t(flow.has_errors ? 'automation.editor.fixErrorsFirst' : 'automation.flows.run')"
                @click="run(flow)">{{ t('automation.flows.run') }}</UiButton>
              <UiButton size="sm" variant="ghost" icon="edit" class="quick-action quick-edit"
                :title="t('automation.flows.edit')" @click="open(flow)">{{ t('automation.flows.edit') }}</UiButton>
              <DropdownMenuRoot>
                <DropdownMenuTrigger as-child>
                  <UiButton size="icon" variant="ghost" icon="more" :aria-label="t('common.moreActions')" />
                </DropdownMenuTrigger>
                <DropdownMenuPortal>
                  <DropdownMenuContent align="end" :side-offset="4" class="popover">
                    <DropdownMenuItem class="menu-item" @select="open(flow)">{{ t('automation.flows.edit') }}</DropdownMenuItem>
                    <DropdownMenuItem class="menu-item" :disabled="flow.has_errors" @select="run(flow)">{{ t('automation.flows.run') }}</DropdownMenuItem>
                    <DropdownMenuItem class="menu-item" @select="defaults(flow)">{{ t('automation.flows.defaults') }}</DropdownMenuItem>
                    <DropdownMenuItem class="menu-item" @select="guard(() => exportFlow(flow))()">{{ t('automation.flows.export') }}</DropdownMenuItem>
                    <DropdownMenuItem class="menu-item" @select="guard(() => copy(flow))()">{{ t('automation.flows.copy') }}</DropdownMenuItem>
                    <DropdownMenuItem class="menu-item danger" @select="del.ask(flow)">{{ t('common.delete') }}</DropdownMenuItem>
                  </DropdownMenuContent>
                </DropdownMenuPortal>
              </DropdownMenuRoot>
            </div>
          </div>
        </div>
      </div>
    </section>
    <TaskDialog v-model:open="runOpen" :flow-id="runFlowId" :profile-ids="null" @created="router.push('/automation/tasks')" />
    <FlowDefaultsDialog v-model:open="defaultsOpen" :flow="defaultsFlow" />
    <ConfirmDialog
      v-model:open="del.open.value"
      :title="t('automation.flows.deleteTitle', { name: del.shown.value?.name ?? '' })"
      :description="t('automation.flows.deleteText')"
      :confirm-label="t('common.delete')"
      :busy="del.busy.value"
      danger
      @confirm="del.confirm"
    />
  </main>
</template>

<style scoped>
.automation-flows {
  overflow: hidden;
}

.search {
  display: flex;
  align-items: center;
  gap: 8px;
  width: 320px;
  flex-shrink: 0;
  color: var(--text-2);
  cursor: text;
}

.search input {
  flex-grow: 1;
  border: 0;
  outline: 0;
  background: transparent;
}

.table {
  display: flex;
  flex-direction: column;
  min-width: 0;
  min-height: 0;
  overflow: hidden;
}

.row {
  grid-template-columns: minmax(0, 1fr) 150px 80px 150px 280px;
}

.name-line { display: flex; align-items: center; gap: 8px; min-width: 0; }
.name-line .main { min-width: 0; }
.name-line .badge { flex-shrink: 0; }

.body {
  flex-grow: 1;
  min-height: 0;
  overflow: auto;
}

.body .row {
  min-height: 56px;
  border-bottom: 1px solid var(--row-border);
}

.body .row:last-child {
  border-bottom: 0;
}

.badge.bad {
  background: var(--danger-bg);
  color: var(--danger-text);
}

.actions {
  display: flex;
  align-items: center;
  gap: 4px;
  justify-content: flex-end;
}

.actions .quick-action { gap: 6px; padding: 0 8px; }
.quick-run { color: var(--accent-ink); }
</style>
