import { Handle, Position, VueFlow, useVueFlow } from '@vue-flow/core'
import { flushPromises } from '@vue/test-utils'
import { beforeEach, describe, expect, it, vi, type MockInstance } from 'vitest'
import { defineComponent, h, ref } from 'vue'
import type { FlowDoc } from '../../../api/types'
import { core } from '../../../api/core'
import { addNode, removeItems, updateNode } from '../../../lib/flowEdit'
import { mountWithApp } from '../../../test/mount'

vi.mock('../../../api/core', () => ({ core: { validateFlow: vi.fn(async () => ({ errors: [], warnings: [] })), getFlow: vi.fn() }, events: {} }))

import { useFlowEditorStore } from '../../../stores/flowEditor'
import { useFlowDebugStore } from '../../../stores/flowDebug'
import FlowCanvas from './FlowCanvas.vue'
import { testCatalog } from './testOps'
import type { PaletteDrag } from './paletteDrag'

async function mountCanvas(doc: FlowDoc, errors: { node: string | null; field: string | null; message: string }[] = []) {
  const store = useFlowEditorStore()
  store.catalog = testCatalog
  store.doc = doc
  store.report = { errors, warnings: [] }
  const wrapper = mountWithApp(FlowCanvas, { attachTo: document.body })
  await flushPromises()
  return { wrapper, store }
}

const doc = (nodes: FlowDoc['nodes']): FlowDoc => ({ format: 'ombra-flow/1', name: 't', nodes, edges: [] })

async function mountConnectingCanvas(document: FlowDoc) {
  const store = useFlowEditorStore()
  store.catalog = testCatalog
  store.doc = document
  let vf!: ReturnType<typeof useVueFlow>
  const Host = defineComponent({
    setup() {
      vf = useVueFlow()
      return () => h(FlowCanvas)
    },
  })
  const host = mountWithApp(Host)
  await flushPromises()
  return { host, wrapper: host.getComponent(FlowCanvas), store, vf }
}

beforeEach(() => { document.body.innerHTML = '' })

