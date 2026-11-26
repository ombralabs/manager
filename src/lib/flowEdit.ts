import dagre from '@dagrejs/dagre'
import type { FlowDoc, FlowEdge, FlowNode, Issue, OpInfo, OpsCatalog } from '../api/types'

/** Базовый размер узла; высоту блока с несколькими портами определяет nodeHeight. */
export const NODE_W = 220
export const NODE_H = 72
export const START_NODE_SIZE = 72
/** Промежутки между узлами: по горизонтали и между ярусами. */
const GAP_X = 40
const GAP_Y = 60

type Ops = Map<string, OpInfo>
type Holder = { nodes?: FlowNode[]; edges?: FlowEdge[] }

const clone = <T>(value: T): T => JSON.parse(JSON.stringify(value)) as T
const portOf = (e: FlowEdge) => e.port ?? 'next'
const toPortOf = (e: FlowEdge) => e.to_port ?? 'in'

export const edgeKey = (e: FlowEdge) => `${e.from}:${portOf(e)}->${e.to}:${toPortOf(e)}`

/** Имя переменной — как `is_var_name` ядра (`src-tauri/src/flow/vars.rs`). */
export const VAR_NAME = /^[A-Za-z_][A-Za-z0-9_]*$/
/** Слова, которые JavaScript не даёт именам констант: копия `JS_RESERVED` из `src-tauri/src/flow/ops/mod.rs`. */
export const JS_RESERVED = [
  'await', 'break', 'case', 'catch', 'class', 'const', 'continue', 'debugger', 'default', 'delete', 'do', 'else', 'enum',
  'export', 'extends', 'false', 'finally', 'for', 'function', 'if', 'implements', 'import', 'in', 'instanceof', 'interface',
  'let', 'new', 'null', 'package', 'private', 'protected', 'public', 'return', 'static', 'super', 'switch', 'this',
  'throw', 'true', 'try', 'typeof', 'var', 'void', 'while', 'with', 'yield', 'arguments', 'eval',
]

function holder(doc: Holder, path: string[]): Holder | undefined {
  let cur: Holder | undefined = doc
  for (const id of path) cur = cur?.nodes?.find((n) => n.id === id && (n.nodes || n.op === 'control.group'))
  return cur
}

export function levelOf(doc: FlowDoc, path: string[]): { nodes: FlowNode[]; edges: FlowEdge[] } {
  const h = holder(doc, path)
  return { nodes: h?.nodes ?? [], edges: h?.edges ?? [] }
}

/** Копия документа, правка уровня `path` на месте; путь не найден — документ как был. */
function edit(doc: FlowDoc, path: string[], fn: (h: Required<Holder>) => void): FlowDoc {
  const copy = clone(doc)
  const h = holder(copy, path)
  if (!h) return doc
  // Списка не было (ядро не пишет пустой `edges`) и он остался пуст — его нет и в копии: правка без изменений
  // («Выровнять» выровненное, перетаскивание на место) не даёт ни шага отмены, ни несохранённого.
  const absent = (['nodes', 'edges'] as const).filter((k) => !h[k])
  h.nodes ??= []
  h.edges ??= []
  fn(h as Required<Holder>)
  for (const k of absent) if (h[k]!.length === 0) delete h[k]
  return copy
}

export function outputsOf(node: FlowNode, op: OpInfo): string[] {
  return node.on_error === 'branch' ? [...op.outputs, 'error'] : op.outputs
}

/** У нескольких боковых портов — отдельные строки ниже заголовка. */
export function nodeHeight(node: FlowNode, op?: OpInfo): number {
  if (node.op === 'start') return START_NODE_SIZE
  const rows = Math.max(op?.inputs.length ?? 0, op ? outputsOf(node, op).length : 0)
  return rows > 1 ? 44 + rows * 24 + 8 : NODE_H
}

export const nodeWidth = (node: FlowNode): number => node.op === 'start' ? START_NODE_SIZE : NODE_W

export function portY(index: number, count: number, height: number): number {
  const top = height > NODE_H ? 44 : 0
  const bottom = height > NODE_H ? 8 : 0
  return top + (height - top - bottom) * (index + 0.5) / count
}

/** Подпись операции: перевод по id (`web.click` → `automation.ops.web_click`), без перевода — название AdsPower. */
export function opLabel(id: string, adspower: string | undefined, te: (key: string) => boolean, t: (key: string) => string): string {
  const key = `automation.ops.${id.replaceAll('.', '_')}`
  return te(key) ? t(key) : (adspower ?? id)
}

