<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { useRoute } from 'vue-router'
import { core } from '../../api/core'
import type { BatchSummary, JournalEntry, RunRow } from '../../api/types'
import UiButton from '../../components/UiButton.vue'
import { formatDateTime } from '../../lib/columns'
import { clock } from '../../lib/format'
import { notify, pollReporter } from '../../lib/notify'
import { useAutomationStore } from '../../stores/automation'
import AutomationHeader from './AutomationHeader.vue'

const { t, locale } = useI18n()
const route = useRoute()
const automation = useAutomationStore()

const taskId = ref<number | null>(Number(route.query.task) || null)
const batches = ref<BatchSummary[]>([])
const batchId = ref<number | null>(null)
const runs = ref<RunRow[]>([])
const openRun = ref<number | null>(null)
const lines = ref<JournalEntry[]>([])

const task = computed(() => automation.tasks.find((row) => row.id === taskId.value))
const batch = computed(() => batches.value.find((b) => b.id === batchId.value))
/** Ключ живого обновления: меняется, когда ядро прислало новые счётчики выбранной задачи. */
const liveKey = computed(() => JSON.stringify([task.value?.running, task.value?.last]))

const pad = (n: number) => String(n).padStart(2, '0')
function duration(b: BatchSummary) {
  if (b.finished_at === null) return '—'
  const s = Math.max(0, b.finished_at - b.started_at)
  return `${pad(Math.floor(s / 3600))}:${pad(Math.floor(s / 60) % 60)}:${pad(s % 60)}`
}
const stamp = (seconds: number | null) => (seconds === null ? '—' : formatDateTime(seconds, locale.value))

// Ответ принимается, только если выбор не менялся с запроса: медленный ответ прежней задачи не затрёт новую.
const stale = (task: number | null, batch: number | null, run: number | null) =>
  task !== taskId.value || batch !== batchId.value || run !== openRun.value
async function loadRuns() {
  const [task, batch] = [taskId.value, batchId.value]
  const list = batch === null ? [] : await core.listRuns(batch)
  if (task === taskId.value && batch === batchId.value) runs.value = list
}
async function loadBatches() {
  const task = taskId.value
  if (task === null) return
  const list = await core.listBatches(task)
  if (task !== taskId.value) return
  batches.value = [...list].sort((a, b) => b.id - a.id)
  if (!batches.value.some((b) => b.id === batchId.value)) batchId.value = batches.value[0]?.id ?? null
  await loadRuns()
  if (task === taskId.value && openRun.value !== null) await loadLines()
}
async function loadLines() {
  const [task, batch, run] = [taskId.value, batchId.value, openRun.value]
  const list = run === null ? [] : await core.runJournal(run)
  if (!stale(task, batch, run)) lines.value = list
}
const guard = (work: () => Promise<unknown>) => () => work().catch((e) => notify.error(e))

function pickTask(id: number) {
  taskId.value = id
  batchId.value = null
  openRun.value = null
  batches.value = []
  runs.value = []
  lines.value = []
  guard(loadBatches)()
}
function pickBatch(id: number) {
  batchId.value = id
  openRun.value = null
  guard(loadRuns)()
}
function toggleRun(id: number) {
  openRun.value = openRun.value === id ? null : id
  lines.value = []
  guard(loadLines)()
}

// Адрес меняется при переходе из «Задач» в уже открытый экран: выбор следует за `?task`.
watch(
  () => Number(route.query.task) || null,
  (id) => {
    if (id !== null && id !== taskId.value) pickTask(id)
  },
)

// Раскрытый идущий запуск дополняется раз в 2 с (ход запуска окну событием не приходит).
const LIVE_MS = 2000
const liveErrors = pollReporter()
let busy = false
let live: ReturnType<typeof setInterval> | undefined
const stopLive = () => {
  clearInterval(live)
  live = undefined
}
/** Новый запрос не начинается, пока не вернулся прежний. */
async function tick() {
  if (busy) return
  busy = true
  try {
    await loadRuns()
    await loadLines()
    liveErrors.ok()
  } catch (e) {
    liveErrors.fail(e)
  } finally {
    busy = false
  }
}
const openRunning = computed(() => runs.value.find((r) => r.id === openRun.value)?.status === 'running')
watch(
  openRunning,
  (running) => {
    stopLive()
    if (running) live = setInterval(tick, LIVE_MS)
  },
  { immediate: true },
)
onBeforeUnmount(stopLive)

onMounted(async () => {
  guard(() => automation.refreshTasks())()
  guard(loadBatches)()
})
watch(liveKey, guard(loadBatches))
</script>

