import { enableAutoUnmount, flushPromises } from '@vue/test-utils'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { createMemoryHistory, createRouter } from 'vue-router'
import { mountWithApp } from './test/mount'

const mocked = vi.hoisted(() => {
  const order: string[] = []
  const subscribe = (name: string) => async (_listener?: unknown) => {
    order.push(name)
    return () => {}
  }
  return {
    order,
    core: {
      // Ядро не открылось: экран ошибки, настройки Local API не грузятся — проверяется только порядок вызовов.
      appInfo: vi.fn(async (): Promise<import('./api/types').AppInfo> => ({
        version: '0.1.0',
        data_dir: '/tmp/ombra',
        language: 'ru',
        engine: null,
        init_error: { code: 'Keychain', message: 'нет доступа к связке ключей' },
      })),
      securityStatus: vi.fn(async () => ({ configured: true, unlocked: false, migrating: false, recovery_remaining: 8, retry_after: 0 })),
      apiConfig: vi.fn(async () => ({ enabled: false, port: 50325, require_key: false, key: null, status: { state: 'off' } })),
      profileSettings: vi.fn(async () => ({ on_quit: 'ask' })),
      getServicesProxy: vi.fn(async () => null),
      googleKeyEmail: vi.fn(async () => null),
      listAnnouncements: vi.fn(async () => []),
      syncState: vi.fn(async () => ({ active: false, mode: 'full_copy', main: null, windows: [] })),
      getSyncSettings: vi.fn(async () => ({})),
      getProfile: vi.fn(),
      engineCatalog: vi.fn(async () => ({ versions: [], automatic: true })),
      updateEngine: vi.fn(async () => ({ state: 'current', catalog: { versions: [], automatic: true } })),
      uiReady: async () => {
        order.push('ui_ready')
        return null
      },
    },
    events: {
      onSupervisor: subscribe('supervisor'),
      onProfilesChanged: subscribe('profiles-changed'),
      onQuitRequested: subscribe('quit-requested'),
      onApiStatus: subscribe('api-status'),
      onTasksChanged: subscribe('tasks-changed'),
      onSync: subscribe('sync'),
      onEngineProgress: subscribe('engine-progress'),
    },
  }
})
vi.mock('./api/core', () => ({ core: mocked.core, events: mocked.events }))

import App from './App.vue'
enableAutoUnmount(afterEach)
beforeEach(() => { mocked.order.length = 0; vi.clearAllMocks() })

describe('App', () => {
  it('preserves the initial settings route while subscriptions are pending', async () => {
    const info: import('./api/types').AppInfo = { version: '0.1.0', data_dir: '/tmp/ombra', language: 'ru', engine: null, init_error: null }
    mocked.core.appInfo.mockResolvedValueOnce(info).mockResolvedValueOnce(info)
    mocked.core.securityStatus.mockResolvedValueOnce({ configured: false, unlocked: false, migrating: false, recovery_remaining: 0, retry_after: 0 })
    let resumeSubscription!: (unlisten: () => void) => void
    const subscription = vi.spyOn(mocked.events, 'onSupervisor').mockImplementationOnce(() => new Promise(resolve => { resumeSubscription = resolve }))
    const router = createRouter({ history: createMemoryHistory(), routes: [
      { path: '/profiles', component: { template: '<div>profiles</div>' } },
      { path: '/settings', component: { template: '<div data-test="initial-settings">settings</div>' } },
    ] })
    await router.push('/settings')
    const wrapper = mountWithApp(App, { global: { plugins: [router] } })
    try {
      await flushPromises()
      expect(wrapper.find('[data-test="initial-settings"]').exists()).toBe(true)
      resumeSubscription(() => {})
      await flushPromises()
      expect(router.currentRoute.value.path).toBe('/settings')
      expect(wrapper.find('[data-test="initial-settings"]').exists()).toBe(true)
    } finally { subscription.mockRestore() }
  })

  it('до включения 2FA открывает рабочие экраны и все настройки', async () => {
    const info: import('./api/types').AppInfo = { version: '0.1.0', data_dir: '/tmp/ombra', language: 'ru', engine: null, init_error: null }
    mocked.core.appInfo.mockResolvedValueOnce(info).mockResolvedValueOnce(info)
    mocked.core.securityStatus.mockResolvedValueOnce({ configured: false, unlocked: false, migrating: false, recovery_remaining: 0, retry_after: 0 })
    const router = createRouter({ history: createMemoryHistory(), routes: [
      { path: '/profiles', component: { template: '<div data-test="profiles">profiles</div>' } },
      { path: '/settings', component: { template: '<div data-test="settings">settings</div>' } },
    ] })
    await router.push('/profiles')
    const wrapper = mountWithApp(App, { global: { plugins: [router] } })
    await flushPromises()
    expect(wrapper.find('.security-gate').exists()).toBe(false)
    expect(wrapper.find('[data-test="profiles"]').exists()).toBe(true)
    expect(mocked.core.apiConfig).toHaveBeenCalled()
    expect(mocked.core.syncState).toHaveBeenCalled()
    expect(mocked.core.updateEngine).toHaveBeenCalledWith(true)
    await router.push('/settings'); await flushPromises()
    expect(wrapper.find('[data-test="settings"]').exists()).toBe(true)
  })

  it('не монтирует приватный экран до проверки 2FA и оставляет доступными настройки', async () => {
    mocked.core.appInfo.mockResolvedValueOnce({ version: '0.1.0', data_dir: '/tmp/ombra', language: 'ru', engine: null, init_error: null })
    const privateSetup = vi.fn()
    const router = createRouter({ history: createMemoryHistory(), routes: [
      { path: '/profiles', component: { setup: privateSetup, template: '<div data-test="private">private data</div>' } },
      { path: '/settings', component: { template: '<div data-test="settings">user settings</div>' } },
    ] })
    await router.push('/profiles')
    const wrapper = mountWithApp(App, { global: { plugins: [router] } })
    await flushPromises()
    expect(wrapper.find('.security-gate').exists()).toBe(true)
    expect(privateSetup).not.toHaveBeenCalled()
    expect(wrapper.find('[data-test="private"]').exists()).toBe(false)
    // До разблокировки команды ядра отказывают: синхронизатор не читается.
    expect(mocked.core.syncState).not.toHaveBeenCalled()
    expect(mocked.core.engineCatalog).not.toHaveBeenCalled()
    expect(mocked.core.updateEngine).not.toHaveBeenCalled()
    await router.push('/settings'); await flushPromises()
    expect(wrapper.find('[data-test="settings"]').exists()).toBe(true)
    expect(privateSetup).not.toHaveBeenCalled()
  })

  it('сообщает ядру о готовности после всех подписок: отложенный вопрос о выходе не теряется', async () => {
    const router = createRouter({ history: createMemoryHistory(), routes: [{ path: '/', component: { template: '<div />' } }] })
    mountWithApp(App, { global: { plugins: [router] } })
    await flushPromises()
    expect(mocked.order).toEqual([
      'supervisor',
      'profiles-changed',
      'quit-requested',
      'api-status',
      'tasks-changed',
      'sync',
      'engine-progress',
      'ui_ready',
    ])
  })
})