describe('FlowCanvas', () => {
  it('Начало — квадрат с иконкой запуска и подписью; описание остаётся в подсказке', async () => {
    const { wrapper } = await mountCanvas(doc([
      { id: 's', op: 'start', description: 'Параметры обхода' }, { id: 'a', op: 'web.click' },
    ]))
    const start = wrapper.get<HTMLElement>('[data-id="s"] .start-node')
    expect(start.element.style.width).toBe('72px')
    expect(start.element.style.height).toBe(start.element.style.width)
    expect(start.find('.start-icon').exists()).toBe(true)
    expect(start.get('.title').text()).toBe('Начало')
    expect(start.find('.sub').exists()).toBe(false)
    expect(start.attributes('title')).toBe('Параметры обхода')
    expect(wrapper.find('[data-id="a"] .start-node').exists()).toBe(false)
    wrapper.unmount()
  })

  it('Начало выбирается, но не двигается стрелками и не записывает перетаскивание в документ', async () => {
    const { wrapper, store } = await mountCanvas(doc([{ id: 's', op: 'start', pos: [50, 60] }, { id: 'a', op: 'web.click', pos: [200, 60] }]))
    const start = wrapper.get<HTMLElement>('.vue-flow__node[data-id="s"]')
    const flow = wrapper.getComponent(VueFlow)
    expect((flow.props('nodes') as { id: string; draggable?: boolean }[]).find((node) => node.id === 's')!.draggable).toBe(false)
    await start.trigger('keydown', { key: 'Enter' })
    expect(store.selected).toBe('s')
    for (const key of ['ArrowDown', 'ArrowRight', 'ArrowUp', 'ArrowLeft']) await start.trigger('keydown', { key })
    flow.vm.$emit('nodeDragStop', { nodes: [{ id: 's', position: { x: 500, y: 600 } }] })
    flow.vm.$emit('nodesChange', [{ id: 's', type: 'position', dragging: false, position: { x: 700, y: 800 } }])
    expect(store.doc!.nodes[0]!.pos).toEqual([50, 60])
    expect(store.canUndo).toBe(false)
    wrapper.unmount()
  })

  it('у всех операций входы слева, выходы справа; подписи веток и возврата цикла идут у своих портов', async () => {
    const { wrapper, store } = await mountCanvas(doc([
      { id: 's', op: 'start' }, { id: 'a', op: 'web.click' },
      { id: 'if', op: 'control.if', on_error: 'branch' }, { id: 'loop', op: 'control.for_times' },
      { id: 'g', op: 'control.group', nodes: [{ id: 'inner', op: 'web.click', on_error: 'branch' }] },
    ]))
    const check = () => {
      for (const handle of wrapper.findAllComponents(Handle)) {
        expect(handle.props('position')).toBe(handle.props('type') === 'target' ? Position.Left : Position.Right)
      }
      expect(wrapper.find('.vue-flow__handle-top, .vue-flow__handle-bottom').exists()).toBe(false)
    }
    check()
    expect(wrapper.findAll('[data-id="if"] .output-label').map((label) => label.text())).toEqual(['Да', 'Нет', 'Ошибка'])
    expect(wrapper.get('[data-id="loop"] .input-label').text()).toBe('Повтор')
    const before = wrapper.get('[data-nodeid="a"][data-handleid="next"]').attributes('style')
    store.apply((doc) => updateNode(doc, [], 'a', { on_error: 'branch' }))
    await flushPromises()
    expect(wrapper.get('[data-nodeid="a"][data-handleid="next"]').attributes('style')).not.toBe(before)
    check()
    store.enterGroup('g')
    await flushPromises()
    check()
    expect(wrapper.get('.output-label.error').text()).toBe('Ошибка')
    wrapper.unmount()
  })

  it('быстрая группировка на среднем блоке собирает всю цепочку; разгруппировка и отмена сохраняют связи', async () => {
    const original: FlowDoc = { ...doc([
      { id: 's', op: 'start' }, { id: 'a', op: 'web.click' }, { id: 'b', op: 'wait.time' }, { id: 'c', op: 'key.press' },
    ]), edges: [{ from: 's', to: 'a' }, { from: 'a', to: 'b' }, { from: 'b', to: 'c' }] }
    const { wrapper, store } = await mountCanvas(original)
    await wrapper.get('[data-id="b"] [aria-label="Сгруппировать цепочку · блоков: 3"]').trigger('click')
    await flushPromises()
    const group = store.doc!.nodes.find((n) => n.op === 'control.group')!
    expect(group.nodes!.map((n) => n.id)).toEqual(['a', 'b', 'c'])
    expect(store.selected).toBe(group.id)
    await wrapper.get(`[data-id="${group.id}"] [aria-label="Разгруппировать"]`).trigger('click')
    await flushPromises()
    expect(store.selection).toEqual(['a', 'b', 'c'])
    expect(store.doc!.edges).toEqual(expect.arrayContaining([
      { from: 's', port: 'next', to: 'a', to_port: 'in' }, { from: 'a', to: 'b' }, { from: 'b', to: 'c' },
    ]))
    store.undo()
    expect(store.doc!.nodes.some((n) => n.id === group.id)).toBe(true)
    store.undo()
    expect(store.doc).toEqual(original)
    wrapper.unmount()
  })

  it('быстрые команды действуют на наведённый блок; Start защищён, открытие отладки не запускает процесс', async () => {
    const { wrapper, store } = await mountCanvas(doc([
      { id: 's', op: 'start' }, { id: 'a', op: 'web.click' }, { id: 'b', op: 'wait.time' },
    ]))
    store.selection = ['b']
    store.selected = 'b'
    await flushPromises()
    const node = () => wrapper.get('[data-id="a"]')
    await node().get('[role="switch"]').trigger('click')
    expect(store.doc!.nodes.find((n) => n.id === 'a')!.disabled).toBe(true)
    expect(store.doc!.nodes.find((n) => n.id === 'b')!.disabled).toBeUndefined()
    await node().get('[aria-label="Открыть отладку с блока"]').trigger('click')
    expect(store.selected).toBe('a')
    expect(useFlowDebugStore().open).toBe(true)
    expect(useFlowDebugStore().running).toBe(false)
    await node().get('[aria-label="Дублировать"]').trigger('click')
    await flushPromises()
    expect(store.doc!.nodes).toHaveLength(4)
    expect(store.doc!.nodes.at(-1)!.disabled).toBe(true)
    await node().get('[aria-label="Удалить"]').trigger('click')
    expect(store.doc!.nodes.map((n) => n.id)).not.toContain('a')
    const start = wrapper.get('[data-id="s"]')
    expect(start.find('[aria-label="Удалить"]').exists()).toBe(false)
    expect(start.find('[aria-label="Дублировать"]').exists()).toBe(false)
    expect(start.find('[role="switch"]').exists()).toBe(false)
    wrapper.unmount()
  })

  it('быстрая группировка использует выбранные блоки; одиночный блок без цепочки не группируется', async () => {
    const { wrapper, store } = await mountCanvas({ ...doc([
      { id: 's', op: 'start' }, { id: 'a', op: 'web.click' }, { id: 'b', op: 'wait.time' }, { id: 'c', op: 'key.press' },
    ]), edges: [{ from: 's', to: 'a' }, { from: 'a', to: 'b' }] })
    expect(wrapper.get('[data-id="c"] [aria-label^="Сгруппировать цепочку"]').attributes('disabled')).toBeDefined()
    store.selection = ['a', 'b']
    await flushPromises()
    await wrapper.get('[data-id="a"] [aria-label="Сгруппировать"]').trigger('click')
    await flushPromises()
    expect(store.doc!.nodes.find((n) => n.op === 'control.group')!.nodes!.map((n) => n.id)).toEqual(['a', 'b'])
    wrapper.unmount()
  })

  it('Branch показывает выходы «Да» и «Нет»', async () => {
    const { wrapper } = await mountCanvas(doc([{ id: 'n1', op: 'start' }, { id: 'n2', op: 'control.if', pos: [0, 100] }]))
    const node = wrapper.find('[data-id="n2"]')
    expect(node.text()).toContain('Условие')
    expect(node.text()).toContain('Да')
    expect(node.text()).toContain('Нет')
    expect(node.text()).not.toContain('Ошибка')
  })

  it('on_error: branch добавляет выход «Ошибка»', async () => {
    const { wrapper } = await mountCanvas(doc([{ id: 'n1', op: 'web.click', on_error: 'branch' }]))
    expect(wrapper.find('[data-id="n1"]').text()).toContain('Ошибка')
  })

  it('отключённый узел — класс disabled', async () => {
    const { wrapper } = await mountCanvas(doc([{ id: 'n1', op: 'web.click', disabled: true }]))
    expect(wrapper.find('[data-id="n1"] .op-node').classes()).toContain('disabled')
  })

  it('узел с ошибкой — класс invalid и число', async () => {
    const { wrapper } = await mountCanvas(doc([{ id: 'n1', op: 'web.click' }]), [
      { node: 'n1', field: 'selector', message: 'a' },
      { node: 'n1', field: null, message: 'b' },
    ])
    const node = wrapper.find('[data-id="n1"] .op-node')
    expect(node.classes()).toContain('invalid')
    expect(node.find('.issues').text()).toBe('2')
  })

  it('предупреждение — приглушённый знак без красной рамки; в группе видно и на её узле', async () => {
    const { wrapper, store } = await mountCanvas(doc([{ id: 'n1', op: 'web.click' }, { id: 'g', op: 'control.group', nodes: [{ id: 'x', op: 'web.click' }] }]))
    store.report = { errors: [], warnings: [{ node: 'n1', field: null, message: 'узел недостижим из start' }, { node: 'x', field: null, message: 'w' }] }
    await flushPromises()
    for (const id of ['n1', 'g']) {
      const node = wrapper.find(`[data-id="${id}"] .op-node`)
      expect(node.classes()).not.toContain('invalid')
      expect(node.find('.issues.warn').text()).toBe('1')
      expect(node.find('.issues.warn').attributes('title')).toBe('Предупреждений: 1')
    }
  })

  it('узел операции без перевода — с названием AdsPower', async () => {
    const store = useFlowEditorStore()
    const { wrapper } = await mountCanvas(doc([{ id: 'n1', op: 'ext.tool' }]))
    store.catalog = { ...testCatalog, ops: [...testCatalog.ops, { id: 'ext.tool', adspower: 'Some Tool', group: 'ext', inputs: ['in'], outputs: ['next'], fields: [] }] }
    await flushPromises()
    expect(wrapper.find('[data-id="n1"] .title').text()).toBe('Some Tool')
  })

  it('узел со своим именем — под ним русское название операции, без английского', async () => {
    const { wrapper } = await mountCanvas(doc([{ id: 'n1', op: 'web.click', description: 'Логин' }, { id: 'n2', op: 'web.click' }]))
    expect(wrapper.find('[data-id="n1"] .title').text()).toBe('Логин')
    expect(wrapper.find('[data-id="n1"] .sub').text()).toBe('Щелчок мышью')
    expect(wrapper.find('[data-id="n2"] .sub').exists()).toBe(false)
    expect(wrapper.text()).not.toContain('Click Element')
    wrapper.unmount()
  })

  it('заголовок группы — её имя, затем описание, затем «Группа»', async () => {
    const { wrapper } = await mountCanvas(doc([
      { id: 'g1', op: 'control.group', description: 'Описание', config: { name: 'Вход' } },
      { id: 'g2', op: 'control.group', description: 'Описание', config: { name: '' } },
      { id: 'g3', op: 'control.group', config: { name: '' } },
    ]))
    expect(wrapper.find('[data-id="g1"] .title').text()).toBe('Вход')
    expect(wrapper.find('[data-id="g2"] .title').text()).toBe('Описание')
    expect(wrapper.find('[data-id="g3"] .title').text()).toBe('Группа')
  })

  it('группа показывает пустоту и число непосредственных блоков; счётчик обновляется после правки и отмены', async () => {
    const { wrapper, store } = await mountCanvas(doc([
      { id: 'empty', op: 'control.group' },
      { id: 'g', op: 'control.group', config: { name: 'Обход страницы' }, nodes: [
        { id: 'a', op: 'web.click', disabled: true },
        { id: 'nested', op: 'control.group', nodes: [{ id: 'b', op: 'wait.time' }] },
      ] },
      { id: 'plain', op: 'web.click' },
    ]))
    const empty = () => wrapper.get('[data-id="empty"] .op-node')
    const group = () => wrapper.get('[data-id="g"] .op-node')
    expect(empty().classes()).toContain('empty-group')
    expect(empty().get('.group-kind').text()).toBe('Группа')
    expect(empty().get('.group-count').text()).toBe('Пусто')
    expect(group().classes()).toContain('group-node')
    expect(group().classes()).not.toContain('empty-group')
    expect(group().get('.group-kind').text()).toBe('Группа')
    expect(group().get('.group-count').text()).toBe('2 блока')
    expect(group().get('.title').text()).toBe('Обход страницы')
    expect(wrapper.find('[data-id="plain"] .group-summary').exists()).toBe(false)
    store.apply((doc) => updateNode(doc, [], 'empty', { nodes: [{ id: 'x', op: 'web.click' }] }))
    await flushPromises()
    expect(empty().classes()).not.toContain('empty-group')
    expect(empty().get('.group-count').text()).toBe('1 блок')
    store.undo()
    await flushPromises()
    expect(empty().classes()).toContain('empty-group')
    expect(empty().get('.group-count').text()).toBe('Пусто')
    wrapper.unmount()
  })

  it('первый показ вписывает процесс не крупнее 1:1, один раз', async () => {
    const store = useFlowEditorStore()
    store.catalog = testCatalog
    store.doc = doc([{ id: 'n1', op: 'start' }])
    let fit!: MockInstance
    const Host = defineComponent({
      setup() {
        fit = vi.spyOn(useVueFlow(), 'fitView').mockResolvedValue(true)
        return () => h(FlowCanvas)
      },
    })
    const wrapper = mountWithApp(Host, { attachTo: document.body })
    await flushPromises()
    const flow = wrapper.findComponent(VueFlow)
    flow.vm.$emit('nodesInitialized', [])
    flow.vm.$emit('nodesInitialized', [])
    expect(fit).toHaveBeenCalledTimes(1)
    expect(fit).toHaveBeenCalledWith(expect.objectContaining({ maxZoom: 1 }))
  })

  it('вход в группу и выход из неё вписывают новый уровень после отрисовки', async () => {
    const store = useFlowEditorStore()
    store.catalog = testCatalog
    store.doc = doc([{ id: 'n1', op: 'start' }, { id: 'g', op: 'control.group', nodes: [{ id: 'x', op: 'web.click', pos: [2000, 1500] }] }])
    let fit!: MockInstance
    const Host = defineComponent({
      setup() {
        fit = vi.spyOn(useVueFlow(), 'fitView').mockResolvedValue(true)
        return () => h(FlowCanvas)
      },
    })
    const wrapper = mountWithApp(Host, { attachTo: document.body })
    await flushPromises()
    const flow = wrapper.findComponent(VueFlow)
    flow.vm.$emit('nodesInitialized', [])
    expect(fit).toHaveBeenCalledTimes(1)
    store.enterGroup('g')
    await flushPromises()
    flow.vm.$emit('nodesInitialized', [])
    flow.vm.$emit('nodesInitialized', [])
    expect(fit).toHaveBeenCalledTimes(2)
    expect(fit).toHaveBeenLastCalledWith(expect.objectContaining({ maxZoom: 1 }))
    store.leaveTo(0)
    await flushPromises()
    flow.vm.$emit('nodesInitialized', [])
    expect(fit).toHaveBeenCalledTimes(3)
  })

  it('отмена и повтор на том же уровне не вписывают холст заново', async () => {
    const store = useFlowEditorStore()
    store.catalog = testCatalog
    store.doc = doc([{ id: 'n1', op: 'start' }, { id: 'n2', op: 'web.click', pos: [0, 100] }])
    let fit!: MockInstance
    const Host = defineComponent({
      setup() {
        fit = vi.spyOn(useVueFlow(), 'fitView').mockResolvedValue(true)
        return () => h(FlowCanvas)
      },
    })
    const wrapper = mountWithApp(Host, { attachTo: document.body })
    await flushPromises()
    const flow = wrapper.findComponent(VueFlow)
    flow.vm.$emit('nodesInitialized', [])
    store.apply((d) => removeItems(d, [], ['n2'], []))
    store.undo()
    await flushPromises()
    // Возвращённый узел измерен заново — vue-flow снова сообщает nodes-initialized.
    flow.vm.$emit('nodesInitialized', [])
    expect(fit).toHaveBeenCalledTimes(1)
  })

  it('перетаскивание на прежнее место не меняет процесс', async () => {
    // Группа из ядра — без пустого `edges`.
    vi.mocked(core.getFlow).mockResolvedValue({
      format: 'ombra-flow/1', name: 't', nodes: [{ id: 'n1', op: 'start', pos: [0, 0] }, { id: 'g', op: 'control.group', pos: [0, 100], nodes: [{ id: 'x', op: 'web.click', pos: [10, 20] }] }],
    })
    const store = useFlowEditorStore()
    store.catalog = testCatalog
    await store.load(1)
    const wrapper = mountWithApp(FlowCanvas, { attachTo: document.body })
    store.enterGroup('g')
    await flushPromises()
    wrapper.findComponent(VueFlow).vm.$emit('nodeDragStop', { event: new MouseEvent('mouseup'), node: { id: 'x', position: { x: 10.2, y: 19.8 } }, nodes: [{ id: 'x', position: { x: 10.2, y: 19.8 } }] })
    expect(store.dirty).toBe(false)
    expect(store.canUndo).toBe(false)
  })

  it('перетаскивание нескольких узлов — Начало закреплён, остальные позиции одной правкой', async () => {
    const { wrapper, store } = await mountCanvas(doc([{ id: 'n1', op: 'start', pos: [0, 0] }, { id: 'n2', op: 'web.click', pos: [0, 100] }]))
    const flow = wrapper.findComponent(VueFlow)
    const at = (id: string, x: number, y: number) => ({ id, position: { x, y } })
    flow.vm.$emit('nodeDragStop', { event: new MouseEvent('mouseup'), node: at('n1', 10.4, 20), nodes: [at('n1', 10.4, 20), at('n2', 30, 140.6)] })
    expect(store.doc!.nodes.map((n) => n.pos)).toEqual([[0, 0], [30, 141]])
    store.undo()
    expect(store.doc!.nodes.map((n) => n.pos)).toEqual([[0, 0], [0, 100]])
    expect(store.canUndo).toBe(false)
  })

  it('клавиатура: Enter выбирает узел, стрелки двигают его одной отменой на серию, Delete удаляет', async () => {
    const { wrapper, store } = await mountCanvas(doc([{ id: 'n1', op: 'start', pos: [0, 0] }, { id: 'n2', op: 'web.click', pos: [0, 100] }]))
    const key = async (id: string, k: string) => {
      const el = wrapper.find<HTMLElement>(`.vue-flow__node[data-id="${id}"]`).element
      el.focus()
      el.dispatchEvent(new KeyboardEvent('keydown', { key: k, bubbles: true, cancelable: true }))
      await flushPromises()
    }
    await key('n2', 'Enter')
    expect(store.selected).toBe('n2')
    await key('n2', 'ArrowDown')
    await key('n2', 'ArrowDown')
    await key('n2', 'ArrowRight')
    expect(store.doc!.nodes[1]!.pos).toEqual([5, 110])
    store.undo()
    expect(store.doc!.nodes[1]!.pos).toEqual([0, 100])
    expect(store.canUndo).toBe(false)
    await flushPromises()
    await key('n2', 'Delete')
    expect(store.doc!.nodes.map((n) => n.id)).toEqual(['n1'])
  })

  it('связь, выбранная клавиатурой (Enter в vue-flow), удаляется Delete', async () => {
    const { wrapper, store } = await mountCanvas({ ...doc([{ id: 'n1', op: 'start', pos: [0, 0] }, { id: 'n2', op: 'web.click', pos: [0, 100] }]), edges: [{ from: 'n1', port: 'next', to: 'n2', to_port: 'in' }] })
    const flow = wrapper.findComponent(VueFlow)
    flow.vm.$emit('nodesChange', [{ id: 'n2', type: 'select', selected: true }])
    expect(store.selected).toBe('n2')
    // happy-dom без раскладки связи не рисует: выбор приходит теми же событиями, что шлёт vue-flow (`addSelectedEdges`).
    flow.vm.$emit('edgesChange', [{ id: 'n1:next->n2:in', type: 'select', selected: true }])
    flow.vm.$emit('nodesChange', [{ id: 'n2', type: 'select', selected: false }])
    expect(store.selected).toBeNull()
    await wrapper.find('.canvas').trigger('keydown', { key: 'Delete' })
    expect(store.doc!.edges).toEqual([])
  })

  it('выделение рамкой остаётся целиком; панель пуста; Delete удаляет всё выделенное, кроме Start, одним шагом', async () => {
    const e = (from: string, to: string) => ({ from, port: 'next', to, to_port: 'in' })
    const { wrapper, store } = await mountCanvas({
      ...doc([{ id: 'n1', op: 'start', pos: [0, 0] }, { id: 'n2', op: 'web.click', pos: [0, 100] }, { id: 'n3', op: 'web.click', pos: [0, 200] }, { id: 'n4', op: 'web.click', pos: [300, 0] }]),
      edges: [e('n1', 'n2'), e('n2', 'n3'), e('n3', 'n4')],
    })
    const flow = wrapper.findComponent(VueFlow)
    // Пара событий панели vue-flow при выделении рамкой n2 и n3: сначала узлы, затем их связи.
    flow.vm.$emit('nodesChange', [{ id: 'n2', type: 'select', selected: true }, { id: 'n3', type: 'select', selected: true }])
    flow.vm.$emit('edgesChange', ['n1:next->n2:in', 'n2:next->n3:in', 'n3:next->n4:in'].map((id) => ({ id, type: 'select', selected: true })))
    await flushPromises()
    const selected = (list: unknown) => (list as { id: string; selected?: boolean }[]).filter((x) => x.selected).map((x) => x.id)
    expect(selected(flow.props('nodes'))).toEqual(['n2', 'n3'])
    expect(selected(flow.props('edges'))).toEqual(['n1:next->n2:in', 'n2:next->n3:in', 'n3:next->n4:in'])
    expect(store.selected).toBeNull()
    flow.vm.$emit('nodesChange', [{ id: 'n1', type: 'select', selected: true }])
    await wrapper.find('.canvas').trigger('keydown', { key: 'Delete' })
    expect(store.doc!.nodes.map((n) => n.id)).toEqual(['n1', 'n4'])
    expect(store.doc!.edges).toEqual([])
    store.undo()
    expect(store.doc!.nodes).toHaveLength(4)
    expect(store.doc!.edges).toHaveLength(3)
    expect(store.canUndo).toBe(false)
  })

  it('один выбранный узел — в панели; выбор извне (палитра) сбрасывает выделение рамкой', async () => {
    const { wrapper, store } = await mountCanvas(doc([{ id: 'n1', op: 'start', pos: [0, 0] }, { id: 'n2', op: 'web.click', pos: [0, 100] }, { id: 'n3', op: 'web.click', pos: [0, 200] }]))
    const flow = wrapper.findComponent(VueFlow)
    flow.vm.$emit('nodesChange', [{ id: 'n2', type: 'select', selected: true }])
    expect(store.selected).toBe('n2')
    flow.vm.$emit('nodesChange', [{ id: 'n3', type: 'select', selected: true }])
    expect(store.selected).toBeNull()
    store.selected = 'n1'
    await flushPromises()
    expect((flow.props('nodes') as { id: string; selected?: boolean }[]).filter((n) => n.selected).map((n) => n.id)).toEqual(['n1'])
  })

  it('отмена убрала часть выделения: в нём только то, что осталось, панель — у оставшегося узла', async () => {
    const { wrapper, store } = await mountCanvas(doc([{ id: 'n1', op: 'start', pos: [0, 0] }, { id: 'n2', op: 'web.click', pos: [0, 100] }]))
    const flow = wrapper.findComponent(VueFlow)
    store.apply((d) => addNode(d, [], testCatalog.ops.find((o) => o.id === 'web.click')!, [0, 200], undefined, new Map()).doc)
    await flushPromises()
    flow.vm.$emit('nodesChange', [{ id: 'n2', type: 'select', selected: true }, { id: 'n3', type: 'select', selected: true }])
    expect(store.selected).toBeNull()
    store.undo()
    await flushPromises()
    expect(store.selected).toBe('n2')
    expect((flow.props('nodes') as { id: string; selected?: boolean }[]).filter((n) => n.selected).map((n) => n.id)).toEqual(['n2'])
    flow.vm.$emit('nodesChange', [{ id: 'n2', type: 'select', selected: false }])
    expect(store.selected).toBeNull()
  })

  it('рамка выделения vue-flow снимается, когда выбран один узел или ни одного', async () => {
    const store = useFlowEditorStore()
    store.catalog = testCatalog
    store.doc = doc([{ id: 'n1', op: 'start', pos: [0, 0] }, { id: 'n2', op: 'web.click', pos: [0, 100] }, { id: 'n3', op: 'web.click', pos: [0, 200] }])
    let vf!: ReturnType<typeof useVueFlow>
    const Host = defineComponent({
      setup() {
        vf = useVueFlow()
        return () => h(FlowCanvas)
      },
    })
    const wrapper = mountWithApp(Host, { attachTo: document.body })
    await flushPromises()
    const flow = wrapper.findComponent(VueFlow)
    // Конец выделения рамкой: узлы выбраны, vue-flow показывает рамку вокруг них.
    flow.vm.$emit('nodesChange', [{ id: 'n2', type: 'select', selected: true }, { id: 'n3', type: 'select', selected: true }])
    vf.nodesSelectionActive.value = true
    await flushPromises()
    expect(vf.nodesSelectionActive.value).toBe(true)
    // Выбор извне (палитра, «Ошибок: N»): рамка не должна обнять новый узел и закрыть его от двойного щелчка.
    store.selected = 'n1'
    await flushPromises()
    expect(vf.nodesSelectionActive.value).toBe(false)
    // Рамка вокруг одного узла тоже не нужна.
    vf.nodesSelectionActive.value = true
    await flushPromises()
    expect(vf.nodesSelectionActive.value).toBe(false)
  })

  it('смена выходов узла обновляет его ручки в vue-flow', async () => {
    const store = useFlowEditorStore()
    store.catalog = testCatalog
    store.doc = doc([{ id: 'n1', op: 'web.click' }])
    let internals!: MockInstance
    const Host = defineComponent({
      setup() {
        internals = vi.spyOn(useVueFlow(), 'updateNodeInternals')
        return () => h(FlowCanvas)
      },
    })
    mountWithApp(Host, { attachTo: document.body })
    await flushPromises()
    internals.mockClear()
    store.apply((d) => updateNode(d, [], 'n1', { on_error: 'branch' }))
    await flushPromises()
    expect(internals).toHaveBeenCalledWith(['n1'])
    internals.mockClear()
    store.apply((d) => updateNode(d, [], 'n1', { description: 'x' }))
    await flushPromises()
    expect(internals).not.toHaveBeenCalled()
  })

  it('щелчок по узлу выбирает его', async () => {
    const { wrapper, store } = await mountCanvas(doc([{ id: 'n1', op: 'web.click' }]))
    await wrapper.find('[data-id="n1"]').trigger('click')
    expect(store.selected).toBe('n1')
  })

  it('контур и добавленный узел используют одну позицию при масштабе и сдвиге; отмена — одним шагом', async () => {
    const store = useFlowEditorStore()
    store.catalog = testCatalog
    store.doc = doc([{ id: 'n1', op: 'start', pos: [0, 0] }])
    const drag = ref<PaletteDrag | null>(null)
    let vf!: ReturnType<typeof useVueFlow>
    const Host = defineComponent({
      setup() {
        vf = useVueFlow()
        // Геометрию окна happy-dom не рассчитывает; преобразование координат учитывает реальную формулу vue-flow.
        vi.spyOn(vf, 'screenToFlowCoordinate').mockImplementation(({ x, y }) => ({ x: (x - 200 - 40) / 2, y: (y - 100 - 80) / 2 }))
        return () => h(FlowCanvas, { paletteDrag: drag.value })
      },
    })
    const wrapper = mountWithApp(Host, { attachTo: document.body })
    await flushPromises()
    vf.viewport.value = { x: 40, y: 80, zoom: 2 }
    const canvas = wrapper.getComponent(FlowCanvas)
    vi.spyOn(canvas.get('.canvas').element, 'getBoundingClientRect').mockReturnValue(new DOMRect(200, 100, 600, 500))
    drag.value = { op: testCatalog.ops.find(op => op.id === 'web.click')!, x: 600, y: 400 }
    await flushPromises()
    expect(canvas.get('.placement-preview').text()).toContain('Щелчок мышью')
    expect(canvas.get('.placement-preview').attributes('style')).toContain('translate(180px, 228px) scale(2)')
    expect(store.doc!.nodes).toHaveLength(1)
    expect(store.canUndo).toBe(false)
    canvas.vm.dropOperation(drag.value)
    drag.value = null
    await flushPromises()
    expect(canvas.find('.placement-preview').exists()).toBe(false)
    expect(store.doc!.nodes).toHaveLength(2)
    expect(store.doc!.nodes[1]).toMatchObject({ op: 'web.click', pos: [70, 74] })
    expect(store.selected).toBe(store.doc!.nodes[1]!.id)
    store.undo()
    expect(store.doc!.nodes).toHaveLength(1)
    expect(store.canUndo).toBe(false)
    store.redo()
    expect(store.doc!.nodes[1]).toMatchObject({ pos: [70, 74] })
    wrapper.unmount()
  })

  it('отпускание вне холста и неизвестная операция не меняют документ', async () => {
    const { wrapper, store } = await mountCanvas(doc([{ id: 'n1', op: 'start' }]))
    vi.spyOn(wrapper.get('.canvas').element, 'getBoundingClientRect').mockReturnValue(new DOMRect(200, 100, 600, 500))
    const op = testCatalog.ops.find(op => op.id === 'web.click')!
    for (const [x, y] of [[100, 300], [500, 700]]) {
      await wrapper.setProps({ paletteDrag: { op, x, y } })
      expect(wrapper.find('.placement-preview').exists()).toBe(false)
      wrapper.vm.dropOperation({ op, x, y })
    }
    wrapper.vm.dropOperation({ op: { ...op, id: 'unknown' }, x: 400, y: 300 })
    wrapper.vm.dropOperation({ op: testCatalog.ops[0]!, x: 400, y: 300 })
    expect(store.doc!.nodes).toHaveLength(1)
    expect(store.canUndo).toBe(false)
    wrapper.unmount()
  })

  it('при соединении подсвечивает только допустимые входы и точку под указателем, затем сбрасывает подсказки', async () => {
    const { host, wrapper, store, vf } = await mountConnectingCanvas(doc([
      { id: 'n1', op: 'start' },
      { id: 'n2', op: 'web.click', on_error: 'branch' },
      { id: 'n3', op: 'control.for_times' },
    ]))
    const handle = (node: string, port: string) => wrapper.get(`.vue-flow__handle[data-nodeid="${node}"][data-handleid="${port}"]`)
    const before = JSON.stringify(store.doc)
    vf.startConnection({ nodeId: 'n2', id: 'error', type: 'source', position: Position.Right, x: 0, y: 0 })
    await flushPromises()
    expect(handle('n2', 'error').classes()).toContain('handle-origin')
    for (const [node, port] of [['n2', 'in'], ['n2', 'next'], ['n1', 'next'], ['n3', 'body']]) {
      expect(handle(node!, port!).classes()).toContain('handle-unavailable')
      expect(handle(node!, port!).classes()).not.toContain('connectableend')
    }
    for (const port of ['in', 'loop']) expect(handle('n3', port).classes()).toContain('handle-available')
    vf.updateConnection({ x: 10, y: 10 }, { nodeId: 'n3', id: 'loop', type: 'target', position: Position.Left, x: 10, y: 10 }, 'valid')
    await flushPromises()
    expect(handle('n3', 'loop').classes()).toContain('handle-ready')
    expect(handle('n3', 'in').classes()).toContain('handle-available')
    expect(JSON.stringify(store.doc)).toBe(before)
    vf.endConnection()
    await flushPromises()
    expect(wrapper.find('.handle-origin, .handle-available, .handle-unavailable, .handle-ready').exists()).toBe(false)
    expect(handle('n2', 'next').classes()).toContain('connectableend')
    host.unmount()
  })

  it('при начале со входа подсвечивает выходы других узлов, включая занятые, ветви условия и ошибки', async () => {
    const { host, wrapper, store, vf } = await mountConnectingCanvas({ ...doc([
      { id: 'n1', op: 'start' }, { id: 'n2', op: 'web.click', on_error: 'branch' },
      { id: 'n3', op: 'control.for_times' }, { id: 'n4', op: 'control.if' },
    ]), edges: [{ from: 'n1', to: 'n2' }] })
    // Режим соединения двумя щелчками использует те же подсказки.
    vf.startConnection({ nodeId: 'n3', id: 'loop', type: 'target', position: Position.Left, x: 0, y: 0 }, undefined, true)
    await flushPromises()
    const available = wrapper.findAll('.handle-available').map(h => `${h.attributes('data-nodeid')}:${h.attributes('data-handleid')}`)
    expect(available.sort()).toEqual(['n1:next', 'n2:error', 'n2:next', 'n4:false', 'n4:true'])
    expect(wrapper.get('[data-nodeid="n3"][data-handleid="loop"]').classes()).toContain('handle-origin')
    expect(wrapper.get('[data-nodeid="n3"][data-handleid="body"]').classes()).toContain('handle-unavailable')
    expect(store.doc!.edges).toEqual([{ from: 'n1', to: 'n2' }])
    vf.endConnection(undefined, true)
    await flushPromises()
    expect(wrapper.find('.handle-available').exists()).toBe(false)
    host.unmount()
  })

  it('предварительная проверка и запись связи отклоняют выход → выход, связь с собой и другой уровень', async () => {
    const { wrapper, store } = await mountCanvas(doc([
      { id: 'n1', op: 'start' }, { id: 'n2', op: 'web.click' },
      { id: 'g', op: 'control.group', nodes: [{ id: 'x', op: 'web.click' }, { id: 'y', op: 'wait.time' }] },
    ]))
    const flow = wrapper.getComponent(VueFlow)
    const valid = wrapper.getComponent(Handle).props('isValidConnection')!
    const elements = { nodes: [], edges: [], sourceNode: {} as never, targetNode: {} as never }
    for (const c of [
      { source: 'n1', sourceHandle: 'next', target: 'n2', targetHandle: 'next' },
      { source: 'n2', sourceHandle: 'next', target: 'n2', targetHandle: 'in' },
      { source: 'n2', sourceHandle: 'next', target: 'x', targetHandle: 'in' },
    ]) {
      expect(valid(c, elements)).toBe(false)
      flow.vm.$emit('connect', c)
    }
    expect(store.canUndo).toBe(false)
    store.enterGroup('g')
    await flushPromises()
    const connection = { source: 'x', sourceHandle: 'next', target: 'y', targetHandle: 'in' }
    expect(valid(connection, elements)).toBe(true)
    expect(valid({ ...connection, target: 'n2' }, elements)).toBe(false)
    flow.vm.$emit('connect', connection)
    expect(store.doc!.nodes[2]!.edges).toEqual([{ from: 'x', port: 'next', to: 'y', to_port: 'in' }])
    expect(store.doc!.edges).toEqual([])
    wrapper.unmount()
  })

  it('ошибочная связь загруженного процесса остаётся видимой для исправления', async () => {
    const { host, wrapper, vf } = await mountConnectingCanvas({
      ...doc([{ id: 'n1', op: 'web.click' }]),
      edges: [{ from: 'n1', port: 'next', to: 'n1', to_port: 'in' }],
    })
    expect(vf.edges.value).toHaveLength(1)
    const valid = wrapper.getComponent(Handle).props('isValidConnection')!
    expect(valid({ source: 'n1', sourceHandle: 'next', target: 'n1', targetHandle: 'in' }, {
      nodes: [], edges: [], sourceNode: {} as never, targetNode: {} as never,
    })).toBe(false)
    host.unmount()
  })

  it('«Выделить всё» выбирает только текущий уровень без правки документа, Escape снимает выделение', async () => {
    const { wrapper, store } = await mountCanvas({ ...doc([
      { id: 'n1', op: 'start' }, { id: 'n2', op: 'web.click' },
      { id: 'g', op: 'control.group', nodes: [{ id: 'x', op: 'web.click' }, { id: 'y', op: 'wait.time' }], edges: [{ from: 'x', to: 'y' }] },
    ]), edges: [{ from: 'n1', to: 'n2' }] })
    const select = () => wrapper.findAll('button').find(b => b.text() === 'Выделить всё')!.trigger('click')
    await select()
    await flushPromises()
    expect(store.selection).toEqual(['n1', 'n2', 'g'])
    expect(store.edgeSelection).toEqual(['n1:next->n2:in'])
    expect(store.selected).toBeNull()
    expect(store.canUndo).toBe(false)
    await wrapper.get('.canvas').trigger('keydown', { key: 'Escape' })
    expect(store.selection).toEqual([])
    expect(store.edgeSelection).toEqual([])
    store.enterGroup('g')
    await flushPromises()
    await select()
    await flushPromises()
    expect(store.selection).toEqual(['x', 'y'])
    expect(store.edgeSelection).toEqual(['x:next->y:in'])
    await wrapper.get('[aria-label="Снять выделение"]').trigger('click')
    expect(store.selection).toEqual([])
    expect(store.canUndo).toBe(false)
    wrapper.unmount()
  })
})