/** Подпись узла от пользователя: у группы — её `name`, иначе описание; пусто — подпись операции. */
export const nodeName = (n: FlowNode): string =>
  (n.op === 'control.group' && typeof n.config?.name === 'string' && n.config.name) || n.description || ''

function allNodes(nodes: FlowNode[]): FlowNode[] {
  return nodes.flatMap((n) => [n, ...allNodes(n.nodes ?? [])])
}

export function newId(doc: FlowDoc): string {
  const used = new Set(allNodes(doc.nodes).map((n) => n.id))
  let i = 1
  while (used.has(`n${i}`)) i++
  return `n${i}`
}

export const mainExit = (op?: OpInfo) => (op?.outputs.includes('false') ? 'false' : 'next')

export function addNode(doc: FlowDoc, path: string[], op: OpInfo, pos: [number, number], after: string | undefined, ops: Ops): { doc: FlowDoc; id: string } {
  const id = newId(doc)
  const config: Record<string, unknown> = {}
  for (const f of op.fields) if (f.default !== null && f.default !== undefined) config[f.name] = f.default
  const node: FlowNode = { id, op: op.id, pos }
  if (Object.keys(config).length > 0) node.config = config
  const next = edit(doc, path, (h) => {
    h.nodes.push(node)
    const prev = after === undefined ? undefined : h.nodes.find((n) => n.id === after)
    if (!prev) return
    const prevOp = ops.get(prev.op)
    const port = mainExit(prevOp)
    if (!prevOp || !outputsOf(prev, prevOp).includes(port)) return
    if (!h.edges.some((e) => e.from === after && portOf(e) === port)) h.edges.push({ from: after!, port, to: id, to_port: 'in' })
  })
  return { doc: next, id }
}

/** Общая проверка для подсветки портов, линии под указателем и записи связи. */
export function canConnect(doc: FlowDoc, path: string[], from: string, port: string, to: string, toPort: string, ops: Ops): boolean {
  const { nodes } = levelOf(doc, path)
  const source = nodes.find((n) => n.id === from)
  const target = nodes.find((n) => n.id === to)
  const sourceOp = source && ops.get(source.op)
  return from !== to && !!source && !!target && !!sourceOp && outputsOf(source, sourceOp).includes(port) &&
    !!ops.get(target.op)?.inputs.includes(toPort)
}

/** Связь только от выхода источника ко входу цели; иначе документ как был — занятый выход не теряет свою связь. */
export function connect(doc: FlowDoc, path: string[], from: string, port: string, to: string, toPort: string, ops: Ops): FlowDoc {
  if (!canConnect(doc, path, from, port, to, toPort, ops)) return doc
  return edit(doc, path, (h) => {
    h.edges = h.edges.filter((e) => !(e.from === from && portOf(e) === port))
    h.edges.push({ from, port, to, to_port: toPort })
  })
}

export function removeItems(doc: FlowDoc, path: string[], nodeIds: string[], edgeKeys: string[]): FlowDoc {
  return edit(doc, path, (h) => {
    const gone = new Set(nodeIds.filter((id) => h.nodes.find((n) => n.id === id)?.op !== 'start'))
    const keys = new Set(edgeKeys)
    h.nodes = h.nodes.filter((n) => !gone.has(n.id))
    h.edges = h.edges.filter((e) => !gone.has(e.from) && !gone.has(e.to) && !keys.has(edgeKey(e)))
  })
}

export const updateNode = (doc: FlowDoc, path: string[], id: string, patch: Partial<FlowNode>): FlowDoc =>
  edit(doc, path, (h) => {
    const node = h.nodes.find((n) => n.id === id)
    if (!node) return
    Object.assign(node, patch)
    // Без «Ветки ошибки» выхода error нет: его связь уходит той же правкой (один шаг отмены).
    if ('on_error' in patch && node.on_error !== 'branch') h.edges = h.edges.filter((e) => !(e.from === id && e.port === 'error'))
  })

export const moveNodes = (doc: FlowDoc, path: string[], at: Map<string, [number, number]>): FlowDoc =>
  edit(doc, path, (h) => {
    for (const n of h.nodes) {
      const pos = at.get(n.id)
      if (pos && n.op !== 'start') n.pos = pos
    }
  })

