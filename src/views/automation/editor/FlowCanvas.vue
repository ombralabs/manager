<script setup lang="ts">
import { ConnectionMode, VueFlow, useVueFlow, type Connection, type Edge, type EdgeChange, type HandleType, type Node, type NodeChange, type NodeComponent } from '@vue-flow/core'
import '@vue-flow/core/dist/style.css'
import { computed, markRaw, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import UiButton from '../../../components/UiButton.vue'
import OpIcon from '../../../components/OpIcon.vue'
import { linearSequences } from '../../../lib/flowClip'
import { edgeKey, issuesByNode, levelOf, moveNodes, nodeHeight, NODE_W, opLabel, outputsOf, pathTo, removeItems, addNode, canConnect, connect } from '../../../lib/flowEdit'
import { useFlowDebugStore } from '../../../stores/flowDebug'
import { useFlowEditorStore } from '../../../stores/flowEditor'
import type { PaletteDrag } from './paletteDrag'
import OpNode, { type HandleState, type OpNodeData } from './OpNode.vue'

const props = defineProps<{ paletteDrag?: PaletteDrag | null }>()
const { t, te } = useI18n()
const canvas = ref<HTMLElement | null>(null)
const store = useFlowEditorStore()
const debug = useFlowDebugStore()
const { screenToFlowCoordinate, viewport, fitView, nodesSelectionActive, selectionKeyCode, connectionStartHandle, connectionClickStartHandle, connectionEndHandle, connectionStatus } = useVueFlow()
// Сетка движется и масштабируется вместе со схемой; центр точки совпадает с координатой холста.
const canvasStyle = computed(() => {
  const { x, y, zoom } = viewport.value
  const step = 20 * zoom
  return {
    backgroundSize: `${step}px ${step}px`,
    backgroundPosition: `${x - step / 2}px ${y - step / 2}px`,
    '--canvas-dot-size': `${0.8 * zoom}px`,
  }
})
const nodeTypes = { op: markRaw(OpNode) as unknown as NodeComponent }
const ops = computed(() => new Map(store.catalog.ops.map((op) => [op.id, op])))
/**
 * Выделение холста — как у vue-flow (по его событиям `select`): рамка с Shift и щелчок с ⌘ выбирают несколько узлов
 * и связей, их тянут и удаляют вместе. Настройки выделения открываются отдельно.
 */
const selNodes = ref(new Set<string>())
const selEdges = ref(new Set<string>())
const mode = ref<'select' | 'pan'>('select')
// В runtime-пропе VueFlow отсутствует строковый KeyFilter, хотя хранилище поддерживает его.
watch(mode, value => { selectionKeyCode.value = value === 'select' ? true : 'Shift' }, { immediate: true })

const level = computed(() => (store.doc ? levelOf(store.doc, store.path) : { nodes: [], edges: [] }))
const sequences = computed(() => store.doc ? linearSequences(store.doc, store.path, ops.value) : [])
const sequenceByNode = computed(() => new Map(sequences.value.flatMap((ids) => ids.map((id) => [id, ids] as const))))
const connectionStart = computed(() => connectionStartHandle.value ?? connectionClickStartHandle.value)

function validConnection(c: Connection): boolean {
  return !!store.doc && !!c.sourceHandle && !!c.targetHandle &&
    canConnect(store.doc, store.path, c.source, c.sourceHandle, c.target, c.targetHandle, ops.value)
}

function handleState(nodeId: string, type: HandleType, id: string): HandleState {
  const start = connectionStart.value!
  if (start.nodeId === nodeId && start.type === type && start.id === id) return 'origin'
  if (start.type === type) return 'unavailable'
  const connection: Connection = type === 'target'
    ? { source: start.nodeId, sourceHandle: start.id, target: nodeId, targetHandle: id }
    : { source: nodeId, sourceHandle: id, target: start.nodeId, targetHandle: start.id }
  if (!validConnection(connection)) return 'unavailable'
  const end = connectionEndHandle.value
  return connectionStatus.value === 'valid' && end?.nodeId === nodeId && end.type === type && end.id === id ? 'ready' : 'available'
}

/** Идущий узел на этом уровне: он сам или группа, внутри которой он идёт. */
const running = computed(() => {
  const id = debug.current
  const path = id && store.doc ? pathTo(store.doc.nodes, id) : null
  if (!id || !path) return null
  const full = [...path, id]
  return store.path.every((g, i) => full[i] === g) ? (full[store.path.length] ?? null) : null
})

const nodes = computed<Node<OpNodeData>[]>(() => {
  const errors = store.doc ? issuesByNode(store.report.errors, store.doc) : new Map()
  const warnings = store.doc ? issuesByNode(store.report.warnings, store.doc) : new Map()
  return level.value.nodes.map((node) => {
    const op = ops.value.get(node.op)
    const connection = connectionStart.value && op ? {
      target: Object.fromEntries(op.inputs.map((id) => [id, handleState(node.id, 'target', id)])),
      source: Object.fromEntries(outputsOf(node, op).map((id) => [id, handleState(node.id, 'source', id)])),
    } : undefined
    return {
      id: node.id,
      type: 'op',
      position: { x: node.pos?.[0] ?? 0, y: node.pos?.[1] ?? 0 },
      selected: selNodes.value.has(node.id),
      draggable: node.op !== 'start',
      deletable: node.op !== 'start',
      data: { node, op, connection, sequence: sequenceByNode.value.get(node.id), isValidConnection: validConnection, issues: errors.get(node.id)?.length ?? 0, warnings: warnings.get(node.id)?.length ?? 0, running: running.value === node.id },
    }
  })
})

const edges = computed<Edge[]>(() =>
  level.value.edges.map((e) => {
    const port = e.port ?? 'next'
    return {
      id: edgeKey(e), source: e.from, target: e.to, sourceHandle: port, targetHandle: e.to_port ?? 'in',
      class: port === 'error' ? 'error-edge' : undefined, selected: selEdges.value.has(edgeKey(e)),
    }
  }),
)

// Первый показ уровня вписывает его, но не крупнее 1:1: иначе одинокий Start новой схемы занимает весь холст.
// Вход в группу и выход из неё — новый уровень: вписывается, когда vue-flow измерит его узлы (`nodes-initialized`).
let fitted = false
function fitOnce() {
  if (fitted) return
  fitted = true
  void fitView({ maxZoom: 1 })
}
// По содержимому: отмена и повтор (`fixPath`) ставят новый массив того же пути — уровень тот же, вид остаётся.
watch(() => store.path.join('/'), () => { fitted = false })

// Новый уровень — без прежнего выделения; выбор извне (палитра, «Ошибок: N», отмена) — один этот узел.
watch([() => store.path.join('/'), () => store.selected], ([path, id], [was]) => {
  const only = selNodes.value.size === 1 ? [...selNodes.value][0] : null
  if (path === was && (id === only || (id === null && selNodes.value.size > 1))) return
  selNodes.value = new Set(id ? [id] : [])
  selEdges.value = new Set()
})
const toggled = (set: Set<string>, id: string, on: boolean) => {
  const next = new Set(set)
  if (on) next.add(id)
  else next.delete(id)
  return next
}
// Выделение — наружу (`store.selection`, связи — `store.edgeSelection`); запись извне (вставка) выбирает на холсте
// ровно эти узлы.
watch(selNodes, (set) => {
  if (set.size !== store.selection.length || store.selection.some((id) => !set.has(id))) store.selection = [...set]
}, { immediate: true, flush: 'sync' })
watch(selEdges, (set) => { store.edgeSelection = [...set] }, { immediate: true, flush: 'sync' })
watch(() => store.selection, (ids) => {
  if (ids.length === selNodes.value.size && ids.every((id) => selNodes.value.has(id))) return
  selNodes.value = new Set(ids)
  selEdges.value = new Set()
  showSingle()
})
const showSingle = () => { store.selected = selNodes.value.size === 1 ? [...selNodes.value][0]! : null }

function selectAll() {
  selNodes.value = new Set(level.value.nodes.map(node => node.id))
  selEdges.value = new Set(level.value.edges.map(edgeKey))
  nodesSelectionActive.value = selNodes.value.size > 1
  showSingle()
  canvas.value?.focus({ preventScroll: true })
}

function clearSelection() {
  selNodes.value = new Set()
  selEdges.value = new Set()
  nodesSelectionActive.value = false
  showSingle()
  canvas.value?.focus({ preventScroll: true })
}
// Отмена и повтор могут убрать часть выделенного (и удаление его убирает): в выделении — только то, что есть на уровне.
watch(level, ({ nodes, edges }) => {
  const ids = new Set(nodes.map((n) => n.id))
  const keys = new Set(edges.map(edgeKey))
  const left = [...selNodes.value].filter((id) => ids.has(id))
  const kept = [...selEdges.value].filter((key) => keys.has(key))
  if (left.length === selNodes.value.size && kept.length === selEdges.value.size) return
  selNodes.value = new Set(left)
  selEdges.value = new Set(kept)
  showSingle()
})
// Рамка vue-flow вокруг выделения (после выделения рамкой) нужна, только пока выбрано несколько узлов: вокруг одного,
// выбранного извне (палитра, «Ошибок: N»), она перехватила бы двойной щелчок по нему.
watch(() => nodesSelectionActive.value && selNodes.value.size <= 1, (stale) => { if (stale) nodesSelectionActive.value = false })

function onConnect(c: Connection) {
  if (!c.sourceHandle || !c.targetHandle) return
  store.apply((doc) => connect(doc, store.path, c.source, c.sourceHandle!, c.target, c.targetHandle!, ops.value))
}

/** Перетащенные узлы (и выделенные вместе с тем, за который тянули) — одной правкой, один шаг отмены. */
function onDragStop({ nodes: moved }: { nodes: Node[] }) {
  const at = new Map(moved.map((n) => [n.id, [Math.round(n.position.x), Math.round(n.position.y)] as [number, number]]))
  store.apply((doc) => moveNodes(doc, store.path, at))
}

/**
 * Выбор (щелчок, рамка, Enter и пробел, Escape) — в выделение холста. Сдвиг стрелками (`position` с `dragging: false`;
 * у конца перетаскивания позиции нет — его пишет `onDragStop`) — в документ; серия нажатий — один шаг отмены.
 */
function onNodesChange(changes: NodeChange[]) {
  const at = new Map<string, [number, number]>()
  for (const c of changes) {
    if (c.type === 'select') selNodes.value = toggled(selNodes.value, c.id, c.selected)
    else if (c.type === 'position' && c.dragging === false && c.position) at.set(c.id, [Math.round(c.position.x), Math.round(c.position.y)])
  }
  showSingle()
  if (at.size > 0) store.apply((doc) => moveNodes(doc, store.path, at), `keys:${[...at.keys()].join()}`)
}
function onEdgesChange(changes: EdgeChange[]) {
  for (const c of changes) if (c.type === 'select') selEdges.value = toggled(selEdges.value, c.id, c.selected)
}

function onNodeDouble({ node, event }: { node: Node; event?: Event }) {
  if ((event?.target as Element | null)?.closest('.vue-flow__handle')) return
  const n = level.value.nodes.find((x) => x.id === node.id)
  if (n && (n.nodes || n.op === 'control.group')) store.enterGroup(n.id)
  else if (n) store.openNodeSettings(n.id)
}

/** Одной правкой — один шаг отмены; Start не удаляется (`removeItems`) и остаётся выделенным. */
function remove(nodeIds: string[], edgeKeys: string[]) {
  store.apply((doc) => removeItems(doc, store.path, nodeIds, edgeKeys))
}

function onKey(event: KeyboardEvent) {
  const target = event.target as HTMLElement | null
  if (target && (target.tagName === 'INPUT' || target.tagName === 'TEXTAREA' || target.isContentEditable)) return
  if (event.key === 'Escape') {
    clearSelection()
    return
  }
  if (event.key !== 'Delete' && event.key !== 'Backspace') return
  if (selNodes.value.size > 0 || selEdges.value.size > 0) remove([...selNodes.value], [...selEdges.value])
}

/** Контур и сохранённая позиция используют один расчёт: центр блока находится под указателем. */
function placement(drag: PaletteDrag) {
  const box = canvas.value?.getBoundingClientRect()
  const op = ops.value.get(drag.op.id)
  if (!store.doc || !op || op.id === 'start' || !box ||
    drag.x < box.left || drag.x >= box.right || drag.y < box.top || drag.y >= box.bottom) return null
  const p = screenToFlowCoordinate({ x: drag.x, y: drag.y })
  const height = nodeHeight({ id: '', op: op.id }, op)
  const pos: [number, number] = [Math.round(p.x - NODE_W / 2), Math.round(p.y - height / 2)]
  return { op, pos, height }
}

const preview = computed(() => props.paletteDrag ? placement(props.paletteDrag) : null)
const previewStyle = computed(() => {
  if (!preview.value) return undefined
  const [x, y] = preview.value.pos
  const view = viewport.value
  return {
    width: `${NODE_W}px`,
    height: `${preview.value.height}px`,
    transform: `translate(${x * view.zoom + view.x}px, ${y * view.zoom + view.y}px) scale(${view.zoom})`,
  }
})
const previewTitle = computed(() => preview.value ? opLabel(preview.value.op.id, preview.value.op.adspower, te, t) : '')

function dropOperation(drag: PaletteDrag) {
  const target = placement(drag)
  if (!target) return
  store.apply((doc) => {
    const added = addNode(doc, store.path, target.op, target.pos, undefined, ops.value)
    store.selected = added.id
    return added.doc
  })
}

defineExpose({ dropOperation, selectAll })
</script>

<template>
  <div ref="canvas" :class="['canvas', `mode-${mode}`]" :style="canvasStyle" tabindex="0" @keydown="onKey">
    <VueFlow :nodes="nodes" :edges="edges" :node-types="nodeTypes" :delete-key-code="null" :nodes-connectable="true"
      :connection-mode="ConnectionMode.Strict"
      :connection-line-options="{ style: { stroke: connectionStatus === 'valid' ? 'var(--ok)' : 'var(--accent)', strokeWidth: 2 } }"
      :zoom-on-double-click="false"
      :pan-on-drag="mode === 'select' ? [1, 2] : true"
      :multi-selection-key-code="['Shift', 'Control', 'Meta']"
      @nodes-initialized="fitOnce" @connect="onConnect" @node-drag-stop="onDragStop" @node-double-click="onNodeDouble"
      @nodes-change="onNodesChange" @edges-change="onEdgesChange"
      @edge-double-click="({ edge }) => remove([], [edge.id])" />
    <div class="canvas-tools" role="toolbar" :aria-label="t('automation.editor.canvasTools')">
      <UiButton size="icon" icon="selectArea" :variant="mode === 'select' ? 'soft' : 'ghost'"
        :aria-pressed="mode === 'select'" :aria-label="t('automation.editor.selectMode')" :title="t('automation.editor.selectMode')"
        @click="mode = 'select'" />
      <UiButton size="icon" icon="hand" :variant="mode === 'pan' ? 'soft' : 'ghost'"
        :aria-pressed="mode === 'pan'" :aria-label="t('automation.editor.panMode')" :title="t('automation.editor.panMode')"
        @click="mode = 'pan'" />
      <span class="tool-separator" aria-hidden="true" />
      <UiButton size="sm" variant="ghost" :disabled="level.nodes.length === 0" @click="selectAll">{{ t('automation.editor.selectAll') }}</UiButton>
      <UiButton v-if="selNodes.size || selEdges.size" size="icon" icon="close" variant="ghost"
        :aria-label="t('automation.editor.clearSelection')" :title="t('automation.editor.clearSelection')" @click="clearSelection" />
    </div>
    <div class="canvas-hint" aria-live="polite">
      <span v-if="selNodes.size > 1">{{ t('automation.editor.selectionMoveHint', { n: selNodes.size }) }}</span>
      <span v-else>{{ t(mode === 'select' ? 'automation.editor.selectHint' : 'automation.editor.panHint') }}</span>
    </div>
    <div v-if="preview" class="placement-preview" :style="previewStyle" aria-hidden="true">
      <div class="preview-heading">
        <OpIcon :op="preview.op.id" :size="18" />
        <div class="preview-label">
          <div class="preview-title">{{ previewTitle }}</div>
        </div>
      </div>
    </div>
  </div>
</template>

<style>
.canvas { position: relative; flex-grow: 1; min-width: 0; min-height: 0; outline: 0; background-color: var(--canvas-bg); background-image: radial-gradient(circle, var(--canvas-grid) var(--canvas-dot-size), transparent calc(var(--canvas-dot-size) + 0.25px)); }
.canvas .vue-flow__edge-path { stroke: var(--text-2); stroke-width: 1.5; }
.canvas .vue-flow__node:hover, .canvas .vue-flow__node:focus-within { z-index: 1001 !important; }
.canvas .vue-flow__edge.selected .vue-flow__edge-path { stroke: var(--accent); }
.canvas .vue-flow__edge.error-edge .vue-flow__edge-path { stroke: var(--danger-dot); stroke-dasharray: 5 4; }
.canvas .vue-flow__handle.error-handle { border-style: dashed; }
.canvas.mode-select .vue-flow__pane.selection { cursor: crosshair; }
.canvas .vue-flow__selection { border: 1px solid var(--accent); background: color-mix(in srgb, var(--accent) 12%, transparent); }
.canvas .vue-flow__nodesselection-rect { border: 1px dashed var(--accent); border-radius: 10px; background: color-mix(in srgb, var(--accent) 4%, transparent); cursor: grab; }
.canvas .vue-flow__nodesselection-rect.dragging { cursor: grabbing; }
.canvas-tools { position: absolute; top: 10px; left: 10px; z-index: 6; display: flex; align-items: center; gap: 2px; padding: 3px; border: 1px solid var(--border); border-radius: 10px; background: var(--surface); box-shadow: 0 2px 8px #1727460a; }
.canvas-tools .btn { height: 30px; }
.canvas-tools .btn.square { width: 30px; padding: 0; }
.canvas-tools .btn.sm { padding: 0 8px; }
.tool-separator { height: 18px; width: 1px; margin: 0 3px; background: var(--border); }
.canvas-hint { position: absolute; bottom: 10px; left: 10px; right: 10px; z-index: 5; width: fit-content; max-width: calc(100% - 20px); padding: 4px 8px; border-radius: 6px; background: var(--surface); color: var(--text-2); font-size: 11px; pointer-events: none; }
.canvas .placement-preview {
  position: absolute;
  top: 0;
  left: 0;
  z-index: 10;
  padding: 10px 12px 18px;
  border: 2px dashed var(--accent);
  border-radius: 12px;
  background: var(--accent-soft);
  color: var(--accent-ink);
  box-shadow: 0 0 0 3px var(--focus-ring);
  transform-origin: 0 0;
  pointer-events: none;
}
.canvas .preview-title { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.canvas .preview-heading { display: flex; align-items: flex-start; gap: 8px; }
.canvas .preview-heading .op-icon { flex-shrink: 0; margin-top: 1px; }
.canvas .preview-label { flex: 1; min-width: 0; }
.canvas .preview-title { font-weight: 500; }
</style>
