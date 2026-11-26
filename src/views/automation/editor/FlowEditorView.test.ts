import { VueFlow } from '@vue-flow/core'
import { enableAutoUnmount, flushPromises } from '@vue/test-utils'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { h } from 'vue'
import { createMemoryHistory, createRouter, RouterView } from 'vue-router'
import type { FlowDoc } from '../../../api/types'
import UiSelect from '../../../components/UiSelect.vue'
import { viewKey } from '../../../router'
import { mountWithApp } from '../../../test/mount'

const api = vi.hoisted(() => ({
  opsCatalog: vi.fn(),
  getFlow: vi.fn(),
  saveFlow: vi.fn(),
  validateFlow: vi.fn(),
  exportFlowFile: vi.fn(),
  importFlowFile: vi.fn(),
  listFlows: vi.fn(),
  createTask: vi.fn(),
  stopDebug: vi.fn(),
}))
vi.mock('../../../api/core', () => ({ core: api, events: {} }))
const dialogs = vi.hoisted(() => ({ pickSaveFlow: vi.fn(), pickFlowFile: vi.fn() }))
vi.mock('../../../api/dialogs', () => dialogs)

import { useFlowDebugStore } from '../../../stores/flowDebug'
import { useFlowEditorStore } from '../../../stores/flowEditor'
import { useGroupsStore } from '../../../stores/groups'
import { useToasts } from '../../../stores/toasts'
import TaskDialog from '../TaskDialog.vue'
import FlowEditorView from './FlowEditorView.vue'
import { testCatalog } from './testOps'

enableAutoUnmount(afterEach)

const stored: FlowDoc = { format: 'ombra-flow/1', name: 'Вход', nodes: [{ id: 'n1', op: 'start' }] }

async function mountAt(path: string) {
  const router = createRouter({
    history: createMemoryHistory(),
    routes: [
      { path: '/automation/flows', component: { template: '<div />' } },
      { path: '/automation/tasks', component: { template: '<div />' } },
      { path: '/automation/flows/new', component: FlowEditorView },
      { path: '/automation/flows/:id', component: FlowEditorView },
    ],
  })
  await router.push(path)
  await router.isReady()
  // Ключ — как у RouterView в App.vue.
  const wrapper = mountWithApp({ render: () => h(RouterView, { key: viewKey(router.currentRoute.value) }) }, { global: { plugins: [router] }, attachTo: document.body })
  await flushPromises()
  return { wrapper, router }
}
const name = (w: Awaited<ReturnType<typeof mountAt>>['wrapper']) => w.find<HTMLInputElement>('input.flow-name')
const button = (w: Awaited<ReturnType<typeof mountAt>>['wrapper'], text: string) => w.findAll('button').find((b) => b.text() === text)!
const toasts = () => useToasts().items.map((t) => t.title)

beforeEach(() => {
  document.body.innerHTML = ''
  for (const mock of Object.values(api)) mock.mockReset()
  api.opsCatalog.mockResolvedValue(testCatalog)
  api.getFlow.mockResolvedValue(structuredClone(stored))
  api.listFlows.mockResolvedValue([])
  api.validateFlow.mockResolvedValue({ errors: [], warnings: [] })
  api.saveFlow.mockResolvedValue({ id: 7 })
  dialogs.pickSaveFlow.mockReset().mockResolvedValue('/tmp/x.json')
  dialogs.pickFlowFile.mockReset().mockResolvedValue(null)
})

