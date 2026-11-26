import { createPinia, setActivePinia } from 'pinia'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { ListQuery, ProfileList, ProfileRow } from '../api/types'
import { profileRow } from '../test/fixtures'

const pending: { query: ListQuery; resolve: (v: ProfileList) => void; reject: (e: unknown) => void }[] = []

vi.mock('../api/core', () => ({
  core: {
    listProfiles: (query: ListQuery) =>
      new Promise<ProfileList>((resolve, reject) => {
        pending.push({ query, resolve, reject })
      }),
  },
}))

import { useProfilesStore } from './profiles'

type Store = ReturnType<typeof useProfilesStore>

const list = (total: number): ProfileList => ({ rows: [], total, running: total })

const row = (n: number): ProfileRow =>
  profileRow({ id: `p${n}`, serial: n, name: `Профиль ${n}`, seed: n, created_at: n })

const rowsList = (count: number): ProfileList => ({
  rows: Array.from({ length: count }, (_, i) => row(i + 1)),
  total: count,
  running: 0,
})

/** Обновить список и ответить на этот запрос (в полёте ничего нет). */
async function answer(store: Store, value: ProfileList) {
  const done = store.refresh()
  pending.at(-1)!.resolve(value)
  await done
}

/** Дать отработать цепочкам промисов: таймеры поддельные, `flushPromises` с ними не дождётся. */
async function settle() {
  for (let i = 0; i < 10; i++) await Promise.resolve()
}

/** Ответить на запрос в полёте и на отложенный, который пойдёт за ним. В полёте всегда последний запрос. */
async function drain(value: ProfileList = list(0)) {
  let seen = -1
  while (pending.length !== seen) {
    seen = pending.length
    pending.at(-1)?.resolve(value)
    await settle()
  }
}

beforeEach(() => {
  pending.length = 0
  vi.useFakeTimers()
  localStorage.clear()
  setActivePinia(createPinia())
})

afterEach(() => {
  vi.useRealTimers()
})

describe('обновление списка', () => {
  it('один запрос в полёте: повтор ждёт его ответа; ответ на прежние фильтры не применяется', async () => {
    const store = useProfilesStore()
    store.filters.search = 'a'
    const first = store.refresh()
    store.filters.search = 'ab'
    const second = store.refresh()
    expect(pending.map((p) => p.query.search)).toEqual(['a'])

    pending[0]!.resolve(list(1))
    await first
    expect(store.total).toBe(0)
    expect(store.loading).toBe(true)
    expect(pending.map((p) => p.query.search)).toEqual(['a', 'ab'])

    pending[1]!.resolve(list(2))
    await second
    expect(store.total).toBe(2)
    expect(store.loading).toBe(false)
  })

  it('loading держится, пока есть отложенный запрос', async () => {
    const store = useProfilesStore()
    const first = store.refresh()
    const second = store.refresh()
    pending[0]!.resolve(list(1))
    await first
    expect(store.total).toBe(1)
    expect(store.loading).toBe(true)
    pending[1]!.resolve(list(2))
    await second
    expect(store.loading).toBe(false)
    expect(store.total).toBe(2)
  })

  it('ошибка запроса, за которым отложен следующий, не всплывает; ошибка последнего — всплывает', async () => {
    const store = useProfilesStore()
    const first = store.refresh()
    const second = store.refresh()
    pending[0]!.reject(new Error('stale'))
    await expect(first).resolves.toBeUndefined()
    pending[1]!.reject(new Error('latest'))
    await expect(second).rejects.toThrow('latest')
    expect(store.loading).toBe(false)
  })

  it('строит запрос ядра из фильтров', () => {
    const store = useProfilesStore()
    expect(store.listQuery()).toEqual({
      search: '',
      sort: 'serial',
      descending: false,
      status: null,
      label_id: null,
      group_id: null,
      ungrouped: false,
      tag: null,
    })
    store.filters.group = 4
    store.filters.tags = ['vip']
    expect(store.listQuery()).toMatchObject({ group_id: 4, ungrouped: false, tag: null })
    store.filters.group = 'ungrouped'
    expect(store.listQuery()).toMatchObject({ group_id: null, ungrouped: true })
  })
})

