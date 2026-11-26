import { createPinia, setActivePinia } from 'pinia'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { CoreError } from '../api/errors'
import type { GeoInfo, ProxyRow } from '../api/types'

const mocks = vi.hoisted(() => ({
  listProxies: vi.fn(),
  checkProxy: vi.fn(),
  setProxyEnabled: vi.fn(),
  importProxies: vi.fn(),
  createProxy: vi.fn(),
}))
vi.mock('../api/core', () => ({ core: mocks }))

import { proxyRow } from '../test/fixtures'
import { useProxiesStore } from './proxies'

const proxy = (id: number, profiles = 0): ProxyRow =>
  proxyRow({ id, host: `gw${id}.example.net`, profiles })

const geo: GeoInfo = { ip: '203.0.113.24', country: 'DE', timezone: 'Europe/Berlin' }

beforeEach(() => {
  setActivePinia(createPinia())
  for (const mock of Object.values(mocks)) mock.mockReset()
  mocks.listProxies.mockResolvedValue([proxy(1, 2), proxy(2), proxy(3, 1)])
})

afterEach(() => {
  vi.useRealTimers()
})

describe('хранилище прокси', () => {
  it('считает записи и закреплённые', async () => {
    const store = useProxiesStore()
    await store.refresh()
    expect([store.total, store.pinnedCount, store.loaded]).toEqual([3, 2, true])
  })

  it('один запрос списка в полёте; повтор уходит после ответа, применяется последним', async () => {
    const pending: ((rows: ProxyRow[]) => void)[] = []
    mocks.listProxies.mockImplementation(() => new Promise((resolve) => pending.push(resolve)))
    const store = useProxiesStore()
    const first = store.refresh()
    const second = store.refresh()
    expect(pending).toHaveLength(1)
    pending[0]!([proxy(1), proxy(2)])
    await first
    expect(pending).toHaveLength(2)
    expect(store.loading).toBe(true)
    pending[1]!([proxy(2)])
    await second
    expect(store.rows.map((row) => row.id)).toEqual([2])
    expect(store.loading).toBe(false)
  })

  it('обновление по событиям — один запрос на пачку', () => {
    vi.useFakeTimers()
    const store = useProxiesStore()
    for (let i = 0; i < 5; i++) store.scheduleRefresh()
    vi.advanceTimersByTime(499)
    expect(mocks.listProxies).not.toHaveBeenCalled()
    vi.advanceTimersByTime(1)
    expect(mocks.listProxies).toHaveBeenCalledTimes(1)
    store.cancelTimers()
  })

  it('выключение меняет строку сразу и откатывается при ошибке', async () => {
    const store = useProxiesStore()
    await store.refresh()
    mocks.setProxyEnabled.mockRejectedValue(new CoreError('NotFound', 'прокси не найден'))
    const done = store.setEnabled(2, false)
    expect(store.byId(2)!.enabled).toBe(false)
    await expect(done).rejects.toMatchObject({ code: 'NotFound' })
    expect(store.byId(2)!.enabled).toBe(true)
    expect(mocks.setProxyEnabled).toHaveBeenCalledWith(2, false)
  })

  it('«Проверить»: строка в checking до ответа, затем список перечитывается', async () => {
    const store = useProxiesStore()
    await store.refresh()
    let answer: (value: GeoInfo) => void = () => {}
    mocks.checkProxy.mockImplementation(() => new Promise<GeoInfo>((resolve) => (answer = resolve)))
    const done = store.check(1)
    expect(store.checking.has(1)).toBe(true)
    mocks.listProxies.mockClear()
    answer(geo)
    await expect(done).resolves.toEqual(geo)
    expect(store.checking.has(1)).toBe(false)
    expect(mocks.listProxies).toHaveBeenCalledTimes(1)
  })

  it('«Нет связи» сразу в строке, страна и IP прошлой проверки остаются', async () => {
    const store = useProxiesStore()
    await store.refresh()
    store.byId(1)!.check = { at: 1, ok: true, exit_ip: '203.0.113.24', country: 'DE' }
    mocks.listProxies.mockReturnValue(new Promise(() => {}))
    mocks.checkProxy.mockRejectedValue(new CoreError('ProxyUnavailable', 'прокси недоступен'))
    void store.check(1).catch(() => undefined)
    await vi.waitFor(() => expect(store.checking.has(1)).toBe(false))
    expect(store.byId(1)!.check).toMatchObject({ ok: false, exit_ip: '203.0.113.24', country: 'DE' })
  })

  it('«Проверить все» проверяет каждую запись и считает доступные', async () => {
    const store = useProxiesStore()
    await store.refresh()
    mocks.checkProxy.mockImplementation(async (id: number) => {
      if (id === 2) throw new CoreError('ProxyUnavailable', 'прокси недоступен')
      return geo
    })
    await expect(store.checkAll()).resolves.toEqual({ ok: 2, total: 3 })
    expect(mocks.checkProxy.mock.calls.map(([id]) => id)).toEqual([1, 2, 3])
    expect([store.checking.size, store.checkingAll]).toEqual([0, false])
  })

  it('не больше 8 проверок одновременно, ожидающие — тоже в checking', async () => {
    mocks.listProxies.mockResolvedValue(Array.from({ length: 10 }, (_, i) => proxy(i + 1)))
    const store = useProxiesStore()
    await store.refresh()
    const answers: (() => void)[] = []
    mocks.checkProxy.mockImplementation(() => new Promise<GeoInfo>((resolve) => answers.push(() => resolve(geo))))
    const done = store.checkAll()
    expect(mocks.checkProxy).toHaveBeenCalledTimes(8)
    expect([store.checking.size, store.checkingAll]).toEqual([10, true])
    for (let i = 0; i < 10; i++) {
      await vi.waitFor(() => expect(answers.length).toBeGreaterThan(i))
      answers[i]!()
    }
    await expect(done).resolves.toEqual({ ok: 10, total: 10 })
  })

  it('импорт с проверкой проверяет только добавленные записи', async () => {
    const store = useProxiesStore()
    mocks.importProxies.mockResolvedValue({ added: 2, duplicates: 1, errors: [], ids: [3, 1] })
    mocks.checkProxy.mockResolvedValue(geo)
    const report = await store.importText('a\nb\nc', 'socks5', true)
    expect(report.ids).toEqual([3, 1])
    expect(mocks.importProxies).toHaveBeenCalledWith('a\nb\nc', 'socks5')
    await vi.waitFor(() => expect(mocks.checkProxy).toHaveBeenCalledTimes(2))
    expect(mocks.checkProxy.mock.calls.map(([id]) => id)).toEqual([3, 1])
  })

  it('импорт без проверки ничего не проверяет', async () => {
    mocks.importProxies.mockResolvedValue({ added: 1, duplicates: 0, errors: [], ids: [4] })
    await useProxiesStore().importText('gw4.example.net:8080', 'http', false)
    expect(mocks.checkProxy).not.toHaveBeenCalled()
    expect(mocks.listProxies).toHaveBeenCalledTimes(1)
  })

  it('удачная проверка сразу пишет «Доступен», IP и страну в строку', async () => {
    const store = useProxiesStore()
    await store.refresh()
    mocks.listProxies.mockReturnValue(new Promise(() => {}))
    mocks.checkProxy.mockResolvedValue(geo)
    void store.check(2).catch(() => undefined)
    await vi.waitFor(() => expect(store.checking.has(2)).toBe(false))
    expect(store.byId(2)!.check).toMatchObject({ ok: true, exit_ip: '203.0.113.24', country: 'DE' })
  })

  it('GeoUnavailable строку не меняет: о самом прокси он ничего не говорит', async () => {
    const store = useProxiesStore()
    await store.refresh()
    const before = { at: 1, ok: false, exit_ip: '198.51.100.7', country: 'NL' }
    store.byId(1)!.check = { ...before }
    mocks.listProxies.mockReturnValue(new Promise(() => {}))
    mocks.checkProxy.mockRejectedValue(new CoreError('GeoUnavailable', 'геоданные недоступны'))
    void store.check(1).catch(() => undefined)
    await vi.waitFor(() => expect(store.checking.has(1)).toBe(false))
    expect(store.byId(1)!.check).toEqual(before)
  })

  it('импорт с проверкой, которая не завершается, всё равно возвращает отчёт', async () => {
    const store = useProxiesStore()
    const report = { added: 2, duplicates: 0, errors: [], ids: [1, 2] }
    mocks.importProxies.mockResolvedValue(report)
    mocks.checkProxy.mockReturnValue(new Promise(() => {}))
    await expect(store.importText('a\nb', 'http', true)).resolves.toEqual(report)
    expect([store.checking.has(1), store.checking.has(2)]).toEqual([true, true])
  })
})
