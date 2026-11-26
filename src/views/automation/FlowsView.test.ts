import { enableAutoUnmount, flushPromises } from '@vue/test-utils'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { createMemoryHistory, createRouter } from 'vue-router'
import type { FlowRow } from '../../api/types'
import { mountWithApp } from '../../test/mount'

const api = vi.hoisted(() => ({
  listFlows: vi.fn(),
  listTasks: vi.fn(async () => []),
  importFlowFile: vi.fn(),
  importFlowJson: vi.fn(),
  exportFlowFile: vi.fn(),
  copyFlow: vi.fn(),
  deleteFlow: vi.fn(),
  setFlowDefaults: vi.fn(),
  createTask: vi.fn(),
}))
vi.mock('../../api/core', () => ({ core: api, events: {} }))
const dialogs = vi.hoisted(() => ({ pickFlowFile: vi.fn(), pickSaveFlow: vi.fn(), pickTableFile: vi.fn() }))
vi.mock('../../api/dialogs', () => dialogs)

import { useToasts } from '../../stores/toasts'
import { useGroupsStore } from '../../stores/groups'
import FlowsView from './FlowsView.vue'
import TaskDialog from './TaskDialog.vue'

enableAutoUnmount(afterEach)

const flow = (patch: Partial<FlowRow>): FlowRow => ({
  id: 1, name: 'Вход', group: 'Кабинеты', remark: 'через CSV', created_at: 1_700_000_000, updated_at: 1_700_000_000,
  has_errors: false, variables: [{ name: 'login', type: 'string', default: 'ivan', description: '' }], ...patch,
})
const flows = () => [flow({}), flow({ id: 2, name: 'Черновик', group: '', remark: '', has_errors: true, variables: [] })]

async function mountView() {
  const router = createRouter({ history: createMemoryHistory(), routes: [{ path: '/:p(.*)*', component: { template: '<div />' } }] })
  const wrapper = mountWithApp(FlowsView, { global: { plugins: [router] }, attachTo: document.body })
  await flushPromises()
  return wrapper
}
type Wrapper = Awaited<ReturnType<typeof mountView>>
const toastTitles = () => useToasts().items.map((t) => t.title)
async function menu(w: Wrapper, row: number, item: string) {
  await w.findAll('.body .row')[row]!.find('button[aria-label="Ещё действия"]').trigger('keydown', { key: 'ArrowDown' })
  await flushPromises()
  const el = w.findAll('[role="menuitem"]').find((i) => i.text() === item)!
  return el
}

beforeEach(() => {
  for (const mock of [...Object.values(api), ...Object.values(dialogs)]) mock.mockReset()
  api.listFlows.mockResolvedValue(flows())
  api.listTasks.mockResolvedValue([])
})

