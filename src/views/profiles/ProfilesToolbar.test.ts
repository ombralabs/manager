import { type VueWrapper, enableAutoUnmount, flushPromises } from '@vue/test-utils'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { ListQuery } from '../../api/types'
import type { OptionalColumn } from '../../lib/columns'
import { mountWithApp } from '../../test/mount'
import { profileLabel } from '../../test/fixtures'

const api = vi.hoisted(() => ({
  listProfileLabels: vi.fn(async () => [profileLabel()]),
  listProfiles: vi.fn(async (_query: ListQuery) => ({ rows: [], total: 0, running: 0 })),
}))
vi.mock('../../api/core', () => ({ core: api, events: {} }))

import { useProfilesStore } from '../../stores/profiles'
import ProfilesToolbar from './ProfilesToolbar.vue'

enableAutoUnmount(afterEach)

function mountToolbar(hidden: OptionalColumn[]) {
  return mountWithApp(ProfilesToolbar, {
    props: { hidden },
    attachTo: document.body,
  })
}

const badge = (wrapper: VueWrapper) => wrapper.find('[data-test="sort-badge"]')

describe('инструменты профилей в шапке', () => {
  beforeEach(() => {
    vi.clearAllMocks()
  })

  it('сортировка по скрытой колонке — метка с колонкой и направлением', async () => {
    const wrapper = mountToolbar(['created', 'last_opened'])
    const profiles = useProfilesStore()
    expect(badge(wrapper).exists()).toBe(false)
    profiles.filters.sort = 'created'
    profiles.filters.descending = true
    await flushPromises()
    expect(badge(wrapper).text()).toContain('Сортировка: Создан')
    expect(badge(wrapper).text()).toContain('по убыванию')
    await wrapper.setProps({ hidden: ['last_opened'] })
    expect(badge(wrapper).exists()).toBe(false)
    profiles.filters.sort = 'name'
    await wrapper.setProps({ hidden: ['created', 'last_opened'] })
    expect(badge(wrapper).exists()).toBe(false)
  })

  it('сброс — «№» по возрастанию одним запросом', async () => {
    const wrapper = mountToolbar(['last_opened'])
    const profiles = useProfilesStore()
    profiles.filters.sort = 'last_opened'
    await flushPromises()
    expect(badge(wrapper).text()).toContain('Сортировка: Последний запуск')
    expect(badge(wrapper).text()).toContain('по возрастанию')
    profiles.filters.descending = true
    await flushPromises()
    api.listProfiles.mockClear()
    await badge(wrapper).find('button[aria-label="Сбросить сортировку"]').trigger('click')
    await flushPromises()
    expect([profiles.filters.sort, profiles.filters.descending]).toEqual(['serial', false])
    expect(api.listProfiles).toHaveBeenCalledTimes(1)
    expect(api.listProfiles.mock.calls[0]![0]).toMatchObject({ sort: 'serial', descending: false })
    expect(badge(wrapper).exists()).toBe(false)
  })

  it('поиск скрыт за лупой, открывается с фокусом и отбирает по вводу без отправки формы', async () => {
    const wrapper = mountToolbar([])
    expect(wrapper.find('input[type="search"]').exists()).toBe(false)
    expect(wrapper.find('[role="combobox"]').exists()).toBe(false)
    expect(wrapper.text()).not.toContain('Фильтры')
    expect(wrapper.text()).not.toContain('Колонки')
    await wrapper.get('button[aria-label="Поиск"]').trigger('click')
    const input = wrapper.get('input[type="search"]')
    expect(document.activeElement).toBe(input.element)
    expect(wrapper.get('.search-toggle').attributes('aria-expanded')).toBe('true')
    await input.setValue('магазин')
    expect(useProfilesStore().filters.search).toBe('магазин')
    await vi.waitFor(() => expect(api.listProfiles).toHaveBeenLastCalledWith(expect.objectContaining({ search: 'магазин' })))
  })

  it.each(['escape', 'toggle'])('закрытие через %s очищает запрос и возвращает фокус на лупу', async (action) => {
    const wrapper = mountToolbar([])
    await wrapper.get('.search-toggle').trigger('click')
    await wrapper.get('input[type="search"]').setValue('shop')
    if (action === 'escape') await wrapper.get('input[type="search"]').trigger('keydown', { key: 'Escape' })
    else await wrapper.get('.search-toggle').trigger('click')
    expect(wrapper.find('input[type="search"]').exists()).toBe(false)
    expect(useProfilesStore().filters.search).toBe('')
    expect(document.activeElement).toBe(wrapper.get('.search-toggle').element)
    await vi.waitFor(() => expect(api.listProfiles).toHaveBeenLastCalledWith(expect.objectContaining({ search: '' })))
  })

  it('активный запрос после возвращения на экран остаётся видимым', async () => {
    const wrapper = mountToolbar([])
    useProfilesStore().filters.search = 'Сохранённый поиск'
    await wrapper.vm.$nextTick()
    expect(wrapper.get<HTMLInputElement>('input[type="search"]').element.value).toBe('Сохранённый поиск')
    wrapper.unmount()
    const reopened = mountToolbar([])
    expect(reopened.get<HTMLInputElement>('input[type="search"]').element.value).toBe('Сохранённый поиск')
    useProfilesStore().cancelTimers()
  })
})
