import { enableAutoUnmount, flushPromises } from '@vue/test-utils'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { mountWithApp } from '../../test/mount'
import { group, profileRow } from '../../test/fixtures'
import { PREF_PROFILE_GROUPS_COLLAPSED } from '../../lib/prefs'

vi.mock('../../api/core', () => ({ core: { listProfiles: vi.fn(async () => ({ rows: [], total: 0, running: 0 })) }, events: {} }))
import { useGroupsStore } from '../../stores/groups'
import { useProfilesStore } from '../../stores/profiles'
import ProfileGroupsSidebar from './ProfileGroupsSidebar.vue'

enableAutoUnmount(afterEach)
beforeEach(() => {
  localStorage.clear()
  localStorage.setItem(PREF_PROFILE_GROUPS_COLLAPSED, 'false')
  const groups = useGroupsStore()
  groups.loaded = true
  groups.groups = [group({ id: 7, name: 'Приёмка', profile_count: 2, quick_access: false }), group({ id: 8, name: 'Архив', profile_count: 3 })]
  groups.rows = Array.from({ length: 6 }, (_, i) => profileRow({ id: `p${i}` }))
  groups.ungrouped = 1
})
afterEach(() => { useProfilesStore().cancelTimers(); vi.restoreAllMocks() })
const mountSidebar = () => mountWithApp(ProfileGroupsSidebar, { global: { stubs: { RouterLink: { template: '<a><slot /></a>' } } } })

describe('панель групп профилей', () => {
  it('показывает все группы со счётчиками; выбор сохраняет поиск, теги и ручной статус', async () => {
    const profiles = useProfilesStore()
    const wrapper = mountSidebar()
    profiles.filters.label = 2
    profiles.filters.tags = ['vip']
    profiles.filters.search = 'магазин'
    await wrapper.get('[data-group-id="7"]').trigger('click')
    expect({ ...profiles.filters }).toMatchObject({ group: 7, label: 2, tags: ['vip'], search: 'магазин' })
    expect(wrapper.get('[data-group-id="7"]').attributes('aria-pressed')).toBe('true')
    expect(wrapper.get('[data-group-id="7"] .group-count').text()).toBe('2')
    expect(wrapper.findAll('.system-groups .group-count').map((el) => el.text())).toEqual(['6', '1'])
    await wrapper.findAll('.system-groups button')[1]!.trigger('click')
    expect(profiles.filters.group).toBe('ungrouped')
    await wrapper.findAll('.system-groups button')[0]!.trigger('click')
    expect(profiles.filters.group).toBe('all')
    expect(profiles.filters.label).toBe(2)
  })

  it('поиск групп не меняет профильный поиск и выбранную группу', async () => {
    const profiles = useProfilesStore()
    profiles.filters.group = 7
    const wrapper = mountSidebar()
    await wrapper.get('input[type="search"]').setValue('прием')
    expect(wrapper.findAll('[data-group-id]')).toHaveLength(1)
    expect(wrapper.find('[data-group-id="7"]').exists()).toBe(true)
    await wrapper.get('input[type="search"]').setValue('нет такой')
    expect(wrapper.find('.groups-empty').text()).toBe('Группы не найдены')
    expect(profiles.filters.search).toBe('')
    expect(profiles.filters.group).toBe(7)
  })

  it('сворачивание сохраняется, выбранная группа остаётся; разворачивание возвращает список', async () => {
    useProfilesStore().filters.group = 7
    const wrapper = mountSidebar()
    await wrapper.get('button[aria-label="Свернуть группы"]').trigger('click')
    expect(wrapper.classes()).toContain('collapsed')
    expect(localStorage.getItem(PREF_PROFILE_GROUPS_COLLAPSED)).toBe('true')
    expect(useProfilesStore().filters.group).toBe(7)
    wrapper.unmount()
    const reopened = mountSidebar()
    expect(reopened.classes()).toContain('collapsed')
    await reopened.get('button[aria-label="Развернуть группы"]').trigger('click')
    expect(reopened.get('[data-group-id="7"]').attributes('aria-pressed')).toBe('true')
    await flushPromises()
  })
})
