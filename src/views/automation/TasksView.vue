<script setup lang="ts">
import { DropdownMenuContent, DropdownMenuItem, DropdownMenuPortal, DropdownMenuRoot, DropdownMenuTrigger } from 'reka-ui'
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { useRouter } from 'vue-router'
import type { TaskRow } from '../../api/types'
import ConfirmDialog from '../../components/ConfirmDialog.vue'
import UiButton from '../../components/UiButton.vue'
import { formatDateTime } from '../../lib/columns'
import { notify, pollReporter } from '../../lib/notify'
import { useConfirm } from '../../lib/useConfirm'
import { useAutomationStore } from '../../stores/automation'
import AutomationHeader from './AutomationHeader.vue'
import TaskDialog from './TaskDialog.vue'

const { t, locale } = useI18n()
const router = useRouter()
const automation = useAutomationStore()
const createOpen = ref(false)
const editOpen = ref(false)
const editingId = ref<number | null>(null)
function edit(task: TaskRow) {
  if (isRunning(task)) return
  editingId.value = task.id
  editOpen.value = true
}

type Status = 'running' | 'done' | 'error' | 'cancelled' | 'never'
/** Идёт здесь (`running`) или в другом процессе Ombra (последний пакет не закончен). */
const isRunning = (task: TaskRow) => task.running || (task.last !== null && task.last.finished_at === null)
/** Чужой пакет: идёт, но не в этом окне (`ombra flow` в другом процессе, `tasks-changed` оттуда не приходит). */
const isForeign = (task: TaskRow) => !task.running && task.last !== null && task.last.finished_at === null
const hasFailed = (task: TaskRow) => (task.last?.error ?? 0) + (task.last?.cancelled ?? 0) > 0
function status(task: TaskRow): Status {
  const last = task.last
  if (isRunning(task)) return 'running'
  if (!last) return 'never'
  if (last.error > 0) return 'error'
  if (last.cancelled > 0) return 'cancelled'
  return 'done'
}

const guard = (action: () => Promise<unknown>) => () => action().catch((e: unknown) => notify.error(e))
const del = useConfirm(async (task: TaskRow) => {
  await automation.deleteTask(task.id)
  notify.info(t('automation.tasks.deleted'))
})

// Пока у какой-то задачи идёт чужой пакет, список опрашивается; опрос стоит, когда таких нет и при уходе с экрана.
const POLL_MS = 3000
const pollErrors = pollReporter()
let poll: ReturnType<typeof setInterval> | undefined
const stopPoll = () => {
  clearInterval(poll)
  poll = undefined
}
watch(
  computed(() => automation.tasks.some(isForeign)),
  (foreign) => {
    stopPoll()
    if (foreign) poll = setInterval(() => automation.refreshTasks().then(pollErrors.ok, pollErrors.fail), POLL_MS)
  },
  { immediate: true },
)
onBeforeUnmount(stopPoll)

onMounted(() => {
  // Пока экран открыт, `tasks-changed` обновляет список через хранилище; флаги `running` берутся из ответа ядра.
  automation.refreshTasks().catch((e: unknown) => notify.error(e))
  automation.refreshFlows().catch((e: unknown) => notify.error(e))
})
</script>

