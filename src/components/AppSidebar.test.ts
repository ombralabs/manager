import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { createMemoryHistory, createRouter } from 'vue-router'
import type { ApiStatus } from '../api/types'
import { i18n } from '../i18n'
import { mountWithApp } from '../test/mount'
import { PREF_SIDEBAR_COLLAPSED } from '../lib/prefs'
import { useGroupsStore } from '../stores/groups'
import { useProfilesStore } from '../stores/profiles'
import { group } from '../test/fixtures'
import { flushPromises } from '@vue/test-utils'

vi.mock('../api/core', () => ({ core: { listProfiles: async () => ({ rows: [], total: 0, running: 0 }) }, events: {} }))

import { useSettingsStore } from '../stores/settings'
import AppSidebar from './AppSidebar.vue'
import OmbraMark from './OmbraMark.vue'

const CYRILLIC = /[А-Яа-яЁё]/
const FAILED: ApiStatus = { state: 'failed', port: 50325, code: 'port_busy', reason: 'порт 50325 занят' }

async function mountSidebar(status: ApiStatus, path = '/profiles') {
  const settings = useSettingsStore()
  settings.loaded = true
  settings.setApiStatus(status)
  const router = createRouter({
    history: createMemoryHistory(),
    routes: ['/profiles', '/groups', '/proxies', '/extensions', '/automation', '/trash', '/settings'].map((path) => ({ path, component: { template: '<div />' } })),
  })
  await router.push(path)
  await router.isReady()
  return mountWithApp(AppSidebar, { global: { plugins: [router] }, attachTo: document.body })
}

describe('боковое меню', () => {
  beforeEach(() => { localStorage.clear(); localStorage.setItem(PREF_SIDEBAR_COLLAPSED, 'false') })
  afterEach(() => {
    i18n.global.locale.value = 'ru'
    document.body.innerHTML = ''
  })

  it('строка Local API называет состояние текстом, а не только цветом точки', async () => {
    const listening = await mountSidebar({ state: 'listening', port: 50325 })
    expect(listening.find('.api-line').text()).toContain('127.0.0.1:50325')
    expect(listening.find('.api-line .sr-only').text()).toBe('работает')
    listening.unmount()

    const off = await mountSidebar({ state: 'off' })
    expect(off.find('.api-line').text()).toContain('выключен')
    expect(off.find('.api-line .sr-only').exists()).toBe(false)
    off.unmount()

    const failed = await mountSidebar(FAILED)
    expect(failed.find('.api-line .sr-only').text()).toBe('не запустился: порт занят другой программой')
    expect(failed.find('.api-line').attributes('title')).toBe('Локальный API не запустился: порт занят другой программой')
    expect(failed.find('.api-line').text()).not.toContain('порт 50325 занят')
  })

  it('в английском интерфейсе причина отказа — переводом кода, без русского текста ядра', async () => {
    i18n.global.locale.value = 'en'
    const line = (await mountSidebar(FAILED)).find('.api-line')
    expect(line.find('.sr-only').text()).toBe('failed to start: the port is used by another program')
    expect(line.attributes('title')).toBe('Local API failed to start: the port is used by another program')
    expect(`${line.text()} ${line.attributes('title')}`).not.toMatch(CYRILLIC)
  })

  it('все разделы — ссылки, «Расширения» ведёт на свой экран', async () => {
    const wrapper = await mountSidebar({ state: 'off' })
    expect(wrapper.find('a[href="/extensions"]').text()).toContain('Расширения')
    expect(wrapper.find('a[href="/automation"]').text()).toContain('Автоматизация')
    expect(wrapper.find('a[href="/trash"]').text()).toContain('Корзина')
    expect(wrapper.find('.item.disabled').exists()).toBe(false)
    expect(wrapper.find('[role="tooltip"]').exists()).toBe(false)
  })

  it('при первом открытии меню компактное, все разделы имеют доступные имена', async () => {
    localStorage.clear()
    const wrapper = await mountSidebar({ state: 'off' })
    expect(wrapper.classes()).toContain('collapsed')
    expect(wrapper.get('a[href="/extensions"]').attributes('aria-label')).toBe('Расширения')
    expect(wrapper.get('.bottom a[href="/trash"]').attributes('aria-label')).toBe('Корзина')
  })

  it('шапка — знак Ombra и слово, без прежней плашки', async () => {
    const wrapper = await mountSidebar({ state: 'off' })
    const brand = wrapper.findComponent(OmbraMark)
    expect(brand.text()).toBe('Ombra')
    expect(brand.find('img.mark').exists()).toBe(true)
    expect(brand.find('img.mark').attributes('aria-hidden')).toBe('true')
    expect(wrapper.find('.logo').exists()).toBe(false)
  })

  it('внизу панели под «Настройками» нет переключателя языка', async () => {
    const wrapper = await mountSidebar({ state: 'off' })
    const bottom = wrapper.find('.bottom')
    expect(bottom.text()).toContain('Настройки')
    expect(bottom.find('[role="combobox"]').exists()).toBe(false)
    expect(bottom.element.nextElementSibling?.classList.contains('footer')).toBe(true)
  })

  it('сворачивается до значков, сохраняет выбор и оставляет ссылки доступными', async () => {
    const wrapper = await mountSidebar({ state: 'off' })
    await wrapper.get('button[aria-label="Свернуть меню"]').trigger('click')
    expect(wrapper.classes()).toContain('collapsed')
    expect(wrapper.get('button[aria-label="Развернуть меню"]').attributes('aria-expanded')).toBe('false')
    const link = wrapper.get('a[href="/extensions"]')
    expect(link.attributes('aria-label')).toBe('Расширения')
    expect(link.attributes('title')).toBe('Расширения')
    expect(link.find('.item-label').exists()).toBe(false)
    expect(localStorage.getItem(PREF_SIDEBAR_COLLAPSED)).toBe('true')
    wrapper.unmount()
    const reopened = await mountSidebar({ state: 'off' })
    expect(reopened.classes()).toContain('collapsed')
    await reopened.get('button[aria-label="Развернуть меню"]').trigger('click')
    expect(reopened.classes()).not.toContain('collapsed')
    expect(reopened.get('a[href="/extensions"]').text()).toContain('Расширения')
    expect(localStorage.getItem(PREF_SIDEBAR_COLLAPSED)).toBe('false')
  })

  it('испорченная настройка не скрывает меню, папка открывает фильтр профилей', async () => {
    localStorage.setItem(PREF_SIDEBAR_COLLAPSED, '"true"')
    useGroupsStore().groups = [group({ id: 7, name: 'Рабочие', quick_access: true })]
    const wrapper = await mountSidebar({ state: 'off' }, '/groups')
    expect(wrapper.classes()).toContain('collapsed')
    await wrapper.get('button[aria-label="Развернуть меню"]').trigger('click')
    await wrapper.get('button[aria-label="Рабочие"]').trigger('click')
    await flushPromises()
    expect(useProfilesStore().filters.group).toBe(7)
    expect(wrapper.find('.folders').exists()).toBe(false)
  })

})
