import { ref } from 'vue'
import type { FlowDoc, FlowEdge, FlowNode, OpInfo } from '../api/types'
import { bodyOf, levelOf, mainExit, newId, outputsOf, updateNode } from './flowEdit'

export type Clip = { nodes: FlowNode[]; edges: FlowEdge[] }

const clone = <T>(value: T): T => JSON.parse(JSON.stringify(value)) as T
const ids = (nodes: FlowNode[]): string[] => nodes.flatMap((n) => [n.id, ...ids(n.nodes ?? [])])

/** Выбранные узлы уровня (с содержимым групп) и связи между ними; Start не копируется. `null` — копировать нечего. */
export function copyItems(doc: FlowDoc, path: string[], selected: string[]): Clip | null {
  const level = levelOf(doc, path)
  const nodes = level.nodes.filter((n) => selected.includes(n.id) && n.op !== 'start')
  if (nodes.length === 0) return null
  const keep = new Set(nodes.map((n) => n.id))
  return clone({ nodes, edges: level.edges.filter((e) => keep.has(e.from) && keep.has(e.to)) })
}

/**
 * Копия буфера на уровне `path`: у всех узлов (и внутри групп) новые id, связи переназначены, узлы сдвинуты на `offset`.
 * Повторная вставка встаёт на следующий шаг сдвига — копии не ложатся друг на друга.
 */
export function pasteClip(doc: FlowDoc, path: string[], clip: Clip, offset = 40): { doc: FlowDoc; ids: string[] } {
  const used = new Set(ids(doc.nodes))
  let i = 1
  const fresh = () => {
    while (used.has(`n${i}`)) i++
    used.add(`n${i}`)
    return `n${i}`
  }
  const map = new Map<string, string>()
  // Сначала новые id всем узлам на любой глубине, затем связи каждого уровня.
  const rename = (n: FlowNode): FlowNode => {
    map.set(n.id, fresh())
    const out: FlowNode = { ...n, id: map.get(n.id)! }
    if (n.nodes) out.nodes = n.nodes.map(rename)
    return out
  }
  const remap = (e: FlowEdge): FlowEdge => ({ ...e, from: map.get(e.from) ?? e.from, to: map.get(e.to) ?? e.to })
  const relink = (n: FlowNode) => {
    if (n.edges) n.edges = n.edges.map(remap)
    n.nodes?.forEach(relink)
  }
  const copy = clone(clip)
  const nodes = copy.nodes.map(rename)
  nodes.forEach(relink)
  const edges = copy.edges.map(remap)

  const level = levelOf(doc, path)
  const taken = new Set(level.nodes.map((n) => `${n.pos?.[0] ?? 0},${n.pos?.[1] ?? 0}`))
  let k = 1
  const at = (n: FlowNode) => `${(n.pos?.[0] ?? 0) + k * offset},${(n.pos?.[1] ?? 0) + k * offset}`
  while (nodes.some((n) => taken.has(at(n)))) k++
  for (const n of nodes) n.pos = [(n.pos?.[0] ?? 0) + k * offset, (n.pos?.[1] ?? 0) + k * offset]

  const next = clone(doc)
  let target: { nodes?: FlowNode[]; edges?: FlowEdge[] } | undefined = next
  for (const id of path) target = target?.nodes?.find((n) => n.id === id)
  if (!target) return { doc, ids: [] }
  target.nodes = [...(target.nodes ?? []), ...nodes]
  target.edges = [...(target.edges ?? []), ...edges]
  return { doc: next, ids: nodes.map((n) => n.id) }
}

/** Включает или отключает выбранные узлы уровня; Start не отключается. */
export const setDisabled = (doc: FlowDoc, path: string[], selected: string[], disabled: boolean): FlowDoc =>
  levelOf(doc, path).nodes
    .filter((n) => selected.includes(n.id) && n.op !== 'start')
    .reduce((d, n) => updateNode(d, path, n.id, { disabled: disabled || undefined }), doc)

type Ops = Map<string, OpInfo>
const portOf = (e: FlowEdge) => e.port ?? 'next'
/** Вход цепочки — первый узел без входящих связей, кроме `loop`: так его ищет `Ctx::group` ядра. */
const entryOf = (nodes: FlowNode[], edges: FlowEdge[]) => nodes.find((n) => !edges.some((e) => e.to === n.id && e.to_port !== 'loop'))