<template>
  <main class="page list-page automation-tasks">
    <AutomationHeader :subtitle="t('automation.tasks.count', automation.tasks.length)">
      <template #actions>
        <UiButton variant="primary" icon="plus" @click="createOpen = true">{{ t('automation.tasks.create') }}</UiButton>
      </template>
    </AutomationHeader>
    <section class="card table" :aria-label="t('automation.tabTasks')">
      <div class="grid-table">
        <div class="row header">
          <div>{{ t('automation.tasks.columns.name') }}</div>
          <div>{{ t('automation.tasks.columns.flow') }}</div>
          <div>{{ t('automation.tasks.columns.profiles') }}</div>
          <div>{{ t('automation.tasks.columns.threads') }}</div>
          <div>{{ t('automation.tasks.columns.last') }}</div>
          <div>{{ t('automation.tasks.columns.started') }}</div>
          <div />
        </div>
        <div v-if="automation.tasks.length === 0" class="empty">
          <template v-if="automation.tasksLoaded">
            <div class="section-title">{{ t('automation.tasks.empty') }}</div>
            <div class="hint">{{ t('automation.tasks.emptyHint') }}</div>
          </template>
        </div>
        <div v-else class="body">
          <div v-for="task in automation.tasks" :key="task.id" class="row">
            <div class="cell">
              <div class="main">{{ task.name }}</div>
              <div class="sub">{{ t(task.distribution === 'queue' ? 'automation.taskDialog.sharedQueue' : 'automation.taskDialog.perProfile') }}</div>
            </div>
            <div class="cell"><div class="main">{{ task.flow_name || '—' }}</div></div>
            <div>{{ task.profiles }}</div>
            <div>{{ task.threads }}</div>
            <div class="cell">
              <div :class="['main', 'st', status(task)]">{{ t(`automation.tasks.status.${status(task)}`) }}</div>
              <div v-if="task.last" class="sub">
                {{ task.queue ? t('automation.tasks.queueProgress', task.queue) : t('automation.tasks.counts', { done: task.last.done, error: task.last.error, total: task.last.total }) }}
              </div>
            </div>
            <div class="muted">{{ task.last ? formatDateTime(task.last.started_at, locale) : '—' }}</div>
            <div class="actions">
              <UiButton size="sm" :disabled="isRunning(task)" :title="isRunning(task) ? t('automation.tasks.editRunning') : undefined" @click="edit(task)">{{ t('common.edit') }}</UiButton>
              <DropdownMenuRoot>
                <DropdownMenuTrigger as-child>
                  <UiButton size="icon" variant="ghost" icon="more" :aria-label="t('common.moreActions')" />
                </DropdownMenuTrigger>
                <DropdownMenuPortal>
                  <DropdownMenuContent align="end" :side-offset="4" class="popover">
                    <DropdownMenuItem class="menu-item" :disabled="isRunning(task)" @select="edit(task)">{{ t('common.edit') }}</DropdownMenuItem>
                    <DropdownMenuItem class="menu-item" :disabled="isRunning(task) || (task.queue !== null && task.queue.done >= task.queue.total)" @select="guard(() => automation.startTask(task.id))()">{{ t(task.distribution === 'queue' ? 'automation.tasks.resumeQueue' : 'automation.tasks.run') }}</DropdownMenuItem>
                    <DropdownMenuItem class="menu-item" :disabled="isRunning(task) || !hasFailed(task)" @select="guard(() => automation.retryFailed(task.id))()">{{ t('automation.tasks.retry') }}</DropdownMenuItem>
                    <DropdownMenuItem v-if="task.running" class="menu-item" @select="guard(() => automation.stopTask(task.id))()">{{ t('automation.tasks.stop') }}</DropdownMenuItem>
                    <DropdownMenuItem v-if="task.running" class="menu-item" @select="guard(() => automation.cancelWaiting(task.id))()">{{ t('automation.tasks.cancelWaiting') }}</DropdownMenuItem>
                    <DropdownMenuItem class="menu-item" @select="router.push(`/automation/journal?task=${task.id}`)">{{ t('automation.tasks.journal') }}</DropdownMenuItem>
                    <DropdownMenuItem class="menu-item danger" :disabled="isRunning(task)" @select="del.ask(task)">{{ t('common.delete') }}</DropdownMenuItem>
                  </DropdownMenuContent>
                </DropdownMenuPortal>
              </DropdownMenuRoot>
            </div>
          </div>
        </div>
      </div>
    </section>
    <TaskDialog v-model:open="createOpen" :flow-id="null" :profile-ids="null" @created="automation.scheduleTasks()" />
    <TaskDialog v-model:open="editOpen" :task-id="editingId" :flow-id="null" :profile-ids="null" @updated="automation.scheduleTasks()" />
    <ConfirmDialog
      v-model:open="del.open.value"
      :title="t('automation.tasks.deleteTitle', { name: del.shown.value?.name ?? '' })"
      :description="t('automation.tasks.deleteText')"
      :confirm-label="t('common.delete')"
      :busy="del.busy.value"
      danger
      @confirm="del.confirm"
    />
  </main>
</template>

<style scoped>
.automation-tasks {
  overflow: hidden;
}

.table {
  display: flex;
  flex-direction: column;
  min-width: 0;
  min-height: 0;
  overflow: hidden;
}

.row {
  grid-template-columns: minmax(0, 1fr) minmax(0, 1fr) 90px 80px 200px 140px 144px;
}

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

.st.running {
  color: var(--info-text);
}

.st.done {
  color: var(--ok-text);
}

.st.error {
  color: var(--danger-text);
}

.st.cancelled,
.st.never {
  color: var(--text-2);
}

.actions {
  display: flex;
  align-items: center;
  gap: 4px;
  justify-content: flex-end;
}
</style>