describe('страницы', () => {
  it('пересечение тегов и массовое выделение учитывают все страницы; отбор меняется без запроса', async () => {
    const store = useProfilesStore()
    const data = rowsList(120)
    data.rows.forEach((row, i) => { row.tags = i === 119 ? [] : i < 60 ? ['wide'] : ['wide', 'vip'] })
    await answer(store, data)
    store.setPage(2)
    store.selectAll()
    store.toggleTag('wide')
    expect([store.page, store.selectedCount, store.visibleRows.length]).toEqual([0, 0, 119])
    expect(store.tagFacets.counts.get('vip')).toBe(59)
    store.toggleTag('vip')
    expect(store.visibleRows).toHaveLength(59)
    expect(store.pageCount).toBe(2)
    store.setPage(1)
    expect(store.pageRows.map((row) => row.id)).toEqual(['p111', 'p112', 'p113', 'p114', 'p115', 'p116', 'p117', 'p118', 'p119'])
    store.selectAll()
    expect(store.selectedIds).toHaveLength(59)
    store.rows[60]!.tags = []
    expect(store.selectedIds).toHaveLength(58)
    expect(store.selected.has('p61')).toBe(false)
    store.clearTagFilters()
    store.toggleUntagged()
    expect(store.visibleRows.map((row) => row.id)).toEqual(['p61', 'p120'])
    store.toggleTag('vip')
    expect(store.filters.tags).toEqual([])
    expect(pending).toHaveLength(1)
  })

  it('режет список и считает диапазон', async () => {
    const store = useProfilesStore()
    expect([store.pageCount, store.rangeFrom, store.rangeTo]).toEqual([1, 0, 0])
    await answer(store, rowsList(120))
    expect([store.pageCount, store.rangeFrom, store.rangeTo, store.pageRows.length]).toEqual([3, 1, 50, 50])
    store.setPage(1)
    expect([store.rangeFrom, store.rangeTo]).toEqual([51, 100])
    store.setPage(2)
    expect([store.rangeFrom, store.rangeTo, store.pageRows[0]!.id]).toEqual([101, 120, 'p101'])
    store.setPage(9)
    expect(store.page).toBe(2)
    store.setPage(-1)
    expect(store.page).toBe(0)
  })

  it('размер страницы возвращает на первую страницу и запоминается', async () => {
    const store = useProfilesStore()
    await answer(store, rowsList(120))
    store.setPage(2)
    store.setPageSize(100)
    expect([store.page, store.pageCount, store.rangeTo]).toEqual([0, 2, 100])
    expect(localStorage.getItem('ombra.profiles.pageSize')).toBe('100')
    setActivePinia(createPinia())
    expect(useProfilesStore().pageSize).toBe(100)
  })

  it('испорченный размер страницы в localStorage — 50', () => {
    localStorage.setItem('ombra.profiles.pageSize', '37')
    expect(useProfilesStore().pageSize).toBe(50)
  })

  it('после обновления страница не выходит за завершение списка', async () => {
    const store = useProfilesStore()
    await answer(store, rowsList(120))
    store.setPage(2)
    await answer(store, rowsList(60))
    expect([store.page, store.rangeFrom, store.rangeTo]).toEqual([1, 51, 60])
  })
})

describe('выделение', () => {
  it('сохраняется между страницами и сбрасывается при смене фильтра', async () => {
    const store = useProfilesStore()
    await answer(store, rowsList(120))
    store.toggle('p1')
    store.setPage(1)
    store.toggle('p60')
    expect(store.selectedCount).toBe(2)
    store.setPage(0)
    expect(store.selectedIds).toEqual(['p1', 'p60'])

    store.filters.label = 2
    expect(store.selectedCount).toBe(0)
    expect(store.page).toBe(0)
    expect(pending.at(-1)!.query.label_id).toBe(2)
  })

  it('«Выбрать все» берёт все профили по фильтру, а не только страницу', async () => {
    const store = useProfilesStore()
    await answer(store, rowsList(120))
    store.toggle('p1')
    expect([store.selectedCount, store.allSelected]).toEqual([1, false])
    store.selectAll()
    expect([store.selectedCount, store.allSelected]).toEqual([120, true])
    store.clearSelection()
    expect([store.selectedCount, store.allSelected]).toEqual([0, false])
  })

  it('обновление убирает из выделения исчезнувшие профили', async () => {
    const store = useProfilesStore()
    await answer(store, rowsList(5))
    store.selectAll()
    await answer(store, { rows: rowsList(5).rows.slice(0, 3), total: 3, running: 0 })
    expect(store.selectedIds).toEqual(['p1', 'p2', 'p3'])
    expect(store.selectedCount).toBe(3)
  })
})