/** Линейные последовательности текущего уровня. Развилки, слияния, циклы и группы остаются границами. */
export function linearSequences(doc: FlowDoc, path: string[], ops: Ops): string[][] {
  const { nodes, edges } = levelOf(doc, path)
  const byId = new Map(nodes.map((node) => [node.id, node]))
  const incoming = new Map<string, FlowEdge[]>()
  const outgoing = new Map<string, FlowEdge[]>()
  for (const edge of edges) {
    incoming.set(edge.to, [...(incoming.get(edge.to) ?? []), edge])
    outgoing.set(edge.from, [...(outgoing.get(edge.from) ?? []), edge])
  }
  const simple = new Set(nodes.filter((node) => {
    const op = ops.get(node.op)
    const outputs = op ? outputsOf(node, op) : []
    return node.op !== 'start' && node.op !== 'control.group' && !node.nodes
      && op?.inputs.length === 1 && op.inputs[0] === 'in' && outputs.length === 1 && outputs[0] === 'next'
      && (incoming.get(node.id)?.length ?? 0) <= 1 && (outgoing.get(node.id)?.length ?? 0) <= 1
  }).map((node) => node.id))
  const next = new Map<string, string>()
  const previous = new Map<string, string>()
  for (const edge of edges) {
    if (simple.has(edge.from) && simple.has(edge.to) && portOf(edge) === 'next' && (edge.to_port ?? 'in') === 'in') {
      next.set(edge.from, edge.to)
      previous.set(edge.to, edge.from)
    }
  }
  const result: string[][] = []
  const seen = new Set<string>()
  for (const node of nodes) {
    if (!simple.has(node.id) || previous.has(node.id)) continue
    const chain: string[] = []
    for (let id: string | undefined = node.id; id && !seen.has(id); id = next.get(id)) {
      seen.add(id)
      chain.push(id)
    }
    // Нельзя поставить границу группы перед wait.request: запрос начинает отслеживаться до предыдущего действия.
    while (chain.length && byId.get(chain[0]!)?.op === 'wait.request' && incoming.has(chain[0]!)) chain.shift()
    while (chain.length && outgoing.get(chain.at(-1)!)?.some((edge) => byId.get(edge.to)?.op === 'wait.request')) chain.pop()
    if (chain.length > 1) result.push(chain)
  }
  return result
}

/** Правка уровня `path` на копии; путь не найден — `null`. */
function editLevel(doc: FlowDoc, path: string[], fn: (h: { nodes: FlowNode[]; edges: FlowEdge[] }) => void): FlowDoc | null {
  const next = clone(doc)
  let h: { nodes?: FlowNode[]; edges?: FlowEdge[] } | undefined = next
  for (const id of path) h = h?.nodes?.find((n) => n.id === id)
  if (!h) return null
  const level = { nodes: h.nodes ?? [], edges: h.edges ?? [] }
  fn(level)
  h.nodes = level.nodes
  if (level.edges.length > 0 || h.edges) h.edges = level.edges
  return next
}

/**
 * Выделенная цепочка с одним входом и одним выходом становится группой на месте входа.
 * Иначе (Start, два входа или два выхода, внешний вход не в узел входа, выход не из одного порта или `error`) — `not_chain`.
 * Граница группы между действием и «Завершением запроса» — `before_wait`.
 */
