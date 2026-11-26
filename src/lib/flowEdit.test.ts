import { describe, expect, it } from 'vitest'
import type { FlowDoc, OpInfo, OpsCatalog, Report } from '../api/types'
import { addNode, align, connect, edgeKey, inBrowserBody, issuesByNode, layoutMissing, levelOf, moveNodes, newId, nodeWidth, NODE_H, NODE_W, outputsOf, removeItems, START_NODE_SIZE, updateNode, variablesFor } from './flowEdit'

const op = (id: string, inputs: string[], outputs: string[], fields: OpInfo['fields'] = []): OpInfo => ({ id, adspower: '', group: 'x', inputs, outputs, fields })
const field = (name: string, kind: string, def: unknown = null) => ({ name, kind, options: [], required: false, default: def })
const catalog: OpsCatalog = {
  system: ['profile_id', 'serial'],
  ops: [
    op('start', [], ['next']),
    op('web.open', ['in'], ['next'], [field('url', 'text', 'about:blank'), field('save_to', 'var_out'), field('x', 'text')]),
    op('control.if', ['in'], ['true', 'false']),
    op('control.loop', ['in', 'loop'], ['body', 'next']),
    op('control.group', ['in'], ['next']),
    op('control.quit', ['in'], []),
    op('proc.vars', ['in'], ['next']),
    op('control.for_times', ['in', 'loop'], ['body', 'next'], [field('index_to', 'var_out', 'for_times_index')]),
  ],
}
const ops = new Map(catalog.ops.map((o) => [o.id, o]))
const o = (id: string) => ops.get(id)!

const base = (): FlowDoc => ({
  format: 'ombra-flow/1',
  name: 't',
  variables: [{ name: 'q', type: 'string', default: '', description: '' }],
  nodes: [
    { id: 'n1', op: 'start' },
    { id: 'n2', op: 'web.open', config: { save_to: 'page' } },
    { id: 'n3', op: 'control.loop' },
  ],
  edges: [{ from: 'n1', to: 'n2' }],
})

