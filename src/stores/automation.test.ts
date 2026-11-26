import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { FlowRow, TaskRow } from '../api/types'

const mocks = vi.hoisted(() => ({
  listFlows: vi.fn(),
  listTasks: vi.fn(),
  importFlowFile: vi.fn(),
  startTask: vi.fn(),
}))
vi.mock('../api/core', () => ({ core: mocks }))

import { useToasts } from './toasts'
import { useAutomationStore } from './automation'

const flow = (id: number): FlowRow => ({
  id, name: `Процесс ${id}`, group: '', remark: '', created_at: 1, updated_at: 1, has_errors: false, variables: [],
})
const task = (id: number): TaskRow => ({
  id, name: `Задача ${id}`, flow_id: 1, flow_name: 'Процесс 1', profiles: 2, threads: 1,
  order: 'ordered', missing_row: 'skip', distribution: 'per_profile', queue: null, created_at: 1, running: false, last: null,
})

beforeEach(() => {
  for (const mock of Object.values(mocks)) mock.mockReset()
  mocks.listFlows.mockResolvedValue([flow(1), flow(2)])
  mocks.listTasks.mockResolvedValue([task(1)])
  mocks.startTask.mockResolvedValue(null)
})

describe('automation store', () => {
  it('refreshFlows и refreshTasks заполняют списки', async () => {
    const store = useAutomationStore()
    await store.refreshFlows()
    await store.refreshTasks()
    expect(store.flows.map((f) => f.id)).toEqual([1, 2])
    expect(store.tasks.map((t) => t.id)).toEqual([1])
    expect([store.flowsLoaded, store.tasksLoaded]).toEqual([true, true])
    expect(store.flowById(2)?.name).toBe('Процесс 2')
  })

  it('importFlow вызывает importFlowFile и перечитывает процессы', async () => {
    mocks.importFlowFile.mockResolvedValue(flow(2))
    const store = useAutomationStore()
    await store.importFlow('/tmp/a.json')
    expect(mocks.importFlowFile).toHaveBeenCalledWith('/tmp/a.json')
    expect(mocks.listFlows).toHaveBeenCalledTimes(1)
    expect(store.flows).toHaveLength(2)
  })

  it('startTask и retryFailed передают признак failed', async () => {
    const store = useAutomationStore()
    await store.retryFailed(5)
    expect(mocks.startTask).toHaveBeenLastCalledWith(5, true)
    await store.startTask(5)
    expect(mocks.startTask).toHaveBeenLastCalledWith(5, false)
  })

  it('applyTasksChanged: ошибка — предупреждение и обновление; без ошибки — тишина', async () => {
    const store = useAutomationStore()
    store.applyTasksChanged({ task_id: 1, error: 'задача 1 ещё выполняется (процесс 42)' })
    const items = useToasts().items
    expect(items).toHaveLength(1)
    expect(items[0]!.body).toBe('задача 1 ещё выполняется (процесс 42)')
    useToasts().items.splice(0)
    store.applyTasksChanged({ task_id: 1, error: null })
    expect(useToasts().items).toHaveLength(0)
  })
})
