import { type VueWrapper, flushPromises } from '@vue/test-utils'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { CookieStats } from '../../api/types'
import { mountWithApp } from '../../test/mount'

const api = vi.hoisted(() => ({
  cookieStats: vi.fn(async (): Promise<CookieStats> => ({ total: 42, unreadable: 3 })),
  clearCookies: vi.fn(async () => null),
  importCookies: vi.fn(),
  exportCookiesTo: vi.fn(),
}))
vi.mock('../../api/core', () => ({ core: api, events: {} }))
vi.mock('../../api/dialogs', () => ({ pickCookieFile: vi.fn(), pickFolder: vi.fn(), pickSaveFile: vi.fn() }))

import CookiesTab from './CookiesTab.vue'

function mountTab(props: { profileId: string | null; running?: boolean }) {
  return mountWithApp(CookiesTab, {
    props: { profileName: 'Магазин', running: false, ...props },
  })
}

function button(wrapper: VueWrapper, label: string) {
  const found = wrapper.findAll('button').find((b) => b.text() === label)
  if (!found) throw new Error(`нет кнопки «${label}»`)
  return found
}

describe('вкладка «Cookies»', () => {
  beforeEach(() => {
    vi.clearAllMocks()
  })

  it('число cookies и предупреждение о нечитаемых записях', async () => {
    const wrapper = mountTab({ profileId: 'k7f2q9' })
    await flushPromises()
    expect(api.cookieStats).toHaveBeenCalledWith('k7f2q9')
    expect(wrapper.text()).toContain('Cookies профиля: 42')
    expect(wrapper.text()).toContain('3 записи не удалось прочитать')
  })

  it('без нечитаемых записей предупреждения нет', async () => {
    api.cookieStats.mockResolvedValueOnce({ total: 5, unreadable: 0 })
    const wrapper = mountTab({ profileId: 'k7f2q9' })
    await flushPromises()
    expect(wrapper.text()).toContain('Cookies профиля: 5')
    expect(wrapper.text()).not.toContain('не удалось прочитать')
  })

  it('у работающего профиля «Очистить cookies» неактивна', async () => {
    const wrapper = mountTab({ profileId: 'k7f2q9', running: true })
    await flushPromises()
    expect(button(wrapper, 'Очистить cookies').attributes('disabled')).toBeDefined()
    expect(wrapper.text()).toContain('Сначала остановите профиль')
  })

  it('очистка — только после подтверждения, затем число перечитывается', async () => {
    const wrapper = mountTab({ profileId: 'k7f2q9' })
    await flushPromises()
    await button(wrapper, 'Очистить cookies').trigger('click')
    await flushPromises()
    expect(wrapper.text()).toContain('Все cookies профиля будут удалены')
    expect(api.clearCookies).not.toHaveBeenCalled()
    api.cookieStats.mockResolvedValueOnce({ total: 0, unreadable: 0 })
    await button(wrapper, 'Очистить').trigger('click')
    await flushPromises()
    expect(api.clearCookies).toHaveBeenCalledWith('k7f2q9')
    expect(api.cookieStats).toHaveBeenCalledTimes(2)
    expect(wrapper.text()).toContain('Cookies профиля: 0')
  })

  it('новый профиль — только подсказка, ядро не вызывается', async () => {
    const wrapper = mountTab({ profileId: null })
    await flushPromises()
    expect(wrapper.text()).toContain('Cookies можно импортировать после создания профиля')
    expect(api.cookieStats).not.toHaveBeenCalled()
    expect(wrapper.findAll('button').some((b) => b.text() === 'Импорт…')).toBe(false)
  })
})
