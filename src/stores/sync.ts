import { defineStore } from 'pinia'
import { reactive, ref } from 'vue'
import { useSettingsSaveStore } from './settingsSave'
import { core, events } from '../api/core'
import type { SyncSettings, SyncState } from '../api/types'

const IDLE: SyncState = { active: false, mode: 'full_copy', main: null, windows: [] }
const DEFAULTS: SyncSettings = {
  mode: 'full_copy', clicks: true, typing: true, scroll: true, mouse: true, click_delay: [100, 600], typing_delay: [100, 600],
}

/** Синхронизатор окон: состояние сессии от ядра, настройки и выбор профилей до старта. */
export const useSyncStore = defineStore('sync', () => {
  const saves = useSettingsSaveStore()
  const state = ref<SyncState>(IDLE)
  const settings = ref<SyncSettings>(DEFAULTS)
  const checked = reactive(new Set<string>())
  const mainId = ref<string | null>(null)

  /** Отметка профиля: первый отмеченный становится главным; снятие главного отдаёт роль следующему отмеченному. */
  function toggle(id: string) {
    if (checked.delete(id)) {
      if (mainId.value === id) mainId.value = checked.values().next().value ?? null
      return
    }
    checked.add(id)
    mainId.value ??= id
  }

  function makeMain(id: string) {
    checked.add(id)
    mainId.value = id
  }

  /** Выбор из «Профилей»: отмечены ровно эти, главный — первый. */
  function pick(ids: string[]) {
    checked.clear()
    for (const id of ids) checked.add(id)
    mainId.value = ids[0] ?? null
  }

  /** Остановленные профили выпадают из выбора; главным становится следующий отмеченный. */
  function prune(alive: Set<string>) {
    for (const id of [...checked]) if (!alive.has(id)) checked.delete(id)
    if (mainId.value !== null && !checked.has(mainId.value)) mainId.value = checked.values().next().value ?? null
  }

  async function start() {
    const main = mainId.value
    if (main === null) return
    const before = seen
    const started = await core.syncStart(main, [...checked].filter((id) => id !== main))
    if (seen === before) state.value = started
  }

  async function stop() {
    await core.syncStop()
    state.value = IDLE
  }

  // Сохранения идут по очереди, каждое — поверх уже сохранённого: два быстрых переключателя не отменяют друг друга.
  let saving: Promise<unknown> = Promise.resolve()
  function saveSettings(patch: Partial<SyncSettings>) {
    const run = saving.then(async () => {
      const next = { ...settings.value, ...patch }
      await core.setSyncSettings(next)
      settings.value = next
    })
    saving = run.catch(() => {})
    return run
  }

  let seen = 0
  /** Подписка на событие `sync`; возвращает отписку. */
  function init() {
    return events.onSync((next) => {
      seen++
      state.value = next
    })
  }

  /** Первое чтение состояния и настроек — когда ядро открыто: до разблокировки защиты его команды отказывают. */
  async function load() {
    const before = seen
    const [first, saved] = await Promise.all([core.syncState(), core.getSyncSettings()])
    // Событие, пришедшее во время чтения, новее ответа.
    if (seen === before) state.value = first
    settings.value = saved
  }

  return {
    state, settings, checked, mainId, toggle, makeMain, pick, prune, start, stop, init, load,
    saveSettings: (patch: Partial<SyncSettings>) => saves.run(
      'sync.' + Object.keys(patch).sort().join('.'), () => saveSettings(patch)),
    release: (id: string) => core.syncRelease(id),
    resume: (id: string) => core.syncResume(id),
    openLikeMain: (id: string) => core.syncOpenLikeMain(id),
    show: (id: string) => core.showWindow(id),
  }
})