/** `undefined` убирает поле из конфига. */
export const setConfig = (doc: FlowDoc, path: string[], id: string, field: string, value: unknown): FlowDoc =>
  edit(doc, path, (h) => {
    const node = h.nodes.find((n) => n.id === id)
    if (!node) return
    const config = { ...node.config }
    if (value === undefined) delete config[field]
    else config[field] = value
    if (Object.keys(config).length > 0) node.config = config
    else delete node.config
  })

export function align(doc: FlowDoc, path: string[], ops: Ops): FlowDoc {
  return edit(doc, path, (h) => {
    const g = new dagre.graphlib.Graph()
    g.setGraph({ rankdir: 'LR', nodesep: GAP_Y, ranksep: GAP_X })
    g.setDefaultEdgeLabel(() => ({}))
    for (const n of h.nodes) g.setNode(n.id, { width: nodeWidth(n), height: nodeHeight(n, ops.get(n.op)) })
    // Возврат во вход loop — обратная связь, на ярусы не влияет.
    for (const e of h.edges) if (toPortOf(e) !== 'loop' && e.from !== e.to) g.setEdge(e.from, e.to)
    dagre.layout(g)
    // Уже установленный Старт — неподвижный якорь; остальная схема раскладывается относительно него.
    const anchor = h.nodes.find((n) => n.op === 'start' && n.pos)
    const anchorPosition = anchor && g.node(anchor.id)
    const dx = anchor ? anchor.pos![0] - (anchorPosition.x - nodeWidth(anchor) / 2) : 0
    const dy = anchor ? anchor.pos![1] - (anchorPosition.y - nodeHeight(anchor, ops.get(anchor.op)) / 2) : 0
    for (const n of h.nodes) {
      if (n === anchor) continue
      const p = g.node(n.id)
      n.pos = [Math.round(p.x - nodeWidth(n) / 2 + dx), Math.round(p.y - nodeHeight(n, ops.get(n.op)) / 2 + dy)]
    }
  })
}

/**
 * Узлы без `pos` (процесс из JSON, CLI, импорт) иначе лежат стопкой в (0,0). Уровень без единой позиции раскладывается
 * целиком; в остальных узлы с `pos` остаются на месте, а недостающие встают в ряд под самым нижним.
 */
export function layoutMissing(doc: FlowDoc, path: string[] = [], ops: Ops = new Map()): FlowDoc {
  const { nodes } = levelOf(doc, path)
  const placed = nodes.filter((n) => n.pos)
  let out = doc
  if (placed.length === 0 && nodes.length > 0) out = align(doc, path, ops)
  else if (placed.length < nodes.length) {
    const x = Math.min(...placed.map((n) => n.pos![0]))
    const y = Math.max(...placed.map((n) => n.pos![1] + nodeHeight(n, ops.get(n.op)))) + GAP_Y
    let nextX = x
    out = edit(doc, path, (h) => {
      for (const n of h.nodes) if (!n.pos) {
        n.pos = [nextX, y]
        nextX += nodeWidth(n) + GAP_X
      }
    })
  }
  for (const n of nodes) if (n.nodes) out = layoutMissing(out, [...path, n.id], ops)
  return out
}

export function variablesFor(doc: FlowDoc, catalog: OpsCatalog): string[] {
  const ops = new Map(catalog.ops.map((o) => [o.id, o]))
  const names = new Set<string>(catalog.system)
  for (const v of doc.variables ?? []) names.add(v.name)
  for (const n of allNodes(doc.nodes)) {
    for (const f of ops.get(n.op)?.fields ?? []) {
      // Поле не задано — ядро берёт имя по умолчанию из реестра (`Cfg::var_out`).
      const v = n.config?.[f.name] ?? f.default
      if (f.kind === 'var_out' && typeof v === 'string' && v) names.add(v)
    }
    if (n.op === 'proc.vars' && Array.isArray(n.config?.vars)) {
      for (const row of n.config.vars as { name?: unknown }[]) if (typeof row.name === 'string' && row.name) names.add(row.name)
    }
  }
  return [...names].sort()
}

function chainTo(nodes: FlowNode[], id: string): string[] | null {
  for (const n of nodes) {
    if (n.id === id) return [n.id]
    const inner = chainTo(n.nodes ?? [], id)
    if (inner) return [n.id, ...inner]
  }
  return null
}