describe('экран редактора', () => {
  it('импорт в верхней панели добавляет и открывает процесс', async () => {
    const { wrapper, router } = await mountAt('/automation/flows/1')
    dialogs.pickFlowFile.mockResolvedValue('/tmp/process.json')
    api.importFlowFile.mockResolvedValue({ id: 9, name: 'Из файла' })
    api.getFlow.mockResolvedValue({ ...structuredClone(stored), name: 'Из файла' })
    await button(wrapper, 'Импорт сценария').trigger('click')
    await flushPromises()
    expect(api.importFlowFile).toHaveBeenCalledWith('/tmp/process.json')
    expect(router.currentRoute.value.path).toBe('/automation/flows/9')
    expect(useFlowEditorStore().doc!.name).toBe('Из файла')
    expect(api.saveFlow).not.toHaveBeenCalled()
  })

  it('импорт сохраняет черновик до решения об уходе; отказ от перехода оставляет его открытым', async () => {
    const { wrapper, router } = await mountAt('/automation/flows/1')
    await name(wrapper).setValue('Черновик')
    dialogs.pickFlowFile.mockResolvedValue('/tmp/process.json')
    api.importFlowFile.mockResolvedValue({ id: 9, name: 'Из файла' })
    await button(wrapper, 'Импорт сценария').trigger('click')
    await flushPromises()
    expect(wrapper.find('[role="alertdialog"]').exists()).toBe(true)
    expect(router.currentRoute.value.path).toBe('/automation/flows/1')
    expect(useFlowEditorStore().doc!.name).toBe('Черновик')
    await button(wrapper, 'Отмена').trigger('click')
    expect(useFlowEditorStore().doc!.name).toBe('Черновик')
    expect(api.saveFlow).not.toHaveBeenCalled()
  })

  it('отмена выбора файла и ошибка импорта оставляют текущий процесс', async () => {
    const { wrapper, router } = await mountAt('/automation/flows/1')
    await button(wrapper, 'Импорт сценария').trigger('click')
    await flushPromises()
    expect(api.importFlowFile).not.toHaveBeenCalled()
    dialogs.pickFlowFile.mockResolvedValue('/tmp/bad.json')
    api.importFlowFile.mockRejectedValue(new Error('bad file'))
    await button(wrapper, 'Импорт сценария').trigger('click')
    await flushPromises()
    expect(router.currentRoute.value.path).toBe('/automation/flows/1')
    expect(useFlowEditorStore().doc!.name).toBe(stored.name)
    expect(useToasts().items.some((toast) => toast.kind === 'error')).toBe(true)
  })

  it('кнопка запуска открывает мастер текущего процесса без старта; новые правки блокируют повторный запуск', async () => {
    useGroupsStore().loaded = true
    api.listFlows.mockResolvedValue([{ id: 1, name: 'Вход', group: '', remark: '', has_errors: false, variables: [], created_at: 1, updated_at: 1 }])
    const { wrapper } = await mountAt('/automation/flows/1')
    expect(button(wrapper, 'Запустить задачу').attributes('disabled')).toBeUndefined()
    await button(wrapper, 'Запустить задачу').trigger('click')
    await flushPromises()
    expect(wrapper.getComponent(TaskDialog).props('open')).toBe(true)
    expect(wrapper.getComponent(TaskDialog).props('flowId')).toBe(1)
    expect(api.createTask).not.toHaveBeenCalled()
    const editor = useFlowEditorStore()
    editor.apply((doc) => ({ ...doc, name: 'Внешняя правка' }))
    await flushPromises()
    document.body.dispatchEvent(new KeyboardEvent('keydown', { key: 'z', metaKey: true, bubbles: true }))
    expect(editor.doc!.name).toBe('Внешняя правка')
    expect(button(wrapper, 'Запустить задачу').attributes('disabled')).toBeDefined()
  })

  it('новый процесс открывается со Start', async () => {
    const { wrapper } = await mountAt('/automation/flows/new')
    expect(wrapper.find('main.flow-editor').exists()).toBe(true)
    expect(wrapper.find('[data-id="n1"]').exists()).toBe(true)
    expect(name(wrapper).element.value).toBe('Новый сценарий')
  })

  it('одиночное выделение оставляет холст свободным; двойной щелчок и кнопка открывают настройки', async () => {
    const { wrapper } = await mountAt('/automation/flows/1')
    const store = useFlowEditorStore()
    const flow = wrapper.getComponent(VueFlow)
    expect(wrapper.find('aside.node-panel').exists()).toBe(false)
    flow.vm.$emit('nodesChange', [{ id: 'n1', type: 'select', selected: true }])
    await flushPromises()
    expect(store.selected).toBe('n1')
    expect(wrapper.find('#node-panel').exists()).toBe(false)

    flow.vm.$emit('nodeDoubleClick', { node: { id: 'n1' } })
    await flushPromises()
    expect(wrapper.find('aside#node-panel #node-description').exists()).toBe(true)
    expect(wrapper.find('[role="dialog"]').exists()).toBe(false)
    expect(wrapper.get('#node-panel').element.parentElement?.classList.contains('canvas-workspace')).toBe(true)
    await wrapper.get('#node-description').setValue('Начало проверки')
    await wrapper.get('#node-description').trigger('keydown', { key: 'z', metaKey: true })
    expect(store.doc!.nodes[0]!.description).toBe('Начало проверки')
    await wrapper.get('#node-panel button[aria-label="Закрыть"]').trigger('click')
    await flushPromises()
    expect(wrapper.find('#node-panel').exists()).toBe(false)
    expect(store.selected).toBe('n1')
    await button(wrapper, 'Настройки блока').trigger('click')
    await flushPromises()
    expect(wrapper.get<HTMLInputElement>('#node-description').element.value).toBe('Начало проверки')
    expect(button(wrapper, 'Настройки блока').attributes('aria-expanded')).toBe('true')
    await button(wrapper, 'Настройки блока').trigger('click')
    expect(wrapper.find('#node-panel').exists()).toBe(false)
    expect(button(wrapper, 'Настройки блока').attributes('aria-expanded')).toBe('false')
    expect(api.saveFlow).not.toHaveBeenCalled()
  })

  it('открытая правая панель следует за выделением и позволяет работать с холстом', async () => {
    api.getFlow.mockResolvedValue({ ...structuredClone(stored), nodes: [...stored.nodes, { id: 'n2', op: 'web.click' }] })
    const { wrapper } = await mountAt('/automation/flows/1')
    const store = useFlowEditorStore()
    store.openNodeSettings('n1')
    await flushPromises()
    wrapper.getComponent(VueFlow).vm.$emit('nodesChange', [
      { id: 'n1', type: 'select', selected: false }, { id: 'n2', type: 'select', selected: true },
    ])
    await flushPromises()
    expect(wrapper.get('#node-panel .panel').attributes('data-node')).toBe('n2')
    expect(store.nodeSettingsOpen).toBe(true)
    store.apply((doc) => ({ ...doc, name: 'Другое имя' }))
    document.body.dispatchEvent(new KeyboardEvent('keydown', { key: 'z', metaKey: true, bubbles: true }))
    expect(store.doc!.name).toBe(stored.name)
    await wrapper.get('#node-panel').trigger('keydown', { key: 'Escape' })
    expect(wrapper.find('#node-panel').exists()).toBe(false)
    expect(store.selected).toBe('n2')
  })

  it('настройки группы не входят внутрь; двойной щелчок по группе сохраняет переход', async () => {
    api.getFlow.mockResolvedValue({ ...structuredClone(stored), nodes: [...stored.nodes,
      { id: 'g', op: 'control.group', nodes: [{ id: 'x', op: 'web.click' }] }] })
    const { wrapper } = await mountAt('/automation/flows/1')
    const store = useFlowEditorStore()
    await wrapper.get('.vue-flow__node[data-id="g"] .node-tools button[aria-label="Настройки блока"]').trigger('click')
    await flushPromises()
    expect(store.nodeSettingsOpen).toBe(true)
    expect(store.selected).toBe('g')
    expect(store.path).toEqual([])
    await wrapper.get('#node-panel button[aria-label="Закрыть"]').trigger('click')
    wrapper.getComponent(VueFlow).vm.$emit('nodeDoubleClick', { node: { id: 'g' } })
    await flushPromises()
    expect(store.path).toEqual(['g'])
    expect(wrapper.find('#node-panel').exists()).toBe(false)
    expect(wrapper.find('.vue-flow__node[data-id="x"]').exists()).toBe(true)
  })

  it('новый процесс сохраняется и без правок', async () => {
    const created = await mountAt('/automation/flows/new')
    expect(button(created.wrapper, 'Сохранить').attributes('disabled')).toBeUndefined()
    await button(created.wrapper, 'Сохранить').trigger('click')
    await flushPromises()
    expect(api.saveFlow).toHaveBeenCalledWith(null, expect.objectContaining({ name: 'Новый сценарий' }))
    document.body.innerHTML = ''
    const { wrapper } = await mountAt('/automation/flows/1')
    expect(button(wrapper, 'Сохранить').attributes('disabled')).toBeDefined()
  })

  it('сохранение нового ведёт на его адрес', async () => {
    const { wrapper, router } = await mountAt('/automation/flows/new')
    await name(wrapper).setValue('Мой')
    await button(wrapper, 'Сохранить').trigger('click')
    await flushPromises()
    expect(api.saveFlow).toHaveBeenCalledWith(null, expect.objectContaining({ name: 'Мой' }))
    expect(router.currentRoute.value.fullPath).toBe('/automation/flows/7')
    expect(document.body.textContent).not.toContain('Закрыть без сохранения?')
    expect(toasts()).toContain('Сценарий сохранён')
  })

  it('сохранение, завершённое уже в другом процессе, не сообщает «Сценарий сохранён»', async () => {
    let done!: (r: unknown) => void
    api.saveFlow.mockImplementationOnce(() => new Promise((resolve) => (done = resolve)))
    const { wrapper } = await mountAt('/automation/flows/1')
    await name(wrapper).setValue('Правка')
    await button(wrapper, 'Сохранить').trigger('click')
    await useFlowEditorStore().load(2)
    done({ id: 1 })
    await flushPromises()
    expect(toasts()).not.toContain('Сценарий сохранён')
  })

  it('загрузка, упавшая после ухода с экрана, не уведомляет и не уводит', async () => {
    let fail!: (e: unknown) => void
    api.getFlow.mockImplementationOnce(() => new Promise((_, reject) => (fail = reject)))
    const { router } = await mountAt('/automation/flows/1')
    await router.push('/automation/tasks')
    await flushPromises()
    fail(new Error('нет'))
    await flushPromises()
    expect(router.currentRoute.value.fullPath).toBe('/automation/tasks')
    expect(useToasts().items).toEqual([])
  })

  /** Первое сохранение завершается, пока открыт вопрос об уходе: адрес — уже записанного процесса, вопрос остаётся. */
  async function saveDuringLeave() {
    let done!: (r: unknown) => void
    api.saveFlow.mockImplementationOnce(() => new Promise((resolve) => (done = resolve)))
    const { wrapper, router } = await mountAt('/automation/flows/new')
    await name(wrapper).setValue('Мой')
    await button(wrapper, 'Сохранить').trigger('click')
    void router.push('/automation/flows')
    await flushPromises()
    expect(document.body.textContent).toContain('Закрыть без сохранения?')
    done({ id: 7 })
    await flushPromises()
    expect(router.currentRoute.value.fullPath).toBe('/automation/flows/7')
    expect(document.body.textContent).toContain('Закрыть без сохранения?')
    return { wrapper, router }
  }

  it('первое сохранение при открытом вопросе об уходе: «Не сохранять» уходит', async () => {
    const { wrapper, router } = await saveDuringLeave()
    await button(wrapper, 'Не сохранять').trigger('click')
    await flushPromises()
    expect(router.currentRoute.value.fullPath).toBe('/automation/flows')
  })

  it('первое сохранение при открытом вопросе об уходе: «Отмена» оставляет на адресе процесса', async () => {
    const { wrapper, router } = await saveDuringLeave()
    await button(wrapper, 'Отмена').trigger('click')
    await flushPromises()
    expect(router.currentRoute.value.fullPath).toBe('/automation/flows/7')
    expect(api.getFlow).not.toHaveBeenCalled()
    expect(document.body.textContent).not.toContain('Закрыть без сохранения?')
  })

  it('уход с несохранёнными правками спрашивает', async () => {
    const { wrapper, router } = await mountAt('/automation/flows/1')
    await name(wrapper).setValue('Правка')
    void router.push('/automation/flows')
    await flushPromises()
    expect(document.body.textContent).toContain('Закрыть без сохранения?')
    expect(router.currentRoute.value.fullPath).toBe('/automation/flows/1')
    await button(wrapper, 'Отмена').trigger('click')
    await flushPromises()
    expect(router.currentRoute.value.fullPath).toBe('/automation/flows/1')
    void router.push('/automation/flows')
    await flushPromises()
    await button(wrapper, 'Не сохранять').trigger('click')
    await flushPromises()
    expect(router.currentRoute.value.fullPath).toBe('/automation/flows')
  })

  it('второй переход при открытом вопросе об уходе отменяется; «Не сохранять» ведёт по первому', async () => {
    const { wrapper, router } = await mountAt('/automation/flows/1')
    await name(wrapper).setValue('Правка')
    void router.push('/automation/flows')
    await flushPromises()
    expect(document.body.textContent).toContain('Закрыть без сохранения?')
    void router.push('/automation/tasks')
    await flushPromises()
    expect(router.currentRoute.value.fullPath).toBe('/automation/flows/1')
    expect(document.body.textContent).toContain('Закрыть без сохранения?')
    await button(wrapper, 'Не сохранять').trigger('click')
    await flushPromises()
    expect(router.currentRoute.value.fullPath).toBe('/automation/flows')
  })

  it('«Назад» с правками: «Не сохранять» повторяет шаг по истории — «Вперёд» снова ведёт в редактор', async () => {
    const { wrapper, router } = await mountAt('/automation/flows')
    await router.push('/automation/flows/1')
    await flushPromises()
    await name(wrapper).setValue('Правка')
    router.back()
    await flushPromises()
    expect(document.body.textContent).toContain('Закрыть без сохранения?')
    expect(router.currentRoute.value.fullPath).toBe('/automation/flows/1')
    await button(wrapper, 'Не сохранять').trigger('click')
    await flushPromises()
    expect(router.currentRoute.value.fullPath).toBe('/automation/flows')
    router.forward()
    await flushPromises()
    expect(router.currentRoute.value.fullPath).toBe('/automation/flows/1')
  })

  it('«Сохранить» с клавиатуры не теряет фокус кнопки', async () => {
    const { wrapper } = await mountAt('/automation/flows/1')
    await name(wrapper).setValue('Правка')
    const save = button(wrapper, 'Сохранить')
    ;(save.element as HTMLButtonElement).focus()
    await save.trigger('click')
    await flushPromises()
    expect(document.activeElement).toBe(save.element)
  })

  it('правка во время первого сохранения остаётся после смены адреса', async () => {
    let done!: (r: unknown) => void
    api.saveFlow.mockImplementationOnce(() => new Promise((resolve) => (done = resolve)))
    const { wrapper, router } = await mountAt('/automation/flows/new')
    await name(wrapper).setValue('Мой')
    await button(wrapper, 'Сохранить').trigger('click')
    await name(wrapper).setValue('Мой 2')
    done({ id: 7 })
    await flushPromises()
    expect(router.currentRoute.value.fullPath).toBe('/automation/flows/7')
    expect(api.getFlow).not.toHaveBeenCalled()
    const store = useFlowEditorStore()
    expect(store.doc!.name).toBe('Мой 2')
    expect(name(wrapper).element.value).toBe('Мой 2')
    expect(store.dirty).toBe(true)
    expect(store.canUndo).toBe(true)
    store.undo()
    expect(store.doc!.name).not.toBe('Мой 2')
  })

  it('другой процесс с правками — тот же вопрос, потом загрузка', async () => {
    const { wrapper, router } = await mountAt('/automation/flows/1')
    await name(wrapper).setValue('Правка')
    void router.push('/automation/flows/2')
    await flushPromises()
    expect(document.body.textContent).toContain('Закрыть без сохранения?')
    expect(router.currentRoute.value.fullPath).toBe('/automation/flows/1')
    await button(wrapper, 'Не сохранять').trigger('click')
    await flushPromises()
    expect(router.currentRoute.value.fullPath).toBe('/automation/flows/2')
    expect(api.getFlow).toHaveBeenLastCalledWith(2)
    expect(useFlowEditorStore().id).toBe(2)
    expect(name(wrapper).element.value).toBe('Вход')
    await router.push('/automation/flows/new')
    await flushPromises()
    expect(useFlowEditorStore().id).toBeNull()
    expect(name(wrapper).element.value).toBe('Новый сценарий')
  })

  it('пока редактор грузится или не загрузился, уход не спрашивает', async () => {
    const store = useFlowEditorStore()
    store.doc = structuredClone(stored)
    expect(store.dirty).toBe(true)
    api.getFlow.mockRejectedValue(new Error('нет'))
    const { router } = await mountAt('/automation/flows/1')
    expect(document.body.textContent).not.toContain('Закрыть без сохранения?')
    expect(router.currentRoute.value.fullPath).toBe('/automation/flows')
  })

  it('ошибка имени — у поля имени; счётчик без узла ведёт к Start наверху', async () => {
    api.getFlow.mockResolvedValue({ ...structuredClone(stored), nodes: [{ id: 's', op: 'start' }, { id: 'g', op: 'control.group', nodes: [{ id: 'x', op: 'web.click' }] }] })
    const { wrapper } = await mountAt('/automation/flows/1')
    const store = useFlowEditorStore()
    store.enterGroup('g')
    store.report = { errors: [{ node: null, field: 'name', message: 'имя процесса — от 1 до 100 символов' }, { node: 'x', field: null, message: 'b' }], warnings: [{ node: 'x', field: null, message: 'w' }] }
    await flushPromises()
    expect(name(wrapper).classes()).toContain('invalid')
    expect(name(wrapper).attributes('title')).toBe('имя процесса — от 1 до 100 символов')
    await button(wrapper, 'Ошибок: 2').trigger('click')
    expect(store.path).toEqual([])
    expect(store.selected).toBe('s')
  })

  it('путь в группу — по имени группы', async () => {
    api.getFlow.mockResolvedValue({ ...structuredClone(stored), nodes: [{ id: 's', op: 'start' }, { id: 'g', op: 'control.group', description: 'd', config: { name: 'Вход' }, nodes: [{ id: 'h', op: 'control.group', nodes: [] }] }] })
    const { wrapper } = await mountAt('/automation/flows/1')
    useFlowEditorStore().enterGroup('g')
    useFlowEditorStore().enterGroup('h')
    await flushPromises()
    expect(wrapper.find('.crumbs').text()).toBe('Сценарий›Группа «Вход»›Группа')
  })

  it('⌘Z отменяет правку имени', async () => {
    const { wrapper } = await mountAt('/automation/flows/1')
    await name(wrapper).setValue('Правка')
    expect(useFlowEditorStore().doc?.name).toBe('Правка')
    document.body.dispatchEvent(new KeyboardEvent('keydown', { key: 'z', metaKey: true, bubbles: true }))
    await flushPromises()
    expect(useFlowEditorStore().doc?.name).toBe('Вход')
    expect(name(wrapper).element.value).toBe('Вход')
  })

  it('⇧⌘Z возвращает отменённое', async () => {
    const { wrapper } = await mountAt('/automation/flows/1')
    await name(wrapper).setValue('Правка')
    const key = (shiftKey: boolean) => document.body.dispatchEvent(new KeyboardEvent('keydown', { key: 'z', metaKey: true, shiftKey, bubbles: true }))
    key(false)
    await flushPromises()
    expect(useFlowEditorStore().doc?.name).toBe('Вход')
    key(true)
    await flushPromises()
    expect(useFlowEditorStore().doc?.name).toBe('Правка')
    expect(name(wrapper).element.value).toBe('Правка')
  })

  it('⌘Z и ⇧⌘Z не действуют под диалогом настроек и вопросом об уходе', async () => {
    const { wrapper, router } = await mountAt('/automation/flows/1')
    await name(wrapper).setValue('Правка')
    const store = useFlowEditorStore()
    const undo = () => document.body.dispatchEvent(new KeyboardEvent('keydown', { key: 'z', metaKey: true, bubbles: true }))
    await button(wrapper, 'Настройки сценария').trigger('click')
    await flushPromises()
    undo()
    await flushPromises()
    expect(store.doc?.name).toBe('Правка')
    await button(wrapper, 'Применить').trigger('click')
    void router.push('/automation/flows')
    await flushPromises()
    expect(document.body.textContent).toContain('Закрыть без сохранения?')
    undo()
    await flushPromises()
    expect(store.doc?.name).toBe('Правка')
  })

  it('сохранение забирает черновик поля в фокусе: «1.» в умолчании числа — 1', async () => {
    api.getFlow.mockResolvedValue({ ...structuredClone(stored), variables: [{ name: 'x', type: 'number', default: null, description: '' }] })
    const { wrapper } = await mountAt('/automation/flows/1')
    useFlowEditorStore().openNodeSettings('n1')
    await flushPromises()
    const input = wrapper.find<HTMLInputElement>('input.var-default')
    input.element.focus()
    await input.setValue('1.')
    await wrapper.get('#node-panel button[aria-label="Закрыть"]').trigger('click')
    await button(wrapper, 'Сохранить').trigger('click')
    await flushPromises()
    expect(api.saveFlow.mock.calls[0]![1].variables[0].default).toBe(1)
  })

  it('щелчок по палитре без выбранного узла ставит узел у центра холста', async () => {
    const { wrapper } = await mountAt('/automation/flows/1')
    const canvas = wrapper.find('.canvas').element
    vi.spyOn(canvas, 'getBoundingClientRect').mockReturnValue({ left: 0, top: 0, width: 800, height: 600, right: 800, bottom: 600, x: 0, y: 0, toJSON: () => ({}) })
    const store = useFlowEditorStore()
    store.selected = null
    await wrapper.findAll('.op-item').find((i) => i.find('.name').text() === 'Щелчок мышью')!.trigger('click')
    const added = store.doc!.nodes.find((n) => n.op === 'web.click')!
    expect(added.pos).toEqual([400 - 110, 300 - 36])
    expect(store.selected).toBe(added.id)
    expect(store.doc!.edges ?? []).toEqual([])
  })

  it('настройки процесса применяются', async () => {
    const { wrapper } = await mountAt('/automation/flows/1')
    await button(wrapper, 'Настройки сценария').trigger('click')
    await flushPromises()
    await wrapper.find('#flow-group').setValue('Кабинеты')
    await wrapper.find('#flow-timeout').setValue('90')
    wrapper.get('[data-test="browser-mode"]').findComponent(UiSelect).vm.$emit('update:modelValue', 'headless')
    await button(wrapper, 'Применить').trigger('click')
    await flushPromises()
    const doc = useFlowEditorStore().doc!
    expect(doc.group).toBe('Кабинеты')
    expect(doc.settings?.timeout_s).toBe(90)
    expect(doc.settings?.headless).toBe(true)
  })

  it('«Применить» без изменений не правит процесс', async () => {
    api.getFlow.mockResolvedValue({ ...structuredClone(stored), group: '', remark: ' ', settings: { timeout_s: 5, after_tabs: 'clear', save_password: true } })
    const { wrapper } = await mountAt('/automation/flows/1')
    await button(wrapper, 'Настройки сценария').trigger('click')
    await flushPromises()
    await button(wrapper, 'Применить').trigger('click')
    await flushPromises()
    const store = useFlowEditorStore()
    expect(store.dirty).toBe(false)
    expect(store.canUndo).toBe(false)
  })

  it('тайм-аут и задержка — только целые от 0: неверное остаётся с ошибкой, диалог не применяет', async () => {
    const { wrapper } = await mountAt('/automation/flows/1')
    await button(wrapper, 'Настройки сценария').trigger('click')
    await flushPromises()
    for (const [id, bad] of [['#flow-timeout', '-1'], ['#flow-delay', '1.5']] as const) {
      await wrapper.find(id).setValue(bad)
      await button(wrapper, 'Применить').trigger('click')
      await flushPromises()
      expect(wrapper.find<HTMLInputElement>(id).element.value).toBe(bad)
      expect(wrapper.find(id).classes()).toContain('invalid')
      expect(wrapper.text()).toContain('Целое число, не меньше 0')
      expect(useFlowEditorStore().doc!.settings).toBeUndefined()
      await wrapper.find(id).setValue('')
    }
    await wrapper.find('#flow-delay').setValue('2')
    await button(wrapper, 'Применить').trigger('click')
    await flushPromises()
    expect(useFlowEditorStore().doc!.settings?.delay_min).toBe(2)
  })

  it('экспорт неактивен у нового и при правках', async () => {
    const created = await mountAt('/automation/flows/new')
    expect(button(created.wrapper, 'Экспорт').attributes('disabled')).toBeDefined()
    document.body.innerHTML = ''
    const { wrapper } = await mountAt('/automation/flows/1')
    expect(button(wrapper, 'Экспорт').attributes('disabled')).toBeUndefined()
    await name(wrapper).setValue('Правка')
    expect(button(wrapper, 'Экспорт').attributes('disabled')).toBeDefined()
  })

  it('уход из редактора останавливает идущую отладку', async () => {
    const { wrapper } = await mountAt('/automation/flows/1')
    const debug = useFlowDebugStore()
    debug.running = true
    debug.runId = 'r1'
    api.stopDebug.mockResolvedValue(true)
    wrapper.unmount()
    expect(api.stopDebug).toHaveBeenCalledWith('r1')
  })
})

