import { defineStore } from 'pinia'
import { ref, watch } from 'vue'
import { core, events } from '../api/core'
import type { UnlistenFn } from '@tauri-apps/api/event'
import type { DebugEvent, JournalEntry, ListQuery } from '../api/types'
import { useFlowEditorStore } from './flowEditor'

/** Строк журнала в панели отладки. */
export const KEEP = 500
const LOOKUP_DELAY = 300
const BY_SERIAL: ListQuery = { search: '', sort: 'serial', descending: false, status: null, group_id: null, ungrouped: false, tag: null }

/** Отладка процесса в окне профиля: ход запуска (`flow-debug`) для панели под холстом и подсветки узла. */
export const useFlowDebugStore = defineStore('flowDebug', () => {
  const open = ref(false)
  const serial = ref<number | null>(null)
  const runId = ref<string | null>(null)
  const running = ref(false)
  const entries = ref<JournalEntry[]>([])
  /** Старые строки отброшены (в панели последние `KEEP`); весь журнал — в файле запуска. */
  const cut = ref(false)
  const current = ref<string | null>(null)
  const status = ref<DebugEvent['status']>(null)
  const error = ref<string | null>(null)
  /** Профиля с таким номером нет (по `listProfiles`); `false` и пока не проверено. */
  const missing = ref(false)
  /** Имя найденного профиля рядом с номером. */
  const profileName = ref<string | null>(null)

  let unlisten: UnlistenFn | null = null
  /**
   * Номер последнего старта. `reset` и новый старт делают прежний устаревшим: после каждого await старт сверяется
   * с номером, устаревший останавливает свой запуск и снимает свою подписку.
   */
  let gen = 0
  /** «Стоп» нажат раньше, чем пришёл номер запуска: остановить, как только номер придёт. */
  let stopAsked = false
  /** События, пришедшие раньше ответа на запуск: чей они запуск, станет известно с номером. */
  let early: DebugEvent[] | null = null

  function apply(e: DebugEvent) {
    if (e.phase === 'node') current.value = e.node
    else if (e.phase === 'entry' && e.entry) {
      const next = [...entries.value, e.entry]
      if (next.length > KEEP) cut.value = true
      entries.value = next.slice(-KEEP)
    }
    else if (e.phase === 'end') {
      running.value = false
      current.value = null
      status.value = e.status
      error.value = e.error
    }
  }
  function onEvent(e: DebugEvent) {
    if (early) early.push(e)
    // Слот отладки освобождается до `end`: события прежнего запуска приходят и после начала нового.
    else if (e.run_id === runId.value) apply(e)
  }

  async function start(from: string[] | null) {
    const doc = useFlowEditorStore().doc
    if (!doc || running.value || serial.value === null) return
    const mine = ++gen
    entries.value = []
    cut.value = false
    current.value = null
    status.value = null
    error.value = null
    runId.value = null
    stopAsked = false
    early = []
    running.value = true
    try {
      if (!unlisten) {
        const off = await events.onFlowDebug(onEvent)
        if (mine !== gen) {
          off()
          return
        }
        unlisten = off
      }
      const id = await core.debugFlow(JSON.parse(JSON.stringify(doc)), serial.value, from)
      if (mine !== gen) {
        void core.stopDebug(id).catch(() => {})
        return
      }
      runId.value = id
      if (stopAsked) void core.stopDebug(id).catch(() => {})
    } catch (e) {
      // Устаревший старт молчит: редактор уже закрыт или идёт другой старт.
      if (mine !== gen) return
      running.value = false
      throw e
    } finally {
      if (mine === gen) {
        const buffered = early ?? []
        early = null
        for (const e of buffered) if (e.run_id === runId.value) apply(e)
      }
    }
  }

  async function stop() {
    if (!running.value) return
    if (runId.value) await core.stopDebug(runId.value)
    else stopAsked = true
  }
  /** Редактор больше не нужен (уход, другой процесс): остановить запуск, забыть ход и снять подписку. */
  function reset() {
    // Старт, ждущий номера, остановит свой запуск сам: номер старта сменился.
    gen++
    if (running.value && runId.value) void core.stopDebug(runId.value).catch(() => {})
    running.value = false
    runId.value = null
    entries.value = []
    cut.value = false
    current.value = null
    status.value = null
    error.value = null
    early = null
    unlisten?.()
    unlisten = null
  }
  function toggle() {
    open.value = !open.value
  }

  let token = 0
  let timer: ReturnType<typeof setTimeout> | undefined
  watch(serial, (n) => {
    clearTimeout(timer)
    const mine = ++token
    missing.value = false
    profileName.value = null
    if (n === null) return
    timer = setTimeout(async () => {
      try {
        const found = await core.listProfiles({ ...BY_SERIAL, search: String(n) })
        if (mine !== token) return
        const row = found.rows.find((r) => r.serial === n)
        missing.value = !row
        profileName.value = row?.name ?? null
      } catch {
        // Подсказка: сбой поиска не мешает запуску, нет профиля — ответит само ядро.
      }
    }, LOOKUP_DELAY)
  })

  return { open, serial, runId, running, entries, cut, current, status, error, missing, profileName, start, stop, reset, toggle }
})
