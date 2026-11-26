import { flushPromises } from '@vue/test-utils'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { createMemoryHistory, createRouter } from 'vue-router'
import type { BatchSummary, TaskRow, TaskDetails } from '../../api/types'
import { CoreError } from '../../api/errors'
import { mountWithApp } from '../../test/mount'
import { useToasts } from '../../stores/toasts'
import { useGroupsStore } from '../../stores/groups'
import { profileRow } from '../../test/fixtures'

const api = vi.hoisted(() => ({
  listFlows: vi.fn(async () => []),
  listTasks: vi.fn(),
  startTask: vi.fn(),
  stopTask: vi.fn(),
  cancelWaiting: vi.fn(),
  deleteTask: vi.fn(),
  getTask: vi.fn(),
  updateTask: vi.fn(),
}))
vi.mock('../../api/core', () => ({ core: api, events: {} }))
vi.mock('../../api/dialogs', () => ({ pickTableFile: vi.fn() }))

import TasksView from './TasksView.vue'

const batch = (patch: Partial<BatchSummary>): BatchSummary => ({
  id: 1, started_at: 1_700_000_000, finished_at: null, total: 10, queued: 0, waiting: 0, running: 0, done: 0,
  error: 0, cancelled: 0, skipped: 0, ...patch,
})
const task = (id: number, name: string, running: boolean, last: BatchSummary | null): TaskRow => ({
  id, name, flow_id: 1, flow_name: 'Вход', profiles: 10, threads: 2, order: 'ordered', missing_row: 'skip', distribution: 'per_profile', queue: null,
  created_at: 1, running, last,
})
const tasks = () => [
  task(1, 'Идёт', true, batch({ running: 2, done: 4, error: 1 })),
  task(2, 'Кончилась', false, batch({ finished_at: 1_700_000_100, done: 7, error: 3 })),
  task(3, 'Чисто', false, batch({ finished_at: 1_700_000_100, done: 10 })),
  task(4, 'Отмена', false, batch({ finished_at: 1_700_000_100, done: 2, cancelled: 8 })),
  task(5, 'Новая', false, null),
  task(6, 'Чужая', false, batch({ done: 1 })),
]

async function mountView() {
  const router = createRouter({ history: createMemoryHistory(), routes: [{ path: '/:p(.*)*', component: { template: '<div />' } }] })
  const wrapper = mountWithApp(TasksView, { global: { plugins: [router] }, attachTo: document.body })
  await flushPromises()
  return wrapper
}
type Wrapper = Awaited<ReturnType<typeof mountView>>
const menuItem = (w: Wrapper, label: string) => w.findAll('[role="menuitem"]').find((i) => i.text() === label)!
const disabled = (w: Wrapper, label: string) => menuItem(w, label).attributes('data-disabled') !== undefined
async function items(w: Wrapper, row: number) {
  await w.findAll('.body .row')[row]!.find('button[aria-label="Ещё действия"]').trigger('keydown', { key: 'ArrowDown' })
  await flushPromises()
  return w.findAll('[role="menuitem"]').map((i) => i.text())
}

beforeEach(() => {
  for (const mock of Object.values(api)) mock.mockReset()
  api.listFlows.mockResolvedValue([])
  api.listTasks.mockResolvedValue(tasks())
  api.getTask.mockImplementation(async (id: number): Promise<TaskDetails> => {
    const row = tasks().find((task) => task.id === id)!
    return { id, revision: 0, name: row.name, flow_id: 1, flow: null,
      process: { id: 1, name: 'Вход', variables: [], has_errors: false, group: '', remark: '', created_at: 1, updated_at: 1 },
      profiles: ['a', 'b'], distribution: 'per_profile', params: {}, data: null, threads: row.threads,
      order: row.order, missing_row: row.missing_row, created_at: 1 }
  })
  const groups = useGroupsStore()
  groups.loaded = true
  groups.rows = [profileRow({ id: 'a', serial: 1 }), profileRow({ id: 'b', serial: 2 })]
})

async function reviewEdit(w: Wrapper) {
  await w.findAll('button').find((button) => button.text() === 'К профилям')!.trigger('click')
  await flushPromises()
  await w.findAll('button').find((button) => button.text() === 'Далее')!.trigger('click')
  await flushPromises()
}

