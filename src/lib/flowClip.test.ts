import { describe, expect, it } from 'vitest'
import type { FlowDoc, FlowEdge } from '../api/types'
import { copyItems, groupItems, linearSequences, pasteClip, setDisabled, ungroup } from './flowClip'
import type { OpInfo } from '../api/types'
import { levelOf } from './flowEdit'
import { testCatalog } from '../views/automation/editor/testOps'

const doc: FlowDoc = {
  format: 'ombra-flow/1', name: 't',
  nodes: [
    { id: 'n1', op: 'start', pos: [0, 0] },
    { id: 'n2', op: 'web.click', pos: [0, 100] },
    { id: 'n3', op: 'web.click', pos: [0, 200] },
    { id: 'g', op: 'control.group', pos: [200, 0], nodes: [
      { id: 'a', op: 'web.click', pos: [0, 0] }, { id: 'b', op: 'control.group', pos: [0, 100], nodes: [{ id: 'c', op: 'web.click', pos: [0, 0] }] },
    ], edges: [{ from: 'a', to: 'b' }] },
  ],
  edges: [{ from: 'n1', to: 'n2' }, { from: 'n2', to: 'n3' }, { from: 'n3', to: 'g' }],
}

describe('автоматический поиск последовательностей', () => {
  const ops = new Map(testCatalog.ops.map((op) => [op.id, op]))
  const chain = (ids: string[]): FlowDoc => ({ format: 'ombra-flow/1', name: 't',
    nodes: [{ id: 's', op: 'start' }, ...ids.map((id) => ({ id, op: 'web.click' }))],
    edges: ['s', ...ids].slice(1).map((to, i) => ({ from: ['s', ...ids][i]!, to })) })

  it('берёт всю цепочку вокруг любого блока, без Start; найденное группируется без изменения связей', () => {
    const original = chain(['a', 'b', 'c'])
    const snapshot = structuredClone(original)
    expect(linearSequences(original, [], ops)).toEqual([['a', 'b', 'c']])
    const result = groupItems(original, [], linearSequences(original, [], ops)[0]!, ops)
    expect('error' in result).toBe(false)
    if ('error' in result) return
    expect(result.doc.edges).toEqual([{ from: 's', port: 'next', to: result.id, to_port: 'in' }])
    expect(result.doc.nodes[1]!.edges).toEqual([{ from: 'a', to: 'b' }, { from: 'b', to: 'c' }])
    expect(original).toEqual(snapshot)
  })

  it('развилки, слияния и ветка ошибки разделяют последовательности, одиночные блоки пропускаются', () => {
    const original = chain(['a', 'b', 'if', 'c', 'd', 'join', 'e', 'f', 'error', 'z'])
    original.nodes.find((n) => n.id === 'if')!.op = 'control.if'
    original.nodes.find((n) => n.id === 'error')!.on_error = 'branch'
    original.edges = original.edges!.filter((e) => e.from !== 'if')
    original.edges.push({ from: 'if', port: 'true', to: 'c' }, { from: 'if', port: 'false', to: 'join' })
    expect(linearSequences(original, [], ops)).toEqual([['a', 'b'], ['c', 'd'], ['e', 'f']])
  })

  it('не включает циклы и группы; внутри группы работает только на текущем уровне', () => {
    const original = chain(['a', 'b', 'g', 'c', 'd'])
    const group = original.nodes.find((n) => n.id === 'g')!
    group.op = 'control.group'
    group.nodes = [{ id: 'x', op: 'web.click' }, { id: 'y', op: 'wait.time' }]
    group.edges = [{ from: 'x', to: 'y' }]
    expect(linearSequences(original, [], ops)).toEqual([['a', 'b'], ['c', 'd']])
    expect(linearSequences(original, ['g'], ops)).toEqual([['x', 'y']])
    const cycle = chain(['a', 'b'])
    cycle.edges = [{ from: 'a', to: 'b' }, { from: 'b', to: 'a' }]
    expect(linearSequences(cycle, [], ops)).toEqual([])
  })

  it('не разрывает действие и отслеживание завершения запроса на границе группы', () => {
    const requestOps = new Map(ops)
    requestOps.set('wait.request', { ...ops.get('wait.time')!, id: 'wait.request' })
    const original = chain(['a', 'b', 'wait', 'c', 'd'])
    original.nodes.find((n) => n.id === 'wait')!.op = 'wait.request'
    expect(linearSequences(original, [], requestOps)).toEqual([['a', 'b', 'wait', 'c', 'd']])
    original.nodes.find((n) => n.id === 'b')!.on_error = 'branch'
    expect(linearSequences(original, [], requestOps)).toEqual([['c', 'd']])
    const ending = chain(['a', 'b', 'c', 'wait1', 'wait2'])
    for (const id of ['wait1', 'wait2']) ending.nodes.find((n) => n.id === id)!.op = 'wait.request'
    ending.nodes.at(-1)!.on_error = 'branch'
    expect(linearSequences(ending, [], requestOps)).toEqual([['a', 'b']])
  })
})