/** Ошибки (или предупреждения) по узлам; найденное внутри группы видно и на узле группы на каждом уровне выше. */
export function issuesByNode(issues: Issue[], doc: FlowDoc): Map<string, Issue[]> {
  const map = new Map<string, Issue[]>()
  for (const issue of issues) {
    if (issue.node === null) continue
    for (const id of chainTo(doc.nodes, issue.node) ?? [issue.node]) map.set(id, [...(map.get(id) ?? []), issue])
  }
  return map
}

/** Путь групп до узла `id` (без него самого); `null` — узла нет. */
export function pathTo(nodes: FlowNode[], id: string, prefix: string[] = []): string[] | null {
  for (const n of nodes) {
    if (n.id === id) return prefix
    const inner = n.nodes ? pathTo(n.nodes, id, [...prefix, n.id]) : null
    if (inner) return inner
  }
  return null
}

/** Узлы тела цикла `loopId` — как `body_of` ядра: вперёд из `body` по любым выходам, без связей во вход `loop` и без прохода через сам цикл. */
export function bodyOf(edges: FlowEdge[], loopId: string): Set<string> {
  const seen = new Set<string>()
  const queue = edges.filter((e) => e.from === loopId && portOf(e) === 'body' && e.to !== loopId).map((e) => e.to)
  for (let n = queue.shift(); n !== undefined; n = queue.shift()) {
    if (seen.has(n)) continue
    seen.add(n)
    queue.push(...edges.filter((e) => e.from === n && toPortOf(e) !== 'loop' && e.to !== loopId).map((e) => e.to))
  }
  return seen
}

/**
 * Узел (`path` — группы и он сам) или группа на пути к нему лежит в теле New Browser своего уровня. Тело идёт
 * в браузере другого профиля: отладка с такого узла не запускается.
 */
export function inBrowserBody(doc: FlowDoc, path: string[]): boolean {
  return path.some((id, i) => {
    const { nodes, edges } = levelOf(doc, path.slice(0, i))
    return nodes.some((b) => b.op === 'control.new_browser' && bodyOf(edges, b.id).has(id))
  })
}

/** Операции с кнопкой «Проверить» в форме узла. */
export const TESTABLE_OPS = ['data.email', 'data.totp', 'ext.google_sheet']

/** «Проверить» у узла: операция из списка; Google Sheet — только чтение (запись и очистку проверяет запуск). */
export const testable = (node: FlowNode) =>
  TESTABLE_OPS.includes(node.op) && (node.op !== 'ext.google_sheet' || (node.config?.operation ?? 'read') === 'read')

/**
 * Поля, нужные не при любом варианте операции: поле → значения поля-переключателя `by`, при которых форма его
 * показывает. Скрытое поле значение сохраняет — форма его только не показывает.
 */
const VARIANT_FIELDS: Record<string, { by: string; show: Record<string, string[]> }> = {
  'ext.2captcha': {
    by: 'kind',
    show: {
      ...Object.fromEntries(['image', 'image_path', 'answer_target', 'phrase', 'case_sensitive', 'math', 'numeric', 'min_len', 'max_len'].map((f) => [f, ['normal']])),
      sitekey: ['recaptcha_v2', 'recaptcha_v3', 'hcaptcha', 'turnstile'],
      action: ['recaptcha_v3'],
      min_score: ['recaptcha_v3'],
    },
  },
  'ext.openai': { by: 'output', show: { size: ['image'], format: ['image'], quality: ['image'], max_tokens: ['text'] } },
  'ext.google_sheet': { by: 'operation', show: { data: ['write'], save_to: ['read'] } },
}

/** Видно ли поле `name` в форме узла операции `op`: переключатель не задан — действует его умолчание. */
export function fieldShown(op: OpInfo, name: string, config: Record<string, unknown> = {}): boolean {
  if (op.id === 'web.scroll') {
    const value = (key: string) => config[key] ?? op.fields.find((field) => field.name === key)?.default
    if (name === 'target') return value('area') === 'target'
    if (name === 'pixels') return value('position') === 'pixels'
    if (name === 'step_px' || name === 'step_ms') return value('kind') === 'smooth'
  }
  const rule = VARIANT_FIELDS[op.id]
  const only = rule?.show[name]
  if (!rule || !only) return true
  return only.includes(String(config[rule.by] ?? op.fields.find((f) => f.name === rule.by)?.default))
}
