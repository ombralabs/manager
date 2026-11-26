import { defineStore } from 'pinia'
import { ref } from 'vue'
import { core } from '../api/core'
import type { FlowRow, TaskForm, TaskRow, TaskSettingsForm, TaskUpdateForm, TasksChanged } from '../api/types'
import { i18n } from '../i18n'
import { notify, reportError } from '../lib/notify'
import { createRefresher } from '../lib/refresher'

export const useAutomationStore = defineStore('automation', () => {
  const flows = ref<FlowRow[]>([])
  const tasks = ref<TaskRow[]>([])
  const flowsLoaded = ref(false)
  const tasksLoaded = ref(false)

  const flowsRefresher = createRefresher(core.listFlows, (list) => {
    flows.value = list
    flowsLoaded.value = true
  })
  const tasksRefresher = createRefresher(core.listTasks, (list) => {
    tasks.value = list
    tasksLoaded.value = true
  })

  const flowById = (id: number) => flows.value.find((flow) => flow.id === id)

  /** Операция удалась, даже если обновление списка потом не прошло: та ошибка — отдельным уведомлением. */
  async function afterWrite<T>(write: Promise<T>, refresher: { refresh: () => Promise<void> }): Promise<T> {
    const result = await write
    await refresher.refresh().catch(reportError)
    return result
  }
  const flowWrite = <T>(write: Promise<T>) => afterWrite(write, flowsRefresher)
  const taskWrite = <T>(write: Promise<T>) => afterWrite(write, tasksRefresher)

  function applyTasksChanged(event: TasksChanged) {
    tasksRefresher.schedule()
    if (event.error) notify.warning(i18n.global.t('automation.tasks.notStarted'), event.error)
  }

  return {
    flows, tasks, flowsLoaded, tasksLoaded, flowById, applyTasksChanged,
    refreshFlows: flowsRefresher.refresh,
    refreshTasks: tasksRefresher.refresh,
    scheduleTasks: tasksRefresher.schedule,
    importTemplate: (source: string) => flowWrite(core.importFlowJson(source)),
    importFlow: (path: string) => flowWrite(core.importFlowFile(path)),
    exportFlow: (id: number, path: string) => core.exportFlowFile(id, path),
    copyFlow: (id: number) => flowWrite(core.copyFlow(id)),
    deleteFlow: (id: number) => flowWrite(core.deleteFlow(id)),
    setDefaults: (id: number, values: Record<string, unknown>) => flowWrite(core.setFlowDefaults(id, values)),
    createTask: (form: TaskForm) => taskWrite(core.createTask(form)),
    updateTask: (id: number, form: TaskUpdateForm) => taskWrite(core.updateTask(id, form)),
    updateTaskSettings: (id: number, form: TaskSettingsForm) => taskWrite(core.updateTaskSettings(id, form)),
    startTask: (id: number) => taskWrite(core.startTask(id, false)),
    retryFailed: (id: number) => taskWrite(core.startTask(id, true)),
    stopTask: (id: number) => taskWrite(core.stopTask(id)),
    cancelWaiting: (id: number) => taskWrite(core.cancelWaiting(id)),
    deleteTask: (id: number) => taskWrite(core.deleteTask(id)),
  }
})