describe('экран «Задачи»', () => {
  it('видимая кнопка открывает настройки, сохранение обновляет строку без запуска', async () => {
    const w = await mountView()
    await w.findAll('.body .row')[1]!.findAll('button').find((button) => button.text() === 'Изменить')!.trigger('click')
    await flushPromises()
    await reviewEdit(w)
    expect(w.find('#task-name').element).toHaveProperty('value', 'Кончилась')
    expect(w.find('#task-threads').element).toHaveProperty('value', '2')
    await w.find('#task-name').setValue('Дневной сбор')
    await w.find('#task-threads').setValue('4')
    api.listTasks.mockResolvedValue(tasks().map((row) => row.id === 2 ? { ...row, name: 'Дневной сбор', threads: 4 } : row))
    await w.findAll('button').find((button) => button.text() === 'Сохранить')!.trigger('click')
    await flushPromises()
    expect(api.updateTask).toHaveBeenCalledWith(2, expect.objectContaining({ name: 'Дневной сбор', threads: 4, flow_id: 1, profile_ids: ['a', 'b'], data: null }))
    expect(api.startTask).not.toHaveBeenCalled()
    expect(w.findAll('.body .row')[1]!.text()).toContain('Дневной сбор')
    expect(w.find('[role="dialog"]').exists()).toBe(false)
  })

  it('изменение недоступно для своего и чужого выполняющегося пакета', async () => {
    const w = await mountView()
    for (const row of [0, 5]) {
      const button = w.findAll('.body .row')[row]!.findAll('button').find((item) => item.text() === 'Изменить')!
      expect(button.attributes('disabled')).toBeDefined()
      expect(button.attributes('title')).toContain('сначала остановите')
      await items(w, row)
      expect(disabled(w, 'Изменить')).toBe(true)
      await w.trigger('keydown', { key: 'Escape' })
      await flushPromises()
    }
  })

  it('проверяет настройки, сохраняет форму при ошибке ядра и отменяет без записи', async () => {
    const w = await mountView()
    await items(w, 1)
    await menuItem(w, 'Изменить').trigger('click')
    await flushPromises()
    await reviewEdit(w)
    await w.find('#task-threads').setValue('0')
    await w.findAll('button').find((button) => button.text() === 'Сохранить')!.trigger('click')
    expect(w.text()).toContain('Укажите целое положительное число профилей.')
    expect(api.updateTask).not.toHaveBeenCalled()
    await w.find('#task-threads').setValue('3')
    api.updateTask.mockRejectedValue(new CoreError('Validation', 'x', [{ field: 'name', code: 'invalid' }]))
    await w.findAll('button').find((button) => button.text() === 'Сохранить')!.trigger('click')
    await flushPromises()
    expect(w.text()).toContain('Неверное значение')
    expect(w.find('#task-threads').element).toHaveProperty('value', '3')
    await w.find('#task-name').setValue('Исправлено')
    expect(w.text()).not.toContain('Неверное значение')
    api.updateTask.mockClear()
    await w.findAll('button').find((button) => button.text() === 'Отмена')!.trigger('click')
    expect(api.updateTask).not.toHaveBeenCalled()
  })

  it('показывает общий прогресс очереди и продолжает только незавершённую', async () => {
    api.startTask.mockResolvedValue(null)
    const queue = { ...task(10, 'Ссылки', false, batch({ finished_at: 1_700_000_100, done: 2, error: 1 })), distribution: 'queue' as const, queue: { total: 100, done: 98 } }
    api.listTasks.mockResolvedValue([queue])
    const w = await mountView()
    expect(w.text()).toContain('Общая очередь')
    expect(w.text()).toContain('Заданий готово: 98 / 100')
    await items(w, 0)
    expect(disabled(w, 'Продолжить очередь')).toBe(false)
    await menuItem(w, 'Продолжить очередь').trigger('click')
    await flushPromises()
    expect(api.startTask).toHaveBeenCalledWith(10, false)
    w.unmount()
    api.listTasks.mockResolvedValue([{ ...queue, queue: { total: 100, done: 100 } }])
    const complete = await mountView()
    await items(complete, 0)
    expect(disabled(complete, 'Продолжить очередь')).toBe(true)
    complete.unmount()
  })

  it('статусы и счётчики', async () => {
    const w = await mountView()
    const rows = w.findAll('.body .row').map((r) => r.text())
    expect(w.find('main').classes()).toContain('automation-tasks')
    expect(rows[0]).toContain('Выполняется')
    expect(rows[0]).toContain('готово 4 · ошибка 1 · всего 10')
    expect(rows[1]).toContain('С ошибками')
    expect(rows[2]).toContain('Готово')
    expect(rows[3]).toContain('Отменён')
    expect(rows[4]).toContain('Не запускалась')
    expect(rows[5]).toContain('Выполняется')
  })

  it('«Повторить неудачные запуски» запускает с флагом', async () => {
    api.startTask.mockResolvedValue(null)
    const w = await mountView()
    await items(w, 1)
    await w.findAll('[role="menuitem"]').find((i) => i.text() === 'Повторить неудачные запуски')!.trigger('click')
    await flushPromises()
    expect(api.startTask).toHaveBeenCalledWith(2, true)
  })

  it('«Остановить» и «Отменить ожидающие» только у идущей', async () => {
    const w = await mountView()
    expect(await items(w, 0)).toEqual(expect.arrayContaining(['Остановить', 'Отменить ожидающие']))
    const w2 = await mountView()
    expect(await items(w2, 1)).not.toContain('Остановить')
  })

  it('удаление с подтверждением', async () => {
    api.deleteTask.mockResolvedValue(null)
    const w = await mountView()
    await items(w, 1)
    await w.findAll('[role="menuitem"]').find((i) => i.text() === 'Удалить')!.trigger('click')
    await flushPromises()
    expect(api.deleteTask).not.toHaveBeenCalled()
    expect(w.text()).toContain('Удалить задачу «Кончилась»?')
    await w.findAll('button').find((b) => b.text() === 'Удалить')!.trigger('click')
    await flushPromises()
    expect(api.deleteTask).toHaveBeenCalledWith(2)
  })

  it('«Повторить неудачные запуски» доступно только после неудач и вне работы', async () => {
    const w = await mountView()
    const retry = async (row: number) => {
      await items(w, row)
      const item = w.findAll('[role="menuitem"]').find((i) => i.text() === 'Повторить неудачные запуски')!
      const off = item.attributes('data-disabled') !== undefined
      await w.trigger('keydown', { key: 'Escape' })
      await flushPromises()
      return off
    }
    expect(await retry(1)).toBe(false)
    expect(await retry(2)).toBe(true)
    expect(await retry(0)).toBe(true)
    expect(await retry(5)).toBe(true)
  })

  it('процесс удалён — «—» в ячейке', async () => {
    api.listTasks.mockResolvedValue([{ ...task(7, 'Сирота', false, null), flow_name: '' }])
    const w = await mountView()
    expect(w.findAll('.body .row')[0]!.findAll('.cell')[1]!.text()).toBe('—')
  })

  it('«Запустить» и «Удалить» недоступны при выполнении (своём и чужом)', async () => {
    const w = await mountView()
    for (const row of [0, 5]) {
      await items(w, row)
      expect(disabled(w, 'Запустить')).toBe(true)
      expect(disabled(w, 'Удалить')).toBe(true)
      await w.trigger('keydown', { key: 'Escape' })
      await flushPromises()
    }
    await items(w, 1)
    expect(disabled(w, 'Запустить')).toBe(false)
    expect(disabled(w, 'Удалить')).toBe(false)
  })

  it('«Журнал» ведёт на журнал задачи', async () => {
    const w = await mountView()
    await items(w, 1)
    await menuItem(w, 'История запусков').trigger('click')
    await flushPromises()
    expect(w.vm.$router.currentRoute.value.fullPath).toBe('/automation/journal?task=2')
  })

  it('«Остановить» и «Отменить ожидающие» вызывают ядро', async () => {
    api.stopTask.mockResolvedValue(true)
    api.cancelWaiting.mockResolvedValue(true)
    const w = await mountView()
    await items(w, 0)
    await menuItem(w, 'Остановить').trigger('click')
    await flushPromises()
    expect(api.stopTask).toHaveBeenCalledWith(1)
    await items(w, 0)
    await menuItem(w, 'Отменить ожидающие').trigger('click')
    await flushPromises()
    expect(api.cancelWaiting).toHaveBeenCalledWith(1)
  })
})

