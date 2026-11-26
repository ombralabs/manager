import { flushPromises } from '@vue/test-utils'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { defineComponent, h } from 'vue'
import type { DebugEvent, FlowDoc, JournalEntry } from '../../../api/types'
import { mountWithApp } from '../../../test/mount'

const mocks = vi.hoisted(() => ({
  api: { debugFlow: vi.fn(), stopDebug: vi.fn(), listProfiles: vi.fn(), validateFlow: vi.fn(), getFlow: vi.fn() },
  handler: { current: null as ((e: DebugEvent) => void) | null },
}))
vi.mock('../../../api/core', () => ({
  core: mocks.api,
  events: { onFlowDebug: vi.fn(async (h: (e: DebugEvent) => void) => { mocks.handler.current = h; return () => {} }) },
}))

import { useFlowDebugStore } from '../../../stores/flowDebug'
import { useFlowEditorStore } from '../../../stores/flowEditor'
import DebugPanel from './DebugPanel.vue'
import EditorToolbar from './EditorToolbar.vue'
import FlowCanvas from './FlowCanvas.vue'
import { testCatalog } from './testOps'

const doc: FlowDoc = {
  format: 'ombra-flow/1', name: 't', edges: [],
  nodes: [
    { id: 'n1', op: 'start' },
    { id: 'n2', op: 'web.click', description: 'Нажать вход', pos: [0, 100] },
    { id: 'g', op: 'control.group', pos: [0, 200], nodes: [{ id: 'x', op: 'wait.time', pos: [0, 0] }] },
  ],
}
const ev = (e: Partial<DebugEvent>): DebugEvent => ({ kind: 'flow_debug', run_id: 'r1', phase: 'entry', node: null, entry: null, status: null, error: null, ...e })
const entry = (e: Partial<JournalEntry>): JournalEntry => ({ ts_ms: 1, status: 'ok', ms: 5, ...e })

async function mountPanel() {
  const editor = useFlowEditorStore()
  editor.catalog = testCatalog
  editor.doc = structuredClone(doc)
  const debug = useFlowDebugStore()
  debug.open = true
  debug.serial = 7
  const wrapper = mountWithApp(DebugPanel, { attachTo: document.body })
  await flushPromises()
  return { wrapper, editor, debug }
}
const btn = (w: Awaited<ReturnType<typeof mountPanel>>['wrapper'], text: string) => w.findAll('button').find((b) => b.text() === text)!

beforeEach(() => {
  document.body.innerHTML = ''
  for (const m of Object.values(mocks.api)) m.mockReset()
  mocks.api.debugFlow.mockResolvedValue('r1')
  mocks.api.stopDebug.mockResolvedValue(true)
  mocks.api.validateFlow.mockResolvedValue({ errors: [], warnings: [] })
  mocks.api.listProfiles.mockResolvedValue({ rows: [{ serial: 7, name: 'Рабочий' }], total: 1, running: 0 })
  mocks.handler.current = null
})

