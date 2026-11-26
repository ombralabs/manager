import { flushPromises } from '@vue/test-utils'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { createMemoryHistory, createRouter } from 'vue-router'
import type { BatchSummary, RunRow, TaskRow } from '../../api/types'
import { mountWithApp } from '../../test/mount'
import { useToasts } from '../../stores/toasts'

const api = vi.hoisted(() => ({
  listFlows: vi.fn(async () => []),
  listTasks: vi.fn(),
  listBatches: vi.fn(),
  listRuns: vi.fn(),
  runJournal: vi.fn(),
  openBatchFiles: vi.fn(),
  cancelWaiting: vi.fn(),
}))
vi.mock('../../api/core', () => ({ core: api, events: {} }))

import JournalView from './JournalView.vue'

const batch = (id: number, patch: Partial<BatchSummary> = {}): BatchSummary => ({
  id, started_at: 1_700_000_000 + id, finished_at: null, total: 4, queued: 0, waiting: 0, running: 0, done: 0,
  error: 0, cancelled: 0, skipped: 0, ...patch,
})
const task = (id: number, name: string, running: boolean): TaskRow => ({
  id, name, flow_id: 1, flow_name: 'Вход', profiles: 4, threads: 2, order: 'ordered', missing_row: 'skip', distribution: 'per_profile', queue: null,
  created_at: 1, running, last: null,
})
const run = (id: number, status: RunRow['status'], error: string | null = null): RunRow => ({
  item_index: null,
  id, profile_id: `p${id}`, serial: 100 + id, name: `Профиль ${id}`, position: id - 1, status, error,
  started_at: 1_700_000_000, finished_at: null,
})

let router: ReturnType<typeof createRouter>
async function mountView(path = '/automation/journal?task=2') {
  router = createRouter({ history: createMemoryHistory(), routes: [{ path: '/:p(.*)*', component: { template: '<div />' } }] })
  await router.push(path)
  const wrapper = mountWithApp(JournalView, { global: { plugins: [router] }, attachTo: document.body })
  await flushPromises()
  return wrapper
}

beforeEach(() => {
  for (const mock of Object.values(api)) mock.mockReset()
  api.listFlows.mockResolvedValue([])
  api.listTasks.mockResolvedValue([task(1, 'Покой', false), task(2, 'Идёт', true)])
  api.listBatches.mockImplementation(async (id: number) =>
    id === 2 ? [batch(20, { finished_at: 1_700_000_100, done: 4 }), batch(21, { running: 1, done: 2, error: 1 })] : [])
  api.listRuns.mockImplementation(async (id: number) =>
    id === 21 ? [run(1, 'done'), run(2, 'error', 'не нашли кнопку'), run(3, 'running'), run(4, 'waiting')] : [run(5, 'queued'), run(6, 'cancelled'), run(7, 'skipped')])
  api.runJournal.mockResolvedValue([])
  api.openBatchFiles.mockResolvedValue(null)
})

