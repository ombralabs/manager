import { createPinia, setActivePinia } from 'pinia'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { CoreError } from '../api/errors'
import type { ApiConfig } from '../api/types'

const mocks = vi.hoisted(() => ({
  apiConfig: vi.fn(),
  profileSettings: vi.fn(),
  setApiConfig: vi.fn(),
  regenerateApiKey: vi.fn(),
  setOnQuit: vi.fn(),
  getServicesProxy: vi.fn(),
  googleKeyEmail: vi.fn(async () => null),
  setServicesProxy: vi.fn(),
  setGoogleKey: vi.fn(),
}))
vi.mock('../api/core', () => ({ core: mocks }))

import { useSettingsStore } from './settings'

const config = (patch: Partial<ApiConfig> = {}): ApiConfig => ({
  enabled: true,
  port: 50325,
  require_key: false,
  key: null,
  status: { state: 'listening', port: 50325 },
  ...patch,
})

beforeEach(() => {
  setActivePinia(createPinia())
  for (const mock of Object.values(mocks)) mock.mockReset()
  mocks.apiConfig.mockResolvedValue(config())
  mocks.profileSettings.mockResolvedValue({ on_quit: 'ask' })
  mocks.getServicesProxy.mockResolvedValue(null)
})

describe('хранилище настроек', () => {
  it('загружает Local API, профили', async () => {
    const store = useSettingsStore()
    expect(store.apiStatus).toEqual({ state: 'off' })
    await store.load()
    expect(store.loaded).toBe(true)
    expect(store.apiStatus).toEqual({ state: 'listening', port: 50325 })
    expect(store.profile).toEqual({ on_quit: 'ask' })
  })

  it('повреждённая настройка прокси для сервисов не мешает остальным', async () => {
    mocks.getServicesProxy.mockRejectedValue(new CoreError('Storage', 'повреждена'))
    const store = useSettingsStore()
    await store.load()
    expect(store.loaded).toBe(true)
    expect(store.profile).toEqual({ on_quit: 'ask' })
    expect(store.servicesProxyError).toBe('повреждена')
    mocks.setServicesProxy.mockResolvedValue(null)
    await store.setServicesProxy(null)
    expect(store.servicesProxyError).toBeNull()
  })

  it('ключ Google не расшифровался — не «нет ключа», а испорченный ключ; новый ключ это снимает', async () => {
    mocks.googleKeyEmail.mockRejectedValue(new CoreError('Storage', 'secret decryption failed'))
    const store = useSettingsStore()
    await store.load()
    expect(store.loaded).toBe(true)
    expect([store.googleEmail, store.googleKeyBroken]).toEqual([null, true])
    mocks.setGoogleKey.mockResolvedValue('bot@p.iam')
    await store.setGoogleKey('/tmp/key.json')
    expect([store.googleEmail, store.googleKeyBroken]).toEqual(['bot@p.iam', false])
  })

  it('applyApi отправляет полную форму с правкой', async () => {
    const store = useSettingsStore()
    await store.load()
    mocks.setApiConfig.mockResolvedValue(config({ port: 50400, status: { state: 'listening', port: 50400 } }))
    await store.applyApi({ port: 50400 })
    expect(mocks.setApiConfig).toHaveBeenCalledWith({ enabled: true, port: 50400, require_key: false })
    expect(store.api!.port).toBe(50400)
    expect(store.apiStatus).toEqual({ state: 'listening', port: 50400 })
  })

  it('занятый порт: ошибка вызывающему, настройки прежние', async () => {
    const store = useSettingsStore()
    await store.load()
    mocks.setApiConfig.mockRejectedValue(new CoreError('Validation', 'порт занят', [{ field: 'port', code: 'busy' }]))
    await expect(store.applyApi({ port: 50400 })).rejects.toMatchObject({ code: 'Validation' })
    expect(store.api!.port).toBe(50325)
  })

  it('до загрузки applyApi не зовёт ядро', async () => {
    await expect(useSettingsStore().applyApi({ enabled: false })).rejects.toMatchObject({ code: 'Internal' })
    expect(mocks.setApiConfig).not.toHaveBeenCalled()
  })

  it('«При закрытии приложения» откатывается при ошибке', async () => {
    const store = useSettingsStore()
    await store.load()
    mocks.setOnQuit.mockRejectedValue(new CoreError('Storage', 'диск'))
    const done = store.setOnQuit('stop_all')
    expect(store.profile!.on_quit).toBe('stop_all')
    await expect(done).rejects.toMatchObject({ code: 'Storage' })
    expect(store.profile!.on_quit).toBe('ask')
  })

  it('событие api-status меняет состояние сервера', async () => {
    const store = useSettingsStore()
    await store.load()
    store.setApiStatus({ state: 'failed', port: 50325, code: 'port_busy', reason: 'порт 50325 занят' })
    expect(store.apiStatus).toEqual({ state: 'failed', port: 50325, code: 'port_busy', reason: 'порт 50325 занят' })
    expect(store.api!.status).toEqual(store.apiStatus)
  })

  it('два applyApi подряд: второй строится из результата первого', async () => {
    const store = useSettingsStore()
    await store.load()
    let answer: (value: ApiConfig) => void = () => {}
    mocks.setApiConfig.mockImplementationOnce(() => new Promise<ApiConfig>((resolve) => (answer = resolve)))
    mocks.setApiConfig.mockImplementationOnce(async (form) => config({ ...form }))
    const first = store.applyApi({ port: 50400 })
    const second = store.applyApi({ require_key: true })
    await Promise.resolve()
    expect(mocks.setApiConfig).toHaveBeenCalledTimes(1)
    answer(config({ port: 50400 }))
    await Promise.all([first, second])
    expect(mocks.setApiConfig).toHaveBeenLastCalledWith({ enabled: true, port: 50400, require_key: true })
    expect([store.api!.port, store.api!.require_key]).toEqual([50400, true])
  })

  it('ошибка первого applyApi не мешает второму', async () => {
    const store = useSettingsStore()
    await store.load()
    mocks.setApiConfig.mockRejectedValueOnce(new CoreError('Validation', 'порт занят', [{ field: 'port', code: 'busy' }]))
    mocks.setApiConfig.mockImplementationOnce(async (form) => config({ ...form }))
    const first = store.applyApi({ port: 50400 })
    const firstResult = expect(first).rejects.toMatchObject({ code: 'Validation' })
    const second = store.applyApi({ require_key: true })
    await firstResult
    await second
    expect(mocks.setApiConfig).toHaveBeenLastCalledWith({ enabled: true, port: 50325, require_key: true })
    expect(store.api!.require_key).toBe(true)
  })

  it('load, начатый раньше applyApi, не перетирает его результат', async () => {
    const store = useSettingsStore()
    let answer: (value: ApiConfig) => void = () => {}
    mocks.apiConfig.mockImplementationOnce(() => new Promise<ApiConfig>((resolve) => (answer = resolve)))
    mocks.setApiConfig.mockImplementationOnce(async (form) => config({ ...form }))
    const loading = store.load()
    const applying = store.applyApi({ port: 50400 })
    await vi.waitFor(() => expect(mocks.apiConfig).toHaveBeenCalled())
    answer(config())
    await Promise.all([loading, applying])
    expect(mocks.setApiConfig).toHaveBeenCalledWith({ enabled: true, port: 50400, require_key: false })
    expect(store.api!.port).toBe(50400)
  })

  it('regenerateKey берёт ключ и состояние сервера из ответа ядра', async () => {
    const store = useSettingsStore()
    await store.load()
    store.setApiStatus({ state: 'off' })
    const next = config({ require_key: true, key: 'N'.repeat(32) })
    mocks.regenerateApiKey.mockResolvedValue(next)
    await expect(store.regenerateKey()).resolves.toEqual(next)
    expect(store.api).toEqual(next)
    expect(store.apiStatus).toEqual({ state: 'listening', port: 50325 })
  })

  it('событие api-status до загрузки меняет только состояние сервера', async () => {
    const store = useSettingsStore()
    store.setApiStatus({ state: 'listening', port: 50325 })
    expect(store.apiStatus).toEqual({ state: 'listening', port: 50325 })
    expect(store.api).toBeNull()
    await store.load()
    expect(store.api!.status).toEqual(store.apiStatus)
  })
})
