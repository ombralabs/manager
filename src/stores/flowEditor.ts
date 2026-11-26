import { defineStore } from 'pinia'
import { computed, nextTick, ref } from 'vue'
import { core } from '../api/core'
import type { FlowDoc, OpsCatalog, Report } from '../api/types'
import { i18n } from '../i18n'
import { copyItems, groupItems, pasteClip, setDisabled, ungroup as ungroupDoc, useFlowClipboard } from '../lib/flowClip'
import { layoutMissing, levelOf } from '../lib/flowEdit'
import { notify } from '../lib/notify'

const HISTORY = 100
const VALIDATE_DELAY = 400
const clone = <T>(value: T): T => JSON.parse(JSON.stringify(value)) as T
const NO_ISSUES: Report = { errors: [], warnings: [] }

export const useFlowEditorStore = defineStore('flowEditor', () => {
  const catalog = ref<OpsCatalog>({ ops: [], system: [] })
  const id = ref<number | null>(null)
  const doc = ref<FlowDoc | null>(null)
  const path = ref<string[]>([])
  const selected = ref<string | null>(null)
  const nodeSettingsOpen = ref(false)
  /** Все выбранные узлы уровня: пишет холст; запись извне (после вставки) выбирает на холсте ровно их. */
  const selection = ref<string[]>([])
  /** Выбранные связи уровня (ключи `edgeKey`); пишет только холст. */
  const edgeSelection = ref<string[]>([])
  const clip = useFlowClipboard()
  const report = ref<Report>(NO_ISSUES)
  const saved = ref('')
  const undoStack = ref<FlowDoc[]>([])
  const redoStack = ref<FlowDoc[]>([])
  let lastKey: string | null = null
  let timer: ReturnType<typeof setTimeout> | undefined
  let token = 0
  /** Сеанс правки: растёт с каждой загрузкой; ответ, пришедший для прежнего сеанса, отбрасывается. */
  let session = 0

  const dirty = computed(() => doc.value !== null && JSON.stringify(doc.value) !== saved.value)
  const canUndo = computed(() => undoStack.value.length > 0)
  const canRedo = computed(() => redoStack.value.length > 0)

  function validateSoon() {
    clearTimeout(timer)
    const mine = ++token
    timer = setTimeout(async () => {
      if (!doc.value) return
      try {
        const result = await core.validateFlow(doc.value)
        if (mine === token) report.value = result
      } catch {
        // Проверка — подсказка: сбой не мешает правке, ошибка придёт при сохранении.
      }
    }, VALIDATE_DELAY)
  }

  /** `false` — пока шла загрузка, начался новый сеанс; её результат (и ошибка) уже не нужны. */
  async function load(flowId: number | null): Promise<boolean> {
    nodeSettingsOpen.value = false
    clearTimeout(timer)
    token++
    const mine = ++session
    let loaded: FlowDoc
    try {
      if (catalog.value.ops.length === 0) catalog.value = await core.opsCatalog()
      loaded = flowId === null
        ? { format: 'ombra-flow/1', name: i18n.global.t('automation.editor.newName'), nodes: [{ id: 'n1', op: 'start' }] }
        : await core.getFlow(flowId)
    } catch (e) {
      if (mine !== session) return false
      throw e
    }
    if (mine !== session) return false
    // Раскладка узлов без позиций — часть открытия, не правка: документ чистый, шага отмены нет.
    loaded = layoutMissing(loaded, [], new Map(catalog.value.ops.map((op) => [op.id, op])))
    id.value = flowId
    doc.value = loaded
    saved.value = JSON.stringify(loaded)
    path.value = []
    selected.value = null
    selection.value = []
    undoStack.value = []
    redoStack.value = []
    lastKey = null
    report.value = NO_ISSUES
    validateSoon()
    return true
  }

  /** Путь должен вести в существующие группы после отката. */
  function fixPath() {
    if (!doc.value) return
    let nodes = doc.value.nodes
    const ok: string[] = []
    for (const step of path.value) {
      const group = nodes.find((n) => n.id === step && (n.nodes || n.op === 'control.group'))
      if (!group) break
      ok.push(step)
      nodes = group.nodes ?? []
    }
    path.value = ok
    if (selected.value && !levelOf(doc.value, ok).nodes.some((n) => n.id === selected.value)) selected.value = null
  }

  function apply(change: (doc: FlowDoc) => FlowDoc, coalesceKey?: string) {
    if (!doc.value) return
    const next = change(doc.value)
    // Правка без изменений («Применить» как было, «Выровнять» выровненное) — ни шага отмены, ни несохранённого.
    if (next === doc.value || JSON.stringify(next) === JSON.stringify(doc.value)) return
    if (coalesceKey === undefined || coalesceKey !== lastKey || undoStack.value.length === 0) {
      undoStack.value.push(clone(doc.value))
      if (undoStack.value.length > HISTORY) undoStack.value.shift()
    }
    lastKey = coalesceKey ?? null
    redoStack.value = []
    doc.value = next
    validateSoon()
  }

  function step(from: typeof undoStack, to: typeof undoStack) {
    const prev = from.value.pop()
    if (!prev || !doc.value) return
    to.value.push(clone(doc.value))
    doc.value = prev
    lastKey = null
    fixPath()
    validateSoon()
  }
  const undo = () => step(undoStack, redoStack)
  const redo = () => step(redoStack, undoStack)

  /** В буфер окна — выбранное (или `ids`); Start не копируется. */
  function copy(ids: string[] = selection.value) {
    if (doc.value) clip.value = copyItems(doc.value, path.value, ids) ?? clip.value
  }
  /** Копия буфера на текущий уровень — один шаг отмены; вставленное становится выделением. */
  function paste() {
    const c = clip.value
    if (!c) return
    let ids: string[] = []
    apply((d) => {
      const r = pasteClip(d, path.value, c)
      ids = r.ids
      return r.doc
    })
    // Выделение — только если вставка изменила документ.
    if (ids.length === 0) return
    selection.value = ids
    selected.value = ids.length === 1 ? ids[0]! : null
  }
  /** Дублирование не меняет буфер обмена; новые узлы и связи — один шаг отмены. */
  function duplicate(ids: string[] = selection.value) {
    if (!doc.value) return
    const copied = copyItems(doc.value, path.value, ids)
    if (!copied) return
    let added: string[] = []
    apply((d) => {
      const result = pasteClip(d, path.value, copied)
      added = result.ids
      return result.doc
    })
    if (added.length) select(added)
  }
  const toggleDisabled = (ids: string[], disabled: boolean) => apply((d) => setDisabled(d, path.value, ids, disabled))

  function openNodeSettings(nodeId?: string) {
    if (!doc.value) return
    const requested = nodeId ? [nodeId] : selection.value.length ? selection.value : selected.value ? [selected.value] : []
    const available = new Set(levelOf(doc.value, path.value).nodes.map((node) => node.id))
    const ids = requested.filter((id) => available.has(id))
    if (!ids.length) return
    selection.value = ids
    selected.value = ids.length === 1 ? ids[0]! : null
    nodeSettingsOpen.value = true
  }

  /** Холст сначала убирает из выделения исчезнувшие узлы, потом принимает новое: запись — после этого. */
  function select(ids: string[]) {
    void nextTick().then(() => {
      selection.value = ids
      selected.value = ids.length === 1 ? ids[0]! : null
    })
  }
  const opsMap = () => new Map(catalog.value.ops.map((o) => [o.id, o]))
  /** Выделенная цепочка — в группу (один шаг отмены); не цепочка — уведомление. */
  function group(ids: string[] = selection.value) {
    if (ids.length === 0) return
    let gid: string | null = null
    let refused: string | null = null
    apply((d) => {
      const r = groupItems(d, path.value, ids, opsMap())
      if ('error' in r) { refused = r.error; return d }
      gid = r.id
      return r.doc
    })
    if (refused) notify.warning(i18n.global.t(refused === 'before_wait' ? 'automation.editor.groupRefusedWait' : 'automation.editor.groupRefused'))
    if (gid === null) return
    nodeSettingsOpen.value = false
    select([gid])
  }
  /** Узлы группы встают на её уровень и становятся выделением (один шаг отмены). */
  function ungroup(groupId: string) {
    if (!doc.value) return
    const inner = levelOf(doc.value, path.value).nodes.find((n) => n.id === groupId && n.op === 'control.group')?.nodes?.map((n) => n.id) ?? []
    let dropped = false
    apply((d) => {
      const r = ungroupDoc(d, path.value, groupId, opsMap())
      dropped = r.droppedError
      return r.doc
    })
    if (dropped) notify.info(i18n.global.t('automation.editor.ungroupErrorDropped'))
    nodeSettingsOpen.value = false
    select(inner)
  }

  function enterGroup(groupId: string) {
    nodeSettingsOpen.value = false
    path.value = [...path.value, groupId]
    selected.value = null
  }
  function leaveTo(depth: number) {
    nodeSettingsOpen.value = false
    path.value = path.value.slice(0, depth)
    selected.value = null
  }

  /** `null` — запись прошла, но редактор уже открыл другой процесс: её номер к нему не относится. */
  async function save(): Promise<number | null> {
    if (!doc.value) throw new Error(i18n.global.t('automation.editor.notLoaded'))
    const mine = session
    // Набор после сохранения — новый шаг отмены: откат вернёт сохранённое.
    lastKey = null
    const snapshot = JSON.stringify(doc.value)
    const row = await core.saveFlow(id.value, JSON.parse(snapshot) as FlowDoc)
    if (mine !== session) return null
    id.value = row.id
    saved.value = snapshot
    // Запись меняет граф вызовов в ядре (рекурсия A→B→A видна, только когда A сохранён) — проверка заново.
    validateSoon()
    return row.id
  }

  return { catalog, id, doc, path, selected, selection, edgeSelection, nodeSettingsOpen, openNodeSettings, report, dirty, canUndo, canRedo, load, apply, undo, redo, copy, paste, duplicate, toggleDisabled, group, ungroup, enterGroup, leaveTo, save, validateSoon }
})