describe('обновления по таймеру', () => {
  it('десять запросов обновления за 500 мс дают одно обращение к ядру', async () => {
    const store = useProfilesStore()
    store.setActive(true)
    for (let i = 0; i < 10; i++) store.scheduleRefresh()
    expect(pending).toHaveLength(0)
    vi.advanceTimersByTime(499)
    expect(pending).toHaveLength(0)
    vi.advanceTimersByTime(1)
    expect(pending).toHaveLength(1)
    pending[0]!.resolve(list(1))
    await settle()
    store.scheduleRefresh()
    vi.advanceTimersByTime(500)
    expect(pending).toHaveLength(2)
  })

  it('поиск ждёт 200 мс после последнего изменения', () => {
    const store = useProfilesStore()
    store.filters.search = 'аб'
    vi.advanceTimersByTime(199)
    store.filters.search = 'абв'
    vi.advanceTimersByTime(199)
    expect(pending).toHaveLength(0)
    vi.advanceTimersByTime(1)
    expect(pending.map((p) => p.query.search)).toEqual(['абв'])
  })

  it('cancelTimers отменяет запланированное', () => {
    const store = useProfilesStore()
    store.setActive(true)
    store.filters.search = 'a'
    store.scheduleRefresh()
    store.cancelTimers()
    vi.advanceTimersByTime(1000)
    expect(pending).toHaveLength(0)
  })

  it('событие супервизора меняет строку сразу, список — через 250 мс', async () => {
    const store = useProfilesStore()
    store.setActive(true)
    await answer(store, rowsList(3))
    store.applyEvent({ kind: 'status', profile_id: 'p2', status: { state: 'starting' } })
    expect(store.rows[1]!.status.state).toBe('starting')
    expect(pending).toHaveLength(1)
    vi.advanceTimersByTime(250)
    expect(pending).toHaveLength(2)
  })

  it('события во время медленного запроса — один отложенный, ответ полёта применяется', async () => {
    const store = useProfilesStore()
    store.setActive(true)
    await answer(store, rowsList(3))
    for (let i = 0; i < 4; i++) {
      store.applyEvent({ kind: 'status', profile_id: 'p1', status: { state: 'starting' } })
      vi.advanceTimersByTime(250)
    }
    expect(pending).toHaveLength(2)
    pending[1]!.resolve({ ...rowsList(3), running: 1 })
    await settle()
    expect(store.running).toBe(1)
    expect(pending).toHaveLength(3)
    expect(store.loading).toBe(true)
    pending[2]!.resolve({ ...rowsList(3), running: 2 })
    await settle()
    expect([store.running, store.loading]).toEqual([2, false])
  })
})

describe('экран «Профили» закрыт', () => {
  it('20 событий не запрашивают список; открытие экрана — один запрос', async () => {
    const store = useProfilesStore()
    await answer(store, rowsList(3))
    for (let i = 0; i < 20; i++) {
      store.applyEvent({ kind: 'status', profile_id: 'p2', status: { state: 'starting' } })
      store.scheduleRefresh()
      vi.advanceTimersByTime(300)
    }
    expect(pending).toHaveLength(1)
    expect(store.rows[1]!.status.state).toBe('starting')
    store.setActive(true)
    void store.refresh()
    vi.advanceTimersByTime(1000)
    expect(pending).toHaveLength(2)
  })

  it('уход с экрана отменяет запланированное обновление', () => {
    const store = useProfilesStore()
    store.setActive(true)
    store.scheduleRefresh()
    store.setActive(false)
    vi.advanceTimersByTime(1000)
    expect(pending).toHaveLength(0)
  })
})

describe('фильтры из кода', () => {
  it('showOnly показывает только эти профили до смены фильтра', async () => {
    const store = useProfilesStore()
    await answer(store, rowsList(10))
    store.filters.label = 2
    store.filters.descending = true
    store.toggle('p1')
    store.showOnly(['p2', 'p5'])
    expect(store.selectedCount).toBe(0)
    await drain(rowsList(10))
    expect(pending.at(-1)!.query).toMatchObject({ label_id: null, status: null, descending: true })
    expect(store.visibleRows.map((r) => r.id)).toEqual(['p2', 'p5'])
    expect(store.rangeTo).toBe(2)
    store.filters.search = 'x'
    expect(store.only).toBeNull()
  })

  it('openGroup ставит фильтр группы и сбрасывает остальные', async () => {
    const store = useProfilesStore()
    store.filters.search = 'x'
    store.filters.sort = 'name'
    await drain()
    const before = pending.length
    store.openGroup(5)
    expect(pending).toHaveLength(before + 1)
    expect({ ...store.filters }).toEqual({ search: '', sort: 'name', descending: false, label: 'any', group: 5, tags: [], untagged: false })
    expect(pending.at(-1)!.query).toMatchObject({ search: '', group_id: 5, ungrouped: false })
    await drain()
    store.openGroup('ungrouped')
    expect(pending.at(-1)!.query).toMatchObject({ group_id: null, ungrouped: true })
    vi.advanceTimersByTime(1000)
    expect(pending).toHaveLength(before + 2)
  })

  it('фильтр по исчезнувшей группе или тегу сбрасывается', async () => {
    const store = useProfilesStore()
    store.filters.group = 7
    store.filters.tags = ['vip']
    await drain()
    const before = pending.length
    store.resetMissing([1, 7], ['VIP'])
    expect(pending).toHaveLength(before)
    expect([store.filters.group, store.filters.tags]).toEqual([7, ['vip']])
    store.resetMissing([1], ['VIP'])
    expect([store.filters.group, store.filters.tags]).toEqual(['all', ['vip']])
    expect(pending).toHaveLength(before + 1)
    await drain()
    store.resetMissing([1], [])
    expect(store.filters.tags).toEqual([])
    expect(pending).toHaveLength(before + 1)
  })

  it('фильтр тега сравнивается со свёрткой ё → е, как в ядре', () => {
    const store = useProfilesStore()
    store.filters.tags = ['еж']
    store.resetMissing([], ['Ёж'])
    expect(store.filters.tags).toEqual(['еж'])
  })
})
