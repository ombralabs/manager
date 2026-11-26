import { flushPromises, type VueWrapper } from '@vue/test-utils'
import { createMemoryHistory, createRouter } from 'vue-router'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { mountWithApp } from '../test/mount'
import { PREF_SETTINGS_SECTION } from '../lib/prefs'

const security = vi.hoisted(() => ({ available: true }))
const load = vi.hoisted(() => vi.fn(async () => {}))
vi.mock('./settings/ApiCard.vue', () => ({ default: { template: '<section>card-api<button id="api-enabled">API</button><input id="api-port" /></section>' } }))
vi.mock('./settings/GeneralCard.vue', () => ({ default: { template: '<section>card-general<button id="ui-language">Language</button></section>' } }))
vi.mock('./settings/DataCard.vue', () => ({ default: { template: '<section>card-data</section>' } }))
vi.mock('./settings/EngineCard.vue', () => ({ default: { template: '<section>card-engine</section>' } }))
vi.mock('./settings/ProfilesCard.vue', () => ({ default: { template: '<section>card-profiles<input id="profile-on-quit" /></section>' } }))
vi.mock('./settings/AutomationCard.vue', () => ({ default: { template: '<section>card-services<button id="services-proxy">Proxy</button></section>' } }))
vi.mock('./settings/SecurityCard.vue', () => ({ default: { template: '<section>card-security</section>' } }))
vi.mock('../stores/settings', () => ({ useSettingsStore: () => ({ load, loaded: true }) }))
vi.mock('../stores/security', () => ({ useSecurityStore: () => security }))

import SettingsView from './SettingsView.vue'
const wrappers: VueWrapper[] = []
async function open(hash = '') {
  const router = createRouter({ history: createMemoryHistory(), routes: [
    { path: '/settings', component: SettingsView },
    ...['/account', '/sync/settings', '/profiles'].map((path) => ({ path, component: { template: '<div>other-page</div>' } })),
  ] })
  await router.push('/settings' + hash)
  const wrapper = mountWithApp({ template: '<RouterView />' }, { global: { plugins: [router] }, attachTo: document.body })
  wrappers.push(wrapper)
  await flushPromises()
  return { router, wrapper }
}
beforeEach(() => { localStorage.clear(); security.available = true; load.mockClear() })
afterEach(() => { wrappers.splice(0).forEach((wrapper) => wrapper.unmount()); document.body.innerHTML = '' })

describe('навигация настроек', () => {
  it('первое открытие — «Общие» и пять разделов', async () => {
    const { wrapper } = await open()
    expect(wrapper.get('nav').attributes('aria-label')).toBe('Разделы настроек')
    expect(wrapper.findAll('nav a').map((a) => a.text())).toEqual(['Общие', 'Запуск профилей', 'Браузер', 'Безопасность и данные', 'Интеграции'])
    expect(wrapper.get('nav a[aria-current="page"]').text()).toBe('Общие')
    expect(wrapper.text()).toContain('card-general')
  })

  it('переход и Back меняют раздел без повторной загрузки и сбрасывают прокрутку', async () => {
    const { router, wrapper } = await open('#profiles')
    wrapper.get('main').element.scrollTop = 500
    await router.push('/settings#security')
    await flushPromises()
    expect(wrapper.get('main').element.scrollTop).toBe(0)
    expect(wrapper.text()).toContain('card-security')
    expect(wrapper.text()).toContain('card-data')
    router.back()
    await flushPromises()
    expect(wrapper.text()).toContain('card-profiles')
    expect(load).toHaveBeenCalledTimes(1)
  })

  it('возвращение через глобальные настройки восстанавливает последний раздел', async () => {
    const { router, wrapper } = await open('#engine')
    expect(localStorage.getItem(PREF_SETTINGS_SECTION)).toBe('"engine"')
    await router.push('/profiles')
    await router.push('/settings')
    await flushPromises()
    expect(wrapper.text()).toContain('card-engine')
  })

  it.each(['#foo', '#license'])('неизвестный адрес %s открывает общие настройки', async (hash) => {
    localStorage.setItem(PREF_SETTINGS_SECTION, '"removed-section"')
    expect((await open(hash)).wrapper.text()).toContain('card-general')
  })

  it.each([['#sync', '/sync/settings']])('старый адрес %s ведёт на %s', async (hash, path) => {
    const { router, wrapper } = await open(hash)
    expect(router.currentRoute.value.path).toBe(path)
    expect(wrapper.text()).toBe('other-page')
  })

  it.each([['#api', 'api-enabled'], ['#automation', 'services-proxy']])('старый адрес %s открывает нужное поле интеграций', async (hash, id) => {
    const { router, wrapper } = await open(hash)
    expect(router.currentRoute.value.hash).toBe('#integrations')
    expect(wrapper.text()).toContain('card-api')
    expect(wrapper.text()).toContain('card-services')
    expect(document.activeElement?.id).toBe(id)
  })

  it('поиск по прежнему термину находит параметр и переводит фокус в поле', async () => {
    const { wrapper, router } = await open()
    await wrapper.get('#settings-search').setValue('При закрытии приложения')
    expect(wrapper.findAll('.result')).toHaveLength(1)
    expect(wrapper.get('.result').text()).toContain('Запуск профилей')
    await wrapper.get('.result').trigger('click')
    await flushPromises()
    expect(router.currentRoute.value.hash).toBe('#profiles')
    expect(document.activeElement?.id).toBe('profile-on-quit')
    expect(wrapper.get<HTMLInputElement>('#settings-search').element.value).toBe('')
  })

  it('пустой результат объясняет, что делать; Escape возвращает текущий раздел', async () => {
    const { wrapper } = await open('#profiles')
    await wrapper.get('#settings-search').setValue('no such setting')
    expect(wrapper.get('[role="status"]').text()).toContain('Ничего не найдено')
    await wrapper.get('#settings-search').trigger('keydown', { key: 'Escape' })
    expect(wrapper.text()).toContain('card-profiles')
  })

  it('сейф недоступен — только безопасность, без поиска и загрузки данных', async () => {
    security.available = false
    const { wrapper } = await open()
    expect(wrapper.find('nav').exists()).toBe(false)
    expect(wrapper.find('#settings-search').exists()).toBe(false)
    expect(wrapper.text()).toContain('card-security')
    expect(load).not.toHaveBeenCalled()
  })
})
