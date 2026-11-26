<script setup lang="ts">
import { Handle, Position, useVueFlow, type Connection, type HandleType } from '@vue-flow/core'
import { computed, nextTick, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import type { FlowNode, OpInfo } from '../../../api/types'
import OpIcon from '../../../components/OpIcon.vue'
import { nodeHeight, nodeWidth, nodeName, opLabel, outputsOf, portY } from '../../../lib/flowEdit'
import NodeQuickActions from './NodeQuickActions.vue'

export type HandleState = 'origin' | 'available' | 'unavailable' | 'ready'

export interface OpNodeData {
  node: FlowNode
  op: OpInfo | undefined
  issues: number
  warnings: number
  /** Узел, который сейчас выполняет отладка. */
  running?: boolean
  sequence?: string[]
  /** Подсказки на время соединения; размеры и расположение портов не меняются. */
  connection?: Record<HandleType, Record<string, HandleState>>
  // Проверяем новые связи на Handle: глобальный валидатор VueFlow также скрыл бы ошибочные связи загруженного процесса.
  isValidConnection?: (connection: Connection) => boolean
}

const props = defineProps<{ id: string; data: OpNodeData; selected?: boolean }>()
const { t, te } = useI18n()
const { updateNodeInternals } = useVueFlow()

const start = computed(() => props.data.node.op === 'start')
const group = computed(() => props.data.node.op === 'control.group')
const groupSize = computed(() => props.data.node.nodes?.length ?? 0)
const groupContents = computed(() => t('automation.editor.groupContents', { n: groupSize.value }, groupSize.value))
const opName = computed(() => opLabel(props.data.node.op, props.data.op?.adspower, te, t))
const title = computed(() => (start.value ? '' : nodeName(props.data.node)) || opName.value)
const inputs = computed(() => props.data.op?.inputs ?? [])
const outputs = computed(() => (props.data.op ? outputsOf(props.data.node, props.data.op) : []))
const height = computed(() => nodeHeight(props.data.node, props.data.op))
// vue-flow запоминает места ручек при их появлении: выход error сдвигает остальные — пересчёт после отрисовки.
watch(() => `${inputs.value.join()}:${outputs.value.join()}`, () => nextTick(() => updateNodeInternals([props.id])))

const PORT_LABELS: Record<string, string> = { true: 'portTrue', false: 'portFalse', body: 'portBody', error: 'portError', loop: 'portLoop' }
const portLabel = (port: string) => (PORT_LABELS[port] ? t(`automation.editor.${PORT_LABELS[port]}`) : '')
const top = (i: number, n: number) => `${portY(i, n, height.value)}px`
const handleClass = (type: HandleType, port: string) => {
  const state = props.data.connection?.[type][port]
  return state ? `handle-${state}` : undefined
}
</script>

<template>
  <div :class="['op-node', { 'start-node': start, 'group-node': group, 'empty-group': group && groupSize === 0, disabled: data.node.disabled, invalid: data.issues > 0, selected, running: data.running }]"
    :title="start ? nodeName(data.node) || undefined : group ? t('automation.editor.groupOpenHint') : undefined"
    :style="{ width: `${nodeWidth(data.node)}px`, minHeight: `${height}px`, height: start ? `${height}px` : undefined }">
    <NodeQuickActions :node="data.node" :sequence="data.sequence" />
    <Handle v-for="(port, i) in inputs" :id="port" :key="port" type="target" :position="Position.Left"
      :class="handleClass('target', port)" :connectable-end="data.connection?.target[port] !== 'unavailable'"
      :is-valid-connection="data.isValidConnection"
      :style="{ top: top(i, inputs.length) }" />
    <span v-if="data.running" class="run-mark" data-test="run-mark" aria-hidden="true">▶</span>
    <div class="node-heading">
      <OpIcon :op="data.node.op" :size="start ? 24 : 18" :class="start ? 'start-icon' : 'node-icon'" />
      <div class="node-label">
        <div class="title">{{ title }}</div>
        <div v-if="group" class="sub group-summary">
          <span class="group-kind">{{ t('automation.ops.control_group') }}</span>
          <span aria-hidden="true">·</span>
          <span class="group-count">{{ groupContents }}</span>
        </div>
        <div v-else-if="!start && opName !== title" class="sub">{{ opName }}</div>
      </div>
    </div>
    <span v-if="data.issues > 0" class="issues" :title="t('automation.editor.errorsCount', { n: data.issues })">{{ data.issues }}</span>
    <span v-else-if="data.warnings > 0" class="issues warn" :title="t('automation.editor.warningsCount', { n: data.warnings })">{{ data.warnings }}</span>
    <template v-for="(port, i) in inputs" :key="`label-in-${port}`">
      <span v-if="portLabel(port)" class="port-label input-label" :style="{ top: top(i, inputs.length) }">{{ portLabel(port) }}</span>
    </template>
    <template v-for="(port, i) in outputs" :key="`label-out-${port}`">
      <span v-if="portLabel(port)" :class="['port-label', 'output-label', { error: port === 'error' }]" :style="{ top: top(i, outputs.length) }">{{ portLabel(port) }}</span>
    </template>
    <Handle v-for="(port, i) in outputs" :id="port" :key="port" type="source" :position="Position.Right"
      :class="[handleClass('source', port), { 'error-handle': port === 'error' }]"
      :is-valid-connection="data.isValidConnection"
      :connectable-end="data.connection?.source[port] !== 'unavailable'" :style="{ top: top(i, outputs.length) }" />
  </div>
</template>

<style scoped>
.op-node { position: relative; box-sizing: border-box; width: 220px; min-height: 72px; padding: 10px 12px 18px; border: 1px solid var(--border-strong); border-radius: 12px; background: var(--surface); box-shadow: 0 1px 3px color-mix(in srgb, var(--text) 8%, transparent); }
.op-node.start-node { display: flex; flex-direction: column; align-items: center; justify-content: center; gap: 6px; padding: 8px; cursor: pointer; }
.node-heading { display: flex; align-items: flex-start; gap: 8px; min-width: 0; }
.node-label { flex: 1; min-width: 0; }
.node-icon { flex-shrink: 0; margin-top: 1px; color: var(--text-2); }
.op-node.group-node { border-color: color-mix(in srgb, var(--accent) 35%, var(--border-strong)); background: color-mix(in srgb, var(--accent-soft) 65%, var(--surface)); }
/* Внутренняя рамка обозначает контейнер, не меняя размеры узла и места его портов. */
.group-node::before { content: ''; position: absolute; inset: 4px; border: 1px solid color-mix(in srgb, var(--accent) 18%, var(--border)); border-radius: 8px; pointer-events: none; }
.group-node .node-icon, .group-kind { color: var(--accent-ink); }
.group-node .title { font-weight: 600; }
.group-node .group-summary { display: flex; align-items: baseline; gap: 5px; font-size: 12px; }
.group-kind { flex-shrink: 0; }
.group-count { overflow: hidden; text-overflow: ellipsis; }
.op-node.empty-group { border-style: dashed; background: var(--surface); }
.empty-group::before { border-style: dashed; }
.start-node .node-heading { flex-direction: column; align-items: center; gap: 6px; }
.start-node .node-label { flex: none; }
.start-node .start-icon { flex-shrink: 0; color: var(--accent-ink); }
.start-node .title { padding: 0; font-size: 12px; line-height: 18px; text-align: center; }
.op-node.selected { border-color: var(--accent); box-shadow: 0 0 0 2px var(--accent-soft); }
.op-node.disabled { background: var(--disabled-bg); border-style: dashed; }
.op-node.disabled .node-heading { opacity: 0.5; }
.op-node :deep(.node-tools) { visibility: hidden; opacity: 0; pointer-events: none; }
.op-node:hover :deep(.node-tools) { visibility: visible; opacity: 1; pointer-events: auto; }
.op-node.running { border-color: var(--ok); border-width: 2px; background: var(--ok-bg); }
.op-node.running.selected { box-shadow: 0 0 0 2px var(--accent); }
.run-mark { position: absolute; top: 8px; right: 8px; font-size: 11px; line-height: 18px; color: var(--ok-text); }
.op-node.invalid { border-color: var(--danger-dot); }
.title { font-weight: 500; line-height: 20px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; padding-right: 22px; }
.sub { font-size: 11px; line-height: 16px; color: var(--text-2); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.issues { position: absolute; top: 8px; right: 8px; min-width: 18px; padding: 0 5px; border-radius: 9px; background: var(--danger-bg); color: var(--danger-text); font-size: 11px; line-height: 18px; text-align: center; }
/* Предупреждение — приглушённо: процесс запустится. */
.issues.warn { background: var(--muted-bg); color: var(--text-2); }
.port-label { position: absolute; transform: translateY(-50%); max-width: calc(50% - 24px); font-size: 10px; line-height: 14px; color: var(--text-2); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; pointer-events: none; }
.input-label { left: 12px; }
.output-label { right: 12px; text-align: right; }
.port-label.error { color: var(--danger-text); }
.op-node :deep(.vue-flow__handle) { box-sizing: border-box; width: 12px; height: 12px; border: 0; border-radius: 50%; background: var(--accent); }
/* VueFlow привязывает линии к краю Handle; область попадания расширяется отдельно от видимого круга. */
.op-node :deep(.vue-flow__handle)::before { content: ''; position: absolute; inset: -6px; border-radius: 50%; }
.op-node :deep(.vue-flow__handle.error-handle) { background-color: var(--danger-dot); }
.op-node :deep(.vue-flow__handle.handle-unavailable) { opacity: 0.2; cursor: not-allowed; }
.op-node :deep(.vue-flow__handle.handle-origin), .op-node :deep(.vue-flow__handle.handle-available) {
  background-color: var(--accent);
  box-shadow: 0 0 0 3px var(--focus-ring);
}
.op-node :deep(.vue-flow__handle.handle-available) { outline: 1px solid var(--accent); outline-offset: 3px; cursor: crosshair; }
.op-node :deep(.vue-flow__handle.handle-ready) {
  background-color: var(--ok);
  box-shadow: 0 0 0 4px var(--ok-bg);
  outline: 2px solid var(--ok);
  outline-offset: 3px;
  cursor: crosshair;
}
.op-node :deep(.vue-flow__handle.source) { background-color: var(--surface); }
.op-node :deep(.vue-flow__handle.source)::after { content: ''; position: absolute; inset: 0; box-sizing: border-box; border: 2px solid var(--accent); border-radius: 50%; pointer-events: none; }
.op-node :deep(.vue-flow__handle.source.error-handle)::after { border-color: var(--danger-dot); }
.op-node :deep(.vue-flow__handle.source.handle-ready)::after { border-color: var(--ok); }
</style>
