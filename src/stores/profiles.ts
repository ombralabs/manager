import { defineStore } from 'pinia'
import { computed, reactive, ref, watch } from 'vue'
import { core } from '../api/core'
import type { ListQuery, ProfileLabelFilter, ProfileList, ProfileRow, SortKey, SupervisorEvent } from '../api/types'
import { labelFilterId } from '../lib/profileLabels'
import { reportError } from '../lib/notify'
import { PREF_PAGE_SIZE, readPref, writePref } from '../lib/prefs'
import { createRefresher } from '../lib/refresher'
import { fold, sameTag } from '../lib/tags'

/** Фильтр группы: все профили, «Без группы» или id группы. */
export type GroupFilter = 'all' | 'ungrouped' | number
export const PAGE_SIZES = [50, 100, 200] as const
export type PageSize = (typeof PAGE_SIZES)[number]

export interface ProfileFilters {
  search: string
  sort: SortKey
  descending: boolean
  label: ProfileLabelFilter
  group: GroupFilter
  tags: string[]
  untagged: boolean
}

/** Поиск ждёт паузы в наборе. */
const SEARCH_DELAY = 200
/** После события супервизора список уточняется не чаще раза в этот срок. */
const EVENT_DELAY = 250

export function defaultFilters(): ProfileFilters {
  return { search: '', sort: 'serial', descending: false, label: 'any', group: 'all', tags: [], untagged: false }
}

function parsePageSize(raw: unknown): PageSize | null {
  return PAGE_SIZES.find((size) => size === raw) ?? null
}

/** Ответ ядра вместе с фильтрами, по которым он получен. */
interface LoadedList {
  query: ListQuery
  list: ProfileList
}

/**
 * Список профилей: весь отфильтрованный ответ ядра одним вызовом, страницы и выделение — здесь.
 * Выделение — набор id: переживает страницы, сбрасывается сменой поиска, фильтров и сортировки.
 */