describe('опрос чужих пакетов', () => {
  beforeEach(() => vi.useFakeTimers())
  afterEach(() => vi.useRealTimers())

  it('раз в 3 с, пока есть чужой пакет; прекращается, когда его нет', async () => {
    const w = await mountView()
    expect(api.listTasks).toHaveBeenCalledTimes(1)
    await vi.advanceTimersByTimeAsync(3000)
    expect(api.listTasks).toHaveBeenCalledTimes(2)
    await vi.advanceTimersByTimeAsync(3000)
    expect(api.listTasks).toHaveBeenCalledTimes(3)
    api.listTasks.mockResolvedValue(tasks().map((t) => (t.id === 6 ? { ...t, last: batch({ finished_at: 1_700_000_100 }) } : t)))
    await vi.advanceTimersByTimeAsync(3000)
    expect(api.listTasks).toHaveBeenCalledTimes(4)
    await vi.advanceTimersByTimeAsync(9000)
    expect(api.listTasks).toHaveBeenCalledTimes(4)
    w.unmount()
  })

  it('без чужого пакета опроса нет; при уходе с экрана опрос стоит', async () => {
    api.listTasks.mockResolvedValue(tasks().slice(0, 5))
    const w = await mountView()
    await vi.advanceTimersByTimeAsync(9000)
    expect(api.listTasks).toHaveBeenCalledTimes(1)
    w.unmount()
  })

  it('уход с экрана останавливает опрос', async () => {
    const w = await mountView()
    w.unmount()
    const n = api.listTasks.mock.calls.length
    await vi.advanceTimersByTimeAsync(9000)
    expect(api.listTasks).toHaveBeenCalledTimes(n)
  })

  it('сбои опроса: одно уведомление на серию, после успеха — снова', async () => {
    const w = await mountView()
    const toasts = () => useToasts().items.length
    api.listTasks.mockRejectedValue(new Error('x'))
    await vi.advanceTimersByTimeAsync(9000)
    expect(toasts()).toBe(1)
    api.listTasks.mockResolvedValue(tasks())
    await vi.advanceTimersByTimeAsync(3000)
    api.listTasks.mockRejectedValue(new Error('y'))
    await vi.advanceTimersByTimeAsync(3000)
    expect(toasts()).toBe(2)
    w.unmount()
  })
})
