import { enableAutoUnmount, flushPromises } from '@vue/test-utils'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { profileRow } from '../../test/fixtures'
import { mountWithApp } from '../../test/mount'
import { useGroupsStore } from '../../stores/groups'
import { useProfilesStore } from '../../stores/profiles'
import ProfileTagFilters from './ProfileTagFilters.vue'

const api = vi.hoisted(() => ({ listProfiles: vi.fn(async () => ({ rows: [], total: 0, running: 0 })) }))
vi.mock('../../api/core', () => ({ core: api }))
enableAutoUnmount(afterEach)
beforeEach(() => {
  vi.clearAllMocks()
  useProfilesStore().rows = [
    profileRow({ id: 'both', tags: ['Ёж', 'vip'] }),
    profileRow({ id: 'one', tags: ['еж'] }),
    profileRow({ id: 'other', tags: ['shop'] }),
    profileRow({ id: 'none', tags: [] }),
  ]
  useGroupsStore().tags = [{ id: 1, name: 'Ёж', profile_count: 2 }, { id: 2, name: 'vip', profile_count: 1 }, { id: 3, name: 'shop', profile_count: 1 }, { id: 4, name: 'elsewhere', profile_count: 1 }]
})

describe('отбор по пересечениям тегов', () => {
  it('сортирует по частоте во всех профилях, затем по алфавиту; фильтрация не переставляет кнопки', async () => {
    const profiles = useProfilesStore()
    const groups = useGroupsStore()
    profiles.rows = [profileRow({ tags: ['Alpha', 'beta'] }), profileRow({ id: 'p2', tags: ['zulu'] })]
    groups.tags = [{ id: 1, name: 'zulu', profile_count: 8 }, { id: 2, name: 'beta', profile_count: 3 }, { id: 3, name: 'Alpha', profile_count: 3 }]
    const wrapper = mountWithApp(ProfileTagFilters)
    const order = () => wrapper.findAll('[data-tag-filter]').map((button) => button.attributes('data-tag-filter'))
    expect(order()).toEqual(['untagged', 'zulu', 'alpha', 'beta'])
    await wrapper.get('[data-tag-filter="alpha"]').trigger('click')
    expect(order()).toEqual(['untagged', 'zulu', 'alpha', 'beta'])
    expect(wrapper.get<HTMLButtonElement>('[data-tag-filter="zulu"]').element.disabled).toBe(true)
    groups.tags[1]!.profile_count = 9
    await flushPromises()
    expect(order()).toEqual(['untagged', 'beta', 'zulu', 'alpha'])
  })

  it('подсвечивает несколько тегов, показывает пересечение и отключает невозможные сочетания', async () => {
    const profiles = useProfilesStore()
    const wrapper = mountWithApp(ProfileTagFilters)
    const tag = (name: string) => wrapper.get<HTMLButtonElement>(`[data-tag-filter="${name}"]`)
    expect(tag('elsewhere').element.disabled).toBe(true)
    await tag('еж').trigger('click')
    expect(tag('еж').attributes('aria-pressed')).toBe('true')
    expect(profiles.visibleRows.map((row) => row.id)).toEqual(['both', 'one'])
    expect(tag('vip').element.disabled).toBe(false)
    expect(tag('shop').element.disabled).toBe(true)
    expect(tag('untagged').element.disabled).toBe(true)
    await tag('vip').trigger('click')
    expect(profiles.visibleRows.map((row) => row.id)).toEqual(['both'])
    expect(tag('еж').element.disabled).toBe(false)
    expect(tag('vip').attributes('aria-pressed')).toBe('true')
    await tag('vip').trigger('click')
    expect(profiles.visibleRows).toHaveLength(2)
    await wrapper.get('.tag-filters-clear').trigger('click')
    expect(profiles.visibleRows).toHaveLength(4)
    expect(tag('shop').element.disabled).toBe(false)
    expect(tag('untagged').element.disabled).toBe(false)
    expect(api.listProfiles).not.toHaveBeenCalled()
  })

  it('«Без тегов» выбирает только пустые профили и снимается повторным нажатием', async () => {
    const profiles = useProfilesStore()
    const wrapper = mountWithApp(ProfileTagFilters)
    await wrapper.get('[data-tag-filter="untagged"]').trigger('click')
    expect(profiles.visibleRows.map((row) => row.id)).toEqual(['none'])
    expect(wrapper.get<HTMLButtonElement>('[data-tag-filter="еж"]').element.disabled).toBe(true)
    expect(wrapper.get('[data-tag-filter="untagged"]').attributes('aria-pressed')).toBe('true')
    await wrapper.get('[data-tag-filter="untagged"]').trigger('click')
    expect(profiles.visibleRows).toHaveLength(4)
    expect(wrapper.get<HTMLButtonElement>('[data-tag-filter="vip"]').element.disabled).toBe(false)
  })

  it('доступность и строки обновляются после правки тегов, выбранный тег можно снять при пустом результате', async () => {
    const profiles = useProfilesStore()
    const wrapper = mountWithApp(ProfileTagFilters)
    await wrapper.get('[data-tag-filter="vip"]').trigger('click')
    profiles.rows[0]!.tags = ['shop']
    await flushPromises()
    expect(profiles.visibleRows).toHaveLength(0)
    expect(wrapper.get<HTMLButtonElement>('[data-tag-filter="vip"]').element.disabled).toBe(false)
    await wrapper.get('[data-tag-filter="vip"]').trigger('click')
    expect(profiles.visibleRows).toHaveLength(4)
  })
})