describe('выделение, буфер и отключение', () => {
  const two: FlowDoc = { format: 'ombra-flow/1', name: 'Два', nodes: [{ id: 'n1', op: 'start', pos: [0, 0] }, { id: 'n2', op: 'web.click', pos: [0, 100] }, { id: 'n3', op: 'web.click', pos: [0, 200] }], edges: [{ from: 'n2', to: 'n3' }] }
  const open = async () => {
    api.getFlow.mockResolvedValue(structuredClone(two))
    const m = await mountAt('/automation/flows/1')
    const store = useFlowEditorStore()
    store.selection = ['n2', 'n3']
    await flushPromises()
    return { ...m, store }
  }
  const key = (k: string, target: EventTarget = document.body) => target.dispatchEvent(new KeyboardEvent('keydown', { key: k, metaKey: true, bubbles: true }))

  it.each(['metaKey', 'ctrlKey'])('%s + A выбирает блоки, а в поле ввода оставляет нативное выделение текста', async (modifier) => {
    const { wrapper, store } = await open()
    const all = new KeyboardEvent('keydown', { key: 'a', [modifier]: true, bubbles: true, cancelable: true })
    document.body.dispatchEvent(all)
    await flushPromises()
    expect(all.defaultPrevented).toBe(true)
    expect(store.selection).toEqual(['n1', 'n2', 'n3'])
    expect(wrapper.find('#node-panel').exists()).toBe(false)
    const edit = new KeyboardEvent('keydown', { key: 'a', [modifier]: true, bubbles: true, cancelable: true })
    name(wrapper).element.dispatchEvent(edit)
    expect(edit.defaultPrevented).toBe(false)
    expect(store.selection).toEqual(['n1', 'n2', 'n3'])
    expect(store.canUndo).toBe(false)
    wrapper.unmount()
  })

  it('два выбранных: панель «Выбрано: 2», «Отключить» одним шагом, повторно «Включить»', async () => {
    const { wrapper, store } = await open()
    store.openNodeSettings()
    await flushPromises()
    expect(wrapper.find('.selection-panel').text()).toContain('Выбрано: 2')
    await button(wrapper, 'Отключить').trigger('click')
    expect(store.doc!.nodes.map((n) => n.disabled === true)).toEqual([false, true, true])
    expect(button(wrapper, 'Включить')).toBeTruthy()
    store.undo()
    await flushPromises()
    expect(store.doc!.nodes.some((n) => n.disabled)).toBe(false)
    await button(wrapper, 'Отключить').trigger('click')
    await button(wrapper, 'Включить').trigger('click')
    expect(store.doc!.nodes.some((n) => n.disabled)).toBe(false)
  })

  it('«Удалить» в панели выделения убирает всё, кроме Start, одним шагом', async () => {
    const { wrapper, store } = await open()
    store.selection = ['n1', 'n2', 'n3']
    store.openNodeSettings()
    await flushPromises()
    await button(wrapper, 'Удалить').trigger('click')
    expect(store.doc!.nodes.map((n) => n.id)).toEqual(['n1'])
    expect(store.doc!.edges ?? []).toEqual([])
    expect(store.nodeSettingsOpen).toBe(false)
    store.undo()
    await flushPromises()
    expect(store.doc!.nodes.map((n) => n.id)).toEqual(['n1', 'n2', 'n3'])
    expect(store.doc!.edges).toEqual([{ from: 'n2', to: 'n3' }])
  })

  it('«Удалить» в панели убирает и связи, выбранные отдельно (⌘-щелчок), как Delete', async () => {
    const four: FlowDoc = { ...two, nodes: [...two.nodes, { id: 'n4', op: 'web.click', pos: [0, 300] }], edges: [{ from: 'n1', to: 'n2' }, { from: 'n3', to: 'n4' }] }
    api.getFlow.mockResolvedValue(structuredClone(four))
    const { wrapper } = await mountAt('/automation/flows/1')
    const store = useFlowEditorStore()
    store.selection = ['n3', 'n4']
    store.openNodeSettings()
    await flushPromises()
    wrapper.findComponent(VueFlow).vm.$emit('edgesChange', [{ id: 'n1:next->n2:in', type: 'select', selected: true }])
    await flushPromises()
    await button(wrapper, 'Удалить').trigger('click')
    expect(store.doc!.nodes.map((n) => n.id)).toEqual(['n1', 'n2'])
    expect(store.doc!.edges ?? []).toEqual([])
  })

  it('⌘C, ⌘V вставляет копию и выделяет её; верхняя панель предлагает «Вставить (2)»', async () => {
    const { wrapper, store } = await open()
    key('c')
    key('v')
    await flushPromises()
    expect(store.doc!.nodes).toHaveLength(5)
    expect(store.selection).toEqual(['n4', 'n5'])
    expect(wrapper.findAll('.vue-flow__node.selected')).toHaveLength(2)
    store.selection = []
    await flushPromises()
    expect(button(wrapper, 'Вставить (2)')).toBeTruthy()
  })

  it('⌘C и ⌘V в поле ввода холст не трогают', async () => {
    const { wrapper, store } = await open()
    const input = wrapper.find<HTMLInputElement>('input.flow-name').element
    key('c', input)
    key('v', input)
    await flushPromises()
    expect(store.doc!.nodes).toHaveLength(3)
  })
})