<template>
  <main class="page list-page automation-journal">
    <AutomationHeader />
    <div class="layout">
      <section class="card tasks" :aria-label="t('automation.journal.tasks')">
        <button
          v-for="row in automation.tasks" :key="row.id" type="button" data-test="task-item"
          :class="['task', { active: row.id === taskId }]" :aria-current="row.id === taskId ? 'true' : undefined"
          @click="pickTask(row.id)"
        >{{ row.name }}</button>
      </section>
      <section class="detail">
        <div v-if="taskId === null || !task" class="card empty"><div class="section-title">{{ t('automation.journal.pick') }}</div></div>
        <template v-else>
          <div class="card">
            <div v-if="batches.length === 0" class="empty"><div class="section-title">{{ t('automation.journal.noBatches') }}</div></div>
            <div
              v-for="b in batches" :key="b.id" data-test="batch-row" role="button" tabindex="0"
              :class="['batch', { active: b.id === batchId }]" @click="pickBatch(b.id)" @keydown.enter="pickBatch(b.id)"
            >
              <span>{{ stamp(b.started_at) }}</span>
              <span class="muted">{{ stamp(b.finished_at) }}</span>
              <span class="muted">{{ duration(b) }}</span>
              <span class="mono" :title="t('automation.journal.countsTitle')">{{ b.total }} · {{ b.done }} · {{ b.error }}</span>
              <UiButton v-if="task.running && b.finished_at === null" size="sm" @click.stop="guard(() => automation.cancelWaiting(task!.id))()" @keydown.enter.stop>{{ t('automation.tasks.cancelWaiting') }}</UiButton>
              <span v-else />
            </div>
          </div>
          <div v-if="batch" class="card">
            <div class="runs-head">
              <div class="section-title">{{ t('automation.journal.runs') }}</div>
              <UiButton size="sm" icon="folder" @click="guard(() => core.openBatchFiles(batch!.id))()">{{ t('automation.journal.openFiles') }}</UiButton>
            </div>
            <div class="grid-table">
              <div class="row header">
                <div>{{ t(task.distribution === 'queue' ? 'automation.journal.queueItem' : 'automation.journal.columns.num') }}</div>
                <div>{{ t('automation.journal.columns.profile') }}</div>
                <div>{{ t('automation.journal.columns.status') }}</div>
                <div>{{ t('automation.journal.columns.error') }}</div>
                <div>{{ t('automation.journal.columns.time') }}</div>
              </div>
              <div class="body">
                <template v-for="r in runs" :key="r.id">
                  <div data-test="run-row" class="row run" role="button" tabindex="0" @click="toggleRun(r.id)" @keydown.enter="toggleRun(r.id)">
                    <div>{{ r.item_index == null ? r.serial : r.item_index + 1 }}</div>
                    <div class="cell"><div class="main">{{ r.name || (r.item_index != null ? t('automation.journal.waitingProfile') : '') }}</div><div v-if="r.item_index != null && r.serial" class="sub">№ {{ r.serial }}</div></div>
                    <div :class="['st', r.status]">{{ t(`automation.journal.run.${r.status}`) }}</div>
                    <div class="cell"><div class="main err">{{ r.error ?? '' }}</div></div>
                    <div class="muted">{{ stamp(r.started_at) }}</div>
                  </div>
                  <div v-if="openRun === r.id" class="lines">
                    <div v-if="lines.length === 0" class="hint">{{ t('automation.journal.emptyLog') }}</div>
                    <div v-for="(line, i) in lines" :key="i" data-test="journal-line" class="line">
                      <span class="mono">{{ clock(line.ts_ms) }}</span>
                      <span>{{ line.node ?? '' }}</span>
                      <span>{{ line.op ?? '' }}</span>
                      <span>{{ t(`automation.journal.status.${line.status}`) }}</span>
                      <span class="mono">{{ line.ms }} {{ t('automation.journal.ms') }}</span>
                      <span class="err">{{ line.msg ?? '' }}</span>
                    </div>
                  </div>
                </template>
              </div>
            </div>
          </div>
        </template>
      </section>
    </div>
  </main>
</template>

<style scoped>
.layout { display: flex; gap: 24px; flex-grow: 1; min-height: 0; }
.tasks { display: flex; flex-direction: column; flex: 0 0 240px; align-self: flex-start; padding: 8px; }
.task { padding: 10px 12px; border: 0; border-radius: 8px; background: transparent; text-align: left; cursor: pointer; }
.task:hover { background: var(--muted-bg); }
.task.active { background: var(--accent-soft); color: var(--accent-ink); font-weight: 500; }
.detail { display: flex; flex-direction: column; gap: 16px; flex-grow: 1; min-width: 0; }
.empty { padding: 48px; text-align: center; }
.batch { display: grid; grid-template-columns: 1fr 1fr 100px 120px 190px; gap: 12px; align-items: center; padding: 10px 16px; border-bottom: 1px solid var(--row-border); cursor: pointer; }
.batch.active { background: var(--accent-soft); }
.runs-head { display: flex; align-items: center; justify-content: space-between; padding: 12px 16px 0; }
.row { grid-template-columns: 48px minmax(0, 1fr) 130px minmax(0, 1.5fr) 150px; }
.run { cursor: pointer; min-height: 44px; }
.st.done { color: var(--ok-text); }
.st.error, .err { color: var(--danger-text); }
.lines { padding: 8px 16px 12px 64px; background: var(--muted-bg); }
.line { display: grid; grid-template-columns: 80px 80px 120px 70px 90px minmax(0, 1fr); gap: 10px; font-size: 13px; line-height: 22px; }
</style>