describe('flowEdit', () => {
  it('a fresh group can be entered and edited', () => {
    const { doc, id } = addNode(base(), [], o('control.group'), [0, 0], undefined, ops)
    expect(levelOf(doc, [id]).nodes).toEqual([])
    const inner = addNode(doc, [id], o('web.open'), [0, 0], undefined, ops)
    expect(inner.doc.nodes.find((n) => n.id === id)?.nodes?.map((n) => n.id)).toEqual([inner.id])
  })

  it('connect replaces the edge from a busy output', () => {
    const doc = connect(base(), [], 'n1', 'next', 'n3', 'in', ops)
    expect(doc.edges).toEqual([{ from: 'n1', port: 'next', to: 'n3', to_port: 'in' }])
  })

  it('loop input only on loop shapes', () => {
    const doc = base()
    expect(connect(doc, [], 'n2', 'next', 'n2', 'in', ops)).toBe(doc)
    expect(connect(doc, [], 'n1', 'next', 'n2', 'loop', ops)).toBe(doc)
    expect(connect(doc, [], 'n2', 'next', 'n3', 'loop', ops).edges).toHaveLength(2)
  })

  it('connect ignores ports the nodes do not have and keeps the old edge', () => {
    const doc = base()
    // Выход → выход и вход → вход (свободный режим vue-flow), error без «Ветки ошибки».
    expect(connect(doc, [], 'n1', 'next', 'n2', 'next', ops)).toBe(doc)
    expect(connect(doc, [], 'n2', 'in', 'n3', 'in', ops)).toBe(doc)
    expect(connect(doc, [], 'n2', 'error', 'n3', 'in', ops)).toBe(doc)
    expect(doc.edges).toEqual([{ from: 'n1', to: 'n2' }])
    const branch = { ...doc, nodes: doc.nodes.map((n) => (n.id === 'n2' ? { ...n, on_error: 'branch' as const } : n)) }
    expect(connect(branch, [], 'n2', 'error', 'n3', 'in', ops).edges).toContainEqual({ from: 'n2', port: 'error', to: 'n3', to_port: 'in' })
  })

  it('leaving the error branch policy drops the error edge', () => {
    const doc = base()
    doc.nodes[1]!.on_error = 'branch'
    doc.edges!.push({ from: 'n2', port: 'error', to: 'n3', to_port: 'in' })
    expect(updateNode(doc, [], 'n2', { description: 'x' }).edges).toHaveLength(2)
    const skip = updateNode(doc, [], 'n2', { on_error: 'skip' })
    expect(skip.edges).toEqual([{ from: 'n1', to: 'n2' }])
    expect(updateNode(doc, [], 'n2', { on_error: undefined }).edges).toEqual([{ from: 'n1', to: 'n2' }])
  })

  it('remove drops the node edges and keeps start', () => {
    const doc = removeItems(base(), [], ['n1', 'n2'], [])
    expect(doc.nodes.map((n) => n.id)).toEqual(['n1', 'n3'])
    expect(doc.edges).toEqual([])
    const withEdge = removeItems(base(), [], [], [edgeKey({ from: 'n1', to: 'n2' })])
    expect(withEdge.edges).toEqual([])
  })

  it('addNode links from the free main exit of the selected node', () => {
    const free = addNode(base(), [], o('web.open'), [1, 2], 'n2', ops)
    expect(free.id).toBe('n4')
    expect(free.doc.edges).toContainEqual({ from: 'n2', port: 'next', to: 'n4', to_port: 'in' })
    expect(free.doc.nodes.find((n) => n.id === 'n4')?.config).toEqual({ url: 'about:blank' })
    const busy = addNode(base(), [], o('web.open'), [1, 2], 'n1', ops)
    expect(busy.doc.edges).toHaveLength(1)
    const quit = addNode({ ...base(), nodes: [...base().nodes, { id: 'n4', op: 'control.quit' }] }, [], o('web.open'), [0, 0], 'n4', ops)
    expect(quit.doc.edges).toHaveLength(1)
    const doc = base()
    doc.nodes.push({ id: 'n4', op: 'control.if' })
    const branch = addNode(doc, [], o('web.open'), [0, 0], 'n4', ops)
    expect(branch.doc.edges).toContainEqual({ from: 'n4', port: 'false', to: 'n5', to_port: 'in' })
  })

  it('ids are unique across groups', () => {
    const doc = base()
    doc.nodes.push({ id: 'n4', op: 'control.group', nodes: [{ id: 'n5', op: 'web.open' }], edges: [] })
    expect(newId(doc)).toBe('n6')
    expect(levelOf(doc, ['n4']).nodes.map((n) => n.id)).toEqual(['n5'])
    const added = addNode(doc, ['n4'], o('web.open'), [0, 0], undefined, ops)
    expect(added.id).toBe('n6')
    expect(doc.nodes[3]!.nodes).toHaveLength(1)
  })

  it('issues bubble to the containing group', () => {
    const doc = base()
    doc.nodes.push({ id: 'n4', op: 'control.group', nodes: [{ id: 'n5', op: 'control.group', nodes: [{ id: 'n6', op: 'web.open' }] }] })
    const report: Report = { errors: [{ node: 'n6', field: 'url', message: 'x' }, { node: null, field: null, message: 'y' }], warnings: [] }
    const map = issuesByNode(report.errors, doc)
    expect([...map.keys()].sort()).toEqual(['n4', 'n5', 'n6'])
    expect(map.get('n4')).toHaveLength(1)
  })

  it('variablesFor collects start, outputs, manage rows and system', () => {
    const doc = base()
    doc.nodes.push({ id: 'n4', op: 'control.group', nodes: [{ id: 'n5', op: 'proc.vars', config: { vars: [{ name: 'row', type: 'string', value: '' }] } }] })
    // Узел без поля в конфиге (импорт) — имя по умолчанию из реестра.
    doc.nodes.push({ id: 'n6', op: 'control.for_times' })
    expect(variablesFor(doc, catalog)).toEqual(['for_times_index', 'page', 'profile_id', 'q', 'row', 'serial'])
  })

  it('align gives every node a position, left to right', () => {
    const doc = connect(base(), [], 'n2', 'next', 'n3', 'in', ops)
    const out = align(doc, [], ops)
    expect(out.nodes.every((n) => n.pos !== undefined)).toBe(true)
    const x = (id: string) => out.nodes.find((n) => n.id === id)!.pos![0]
    expect(x('n1') + START_NODE_SIZE).toBeLessThan(x('n2'))
    expect(x('n2') + NODE_W).toBeLessThan(x('n3'))
  })

  it('align keeps an existing Start pinned and uses its square width for spacing', () => {
    const doc = connect(base(), [], 'n2', 'next', 'n3', 'in', ops)
    doc.nodes[0]!.pos = [300, 150]
    const out = align(doc, [], ops)
    expect(out.nodes[0]!.pos).toEqual([300, 150])
    expect(out.nodes[1]!.pos![0]).toBe(300 + nodeWidth(out.nodes[0]!) + 40)
    expect(align(out, [], ops)).toEqual(out)
  })

  it('moving a selection keeps Start in place while other nodes move', () => {
    const doc = base()
    doc.nodes[0]!.pos = [10, 20]
    const out = moveNodes(doc, [], new Map([['n1', [60, 80]], ['n2', [200, 100]]]))
    expect(out.nodes[0]!.pos).toEqual([10, 20])
    expect(out.nodes[1]!.pos).toEqual([200, 100])
  })

  it('layoutMissing keeps placed nodes and puts the rest in a row below them', () => {
    const doc: FlowDoc = {
      format: 'ombra-flow/1',
      name: 't',
      nodes: [
        { id: 'a', op: 'start', pos: [100, 50] },
        { id: 'b', op: 'web.open', pos: [400, 200] },
        { id: 'c', op: 'web.open' },
        { id: 'd', op: 'control.group', nodes: [{ id: 'e', op: 'web.open' }, { id: 'f', op: 'web.open' }], edges: [{ from: 'e', to: 'f' }] },
      ],
      edges: [{ from: 'a', to: 'c' }],
    }
    const out = layoutMissing(doc)
    const pos = (id: string) => out.nodes.find((n) => n.id === id)!.pos!
    expect(pos('a')).toEqual([100, 50])
    expect(pos('b')).toEqual([400, 200])
    const [c, d] = [pos('c'), pos('d')]
    expect(c[1]).toBeGreaterThanOrEqual(200 + NODE_H)
    expect(d[1]).toBe(c[1])
    expect(Math.abs(d[0] - c[0])).toBeGreaterThanOrEqual(NODE_W)
    // Уровень группы без единой позиции — раскладка dagre слева направо.
    const inner = out.nodes[3]!.nodes!
    expect(inner[0]!.pos![0]).toBeLessThan(inner[1]!.pos![0])
  })

  it('outputs gain error with the branch policy', () => {
    expect(outputsOf({ id: 'a', op: 'web.open' }, o('web.open'))).toEqual(['next'])
    expect(outputsOf({ id: 'a', op: 'web.open', on_error: 'branch' }, o('web.open'))).toEqual(['next', 'error'])
  })
})