describe('панель отладки', () => {
  it('«С начала» запускает отладку всего процесса', async () => {
    const { wrapper } = await mountPanel()
    await btn(wrapper, 'С начала').trigger('click')
    await flushPromises()
    expect(mocks.api.debugFlow).toHaveBeenCalledWith(doc, 7, null)
  })

  it('«С этого блока» недоступна без выбора, у Start и при нескольких узлах', async () => {
    const { wrapper, editor } = await mountPanel()
    const from = () => btn(wrapper, 'С этого блока').attributes('disabled')
    expect(from()).toBeDefined()
    editor.selected = 'n1'
    await flushPromises()
    expect(from()).toBeDefined()
    editor.selected = null // несколько узлов на холсте — единственного выбранного нет
    await flushPromises()
    expect(from()).toBeDefined()
    editor.selected = 'n2'
    await flushPromises()
    expect(from()).toBeUndefined()
  })

  it('«С этого блока» недоступна в теле New Browser, с подсказкой', async () => {
    const { wrapper, editor } = await mountPanel()
    editor.doc = {
      ...structuredClone(doc),
      nodes: [...structuredClone(doc).nodes, { id: 'b', op: 'control.new_browser', pos: [300, 0] }, { id: 'b1', op: 'web.click', pos: [300, 100] }],
      edges: [{ from: 'b', port: 'body', to: 'b1' }, { from: 'b1', to: 'b', to_port: 'loop' }],
    }
    editor.selected = 'b1'
    await flushPromises()
    const from = btn(wrapper, 'С этого блока')
    expect(from.attributes('disabled')).toBeDefined()
    expect(from.attributes('title')).toBe('Начать проверку с блока внутри «Нового браузера» нельзя. Запустите сценарий с начала.')
    editor.selected = 'b'
    await flushPromises()
    expect(from.attributes('disabled')).toBeUndefined()
    expect(from.attributes('title')).toBeUndefined()
  })

  it('«С этого блока» в группе передаёт путь и узел', async () => {
    const { wrapper, editor } = await mountPanel()
    editor.path = ['g']
    editor.selected = 'x'
    await flushPromises()
    await btn(wrapper, 'С этого блока').trigger('click')
    await flushPromises()
    expect(mocks.api.debugFlow).toHaveBeenCalledWith(doc, 7, ['g', 'x'])
  })

  it('строки журнала по событиям; щелчок выбирает узел и открывает его группу', async () => {
    const { wrapper, editor } = await mountPanel()
    await btn(wrapper, 'С начала').trigger('click')
    await flushPromises()
    mocks.handler.current!(ev({ entry: entry({ node: 'n2', op: 'web.click', desc: 'Нажать вход' }) }))
    mocks.handler.current!(ev({ entry: entry({ node: 'x', op: 'wait.time' }) }))
    mocks.handler.current!(ev({ run_id: 'old', entry: entry({ node: 'n1', op: 'start' }) }))
    await flushPromises()
    const rows = wrapper.findAll('[data-test="debug-row"]')
    expect(rows).toHaveLength(2)
    expect(rows[0]!.text()).toContain('Нажать вход')
    expect(rows[0]!.text()).toContain('готово')
    expect(rows[1]!.text()).toContain('Пауза')
    await rows[1]!.trigger('click')
    expect(editor.path).toEqual(['g'])
    expect(editor.selected).toBe('x')
  })

  it('в панели последние 500 строк; о срезанных — где весь журнал', async () => {
    const { wrapper } = await mountPanel()
    await btn(wrapper, 'С начала').trigger('click')
    await flushPromises()
    for (let i = 0; i < 500; i++) mocks.handler.current!(ev({ entry: entry({ node: 'n2', op: 'web.click', msg: `m${i}` }) }))
    await flushPromises()
    expect(wrapper.find('[data-test="debug-cut"]').exists()).toBe(false)
    mocks.handler.current!(ev({ entry: entry({ node: 'n2', op: 'web.click', msg: 'm500' }) }))
    await flushPromises()
    const rows = wrapper.findAll('[data-test="debug-row"]')
    expect(rows).toHaveLength(500)
    expect(rows[0]!.text()).toContain('m1')
    expect(wrapper.find('[data-test="debug-cut"]').text()).toContain('logs/flows/r1.jsonl')
  })

  it('журнал прокручивается к каждой новой строке и после 500; заметка о срезанных — вне списка', async () => {
    const { wrapper } = await mountPanel()
    await btn(wrapper, 'С начала').trigger('click')
    await flushPromises()
    for (let i = 0; i < 500; i++) mocks.handler.current!(ev({ entry: entry({ node: 'n2', op: 'web.click' }) }))
    await flushPromises()
    // happy-dom без раскладки: прокрутку видно по записи scrollTop.
    const scrolled = vi.fn()
    Object.defineProperty(wrapper.find('.log').element, 'scrollTop', { configurable: true, get: () => 0, set: scrolled })
    mocks.handler.current!(ev({ entry: entry({ node: 'n2', op: 'web.click' }) }))
    await flushPromises()
    mocks.handler.current!(ev({ entry: entry({ node: 'n2', op: 'web.click' }) }))
    await flushPromises()
    expect(scrolled).toHaveBeenCalledTimes(2)
    expect(wrapper.find('[data-test="debug-cut"]').exists()).toBe(true)
    expect(wrapper.find('.log [data-test="debug-cut"]').exists()).toBe(false)
  })

  it('у строки журнала — время, как на экране «Журнал»', async () => {
    const { wrapper } = await mountPanel()
    await btn(wrapper, 'С начала').trigger('click')
    await flushPromises()
    mocks.handler.current!(ev({ entry: entry({ node: 'n2', op: 'web.click', ts_ms: new Date(2026, 9, 21, 9, 5, 7).getTime() }) }))
    await flushPromises()
    expect(wrapper.find('[data-test="debug-row"] .time').text()).toBe('09:05:07')
  })

  it('строки подпроцесса помечены отступом и не выбирают узел процесса с тем же id', async () => {
    const { wrapper, editor } = await mountPanel()
    await btn(wrapper, 'С начала').trigger('click')
    await flushPromises()
    mocks.handler.current!(ev({ entry: entry({ node: 'n2', op: 'web.click', depth: 1 }) }))
    await flushPromises()
    const row = wrapper.find('[data-test="debug-row"]')
    expect(row.classes()).toContain('sub')
    expect(row.attributes('title')).toBe('Блок вложенного сценария')
    expect(row.attributes('disabled')).toBeDefined()
    await row.trigger('click')
    expect(editor.selected).toBeNull()
  })

  it('событие, пришедшее раньше ответа на запуск, не теряется', async () => {
    mocks.api.debugFlow.mockImplementation(async () => {
      mocks.handler.current!(ev({ entry: entry({ node: 'n2', op: 'web.click' }) }))
      return 'r1'
    })
    const { wrapper } = await mountPanel()
    await btn(wrapper, 'С начала').trigger('click')
    await flushPromises()
    expect(wrapper.findAll('[data-test="debug-row"]')).toHaveLength(1)
  })

  it('текущий узел подсвечен классом running, по окончании подсветка снимается', async () => {
    const { wrapper, debug } = await mountPanel()
    const canvas = mountWithApp(FlowCanvas, { attachTo: document.body })
    await btn(wrapper, 'С начала').trigger('click')
    await flushPromises()
    mocks.handler.current!(ev({ phase: 'node', node: 'n2' }))
    await flushPromises()
    expect(canvas.find('[data-id="n2"] .op-node').classes()).toContain('running')
    expect(canvas.find('[data-id="n1"] .op-node').classes()).not.toContain('running')
    mocks.handler.current!(ev({ phase: 'end', status: 'done' }))
    await flushPromises()
    expect(debug.running).toBe(false)
    expect(canvas.find('[data-id="n2"] .op-node').classes()).not.toContain('running')
  })

  it('узел внутри группы идёт — на уровне выше подсвечена группа', async () => {
    const { wrapper, editor } = await mountPanel()
    const canvas = mountWithApp(FlowCanvas, { attachTo: document.body })
    await btn(wrapper, 'С начала').trigger('click')
    await flushPromises()
    mocks.handler.current!(ev({ phase: 'node', node: 'x' }))
    await flushPromises()
    expect(canvas.find('[data-id="g"] .op-node').classes()).toContain('running')
    expect(canvas.find('[data-id="n2"] .op-node').classes()).not.toContain('running')
    editor.path = ['g']
    await flushPromises()
    expect(canvas.find('[data-id="x"] .op-node').classes()).toContain('running')
  })

  it('«Остановить» останавливает идущую отладку', async () => {
    const { wrapper } = await mountPanel()
    await btn(wrapper, 'Остановить').trigger('click')
    expect(mocks.api.stopDebug).not.toHaveBeenCalled()
    await btn(wrapper, 'С начала').trigger('click')
    await flushPromises()
    await btn(wrapper, 'Остановить').trigger('click')
    expect(mocks.api.stopDebug).toHaveBeenCalledWith('r1')
  })

  it('при ошибках проверки запуск недоступен', async () => {
    const { wrapper, editor } = await mountPanel()
    editor.report = { errors: [{ node: 'n2', field: null, message: 'x' }], warnings: [] }
    await flushPromises()
    expect(btn(wrapper, 'С начала').attributes('disabled')).toBeDefined()
  })

  it('номер без профиля — «нет такого», запуск недоступен', async () => {
    mocks.api.listProfiles.mockResolvedValue({ rows: [], total: 0, running: 0 })
    const { wrapper } = await mountPanel()
    await wrapper.find('input').setValue('99')
    await new Promise((r) => setTimeout(r, 400))
    await flushPromises()
    expect(mocks.api.listProfiles).toHaveBeenLastCalledWith(expect.objectContaining({ search: '99' }))
    expect(wrapper.text()).toContain('Нет профиля с таким номером')
    expect(btn(wrapper, 'С начала').attributes('disabled')).toBeDefined()
  })

  it('кнопка «Отладка» в верхней строке открывает и закрывает панель', async () => {
    useFlowEditorStore().doc = structuredClone(doc)
    const debug = useFlowDebugStore()
    const wrapper = mountWithApp(defineComponent({ render: () => h(EditorToolbar, { saving: false }) }))
    const button = wrapper.findAll('button').find((b) => b.text() === 'Отладка')!
    await button.trigger('click')
    expect(debug.open).toBe(true)
    await button.trigger('click')
    expect(debug.open).toBe(false)
  })

  it('имя найденного профиля показано рядом с номером', async () => {
    const { wrapper } = await mountPanel()
    await new Promise((r) => setTimeout(r, 400))
    await flushPromises()
    expect(wrapper.find('[data-test="debug-profile"]').text()).toBe('Рабочий')
  })

  it('идущий узел — свой класс и значок, и когда он же выбран', async () => {
    const { wrapper, editor } = await mountPanel()
    const canvas = mountWithApp(FlowCanvas, { attachTo: document.body })
    await btn(wrapper, 'С начала').trigger('click')
    await flushPromises()
    editor.selected = 'n2'
    mocks.handler.current!(ev({ phase: 'node', node: 'n2' }))
    await flushPromises()
    const node = canvas.find('[data-id="n2"] .op-node')
    expect(node.classes()).toEqual(expect.arrayContaining(['running', 'selected']))
    expect(node.find('[data-test="run-mark"]').exists()).toBe(true)
    expect(canvas.find('[data-id="n1"] [data-test="run-mark"]').exists()).toBe(false)
  })

  it('reset во время запуска останавливает его, когда придёт номер, и снимает подписку', async () => {
    const unlisten = vi.fn()
    const { events } = await import('../../../api/core')
    vi.mocked(events.onFlowDebug).mockImplementationOnce(async () => unlisten)
    let done!: (id: string) => void
    mocks.api.debugFlow.mockReturnValue(new Promise<string>((r) => { done = r }))
    const { wrapper, debug } = await mountPanel()
    await btn(wrapper, 'С начала').trigger('click')
    await flushPromises()
    debug.reset()
    expect(unlisten).toHaveBeenCalled()
    done('r9')
    await flushPromises()
    expect(mocks.api.stopDebug).toHaveBeenCalledWith('r9')
    expect(debug.running).toBe(false)
  })

  it('«Остановить» до номера запуска останавливает его, как только номер придёт', async () => {
    let done!: (id: string) => void
    mocks.api.debugFlow.mockReturnValue(new Promise<string>((r) => { done = r }))
    const { wrapper, debug } = await mountPanel()
    await btn(wrapper, 'С начала').trigger('click')
    await flushPromises()
    await btn(wrapper, 'Остановить').trigger('click')
    expect(mocks.api.stopDebug).not.toHaveBeenCalled()
    done('r1')
    await flushPromises()
    expect(mocks.api.stopDebug).toHaveBeenCalledWith('r1')
    expect(debug.runId).toBe('r1')
  })

  it('начало, сменённый уходом и новым стартом, останавливает свой запуск, новый идёт', async () => {
    const replies: ((id: string) => void)[] = []
    mocks.api.debugFlow.mockImplementation(() => new Promise<string>((r) => { replies.push(r) }))
    const { wrapper, debug } = await mountPanel()
    await btn(wrapper, 'С начала').trigger('click')
    await flushPromises()
    debug.reset()
    await flushPromises()
    await btn(wrapper, 'С начала').trigger('click')
    await flushPromises()
    expect(replies).toHaveLength(2)
    replies[0]!('r1')
    await flushPromises()
    expect(mocks.api.stopDebug).toHaveBeenCalledWith('r1')
    replies[1]!('r2')
    await flushPromises()
    expect(debug.runId).toBe('r2')
    expect(debug.running).toBe(true)
    expect(mocks.api.stopDebug).not.toHaveBeenCalledWith('r2')
  })

  it('уход во время подписки: подписка снимается, запуск не начинается', async () => {
    const unlisten = vi.fn()
    let subscribed!: (off: () => void) => void
    const { events } = await import('../../../api/core')
    vi.mocked(events.onFlowDebug).mockImplementationOnce(() => new Promise((r) => { subscribed = r }))
    const { wrapper, debug } = await mountPanel()
    await btn(wrapper, 'С начала').trigger('click')
    await flushPromises()
    debug.reset()
    subscribed(unlisten)
    await flushPromises()
    expect(unlisten).toHaveBeenCalled()
    expect(mocks.api.debugFlow).not.toHaveBeenCalled()
    expect(debug.running).toBe(false)
  })

})