describe('«Сгруппировать» и «Разгруппировать»', () => {
  const doc: FlowDoc = {
    format: 'ombra-flow/1', name: 'Цепочка',
    nodes: [{ id: 'n1', op: 'start', pos: [0, 0] }, { id: 'n2', op: 'web.click', pos: [0, 100] }, { id: 'n3', op: 'web.click', pos: [0, 200] }, { id: 'n4', op: 'web.click', pos: [0, 300] }],
    edges: [{ from: 'n1', to: 'n2' }, { from: 'n2', to: 'n3' }, { from: 'n3', to: 'n4' }],
  }
  const open = async (selection: string[]) => {
    api.getFlow.mockResolvedValue(structuredClone(doc))
    const m = await mountAt('/automation/flows/1')
    const store = useFlowEditorStore()
    store.selection = selection
    await flushPromises()
    return { ...m, store }
  }
  const key = (k: string, shiftKey = false) => document.body.dispatchEvent(new KeyboardEvent('keydown', { key: k, metaKey: true, shiftKey, bubbles: true }))
  const refusal = 'Сгруппировать можно цепочку с одним входом и одним выходом'

  it('кнопка «Сгруппировать»: группа выделена, один шаг отмены', async () => {
    const { wrapper, store } = await open(['n2', 'n3'])
    await button(wrapper, 'Сгруппировать').trigger('click')
    await flushPromises()
    const g = store.doc!.nodes.find((n) => n.op === 'control.group')!
    expect(g.nodes!.map((n) => n.id)).toEqual(['n2', 'n3'])
    expect(store.selection).toEqual([g.id])
    store.undo()
    await flushPromises()
    expect(store.doc!.nodes).toHaveLength(4)
  })

  it('не цепочка: уведомление и кнопкой, и ⌘G, документ не меняется', async () => {
    const { wrapper, store } = await open(['n1', 'n2'])
    await button(wrapper, 'Сгруппировать').trigger('click')
    expect(toasts()).toEqual([refusal])
    key('g')
    await flushPromises()
    expect(toasts()).toEqual([refusal, refusal])
    expect(store.doc!.nodes).toHaveLength(4)
    expect(store.canUndo).toBe(false)
  })

  it('⌘G группирует, ⇧⌘G и кнопка у формы группы разгруппируют — по шагу отмены', async () => {
    const { wrapper, store } = await open(['n2', 'n3'])
    key('g')
    await flushPromises()
    const gid = store.selection[0]!
    expect(store.doc!.nodes.find((n) => n.id === gid)?.op).toBe('control.group')
    expect(button(wrapper, 'Разгруппировать')).toBeTruthy()
    key('g', true)
    await flushPromises()
    expect(store.doc!.nodes.map((n) => n.id)).toEqual(['n1', 'n4', 'n2', 'n3'])
    expect(store.selection).toEqual(['n2', 'n3'])
    store.undo()
    await flushPromises()
    expect(store.doc!.nodes.some((n) => n.id === gid)).toBe(true)
    store.selection = [gid]
    await flushPromises()
    await button(wrapper, 'Разгруппировать').trigger('click')
    expect(store.doc!.nodes.some((n) => n.id === gid)).toBe(false)
  })
})