describe('inBrowserBody', () => {
  const d: FlowDoc = {
    format: 'ombra-flow/1', name: 't',
    nodes: [
      { id: 's', op: 'start' }, { id: 'b', op: 'control.new_browser' }, { id: 'b1', op: 'web.click' },
      { id: 'g', op: 'control.group', nodes: [{ id: 'x', op: 'web.click' }] },
      { id: 'l', op: 'control.for_times' }, { id: 'l1', op: 'web.click' }, { id: 'after', op: 'web.click' },
    ],
    edges: [
      { from: 's', to: 'b' }, { from: 'b', port: 'body', to: 'b1' }, { from: 'b1', to: 'g' }, { from: 'g', to: 'l' },
      { from: 'l', port: 'body', to: 'l1' }, { from: 'l1', to: 'l', to_port: 'loop' }, { from: 'l', to: 'b', to_port: 'loop' },
      { from: 'b', to: 'after' },
    ],
  }
  it('узел или группа на пути — в теле New Browser своего уровня (как в ядре)', () => {
    expect(inBrowserBody(d, ['b1'])).toBe(true)
    expect(inBrowserBody(d, ['g', 'x'])).toBe(true)
    expect(inBrowserBody(d, ['l1'])).toBe(true)
    expect(inBrowserBody(d, ['b'])).toBe(false)
    expect(inBrowserBody(d, ['after'])).toBe(false)
  })
})