describe('экран «Процессы»', () => {
  it('строки, метка ошибок и вкладки', async () => {
    const w = await mountView()
    const rows = w.findAll('.body .row')
    expect(rows).toHaveLength(2)
    expect(rows[0]!.text()).toContain('Вход')
    expect(rows[0]!.text()).toContain('через CSV')
    expect(rows[0]!.text()).not.toContain('есть ошибки')
    expect(rows[1]!.text()).toContain('есть ошибки')
    expect(w.find('main').classes()).toContain('automation-flows')
    expect(w.text()).toContain('История запусков')
  })

  it('поиск по имени и группе без учёта регистра', async () => {
    const w = await mountView()
    await w.find('input[type="search"]').setValue('кабинеты')
    expect(w.findAll('.body .row')).toHaveLength(1)
    await w.find('input[type="search"]').setValue('ЧЕРН')
    expect(w.findAll('.body .row')[0]!.text()).toContain('Черновик')
  })

  it('пусто — подсказка', async () => {
    api.listFlows.mockResolvedValue([])
    const w = await mountView()
    expect(w.text()).toContain('Сценариев пока нет')
  })

  it('импорт: файл, процесс, тост', async () => {
    dialogs.pickFlowFile.mockResolvedValue('/tmp/a.json')
    api.importFlowFile.mockResolvedValue(flow({}))
    const w = await mountView()
    await w.findAll('button').find((b) => b.text() === 'Импорт')!.trigger('click')
    await flushPromises()
    expect(api.importFlowFile).toHaveBeenCalledWith('/tmp/a.json')
    expect(toastTitles()).toContain('Сценарий «Вход» импортирован')
  })

  it('удаление с подтверждением', async () => {
    api.deleteFlow.mockResolvedValue(null)
    const w = await mountView()
    await (await menu(w, 0, 'Удалить')).trigger('click')
    await flushPromises()
    expect(w.text()).toContain('Удалить сценарий «Вход»?')
    expect(w.text()).toContain('Задачи этого сценария не смогут повторить запуск.')
    await w.findAll('button').find((b) => b.text() === 'Удалить')!.trigger('click')
    await flushPromises()
    expect(api.deleteFlow).toHaveBeenCalledWith(1)
  })

  it('«Запустить…» неактивно у процесса с ошибками', async () => {
    const w = await mountView()
    const rows = w.findAll('.body .row')
    expect(rows[0]!.get('.quick-run').attributes('disabled')).toBeUndefined()
    expect(rows[1]!.get('.quick-run').attributes('disabled')).toBeDefined()
    await rows[1]!.get('.quick-run').trigger('click')
    expect(w.getComponent(TaskDialog).props('open')).toBe(false)
    expect((await menu(w, 1, 'Запустить…')).attributes('data-disabled')).toBeDefined()
  })

  it('быстрый запуск открывает мастер выбранного процесса без создания задачи', async () => {
    useGroupsStore().loaded = true
    const w = await mountView()
    await w.vm.$router.push('/automation/flows')
    await w.findAll('.body .row')[0]!.get('.quick-run').trigger('click')
    await flushPromises()
    expect(w.getComponent(TaskDialog).props('open')).toBe(true)
    expect(w.getComponent(TaskDialog).props('flowId')).toBe(1)
    expect(w.find('[role="dialog"]').exists()).toBe(true)
    expect(w.vm.$router.currentRoute.value.path).toBe('/automation/flows')
    expect(api.createTask).not.toHaveBeenCalled()
  })

  it('быстрое редактирование открывает выбранный процесс, включая черновик с ошибками', async () => {
    const w = await mountView()
    const row = w.findAll('.body .row')[1]!
    expect(row.get('.quick-edit').text()).toBe('Изменить')
    await row.get('.quick-edit').trigger('click')
    await flushPromises()
    expect(w.vm.$router.currentRoute.value.path).toBe('/automation/flows/2')
  })

  it('«Создать» и «Изменить» ведут в редактор', async () => {
    const w = await mountView()
    const router = w.vm.$router
    await w.findAll('button').find((b) => b.text() === 'Создать')!.trigger('click')
    await flushPromises()
    expect(router.currentRoute.value.fullPath).toBe('/automation/flows/new')
    await (await menu(w, 1, 'Изменить')).trigger('click')
    await flushPromises()
    expect(router.currentRoute.value.fullPath).toBe('/automation/flows/2')
    await w.findAll('.body .row')[0]!.trigger('dblclick')
    await flushPromises()
    expect(router.currentRoute.value.fullPath).toBe('/automation/flows/1')
  })

  it('двойной щелчок по кнопке «ещё» и в её меню редактор не открывает', async () => {
    const w = await mountView()
    const router = w.vm.$router
    await router.push('/automation/flows')
    await w.findAll('.body .row')[0]!.find('button[aria-label="Ещё действия"]').trigger('dblclick')
    await w.findAll('.body .row')[0]!.get('.quick-run').trigger('dblclick')
    await w.findAll('.body .row')[0]!.get('.quick-edit').trigger('dblclick')
    await flushPromises()
    expect(router.currentRoute.value.fullPath).toBe('/automation/flows')
    // В тестах меню не вынесено в body (заглушка teleport) — оно внутри строки, как кнопка.
    const item = await menu(w, 0, 'Экспорт')
    await item.trigger('dblclick')
    await flushPromises()
    expect(router.currentRoute.value.fullPath).toBe('/automation/flows')
  })

  it('экспорт и копирование', async () => {
    dialogs.pickSaveFlow.mockResolvedValue('/tmp/out.json')
    api.exportFlowFile.mockResolvedValue(null)
    api.copyFlow.mockResolvedValue(flow({ id: 3 }))
    const w = await mountView()
    await (await menu(w, 0, 'Экспорт')).trigger('click')
    await flushPromises()
    expect(dialogs.pickSaveFlow).toHaveBeenCalledWith('Вход.json')
    expect(api.exportFlowFile).toHaveBeenCalledWith(1, '/tmp/out.json')
    await (await menu(w, 0, 'Копировать')).trigger('click')
    await flushPromises()
    expect(api.copyFlow).toHaveBeenCalledWith(1)
  })

  it('умолчания уходят в setFlowDefaults', async () => {
    api.setFlowDefaults.mockResolvedValue(flow({}))
    const w = await mountView()
    await (await menu(w, 0, 'Параметры по умолчанию')).trigger('click')
    await flushPromises()
    await w.find('#task-param-login').setValue('petr')
    await w.findAll('button').find((b) => b.text() === 'Сохранить')!.trigger('click')
    await flushPromises()
    expect(api.setFlowDefaults).toHaveBeenCalledWith(1, { login: 'petr' })
  })

  it('очищенное умолчание уходит как null, нетронутое не уходит', async () => {
    api.setFlowDefaults.mockResolvedValue(flow({}))
    const w = await mountView()
    await (await menu(w, 0, 'Параметры по умолчанию')).trigger('click')
    await flushPromises()
    await w.find('#task-param-login').setValue('')
    await w.findAll('button').find((b) => b.text() === 'Сохранить')!.trigger('click')
    await flushPromises()
    expect(api.setFlowDefaults).toHaveBeenCalledWith(1, { login: null })
  })

  it('булево умолчание "true" показано включённым и не уходит без правки', async () => {
    api.listFlows.mockResolvedValue([flow({ variables: [{ name: 'on', type: 'boolean', default: 'true', description: '' }] })])
    api.setFlowDefaults.mockResolvedValue(flow({}))
    const w = await mountView()
    await (await menu(w, 0, 'Параметры по умолчанию')).trigger('click')
    await flushPromises()
    expect(w.find('[role="switch"]').attributes('data-state')).toBe('checked')
    await w.findAll('button').find((b) => b.text() === 'Сохранить')!.trigger('click')
    await flushPromises()
    expect(api.setFlowDefaults).toHaveBeenCalledWith(1, {})
  })

  it('булево без умолчания не уходит как false, пока его не меняли', async () => {
    api.listFlows.mockResolvedValue([flow({ variables: [{ name: 'on', type: 'boolean', default: null, description: '' }] })])
    api.setFlowDefaults.mockResolvedValue(flow({}))
    const w = await mountView()
    await (await menu(w, 0, 'Параметры по умолчанию')).trigger('click')
    await flushPromises()
    expect(w.find('[role="switch"]').attributes('data-state')).toBe('unchecked')
    await w.findAll('button').find((b) => b.text() === 'Сохранить')!.trigger('click')
    await flushPromises()
    expect(api.setFlowDefaults).toHaveBeenCalledWith(1, {})
  })
})


describe('built-in library', () => {
  it('opens the dedicated catalog from the workflows screen', async () => {
    const w = await mountView()
    await w.findAll('button').find(b => b.text() === 'Библиотека')!.trigger('click')
    await flushPromises()
    expect(w.vm.$router.currentRoute.value.path).toBe('/automation/library')
    expect(w.find('[role="dialog"]').exists()).toBe(false)
  })
})
