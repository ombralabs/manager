import { enableAutoUnmount, flushPromises } from '@vue/test-utils'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { createMemoryHistory, createRouter } from 'vue-router'
import type { Announcement } from '../api/types'
import { mountWithApp } from '../test/mount'
const api = vi.hoisted(() => ({ listAnnouncements: vi.fn(), openAnnouncement: vi.fn(async () => null) }))
vi.mock('../api/core', () => ({ core: api }))
import { useSecurityStore } from '../stores/security'
import AppBanners from './AppBanners.vue'
enableAutoUnmount(afterEach)
const campaign = (patch: Partial<Announcement> = {}): Announcement => ({ id: 'product', revision: '1', title: { ru: 'Внутренний продукт', en: 'Internal product' }, body: { ru: 'Описание продукта', en: 'Product description' }, action_label: { ru: 'Подробнее', en: 'Learn more' }, url: 'https://example.test/product', tone: 'promotion', dismissible: true, ...patch })
beforeEach(() => { vi.clearAllMocks(); localStorage.clear(); api.listAnnouncements.mockResolvedValue([campaign()]); useSecurityStore().status = { configured: true, unlocked: true, migrating: false, recovery_remaining: 8, retry_after: 0 } })
function mountBanners(securityNotice = false) {
  const router = createRouter({ history: createMemoryHistory(), routes: [{ path: '/', component: { template: '<div />' } }, { path: '/settings', component: { template: '<div />' } }] })
  return { wrapper: mountWithApp(AppBanners, { props: { securityNotice }, global: { plugins: [router] } }), router }
}
describe('баннеры профилей', () => {
  it('скрытие сохраняется для ревизии, новая ревизия появляется снова', async () => {
    const first = mountBanners().wrapper; await flushPromises()
    await first.get('button[aria-label="Закрыть"]').trigger('click')
    expect(first.find('[data-campaign]').exists()).toBe(false)
    first.unmount()
    const second = mountBanners().wrapper; await flushPromises()
    expect(second.find('[data-campaign]').exists()).toBe(false)
    api.listAnnouncements.mockResolvedValue([campaign({ revision: '2' })])
    const third = mountBanners().wrapper; await flushPromises()
    expect(third.find('[data-campaign]').exists()).toBe(true)
  })
  it('ссылка открывается через проверяемый идентификатор кампании; HTML отображается текстом', async () => {
    api.listAnnouncements.mockResolvedValue([campaign({ title: { ru: '<script>bad()</script>', en: 'Product' }, dismissible: false })])
    const { wrapper } = mountBanners(); await flushPromises()
    expect(wrapper.find('script').exists()).toBe(false)
    expect(wrapper.text()).toContain('<script>bad()</script>')
    expect(wrapper.find('button[aria-label="Закрыть"]').exists()).toBe(false)
    await wrapper.get('button').trigger('click')
    expect(api.openAnnouncement).toHaveBeenCalledExactlyOnceWith('product')
  })
  it('постоянное напоминание 2FA ведёт в настройки и не имеет скрытия', async () => {
    api.listAnnouncements.mockResolvedValue([])
    useSecurityStore().status!.configured = false
    const { wrapper, router } = mountBanners(true); await flushPromises()
    expect(wrapper.find('[data-test="security-banner"]').exists()).toBe(true)
    expect(wrapper.find('button[aria-label="Закрыть"]').exists()).toBe(false)
    await wrapper.get('button').trigger('click'); await flushPromises()
    expect(router.currentRoute.value.path).toBe('/settings')
    expect(router.currentRoute.value.hash).toBe('#security')
  })
})
