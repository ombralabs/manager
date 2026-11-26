import { createPinia, setActivePinia } from 'pinia'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { GroupRecord, ProfileRow } from '../api/types'
import { group, profileRow } from '../test/fixtures'

const mocks = vi.hoisted(() => ({
  listGroups: vi.fn(),
  listTags: vi.fn(),
  listProfiles: vi.fn(),
  createGroup: vi.fn(),
  changeProfileTags: vi.fn(),
}))
vi.mock('../api/core', () => ({ core: mocks }))

import { useGroupsStore } from './groups'
import { useProfilesStore } from './profiles'

const g = (id: number, name: string, quick_access = false): GroupRecord => group({ id, name, quick_access })

const row = (id: string): ProfileRow => profileRow({ id, name: id })

beforeEach(() => {
  vi.useFakeTimers()
  localStorage.clear()
  setActivePinia(createPinia())
  mocks.listGroups.mockReset().mockResolvedValue({ groups: [g(1, 'Магазины', true), g(2, 'Реклама')], ungrouped: 3 })
  mocks.listTags.mockReset().mockResolvedValue([{ id: 1, name: 'vip', profile_count: 2 }])
  mocks.listProfiles.mockReset().mockResolvedValue({ rows: [row('a'), row('b')], total: 2, running: 0 })
  mocks.createGroup.mockReset()
  mocks.changeProfileTags.mockReset()
})

afterEach(() => {
  vi.useRealTimers()
})

describe('хранилище групп', () => {
  it('быстрая правка тегов обновляет обе копии профиля и общий справочник', async () => {
    const store = useGroupsStore()
    const profiles = useProfilesStore()
    const before = profileRow({ id: 'a', name: 'Магазин', tags: ['old', 'Shop'], status: { state: 'starting' } })
    profiles.rows = [{ ...before }]
    store.rows = [{ ...before }]
    mocks.changeProfileTags.mockResolvedValueOnce(['Shop', 'vip'])
    mocks.listProfiles.mockResolvedValueOnce({ rows: [{ ...before, tags: ['Shop', 'vip'] }], total: 1, running: 1 })
    mocks.listTags.mockResolvedValueOnce([{ id: 2, name: 'Shop', profile_count: 1 }, { id: 3, name: 'vip', profile_count: 1 }])
    await store.changeTags('a', ['VIP'], ['old'])
    expect(profiles.rows[0]!.tags).toEqual(['Shop', 'vip'])
    expect(store.rows[0]!.tags).toEqual(['Shop', 'vip'])
    expect(profiles.rows[0]!.status).toEqual(before.status)
    expect(profiles.rows[0]!.name).toBe('Магазин')
    expect(store.tags.map((tag) => tag.name)).toEqual(['Shop', 'vip'])
    expect(mocks.changeProfileTags).toHaveBeenCalledWith('a', ['VIP'], ['old'])
  })

  it('ошибка записи не меняет теги и не обновляет справочник как будто запись прошла', async () => {
    const store = useGroupsStore()
    const profiles = useProfilesStore()
    store.rows = [profileRow({ tags: ['old'] })]
    profiles.rows = [profileRow({ tags: ['old'] })]
    mocks.changeProfileTags.mockRejectedValueOnce(new Error('offline'))
    await expect(store.changeTags('p1', ['vip'], ['old'])).rejects.toThrow('offline')
    expect(store.rows[0]!.tags).toEqual(['old'])
    expect(profiles.rows[0]!.tags).toEqual(['old'])
    expect(mocks.listTags).not.toHaveBeenCalled()
  })

  it('загружает группы, теги и все профили без фильтров', async () => {
    const store = useGroupsStore()
    await store.refresh()
    expect(store.loaded).toBe(true)
    expect(store.quickAccess.map((g) => g.name)).toEqual(['Магазины'])
    expect([store.ungrouped, store.rows.length, store.tags[0]!.name]).toEqual([3, 2, 'vip'])
    expect(store.byId(2)?.name).toBe('Реклама')
    expect(mocks.listProfiles).toHaveBeenCalledWith({
      search: '',
      sort: 'serial',
      descending: false,
      status: null,
      group_id: null,
      ungrouped: false,
      tag: null,
    })
  })

  it('после загрузки сбрасывает фильтр списка по исчезнувшей группе', async () => {
    const profiles = useProfilesStore()
    profiles.filters.group = 9
    profiles.filters.tags = ['VIP']
    await useGroupsStore().refresh()
    expect(profiles.filters.group).toBe('all')
    expect(profiles.filters.tags).toEqual(['VIP'])
  })

  it('событие меняет статус строки без запроса к ядру', async () => {
    const store = useGroupsStore()
    await store.refresh()
    mocks.listProfiles.mockClear()
    store.applyEvent({ kind: 'status', profile_id: 'b', status: { state: 'crashed', code: 1, signal: null, at: 5 } })
    expect(store.rows[1]!.status.state).toBe('crashed')
    vi.advanceTimersByTime(1000)
    expect(mocks.listProfiles).not.toHaveBeenCalled()
  })

  it('создание группы обновляет списки', async () => {
    const store = useGroupsStore()
    const form = { name: 'Почта', color: '#6B46C1', note: '', quick_access: false }
    mocks.createGroup.mockResolvedValue(g(3, 'Почта'))
    const created = await store.create(form)
    expect(created.id).toBe(3)
    expect(mocks.createGroup).toHaveBeenCalledWith(form)
    expect(mocks.listGroups).toHaveBeenCalledTimes(1)
  })

  it('обновление по событиям — не чаще раза в 500 мс', () => {
    const store = useGroupsStore()
    for (let i = 0; i < 5; i++) store.scheduleRefresh()
    vi.advanceTimersByTime(499)
    expect(mocks.listGroups).not.toHaveBeenCalled()
    vi.advanceTimersByTime(1)
    expect(mocks.listGroups).toHaveBeenCalledTimes(1)
  })
})
