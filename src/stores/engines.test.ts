import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { EngineCatalog } from '../api/types'
import { CoreError } from '../api/errors'

const api = vi.hoisted(() => ({ engineCatalog: vi.fn(), updateEngine: vi.fn(), setEngineAutomatic: vi.fn() }))
vi.mock('../api/core', () => ({ core: api, events: {} }))
import { useEnginesStore } from './engines'
import { useAppStore } from './app'

const catalog = (automatic = true): EngineCatalog => ({ automatic, versions: [
  { id: 'legacy', version: '150.0.7871.114', chromium: '150.0.7871.114', binary: '/tmp/chrome', active: true },
] })

beforeEach(() => {
  vi.resetAllMocks()
  useAppStore().info = { version: '1.1.23', data_dir: '/tmp/ombra', language: 'ru', engine: null, init_error: null }
  api.engineCatalog.mockResolvedValue(catalog())
  api.updateEngine.mockResolvedValue({ state: 'current', catalog: catalog() })
})

describe('обновление браузера', () => {
  it('проверяет стабильную сборку один раз при запуске', async () => {
    const engines = useEnginesStore()
    await Promise.all([engines.startup(), engines.startup()])
    expect(api.updateEngine).toHaveBeenCalledExactlyOnceWith(true)
    expect(useAppStore().info?.engine?.version).toBe('150.0.7871.114')
  })

  it('не проверяет закреплённую версию до ручного запроса', async () => {
    api.engineCatalog.mockResolvedValue(catalog(false))
    const engines = useEnginesStore()
    await engines.startup()
    expect(api.updateEngine).not.toHaveBeenCalled()
    await engines.update()
    expect(api.updateEngine).toHaveBeenCalledExactlyOnceWith(false)
  })

  it('при недоступном каталоге сохраняет установленный браузер и разрешает повтор', async () => {
    const engines = useEnginesStore()
    api.updateEngine.mockRejectedValueOnce(new CoreError('EngineUpdateUnavailable', 'offline'))
    await engines.startup()
    expect(engines.errorCode).toBe('EngineUpdateUnavailable')
    expect(useAppStore().info?.engine?.binary).toBe('/tmp/chrome')
    await engines.update()
    expect(engines.errorCode).toBeNull()
    expect(engines.result).toBe('current')
  })

  it('не запускает две загрузки одновременно', async () => {
    let finish!: (value: unknown) => void
    api.updateEngine.mockImplementation(() => new Promise((resolve) => { finish = resolve }))
    const engines = useEnginesStore()
    const first = engines.update()
    await engines.update()
    expect(api.updateEngine).toHaveBeenCalledOnce()
    finish({ state: 'current', catalog: catalog() })
    await first
    expect(engines.busy).toBe(false)
  })

  it('включение автообновления сразу проверяет стабильную сборку', async () => {
    const engines = useEnginesStore()
    await engines.automatic(true)
    expect(api.setEngineAutomatic).toHaveBeenCalledExactlyOnceWith(true)
    expect(api.updateEngine).toHaveBeenCalledExactlyOnceWith(true)
  })
})
