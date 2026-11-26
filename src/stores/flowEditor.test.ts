import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { FlowDoc } from '../api/types'
import { i18n } from '../i18n'
import { align } from '../lib/flowEdit'
import { useFlowClipboard } from '../lib/flowClip'
import { notify } from '../lib/notify'
import { testCatalog } from '../views/automation/editor/testOps'

const api = vi.hoisted(() => ({ opsCatalog: vi.fn(), getFlow: vi.fn(), validateFlow: vi.fn(), saveFlow: vi.fn() }))
vi.mock('../api/core', () => ({ core: api }))
import { useFlowEditorStore } from './flowEditor'

const rename = (name: string) => (doc: FlowDoc) => ({ ...doc, name })

beforeEach(() => {
  vi.useFakeTimers()
  for (const m of Object.values(api)) m.mockReset()
  api.opsCatalog.mockResolvedValue({ ops: [], system: [] })
  api.validateFlow.mockResolvedValue({ errors: [], warnings: [] })
})
afterEach(() => vi.useRealTimers())

describe('flowEditor store', () => {
  it('дублирование сохраняет буфер, копирует внутренние связи и отменяется одной правкой', async () => {
    const s = useFlowEditorStore()
    const original: FlowDoc = { format: 'ombra-flow/1', name: 't', nodes: [
      { id: 's', op: 'start' }, { id: 'a', op: 'web.click', pos: [0, 100] }, { id: 'b', op: 'wait.time', pos: [0, 200] },
    ], edges: [{ from: 's', to: 'a' }, { from: 'a', to: 'b' }] }
    s.doc = original
    s.copy(['a'])
    const before = JSON.stringify(useFlowClipboard().value)
    s.duplicate(['a', 'b'])
    await vi.advanceTimersByTimeAsync(0)
    expect(s.doc!.nodes).toHaveLength(5)
    expect(s.selection).toHaveLength(2)
    const [a, b] = s.selection
    expect(s.doc!.edges).toContainEqual({ from: a, to: b })
    expect(JSON.stringify(useFlowClipboard().value)).toBe(before)
    s.undo()
    expect(s.doc).toEqual(original)
    expect(s.canUndo).toBe(false)
    s.redo()
    expect(s.doc!.nodes).toHaveLength(5)
  })
  it('history coalesces field edits and keeps 100 steps', async () => {
    const s = useFlowEditorStore()
    await s.load(null)
    for (let i = 0; i < 5; i++) s.apply(rename(`a${i}`), 'name')
    s.undo()
    expect(s.doc!.name).toBe('Новый сценарий')
    expect(s.canUndo).toBe(false)
    for (let i = 0; i < 150; i++) s.apply(rename(`b${i}`))
    let steps = 0
    while (s.canUndo) {
      s.undo()
      steps++
    }
    expect(steps).toBe(100)
  })

  it('the new flow name follows the interface language', async () => {
    const s = useFlowEditorStore()
    i18n.global.locale.value = 'en'
    try {
      await s.load(null)
    } finally {
      i18n.global.locale.value = 'ru'
    }
    expect(s.doc!.name).toBe('New workflow')
  })

  it('undo and redo restore documents', async () => {
    const s = useFlowEditorStore()
    await s.load(null)
    s.apply(rename('one'))
    s.apply(rename('two'))
    s.undo()
    expect(s.doc!.name).toBe('one')
    s.redo()
    expect(s.doc!.name).toBe('two')
    expect(s.canRedo).toBe(false)
  })

  it('a change that leaves the doc identical adds no step and keeps it clean', async () => {
    const s = useFlowEditorStore()
    await s.load(null)
    s.apply((doc) => JSON.parse(JSON.stringify(doc)) as FlowDoc)
    s.apply((doc) => align(doc, [], new Map()))
    expect(s.canUndo).toBe(false)
    expect(s.dirty).toBe(false)
  })

  it('dirty follows the saved document', async () => {
    const s = useFlowEditorStore()
    await s.load(null)
    expect(s.dirty).toBe(false)
    s.apply(rename('x'))
    expect(s.dirty).toBe(true)
    s.undo()
    expect(s.dirty).toBe(false)
  })

  it('save of a new flow returns its id and clears dirty', async () => {
    api.saveFlow.mockResolvedValue({ id: 7 })
    const s = useFlowEditorStore()
    await s.load(null)
    s.apply(rename('x'))
    expect(await s.save()).toBe(7)
    expect(api.saveFlow).toHaveBeenCalledWith(null, expect.objectContaining({ name: 'x' }))
    expect(s.id).toBe(7)
    expect(s.dirty).toBe(false)
  })

  it('typing after a save starts a new undo step', async () => {
    api.saveFlow.mockResolvedValue({ id: 3 })
    const s = useFlowEditorStore()
    await s.load(null)
    s.apply(rename('a'), 'name')
    await s.save()
    s.apply(rename('ab'), 'name')
    s.undo()
    expect(s.doc!.name).toBe('a')
    expect(s.dirty).toBe(false)
  })

  it('an edit during a pending save keeps dirty', async () => {
    let done!: (r: unknown) => void
    api.saveFlow.mockImplementationOnce(() => new Promise((resolve) => (done = resolve)))
    const s = useFlowEditorStore()
    await s.load(null)
    s.apply(rename('x'))
    const saving = s.save()
    s.apply(rename('y'))
    done({ id: 3 })
    await saving
    expect(s.dirty).toBe(true)
  })

  it('a save from the previous session does not touch the new one', async () => {
    let done!: (r: unknown) => void
    api.getFlow.mockResolvedValue({ format: 'ombra-flow/1', name: 'old', nodes: [{ id: 'n1', op: 'start', pos: [0, 0] }] })
    api.saveFlow.mockImplementationOnce(() => new Promise((resolve) => (done = resolve)))
    const s = useFlowEditorStore()
    await s.load(1)
    s.apply(rename('x'))
    const saving = s.save()
    await s.load(null)
    done({ id: 1 })
    await saving
    expect(s.id).toBeNull()
    expect(s.doc!.name).toBe('Новый сценарий')
    expect(s.dirty).toBe(false)
    api.saveFlow.mockResolvedValue({ id: 2 })
    await s.save()
    expect(api.saveFlow).toHaveBeenLastCalledWith(null, expect.objectContaining({ name: 'Новый сценарий' }))
  })

  it('a save re-runs validation and its report replaces the old one', async () => {
    // Граф вызовов меняется только записью: рекурсия A→B→A видна после сохранения A.
    api.saveFlow.mockResolvedValue({ id: 5 })
    const s = useFlowEditorStore()
    await s.load(null)
    await vi.advanceTimersByTimeAsync(400)
    expect(api.validateFlow).toHaveBeenCalledTimes(1)
    api.validateFlow.mockResolvedValueOnce({ errors: [{ node: null, field: null, message: 'цикл' }], warnings: [] })
    await s.save()
    await vi.advanceTimersByTimeAsync(400)
    expect(api.validateFlow).toHaveBeenCalledTimes(2)
    expect(s.report.errors.map((e) => e.message)).toEqual(['цикл'])
  })

  it('a late flow answer does not replace a newer load', async () => {
    let late!: (r: unknown) => void
    api.getFlow.mockImplementationOnce(() => new Promise((resolve) => (late = resolve)))
    const s = useFlowEditorStore()
    const first = s.load(1)
    await vi.advanceTimersByTimeAsync(0)
    await s.load(null)
    late({ format: 'ombra-flow/1', name: 'old', nodes: [{ id: 'n1', op: 'start', pos: [0, 0] }] })
    await first
    expect(s.id).toBeNull()
    expect(s.doc!.name).toBe('Новый сценарий')
  })

  it('a flow without positions opens laid out, clean and without an undo step', async () => {
    // Как tools/flows/examples/login-csv.json: узлы без pos, цепочка связей; плюс группа без pos внутри.
    const chain = ['start', 'open', 'user', 'secret', 'go', 'wait', 'hello', 'shot', 'out']
    api.getFlow.mockResolvedValue({
      format: 'ombra-flow/1',
      name: 'Вход из CSV',
      nodes: [
        { id: 'start', op: 'start' },
        ...chain.slice(1).map((id) => ({ id, op: 'web.click' })),
        { id: 'g', op: 'control.group', nodes: [{ id: 'g1', op: 'web.click' }, { id: 'g2', op: 'web.click' }], edges: [{ from: 'g1', to: 'g2' }] },
      ],
      edges: chain.slice(1).map((id, i) => ({ from: chain[i]!, to: id })),
    })
    const s = useFlowEditorStore()
    await s.load(1)
    const all = [...s.doc!.nodes, ...s.doc!.nodes.at(-1)!.nodes!]
    expect(all.every((n) => n.pos !== undefined)).toBe(true)
    const x = chain.map((id) => s.doc!.nodes.find((n) => n.id === id)!.pos![0])
    expect(x).toEqual([...x].sort((a, b) => a - b))
    expect(new Set(x).size).toBe(chain.length)
    expect(s.dirty).toBe(false)
    expect(s.canUndo).toBe(false)
  })

  it('stale validation answers are ignored', async () => {
    let first!: (r: unknown) => void
    api.validateFlow.mockImplementationOnce(() => new Promise((resolve) => (first = resolve)))
    const s = useFlowEditorStore()
    await s.load(null)
    s.apply(rename('a'))
    await vi.advanceTimersByTimeAsync(400)
    s.apply(rename('b'))
    first({ errors: [{ node: null, field: null, message: 'old' }], warnings: [] })
    await vi.advanceTimersByTimeAsync(0)
    expect(s.report.errors).toEqual([])
  })

  it('grouping right before Wait for request is refused with its own notice and no undo step', () => {
    const s = useFlowEditorStore()
    const op = (id: string) => ({ id, adspower: id, group: 'wait', inputs: ['in'], outputs: ['next'], fields: [] })
    s.catalog = { ...testCatalog, ops: [...testCatalog.ops, op('wait.request')] }
    const doc: FlowDoc = {
      format: 'ombra-flow/1', name: 't',
      nodes: [{ id: 's', op: 'start' }, { id: 'a', op: 'web.click' }, { id: 'b', op: 'web.click' }, { id: 'wr', op: 'wait.request' }],
      edges: [{ from: 's', to: 'a' }, { from: 'a', to: 'b' }, { from: 'b', to: 'wr' }],
    }
    s.doc = doc
    const warn = vi.spyOn(notify, 'warning').mockImplementation(() => {})
    s.group(['a', 'b'])
    expect(warn).toHaveBeenCalledWith('Сохраните блок «Завершение запроса» сразу после действия, ответ на которое он ожидает.')
    expect(s.doc).toEqual(structuredClone(doc))
    expect(s.canUndo).toBe(false)
    warn.mockRestore()
  })
})