describe('flowClip', () => {
  it('копируются только связи между выбранными, Start — нет', () => {
    const clip = copyItems(doc, [], ['n1', 'n2', 'n3'])!
    expect(clip.nodes.map((n) => n.id)).toEqual(['n2', 'n3'])
    expect(clip.edges).toEqual([{ from: 'n2', to: 'n3' }])
    expect(copyItems(doc, [], ['n1'])).toBeNull()
  })

  it('вставка: новые id везде, связи переназначены, позиции сдвинуты', () => {
    const clip = copyItems(doc, [], ['n2', 'n3', 'g'])!
    const { doc: next, ids } = pasteClip(doc, [], clip)
    expect(ids).toHaveLength(3)
    const all = JSON.stringify(next.nodes.slice(4))
    for (const old of ['"n2"', '"n3"', '"a"', '"b"', '"c"']) expect(all).not.toContain(`"id":${old}`)
    const [x, y, group] = next.nodes.slice(4)
    expect(x!.pos).toEqual([40, 140])
    expect(next.edges).toContainEqual({ from: x!.id, to: y!.id })
    const inner = group!.nodes!
    expect(group!.edges).toEqual([{ from: inner[0]!.id, to: inner[1]!.id }])
    const used = [...next.nodes.map((n) => n.id), ...next.nodes.slice(0, 4).flatMap((n) => n.nodes?.map((m) => m.id) ?? [])]
    expect(new Set(used).size).toBe(used.length)
    expect(inner[1]!.nodes![0]!.id).not.toBe('c')
    expect(doc.nodes).toHaveLength(4)
  })

  it('две вставки подряд не пересекаются', () => {
    const clip = copyItems(doc, [], ['n2'])!
    const one = pasteClip(doc, [], clip)
    const two = pasteClip(one.doc, [], clip)
    const pos = levelOf(two.doc, []).nodes.map((n) => String(n.pos))
    expect(new Set(pos).size).toBe(pos.length)
  })

  it('setDisabled меняет только выбранных', () => {
    const off = setDisabled(doc, [], ['n2', 'n1'], true)
    expect(off.nodes.map((n) => n.disabled === true)).toEqual([false, true, false, false])
    expect(setDisabled(off, [], ['n2'], false).nodes[1]!.disabled).toBeFalsy()
  })
})

describe('вставка вложенных групп', () => {
  const nested: FlowDoc = {
    format: 'ombra-flow/1', name: 't',
    nodes: [
      { id: 'n1', op: 'start', pos: [0, 0] },
      { id: 'g', op: 'control.group', pos: [0, 100], nodes: [
        { id: 'h', op: 'control.group', pos: [0, 0], nodes: [{ id: 'x', op: 'web.click', pos: [0, 0] }, { id: 'y', op: 'web.click', pos: [0, 100] }], edges: [{ from: 'x', to: 'y' }] },
      ] },
    ],
  }
  const edgesOf = (ns: FlowDoc['nodes']): { from: string; to: string }[] => ns.flatMap((n) => [...(n.edges ?? []), ...edgesOf(n.nodes ?? [])])
  const idsOf = (ns: FlowDoc['nodes']): string[] => ns.flatMap((n) => [n.id, ...idsOf(n.nodes ?? [])])

  it('связи на любой глубине получают новые id; вторая вставка не пересекается с первой', () => {
    const clip = copyItems(nested, [], ['g'])!
    const one = pasteClip(nested, [], clip)
    const two = pasteClip(one.doc, [], clip)
    for (const d of [one.doc, two.doc]) {
      const all = idsOf(d.nodes)
      expect(new Set(all).size).toBe(all.length)
      for (const e of edgesOf(d.nodes)) {
        expect(all).toContain(e.from)
        expect(all).toContain(e.to)
      }
    }
    const copy = one.doc.nodes[2]!.nodes![0]!
    expect(copy.edges).toEqual([{ from: copy.nodes![0]!.id, to: copy.nodes![1]!.id }])
    expect(edgesOf(one.doc.nodes).filter((e) => e.from === 'x')).toHaveLength(1)
    expect(edgesOf(two.doc.nodes)).toHaveLength(3)
  })
})

