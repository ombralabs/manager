<script setup lang="ts">
import { computed, nextTick, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import type { JournalEntry } from '../../../api/types'
import UiButton from '../../../components/UiButton.vue'
import { inBrowserBody, levelOf, opLabel, pathTo } from '../../../lib/flowEdit'
import { clock } from '../../../lib/format'
import { notify } from '../../../lib/notify'
import { KEEP, useFlowDebugStore } from '../../../stores/flowDebug'
import { useFlowEditorStore } from '../../../stores/flowEditor'

const { t, te } = useI18n()
const debug = useFlowDebugStore()
const editor = useFlowEditorStore()

const serial = computed({
  get: () => debug.serial ?? '',
  set: (value: string | number) => {
    const n = Number(value)
    debug.serial = value !== '' && Number.isInteger(n) && n > 0 ? n : null
  },
})
const canRun = computed(() => !debug.running && debug.serial !== null && !debug.missing && editor.report.errors.length === 0)
/** Узел для «С этого узла»: единственный выбранный, не Start. */
const fromNode = computed(() => {
  const id = editor.selected
  const node = id && editor.doc ? levelOf(editor.doc, editor.path).nodes.find((n) => n.id === id) : undefined
  return node && node.op !== 'start' ? node.id : null
})
/** Тело New Browser идёт в браузере другого профиля — с его узла отладка не запускается (ядро тоже откажет). */
const fromBody = computed(() => !!fromNode.value && !!editor.doc && inBrowserBody(editor.doc, [...editor.path, fromNode.value]))

async function run(from: string[] | null) {
  try {
    await debug.start(from)
  } catch (e) {
    notify.error(e)
  }
}
async function stop() {
  try {
    await debug.stop()
  } catch (e) {
    notify.error(e)
  }
}

const ops = computed(() => new Map(editor.catalog.ops.map((op) => [op.id, op])))
const label = (e: JournalEntry) => e.desc || (e.op ? opLabel(e.op, ops.value.get(e.op)?.adspower, te, t) : '')
/** Строка узла процесса выбирает его на холсте; у подпроцесса свои id — его строки не выбирают ничего. */
function show(e: JournalEntry) {
  const path = e.node && !e.depth && editor.doc ? pathTo(editor.doc.nodes, e.node) : null
  if (!e.node || !path) return
  editor.path = path
  editor.selected = e.node
}

const log = ref<HTMLElement>()
// По последней строке, не по длине: у полного журнала (последние 500) длина больше не меняется.
watch(() => debug.entries.at(-1), async () => {
  await nextTick()
  if (log.value) log.value.scrollTop = log.value.scrollHeight
})
</script>

<template>
  <section v-if="debug.open" class="debug-panel" :aria-label="t('automation.editor.debug')">
    <header class="bar">
      <label class="serial">
        {{ t('automation.editor.debugSerial') }}
        <input v-model="serial" class="input mono" inputmode="numeric" autocomplete="off" :aria-invalid="debug.missing ? true : undefined" />
      </label>
      <span v-if="debug.missing" class="hint bad">{{ t('automation.editor.debugNoProfile') }}</span>
      <span v-else-if="debug.profileName" class="hint name" data-test="debug-profile">{{ debug.profileName }}</span>
      <div class="spacer" />
      <span v-if="debug.running" class="hint">{{ t('automation.editor.debugRunning') }}</span>
      <span v-else-if="debug.status" :class="['hint', { bad: debug.status === 'error' }]">
        {{ t(`automation.editor.debugStatus_${debug.status}`) }}<template v-if="debug.error">: {{ debug.error }}</template>
      </span>
      <UiButton size="sm" variant="primary" :disabled="!canRun" @click="run(null)">{{ t('automation.editor.debugFromStart') }}</UiButton>
      <UiButton size="sm" :disabled="!canRun || !fromNode || fromBody" :title="fromBody ? t('automation.editor.debugFromBody') : undefined"
        @click="run([...editor.path, fromNode!])">{{ t('automation.editor.debugFromNode') }}</UiButton>
      <UiButton size="sm" :disabled="!debug.running" @click="stop">{{ t('automation.editor.debugStop') }}</UiButton>
    </header>
    <p v-if="debug.cut" class="cut" data-test="debug-cut">{{ t('automation.editor.debugCut', { n: KEEP, file: `logs/flows/${debug.runId}.jsonl` }) }}</p>
    <div ref="log" class="log" role="log">
      <p v-if="debug.entries.length === 0" class="empty">{{ t('automation.editor.debugEmpty') }}</p>
      <button v-for="(e, i) in debug.entries" :key="i" type="button" :class="['row', e.status, { sub: e.depth }]" data-test="debug-row"
        :disabled="!!e.depth" :title="e.depth ? t('automation.editor.debugSubflow') : undefined"
        :style="e.depth ? { paddingLeft: `${12 + 20 * e.depth}px` } : undefined" @click="show(e)">
        <span class="time mono">{{ clock(e.ts_ms) }}</span>
        <span class="status">{{ t(`automation.journal.status.${e.status}`) }}</span>
        <span class="name">{{ label(e) }}</span>
        <span class="msg">{{ e.msg }}</span>
        <span class="ms">{{ e.ms }} {{ t('automation.editor.debugMs') }}</span>
      </button>
    </div>
  </section>
</template>

<style scoped>
.debug-panel { display: flex; flex-direction: column; flex-shrink: 0; height: 240px; margin-top: 12px; border: 1px solid var(--border); border-radius: var(--radius-card); background: var(--surface); overflow: hidden; }
.bar { display: flex; flex-wrap: wrap; align-items: center; gap: 8px 10px; flex-shrink: 0; padding: 8px 12px; border-bottom: 1px solid var(--border); }
.serial { display: flex; white-space: nowrap; align-items: center; gap: 8px; font-size: 13px; }
.serial .input { width: 72px; }
.name { max-width: 140px; overflow: hidden; text-overflow: ellipsis; color: var(--text); font-weight: 500; }
.spacer { flex-grow: 1; }
.hint { white-space: nowrap; font-size: 13px; color: var(--text-2); }
.hint.bad { color: var(--danger-text); }
.log { flex-grow: 1; min-height: 0; overflow-y: auto; padding: 4px 0; font-size: 13px; }
.empty { margin: 0; padding: 12px; color: var(--text-2); }
.cut { flex-shrink: 0; margin: 0; padding: 4px 12px; border-bottom: 1px solid var(--border); font-size: 12px; color: var(--text-2); }
.row { display: flex; align-items: baseline; gap: 12px; width: 100%; padding: 4px 12px; border: 0; background: none; text-align: left; font: inherit; color: inherit; cursor: pointer; }
.row:hover { background: var(--muted-bg); }
.row.sub { cursor: default; }
.row.sub:hover { background: none; }
.time { flex-shrink: 0; color: var(--text-2); font-size: 11px; }
.status { width: 80px; flex-shrink: 0; font-size: 11px; color: var(--text-2); }
.row.error .status, .row.error .msg { color: var(--danger-text); }
.name { flex-shrink: 0; max-width: 240px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-weight: 500; }
.msg { flex-grow: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; color: var(--text-2); }
.ms { flex-shrink: 0; color: var(--text-2); font-size: 11px; }
</style>