describe('экран «Журнал»', () => {
  it('различает задания очереди на одном профиле и ещё не назначенные задания', async () => {
    api.listTasks.mockResolvedValue([{ ...task(2, 'Очередь', true), distribution: 'queue', queue: { total: 3, done: 1 } }])
    api.listRuns.mockResolvedValue([
      { ...run(1, 'done'), item_index: 0 },
      { ...run(2, 'running'), serial: 101, name: 'Профиль 1', profile_id: 'p1', item_index: 1 },
      { ...run(3, 'queued'), serial: 0, name: '', profile_id: '', item_index: 2 },
    ])
    const w = await mountView()
    const rows = w.findAll('[data-test="run-row"]')
    expect(w.find('.row.header').text()).toContain('Задание')
    expect(rows.map((r) => r.find('div').text())).toEqual(['1', '2', '3'])
    expect(rows[1]!.text()).toContain('№ 101')
    expect(rows[2]!.text()).toContain('Ожидает свободный профиль')
    w.unmount()
  })

  it('?task=2 выбирает задачу и грузит её пакеты, новый сверху', async () => {
    const w = await mountView()
    expect(w.find('main').classes()).toContain('automation-journal')
    expect(api.listBatches).toHaveBeenCalledWith(2)
    expect(w.find('[data-test="task-item"][aria-current="true"]').text()).toContain('Идёт')
    const batches = w.findAll('[data-test="batch-row"]')
    expect(batches).toHaveLength(2)
    expect(batches[0]!.text()).toContain('4 · 2 · 1')
  })

  it('выбор пакета грузит запуски со статусами по-русски', async () => {
    const w = await mountView()
    await w.findAll('[data-test="batch-row"]')[1]!.trigger('click')
    await flushPromises()
    expect(api.listRuns).toHaveBeenLastCalledWith(20)
    const text = w.findAll('[data-test="run-row"]').map((r) => r.text()).join('|')
    for (const s of ['В очереди', 'Отменён', 'Пропущен']) expect(text).toContain(s)
    await w.findAll('[data-test="batch-row"]')[0]!.trigger('click')
    await flushPromises()
    const live = w.findAll('[data-test="run-row"]').map((r) => r.text()).join('|')
    for (const s of ['Готово', 'Ошибка', 'Выполняется', 'Ожидает', 'не нашли кнопку']) expect(live).toContain(s)
    expect(w.findAll('[data-test="run-row"]')[0]!.text()).toContain('101')
  })

  it('раскрытие запуска показывает строки журнала; пустой — «Журнал пуст»', async () => {
    api.runJournal.mockResolvedValueOnce([{ ts_ms: 1000, node: 'n1', op: 'click', status: 'error', ms: 30, msg: 'элемент не найден' }])
    const w = await mountView()
    await w.findAll('[data-test="run-row"]')[1]!.trigger('click')
    await flushPromises()
    expect(api.runJournal).toHaveBeenCalledWith(2)
    expect(w.find('[data-test="journal-line"]').text()).toContain('элемент не найден')
    expect(w.find('[data-test="journal-line"]').text()).toContain('ошибка')
    await w.findAll('[data-test="run-row"]')[0]!.trigger('click')
    await flushPromises()
    expect(w.text()).toContain('Журнал пуст')
  })

  it('«Открыть папку файлов» открывает выбранный пакет', async () => {
    const w = await mountView()
    await w.findAll('button').find((b) => b.text() === 'Открыть папку файлов')!.trigger('click')
    expect(api.openBatchFiles).toHaveBeenCalledWith(21)
  })

  it('«Отменить ожидающие» — только у незаконченного пакета идущей задачи', async () => {
    api.cancelWaiting.mockResolvedValue(true)
    const w = await mountView()
    const buttons = () => w.findAll('[data-test="batch-row"] button').filter((b) => b.text() === 'Отменить ожидающие')
    expect(buttons()).toHaveLength(1)
    expect(w.findAll('[data-test="batch-row"]')[1]!.text()).not.toContain('Отменить ожидающие')
    await buttons()[0]!.trigger('click')
    await flushPromises()
    expect(api.cancelWaiting).toHaveBeenCalledWith(2)
  })

  it('медленный ответ прежней задачи не затирает выбранную', async () => {
    let resolveSlow: (v: BatchSummary[]) => void = () => {}
    api.listBatches.mockImplementation((id: number) =>
      id === 2 ? new Promise<BatchSummary[]>((r) => (resolveSlow = r)) : Promise.resolve([batch(10, { finished_at: 1_700_000_100 })]))
    api.listRuns.mockResolvedValue([run(1, 'done')])
    const w = await mountView()
    await w.findAll('[data-test="task-item"]')[0]!.trigger('click')
    await flushPromises()
    resolveSlow([batch(20), batch(21)])
    await flushPromises()
    expect(w.findAll('[data-test="batch-row"]')).toHaveLength(1)
    expect(api.listRuns).not.toHaveBeenCalledWith(21)
    await w.findAll('button').find((b) => b.text() === 'Открыть папку файлов')!.trigger('click')
    expect(api.openBatchFiles).toHaveBeenCalledWith(10)
  })

  it('?task в адресе отслеживается', async () => {
    const w = await mountView()
    await router.push('/automation/journal?task=1')
    await flushPromises()
    expect(api.listBatches).toHaveBeenLastCalledWith(1)
    expect(w.find('[data-test="task-item"][aria-current="true"]').text()).toContain('Покой')
  })

  it('Enter на «Отменить ожидающие» не выбирает пакет', async () => {
    const w = await mountView()
    api.listRuns.mockClear()
    const button = w.findAll('[data-test="batch-row"] button').find((b) => b.text() === 'Отменить ожидающие')!
    await button.trigger('keydown', { key: 'Enter' })
    await flushPromises()
    expect(api.listRuns).not.toHaveBeenCalled()
  })

  it('без задачи — подсказка по-русски', async () => {
    api.listTasks.mockResolvedValue([])
    const w = await mountView('/automation/journal')
    expect(w.text()).toContain('Выберите задачу')
    expect(w.text()).not.toContain('Pick a task')
  })

  describe('живое обновление', () => {
    beforeEach(() => vi.useFakeTimers())
    afterEach(() => vi.useRealTimers())

    it('раскрытый идущий запуск обновляет строки раз в 2 с, пока идёт', async () => {
      const w = await mountView()
      await w.findAll('[data-test="run-row"]')[2]!.trigger('click')
      await flushPromises()
      expect(api.runJournal).toHaveBeenCalledTimes(1)
      api.runJournal.mockResolvedValue([{ ts_ms: 1000, node: 'n1', op: 'click', status: 'ok', ms: 5, msg: null }])
      await vi.advanceTimersByTimeAsync(2000)
      expect(api.runJournal).toHaveBeenCalledTimes(2)
      expect(w.findAll('[data-test="journal-line"]')).toHaveLength(1)
      // запуск закончился: следующее обновление приносит финальные строки, дальше опроса нет
      api.listRuns.mockImplementation(async () => [run(1, 'done'), run(2, 'error'), run(3, 'done'), run(4, 'done')])
      await vi.advanceTimersByTimeAsync(2000)
      const n = api.runJournal.mock.calls.length
      await vi.advanceTimersByTimeAsync(10000)
      expect(api.runJournal).toHaveBeenCalledTimes(n)
      w.unmount()
    })

    it('сбои опроса: одно уведомление на серию, после успеха — снова', async () => {
      const w = await mountView()
      await w.findAll('[data-test="run-row"]')[2]!.trigger('click')
      await flushPromises()
      api.runJournal.mockRejectedValue(new Error('x'))
      await vi.advanceTimersByTimeAsync(6000)
      expect(useToasts().items).toHaveLength(1)
      api.runJournal.mockResolvedValue([])
      await vi.advanceTimersByTimeAsync(2000)
      api.runJournal.mockRejectedValue(new Error('y'))
      await vi.advanceTimersByTimeAsync(2000)
      expect(useToasts().items).toHaveLength(2)
      w.unmount()
    })

    it('новый запрос не начинается, пока прежний не вернулся', async () => {
      const w = await mountView()
      await w.findAll('[data-test="run-row"]')[2]!.trigger('click')
      await flushPromises()
      api.runJournal.mockClear()
      api.runJournal.mockReturnValue(new Promise(() => {}))
      await vi.advanceTimersByTimeAsync(10000)
      expect(api.runJournal).toHaveBeenCalledTimes(1)
      w.unmount()
    })

    it('завершённый запуск не опрашивается; уход с экрана останавливает опрос', async () => {
      const w = await mountView()
      await w.findAll('[data-test="run-row"]')[0]!.trigger('click')
      await flushPromises()
      await vi.advanceTimersByTimeAsync(6000)
      expect(api.runJournal).toHaveBeenCalledTimes(1)
      await w.findAll('[data-test="run-row"]')[2]!.trigger('click')
      await flushPromises()
      w.unmount()
      const n = api.runJournal.mock.calls.length
      await vi.advanceTimersByTimeAsync(6000)
      expect(api.runJournal).toHaveBeenCalledTimes(n)
    })
  })
})