describe('сгруппировать и разгруппировать', () => {
  const ops = new Map<string, OpInfo>([
    ...testCatalog.ops.map((o) => [o.id, o] as [string, OpInfo]),
    ['control.group', { id: 'control.group', adspower: 'Group', group: 'control', inputs: ['in'], outputs: ['next'], fields: [] }],
    ['control.new_browser', { id: 'control.new_browser', adspower: 'New Browser', group: 'control', inputs: ['in', 'loop'], outputs: ['body', 'next'], fields: [] }],
    ['wait.request', { id: 'wait.request', adspower: 'Wait for request', group: 'wait', inputs: ['in'], outputs: ['next'], fields: [] }],
  ])
  const w = (id: string, y: number, extra: object = {}) => ({ id, op: 'web.click', pos: [100, y] as [number, number], ...extra })
  const chain: FlowDoc = {
    format: 'ombra-flow/1', name: 't',
    nodes: [{ id: 's', op: 'start', pos: [0, 0] }, w('p', 50), w('a', 100), w('b', 200), w('c', 300), w('q', 400)],
    edges: [{ from: 's', to: 'p' }, { from: 'p', to: 'a' }, { from: 'a', to: 'b' }, { from: 'b', to: 'c' }, { from: 'c', to: 'q' }],
  }
  const flat = (ns: FlowDoc['nodes']): FlowEdge[] => ns.flatMap((n) => [...(n.edges ?? []), ...flat(n.nodes ?? [])])
  const pairs = (d: FlowDoc): string[] => [...(d.edges ?? []), ...flat(d.nodes)].map((e) => `${e.from}:${e.port ?? 'next'}>${e.to}`)
  const grouped = () => groupItems(chain, [], ['a', 'b', 'c'], ops) as { doc: FlowDoc; id: string }

  it('цепочка с внешними входом и выходом: группа на месте входа', () => {
    const { doc: d, id } = grouped()
    const g = d.nodes.find((n) => n.id === id)!
    expect(g.op).toBe('control.group')
    expect(g.pos).toEqual([100, 100])
    expect(g.nodes!.map((n) => n.id)).toEqual(['a', 'b', 'c'])
    expect(g.edges).toEqual([{ from: 'a', to: 'b' }, { from: 'b', to: 'c' }])
    expect(d.nodes.map((n) => n.id)).toEqual(['s', 'p', 'q', id])
    expect(d.edges).toEqual([
      { from: 's', to: 'p' },
      { from: 'p', port: 'next', to: id, to_port: 'in' },
      { from: id, port: 'next', to: 'q', to_port: 'in' },
    ])
  })

  const refused = (d: FlowDoc, sel: string[]) => {
    expect(groupItems(d, [], sel, ops)).toEqual({ error: 'not_chain' })
  }
  it('отказы', () => {
    refused(chain, ['s', 'p'])
    // два входа: b и c не связаны между собой
    refused({ ...chain, edges: [{ from: 'a', to: 'b' }, { from: 'p', to: 'a' }, { from: 'c', to: 'q' }] }, ['a', 'b', 'c'])
    // выход наружу из двух узлов
    refused({ ...chain, edges: [...chain.edges!, { from: 'a', port: 'next', to: 'q' }] }, ['a', 'b'])
    refused({ ...chain, edges: [{ from: 'p', to: 'a' }, { from: 'a', to: 'b' }, { from: 'b', to: 'q' }, { from: 'a', port: 'next', to: 'c' }] }, ['a', 'b'])
    // вход снаружи не в узел входа
    refused({ ...chain, edges: [...chain.edges!, { from: 'p', to: 'c' }] }, ['a', 'b', 'c'])
    // выход error
    refused({ ...chain, nodes: chain.nodes.map((n) => (n.id === 'c' ? { ...n, on_error: 'branch' as const } : n)), edges: [{ from: 'a', to: 'b' }, { from: 'b', to: 'c' }, { from: 'c', port: 'error', to: 'q' }] }, ['a', 'b', 'c'])
    // ветки true и false If наружу
    refused({ ...chain, nodes: [...chain.nodes, { id: 'i', op: 'control.if', pos: [0, 0] }], edges: [{ from: 'i', port: 'true', to: 'p' }, { from: 'i', port: 'false', to: 'q' }] }, ['i'])
    refused(chain, [])
  })

  it('разгруппировать: сдвиг к позиции группы, связи p → a и c → q', () => {
    const { doc: d, id } = grouped()
    const moved = { ...d, nodes: d.nodes.map((n) => (n.id === id ? { ...n, pos: [500, 600] as [number, number] } : n)) }
    const r = ungroup(moved, [], id, ops)
    expect(r.droppedError).toBe(false)
    expect(r.doc.nodes.filter((n) => ['a', 'b', 'c'].includes(n.id)).map((n) => n.pos)).toEqual([[500, 600], [500, 700], [500, 800]])
    expect(r.doc.nodes.some((n) => n.id === id)).toBe(false)
    expect(pairs(r.doc)).toEqual(expect.arrayContaining(['p:next>a', 'c:next>q', 'a:next>b', 'b:next>c']))
    expect(r.doc.edges).toContainEqual({ from: 'p', port: 'next', to: 'a', to_port: 'in' })
  })

  it('связь error группы снимается', () => {
    const { doc: d, id } = grouped()
    const withError = { ...d, edges: [...d.edges!, { from: id, port: 'error', to: 'p' }] }
    const r = ungroup(withError, [], id, ops)
    expect(r.droppedError).toBe(true)
    expect(pairs(r.doc).some((e) => e.includes(':error>'))).toBe(false)
  })

  it('несколько свободных выходов — связь из next не ставится', () => {
    const { doc: d, id } = grouped()
    const open = { ...d, nodes: d.nodes.map((n) => (n.id === id ? { ...n, edges: [{ from: 'a', to: 'b' }, { from: 'a', port: 'x', to: 'c' }] } : n)) }
    const r = ungroup(open, [], id, ops)
    expect(pairs(r.doc).filter((e) => e.endsWith('>q'))).toEqual([])
  })

  it('сгруппировать и разгруппировать — те же связи', () => {
    const { doc: d, id } = grouped()
    const back = ungroup(d, [], id, ops).doc
    expect(pairs(back).sort()).toEqual(pairs(chain).sort())
  })

  it('выход наружу и другие пути: тупик внутри меняет обход — отказ', () => {
    const base = (nodes: FlowDoc['nodes'], edges: FlowDoc['edges']): FlowDoc => ({ ...chain, nodes: [chain.nodes[0]!, w('p', 50), w('q', 400), ...nodes], edges })
    const iff = { id: 'i', op: 'control.if', pos: [100, 100] as [number, number] }
    // false наружу, true в тупик x
    refused(base([iff, w('x', 200)], [{ from: 'p', to: 'i' }, { from: 'i', port: 'true', to: 'x' }, { from: 'i', port: 'false', to: 'q' }]), ['i', 'x'])
    // пути сходятся в узле выхода
    const merge = base([iff, w('x', 200), w('e', 300)], [{ from: 'p', to: 'i' }, { from: 'i', port: 'true', to: 'x' }, { from: 'x', to: 'e' }, { from: 'i', port: 'false', to: 'e' }, { from: 'e', to: 'q' }])
    expect('error' in groupItems(merge, [], ['i', 'x', 'e'], ops)).toBe(false)
    // «Ветка ошибки» без связи error
    const branchy = base([w('a', 100, { on_error: 'branch' }), w('b', 200)], [{ from: 'p', to: 'a' }, { from: 'a', to: 'b' }, { from: 'b', to: 'q' }])
    refused(branchy, ['a', 'b'])
    // без выхода наружу два тупика — у группы два выхода, ядро её отвергнет
    const open = base([iff, w('x', 200), w('y', 300)], [{ from: 'p', to: 'i' }, { from: 'i', port: 'true', to: 'x' }, { from: 'i', port: 'false', to: 'y' }])
    refused(open, ['i', 'x', 'y'])
  })

  it('цикл в конце выделения группируется: его выход — свободный next', () => {
    const loop = { id: 'l', op: 'control.for_times', pos: [100, 200] as [number, number] }
    const edges = [{ from: 'p', to: 'a' }, { from: 'a', to: 'l' }, { from: 'l', port: 'body', to: 'b' }, { from: 'b', to: 'l', to_port: 'loop' }]
    const looped: FlowDoc = { ...chain, nodes: [chain.nodes[0]!, w('p', 50), w('a', 100), loop, w('b', 300), w('q', 400)], edges: [...edges, { from: 'l', to: 'q' }] }
    const r = groupItems(looped, [], ['a', 'l', 'b'], ops) as { doc: FlowDoc; id: string }
    expect('error' in r).toBe(false)
    expect(r.doc.edges).toContainEqual({ from: r.id, port: 'next', to: 'q', to_port: 'in' })
    // и без связи наружу
    const last: FlowDoc = { ...looped, edges }
    expect('error' in groupItems(last, [], ['a', 'l', 'b'], ops)).toBe(false)
  })

  it('связь наружу в «Завершение запроса» или вход в него снаружи — отказ before_wait', () => {
    const wr = { id: 'wr', op: 'wait.request', pos: [100, 300] as [number, number] }
    const after: FlowDoc = { ...chain, nodes: [chain.nodes[0]!, w('p', 50), w('a', 100), w('b', 200), wr], edges: [{ from: 'p', to: 'a' }, { from: 'a', to: 'b' }, { from: 'b', to: 'wr' }] }
    expect(groupItems(after, [], ['a', 'b'], ops)).toEqual({ error: 'before_wait' })
    // ожидание — вход выделения: действие перед ним остаётся снаружи
    const entry: FlowDoc = { ...chain, nodes: [chain.nodes[0]!, w('p', 50), wr, w('x', 400)], edges: [{ from: 'p', to: 'wr' }, { from: 'wr', to: 'x' }] }
    expect(groupItems(entry, [], ['wr', 'x'], ops)).toEqual({ error: 'before_wait' })
    // действие и ожидание вместе — можно
    expect('error' in groupItems(after, [], ['a', 'b', 'wr'], ops)).toBe(false)
  })

  it('годится старое или новое правило выхода: If → цикл | тупик группируется', () => {
    const iff = { id: 'i', op: 'control.if', pos: [100, 100] as [number, number] }
    const loop = { id: 'l', op: 'control.for_times', pos: [100, 200] as [number, number] }
    const d: FlowDoc = {
      ...chain,
      nodes: [chain.nodes[0]!, w('p', 50), iff, loop, w('b1', 300), w('y', 300)],
      edges: [{ from: 'p', to: 'i' }, { from: 'i', port: 'true', to: 'l' }, { from: 'i', port: 'false', to: 'y' }, { from: 'l', port: 'body', to: 'b1' }, { from: 'b1', to: 'l', to_port: 'loop' }],
    }
    expect('error' in groupItems(d, [], ['i', 'l', 'b1', 'y'], ops)).toBe(false)
  })

  it('тупик в теле цикла — не выход группы: тело без возврата в loop группируется', () => {
    const nb = { id: 'nb', op: 'control.new_browser', pos: [100, 100] as [number, number] }
    const browser: FlowDoc = { ...chain, nodes: [chain.nodes[0]!, w('p', 50), nb, w('i', 200)], edges: [{ from: 'p', to: 'nb' }, { from: 'nb', port: 'body', to: 'i' }] }
    expect('error' in groupItems(browser, [], ['nb', 'i'], ops)).toBe(false)
    const loop = { id: 'l', op: 'control.for_times', pos: [100, 200] as [number, number] }
    const looped: FlowDoc = { ...chain, nodes: [chain.nodes[0]!, w('p', 50), w('a', 100), loop, w('b1', 300)], edges: [{ from: 'p', to: 'a' }, { from: 'a', to: 'l' }, { from: 'l', port: 'body', to: 'b1' }] }
    expect('error' in groupItems(looped, [], ['a', 'l', 'b1'], ops)).toBe(false)
  })

  it('разгруппировать: политика и отключение группы переходят узлам', () => {
    const { doc: d, id } = grouped()
    const pol = { ...d, nodes: d.nodes.map((n) => (n.id === id ? { ...n, on_error: 'skip' as const, disabled: true, nodes: n.nodes!.map((m) => (m.id === 'b' ? { ...m, on_error: 'interrupt' as const } : m)) } : n)) }
    const r = ungroup(pol, [], id, ops).doc.nodes
    expect(['a', 'b', 'c'].map((i) => r.find((n) => n.id === i)!.on_error)).toEqual(['skip', 'interrupt', 'skip'])
    expect(['a', 'b', 'c'].every((i) => r.find((n) => n.id === i)!.disabled === true)).toBe(true)
  })

  it('пустая группа: входящая связь идёт сразу к цели next', () => {
    const d: FlowDoc = { ...chain, nodes: [chain.nodes[0]!, w('p', 50), w('q', 400), { id: 'g', op: 'control.group', pos: [0, 0] }], edges: [{ from: 'p', to: 'g' }, { from: 'g', to: 'q' }] }
    const r = ungroup(d, [], 'g', ops).doc
    expect(r.edges).toEqual([{ from: 'p', to: 'q', to_port: 'in' }])
  })
})
