import { defineStore } from 'pinia'
import { computed, reactive, ref } from 'vue'
import { core } from '../api/core'
import { type CoreError, toCoreError } from '../api/errors'
import type { GeoInfo, ProxyEditForm, ProxyImportReport, ProxyKind, ProxyRow } from '../api/types'
import { reportError } from '../lib/notify'
import { CHECK_PARALLEL } from '../lib/proxies'
import { createRefresher } from '../lib/refresher'

/** Список прокси: экран «Прокси» и вкладка «Шлюз» редактора. */
export const useProxiesStore = defineStore('proxies', () => {
  const rows = ref<ProxyRow[]>([])
  const loaded = ref(false)
  /** Записи, которые проверяются или ждут проверки в очереди. */
  const checking = reactive(new Set<number>())
  /** Идёт «Проверить все». */
  const checkingAll = ref(false)

  const total = computed(() => rows.value.length)
  const pinnedCount = computed(() => rows.value.filter((row) => row.profiles > 0).length)

  const refresher = createRefresher(
    () => core.listProxies(),
    (list) => {
      rows.value = list
      loaded.value = true
    },
  )
  const loading = refresher.loading

  function byId(id: number): ProxyRow | undefined {
    return rows.value.find((row) => row.id === id)
  }

  /** Итог проверки сразу в строке — так же, как его записывает `Core::check_proxy`; свежий список придёт следом. */
  function applyCheck(id: number, geo: GeoInfo | null, error: CoreError | null) {
    const row = byId(id)
    if (!row) return
    const at = Math.floor(Date.now() / 1000)
    if (geo) {
      row.check = { at, ok: true, exit_ip: geo.ip, country: geo.country }
    } else if (error?.code === 'ProxyUnavailable') {
      row.check = { at, ok: false, exit_ip: row.check?.exit_ip ?? null, country: row.check?.country ?? null }
    }
    // Сервисы геоданных недоступны — о самом прокси это ничего не говорит: ядро запись не меняет.
  }

  async function runCheck(id: number): Promise<GeoInfo> {
    checking.add(id)
    try {
      const geo = await core.checkProxy(id)
      applyCheck(id, geo, null)
      return geo
    } catch (e) {
      const error = toCoreError(e)
      applyCheck(id, null, error)
      throw error
    } finally {
      checking.delete(id)
    }
  }

  /** «Проверить» у одной записи. Ошибка — вызывающему; список перечитывается в любом случае. */
  async function check(id: number): Promise<GeoInfo> {
    try {
      return await runCheck(id)
    } finally {
      await refresher.refresh().catch(reportError)
    }
  }

  /** Проверка записей по `CHECK_PARALLEL` одновременно; строки обновляются по мере ответов. Возвращает число доступных. */
  async function checkMany(ids: number[]): Promise<number> {
    for (const id of ids) checking.add(id)
    const queue = [...ids]
    let ok = 0
    async function worker() {
      for (let id = queue.shift(); id !== undefined; id = queue.shift()) {
        try {
          await runCheck(id)
          ok += 1
        } catch {
          // Неудача видна в строке («Нет связи»); в итоге запись просто не считается доступной.
        }
      }
    }
    await Promise.all(Array.from({ length: Math.min(CHECK_PARALLEL, ids.length) }, worker))
    await refresher.refresh().catch(reportError)
    return ok
  }

  async function checkAll(): Promise<{ ok: number; total: number }> {
    const ids = rows.value.map((row) => row.id)
    checkingAll.value = true
    try {
      return { ok: await checkMany(ids), total: ids.length }
    } finally {
      checkingAll.value = false
    }
  }

  /** Переключатель «Вкл.»: строка меняется сразу, при ошибке — обратно. */
  async function setEnabled(id: number, enabled: boolean): Promise<void> {
    const row = byId(id)
    const before = row?.enabled
    if (row) row.enabled = enabled
    try {
      await core.setProxyEnabled(id, enabled)
    } catch (e) {
      // Пока шёл запрос, список мог обновиться: откатывается текущая строка.
      const current = byId(id)
      if (current && before !== undefined) current.enabled = before
      throw e
    }
  }

  /** Операция удалась, даже если обновление списка потом не прошло: та ошибка — отдельным уведомлением. */
  async function afterWrite<T>(write: Promise<T>): Promise<T> {
    const result = await write
    await refresher.refresh().catch(reportError)
    return result
  }

  const create = (form: ProxyEditForm) => afterWrite(core.createProxy(form))
  const update = (id: number, form: ProxyEditForm) => afterWrite(core.updateProxy(id, form))
  const remove = (id: number) => afterWrite(core.deleteProxy(id))

  /** Импорт строками. С `check` добавленные записи проверяются в фоне: отчёт возвращается сразу. */
  async function importText(text: string, kind: ProxyKind, check: boolean): Promise<ProxyImportReport> {
    const report = await afterWrite(core.importProxies(text, kind))
    if (check && report.ids.length > 0) void checkMany(report.ids)
    return report
  }

  return {
    rows,
    loading,
    loaded,
    checking,
    checkingAll,
    total,
    pinnedCount,
    refresh: refresher.refresh,
    scheduleRefresh: refresher.schedule,
    cancelTimers: refresher.cancel,
    byId,
    check,
    checkAll,
    setEnabled,
    create,
    update,
    remove,
    importText,
  }
})