export const useProfilesStore = defineStore('profiles', () => {
  const rows = ref<ProfileRow[]>([])
  const total = ref(0)
  const running = ref(0)
  /** Профили, по которым идёт запуск из этого окна. */
  const busy = reactive(new Set<string>())
  /** Профили, по которым идёт остановка: отдельно — запуск, ждущий поток, может не кончаться долго, а «Стоп» нужен. */
  const stopping = reactive(new Set<string>())
  const filters = reactive<ProfileFilters>(defaultFilters())
  /** Временный фильтр «эти профили» из отчёта массовой операции. */
  const only = ref<string[] | null>(null)
  const page = ref(0)
  const pageSize = ref<PageSize>(readPref(PREF_PAGE_SIZE, parsePageSize, 50))
  const selected = reactive(new Set<string>())
  const hasTagFilters = computed(() => filters.tags.length > 0 || filters.untagged)

  const visibleRows = computed(() => {
    const wanted = only.value === null ? null : new Set(only.value)
    const tags = filters.tags.map(fold)
    return rows.value.filter((row) => (!wanted || wanted.has(row.id))
      && (!filters.untagged || row.tags.length === 0)
      && tags.every((tag) => row.tags.some((name) => fold(name) === tag)))
  })
  /** Доступные пересечения по всему отбору, включая профили на других страницах. */
  const tagFacets = computed(() => {
    const counts = new Map<string, number>()
    let untagged = 0
    for (const row of visibleRows.value) {
      if (!row.tags.length) untagged++
      for (const tag of new Set(row.tags.map(fold))) counts.set(tag, (counts.get(tag) ?? 0) + 1)
    }
    return { counts, untagged }
  })
  const pageCount = computed(() => Math.max(1, Math.ceil(visibleRows.value.length / pageSize.value)))
  const pageRows = computed(() =>
    visibleRows.value.slice(page.value * pageSize.value, (page.value + 1) * pageSize.value),
  )
  const rangeFrom = computed(() => (pageRows.value.length === 0 ? 0 : page.value * pageSize.value + 1))
  const rangeTo = computed(() => page.value * pageSize.value + pageRows.value.length)
  const selectedIds = computed(() => visibleRows.value.filter((row) => selected.has(row.id)).map((row) => row.id))
  const selectedCount = computed(() => selected.size)
  const allSelected = computed(
    () => visibleRows.value.length > 0 && visibleRows.value.every((row) => selected.has(row.id)),
  )

  function listQuery(): ListQuery {
    return {
      search: filters.search,
      sort: filters.sort,
      descending: filters.descending,
      status: null,
      label_id: labelFilterId(filters.label),
      group_id: typeof filters.group === 'number' ? filters.group : null,
      ungrouped: filters.group === 'ungrouped',
      // Ядро уже возвращает все строки до разбиения на страницы. Общий набор нужен и для пересечений,
      // и для доступности кнопок; выбранные теги применяются к нему сразу, без повторного запроса.
      tag: null,
    }
  }

  /** Экран «Профили» открыт: только тогда события перечитывают список (`scheduleRefresh`). */
  const active = ref(false)

  /** Ответ на прежние фильтры не применяется: запрос по новым уже отложен следом или ждёт паузы в поиске. */
  function applyList({ query, list }: LoadedList) {
    if (JSON.stringify(query) !== JSON.stringify(listQuery())) return
    rows.value = list.rows
    total.value = list.total
    running.value = list.running
  }

  watch(visibleRows, (matching) => {
    const present = new Set(matching.map((row) => row.id))
    for (const id of [...selected]) {
      if (!present.has(id)) selected.delete(id)
    }
    if (page.value > pageCount.value - 1) page.value = pageCount.value - 1
  }, { flush: 'sync' })

  const refresher = createRefresher(async (): Promise<LoadedList> => {
    const query = listQuery()
    return { query, list: await core.listProfiles(query) }
  }, applyList)
  const { loading, refresh } = refresher

  let searchTimer: ReturnType<typeof setTimeout> | undefined

  /**
   * Обновление по событию «не чаще раза в `delay`» — только пока экран открыт: закрытый экран перечитает список
   * сам при открытии. Ошибка фонового обновления не всплывает — её покажет следующее действие пользователя.
   */
  function scheduleRefresh(delay = 500) {
    if (active.value) refresher.schedule(delay)
  }

  /** Экран уходит: запланированные обновления и поиск больше не нужны. */
  function cancelTimers() {
    refresher.cancel()
    clearTimeout(searchTimer)
    searchTimer = undefined
  }

  /** `ProfilesView`: `true` при монтировании, `false` при уходе — тогда и запланированное отменяется. */
  function setActive(value: boolean) {
    active.value = value
    if (!value) cancelTimers()
  }

  /** Событие супервизора: строка меняется сразу, счётчики, шлюз и фильтр — следом. */
  function applyEvent(event: SupervisorEvent) {
    if (event.kind !== 'status') return
    const row = rows.value.find((r) => r.id === event.profile_id)
    if (row) row.status = event.status
    scheduleRefresh(EVENT_DELAY)
  }

  function resetView() {
    page.value = 0
    selected.clear()
    only.value = null
  }

  /** Пока код меняет несколько полей фильтра разом, наблюдатели молчат: обновление одно. */
  let quiet = false

  function applyFilters(next: ProfileFilters) {
    quiet = true
    try {
      Object.assign(filters, next)
    } finally {
      quiet = false
    }
    clearTimeout(searchTimer)
    searchTimer = undefined
    resetView()
    refresh().catch(reportError)
  }

  watch(
    () => filters.search,
    () => {
      if (quiet) return
      resetView()
      clearTimeout(searchTimer)
      searchTimer = setTimeout(() => {
        searchTimer = undefined
        refresh().catch(reportError)
      }, SEARCH_DELAY)
    },
    { flush: 'sync' },
  )

  watch(
    [() => filters.sort, () => filters.descending, () => filters.label, () => filters.group],
    () => {
      if (quiet) return
      resetView()
      refresh().catch(reportError)
    },
    { flush: 'sync' },
  )

  watch([() => filters.tags.slice(), () => filters.untagged], () => {
    if (!quiet) resetView()
  }, { flush: 'sync' })

  function toggleTag(tag: string) {
    if (filters.tags.some((selected) => sameTag(selected, tag))) {
      filters.tags = filters.tags.filter((selected) => !sameTag(selected, tag))
    } else if (!filters.untagged && (tagFacets.value.counts.get(fold(tag)) ?? 0) > 0) {
      filters.tags = [...filters.tags, tag]
    }
  }

  function toggleUntagged() {
    if (filters.untagged || (!filters.tags.length && tagFacets.value.untagged > 0)) filters.untagged = !filters.untagged
  }

  function clearTagFilters() {
    filters.tags = []
    filters.untagged = false
  }

  function setPage(n: number) {
    page.value = Math.min(Math.max(0, n), pageCount.value - 1)
  }

  function setPageSize(size: PageSize) {
    pageSize.value = size
    page.value = 0
    writePref(PREF_PAGE_SIZE, size)
  }

  function toggle(id: string) {
    if (selected.has(id)) selected.delete(id)
    else selected.add(id)
  }

  /** «Выбрать все N по фильтру» — на всех страницах. */
  function selectAll() {
    for (const row of visibleRows.value) selected.add(row.id)
  }

  function clearSelection() {
    selected.clear()
  }

  /** «Показать в списке» из отчёта: фильтры сброшены (сортировка остаётся), видны только эти профили. */
  function showOnly(ids: string[]) {
    applyFilters({ ...defaultFilters(), sort: filters.sort, descending: filters.descending })
    only.value = [...ids]
  }

  /** «Открыть в списке» и облако групп из экрана «Группы»: только фильтр группы. */
  function openGroup(group: GroupFilter) {
    applyFilters({ ...defaultFilters(), sort: filters.sort, descending: filters.descending, group })
  }

  /** Группу удалили или тег сняли со всех профилей: фильтр возвращается к «все» / «любой». Теги — со свёрткой ядра. */
  function resetMissing(groupIds: number[], tags: string[]) {
    const group = typeof filters.group === 'number' && !groupIds.includes(filters.group) ? 'all' : filters.group
    const remaining = filters.tags.filter((selected) => tags.some((name) => sameTag(name, selected)))
    if (group !== filters.group) applyFilters({ ...filters, group, tags: remaining })
    else if (remaining.length !== filters.tags.length) filters.tags = remaining
  }

  /** Метка «Сортировка: …» на панели фильтров: «№» по возрастанию одним обновлением, а не двумя. */
  function resetSort() {
    if (filters.sort === 'serial' && !filters.descending) return
    applyFilters({ ...filters, sort: 'serial', descending: false })
  }

  async function track(set: Set<string>, id: string, action: () => Promise<unknown>) {
    set.add(id)
    try {
      await action()
    } finally {
      set.delete(id)
    }
  }

  const start = (id: string) => track(busy, id, () => core.startProfile(id))
  const stop = (id: string) => track(stopping, id, () => core.stopProfile(id))

  return {
    rows,
    total,
    running,
    loading,
    active,
    busy,
    stopping,
    filters,
    only,
    page,
    pageSize,
    selected,
    visibleRows,
    hasTagFilters,
    tagFacets,
    pageRows,
    pageCount,
    rangeFrom,
    rangeTo,
    selectedIds,
    selectedCount,
    allSelected,
    listQuery,
    refresh,
    scheduleRefresh,
    cancelTimers,
    setActive,
    applyEvent,
    setPage,
    setPageSize,
    toggle,
    selectAll,
    clearSelection,
    showOnly,
    openGroup,
    resetMissing,
    toggleTag,
    toggleUntagged,
    clearTagFilters,
    resetSort,
    start,
    stop,
  }
})