export function groupItems(doc: FlowDoc, path: string[], ids: string[], ops: Ops): { doc: FlowDoc; id: string } | { error: 'not_chain' | 'before_wait' } {
  const level = levelOf(doc, path)
  const chosen = level.nodes.filter((n) => ids.includes(n.id))
  const inside = new Set(chosen.map((n) => n.id))
  if (chosen.length === 0 || chosen.some((n) => n.op === 'start')) return { error: 'not_chain' }
  const inner = level.edges.filter((e) => inside.has(e.from) && inside.has(e.to))
  const entries = chosen.filter((n) => !inner.some((e) => e.to === n.id && e.to_port !== 'loop'))
  // Выход — как в `one_chain` ядра: ровно один по старому правилу (узел без исходящих связей) или по новому (тупик
  // или цикл/New Browser со свободным `next`, кроме узлов тела цикла — их тупик кончает итерацию).
  const out = (n: FlowNode, port?: string) => inner.some((e) => e.from === n.id && (port === undefined || portOf(e) === port))
  const looped = (n: FlowNode) => !!ops.get(n.op)?.outputs.includes('body')
  const bodies = new Set(chosen.filter(looped).flatMap((n) => [...bodyOf(inner, n.id)]))
  const exits = chosen.filter((n) => !bodies.has(n.id) && (!out(n) || (looped(n) && !out(n, 'next'))))
  const deadEnds = chosen.filter((n) => !out(n))
  if (entries.length !== 1 || (deadEnds.length !== 1 && exits.length !== 1)) return { error: 'not_chain' }
  const entry = entries[0]!
  const incoming = level.edges.filter((e) => !inside.has(e.from) && inside.has(e.to))
  const outgoing = level.edges.filter((e) => inside.has(e.from) && !inside.has(e.to))
  if (incoming.some((e) => e.to !== entry.id || (e.to_port ?? 'in') !== 'in')) return { error: 'not_chain' }
  if (new Set(outgoing.map((e) => `${e.from}:${portOf(e)}`)).size > 1 || outgoing.some((e) => portOf(e) === 'error')) return { error: 'not_chain' }
  // Ядро идёт по цепочке группы до любого тупика и выходит через `next`: пока наружу ведёт связь,
  // все остальные выходы выделенных узлов должны вести внутрь, иначе обход изменится.
  if (outgoing.length > 0) {
    const exit = outgoing[0]!
    for (const n of chosen) {
      const op = ops.get(n.op)
      const ports = op ? outputsOf(n, op) : []
      for (const port of ports) {
        if (n.id === exit.from && port === portOf(exit)) continue
        if (!inner.some((e) => e.from === n.id && portOf(e) === port)) return { error: 'not_chain' }
      }
    }
  }
  // Сеть для «Завершения запроса» ядро включает перед действием, за которым он идёт (`arms_wait`), но не перед
  // группой: граница группы между ними — запрос, вызванный действием, ожидание пропустит.
  const opOf = (id: string) => level.nodes.find((n) => n.id === id)?.op
  if (outgoing.some((e) => opOf(e.to) === 'wait.request') || (incoming.length > 0 && entry.op === 'wait.request')) return { error: 'before_wait' }
  const id = newId(doc)
  const group: FlowNode = { id, op: 'control.group', pos: entry.pos, nodes: chosen, edges: inner }
  const next = editLevel(doc, path, (h) => {
    h.nodes = [...h.nodes.filter((n) => !inside.has(n.id)), group]
    h.edges = [
      ...h.edges.filter((e) => !inside.has(e.from) && !inside.has(e.to)),
      ...incoming.map((e) => ({ ...e, port: portOf(e), to: id, to_port: 'in' })),
      ...outgoing.map((e) => ({ from: id, port: 'next', to: e.to, to_port: e.to_port ?? 'in' })),
    ]
  })
  return next ? { doc: next, id } : { error: 'not_chain' }
}

/**
 * Узлы группы встают на уровень группы (левый верхний угол — в её позиции), связи внутри сохраняются.
 * Вход группы идёт в её первый узел; `next` — из единственного узла со свободным основным выходом (иначе не ставится);
 * связь `error` снимается — `droppedError`.
 */
export function ungroup(doc: FlowDoc, path: string[], groupId: string, ops: Ops): { doc: FlowDoc; droppedError: boolean } {
  const group = levelOf(doc, path).nodes.find((n) => n.id === groupId && n.op === 'control.group')
  if (!group) return { doc, droppedError: false }
  const nodes = clone(group.nodes ?? [])
  // Политика и отключение группы действовали на все её узлы: теперь каждый несёт их сам.
  for (const n of nodes) {
    if (group.on_error && !n.on_error) n.on_error = group.on_error
    if (group.disabled && !n.disabled) n.disabled = true
  }
  const edges = group.edges ?? []
  const entry = entryOf(nodes, edges) ?? nodes[0]
  const free = nodes.filter((n) => {
    const op = ops.get(n.op)
    const port = mainExit(op)
    return !!op && outputsOf(n, op).includes(port) && !edges.some((e) => e.from === n.id && portOf(e) === port)
  })
  const [gx, gy] = group.pos ?? [0, 0]
  const dx = nodes.length === 0 ? 0 : gx - Math.min(...nodes.map((n) => n.pos?.[0] ?? 0))
  const dy = nodes.length === 0 ? 0 : gy - Math.min(...nodes.map((n) => n.pos?.[1] ?? 0))
  for (const n of nodes) if (n.pos) n.pos = [n.pos[0] + dx, n.pos[1] + dy]
  let droppedError = false
  const next = editLevel(doc, path, (h) => {
    const outer = h.edges.filter((e) => e.from !== groupId || portOf(e) === 'next')
    droppedError = outer.length !== h.edges.length
    const rest: FlowEdge[] = []
    for (const e of outer) {
      if (e.to === groupId) {
        if (entry) rest.push({ ...e, to: entry.id, to_port: 'in' })
        else for (const o of outer.filter((x) => x.from === groupId)) rest.push({ ...e, to: o.to, to_port: o.to_port ?? 'in' })
      } else if (e.from === groupId) {
        if (free.length === 1) rest.push({ ...e, from: free[0]!.id, port: mainExit(ops.get(free[0]!.op)) })
      } else rest.push(e)
    }
    h.nodes = [...h.nodes.filter((n) => n.id !== groupId), ...nodes]
    h.edges = [...rest, ...clone(edges)]
  })
  return { doc: next ?? doc, droppedError }
}

/** Буфер окна: живёт в модуле, переживает уход из редактора. */
const clip = ref<Clip | null>(null)
export const useFlowClipboard = () => clip
