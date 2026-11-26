import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { GatewayView, ProfileRow } from '../api/types'
import {
  DEFAULT_HIDDEN,
  formatDateTime,
  gatewayCell,
  gridTemplate,
  isActive,
  isLive,
  loadHidden,
  nameSubline,
  saveHidden,
} from './columns'
import { profileRow } from '../test/fixtures'
import { PREF_COLUMNS } from './prefs'

const running = {
  pid: 1,
  cdp_ws: 'ws://127.0.0.1:2/devtools/browser/x',
  debug_port: 2,
  relay_port: 3,
  proxy_id: 7,
  exit_ip: null,
  country: null,
  timezone: 'UTC',
  started_at: 4,
}

const row = (patch: Partial<ProfileRow> = {}): ProfileRow =>
  profileRow({
    id: 'k7f2q9',
    name: 'Магазин — основной',
    note: 'кабинет',
    os_version: '15.2',
    seed: 48213,
    created_at: 0,
    ...patch,
  })

const gateway: GatewayView = {
  kind: 'socks5',
  host: 'gw1.example.net',
  port: 1080,
  ip: '203.0.113.24',
  country: 'DE',
  ok: true,
}

describe('колонки списка профилей', () => {
  beforeEach(() => localStorage.clear())

  afterEach(() => vi.restoreAllMocks())

  it('сетка списка с выбранными колонками', () => {
    expect(gridTemplate(DEFAULT_HIDDEN)).toBe('28px minmax(160px, 1.5fr) 132px minmax(130px, 1fr) minmax(90px, .7fr) 160px')
    expect(gridTemplate([])).toBe('28px 44px minmax(160px, 1.5fr) 132px minmax(130px, 1fr) minmax(90px, .7fr) 184px 136px 136px 160px')
    expect(gridTemplate(['serial', 'tags', 'gateway', 'fingerprint', 'created', 'last_opened'])).toBe(
      '28px minmax(160px, 1.5fr) 132px 160px',
    )
  })

  it('скрытые колонки переживают перезапуск, мусор даёт значение по умолчанию', () => {
    expect(loadHidden()).toEqual(DEFAULT_HIDDEN)
    saveHidden(['gateway'])
    expect(loadHidden()).toEqual(['gateway'])
    localStorage.setItem(PREF_COLUMNS, '{не json')
    expect(loadHidden()).toEqual(DEFAULT_HIDDEN)
    localStorage.setItem(PREF_COLUMNS, '["group","gateway","bogus",7]')
    expect(loadHidden()).toEqual(['gateway'])
  })

  it('недоступный localStorage не ломает экран', () => {
    const denied = () => {
      throw new Error('denied')
    }
    vi.spyOn(Storage.prototype, 'getItem').mockImplementation(denied)
    vi.spyOn(Storage.prototype, 'setItem').mockImplementation(denied)
    expect(loadHidden()).toEqual(DEFAULT_HIDDEN)
    expect(() => saveHidden(['gateway'])).not.toThrow()
  })

  it('вторая строка имени — теги, без тегов — заметка', () => {
    expect(nameSubline(row({ tags: ['Shop', 'vip'] }))).toBe('Shop, vip')
    expect(nameSubline(row())).toBe('кабинет')
  })

  it('колонка «Шлюз»', () => {
    expect(gatewayCell(row({ gateway }))).toEqual({ title: 'DE · 203.0.113.24', kind: 'socks5', note: 'pinned' })
    expect(gatewayCell(row())).toEqual({ title: null, kind: null, note: 'unassigned' })
    expect(gatewayCell(row({ gateway, gateway_replaced_at: 5 })).note).toBe('replaced')
    expect(gatewayCell(row({ gateway, status: { state: 'paused', ...running } })).note).toBe('unreachable')
    expect(gatewayCell(row({ gateway: { ...gateway, ip: null, country: null } })).title).toBe('gw1.example.net:1080')
  })

  it('активный профиль нельзя удалить, «Пауза» — открытый браузер', () => {
    expect(isLive(row({ status: { state: 'paused', ...running } }))).toBe(true)
    expect(isLive(row({ status: { state: 'starting' } }))).toBe(false)
    expect(isActive(row({ status: { state: 'starting' } }))).toBe(true)
    expect(isActive(row({ status: { state: 'crashed', code: 1, signal: null, at: 0 } }))).toBe(false)
  })

  it('дата и время по локали', () => {
    expect(formatDateTime(0, 'ru', 'UTC')).toBe('01.01.1970, 00:00')
  })

  it('форматтер дат кэшируется, но язык и часовой пояс различаются', () => {
    const spy = vi.spyOn(Intl, 'DateTimeFormat')
    try {
      const first = formatDateTime(0, 'ru', 'Asia/Tokyo')
      const created = spy.mock.calls.length
      expect(formatDateTime(0, 'ru', 'Asia/Tokyo')).toBe(first)
      expect(formatDateTime(3600, 'ru', 'Asia/Tokyo')).toBe('01.01.1970, 10:00')
      expect(spy.mock.calls.length).toBe(created)
    } finally {
      spy.mockRestore()
    }
    expect(formatDateTime(0, 'ru', 'Asia/Tokyo')).toBe('01.01.1970, 09:00')
    expect(formatDateTime(0, 'ru', 'UTC')).toBe('01.01.1970, 00:00')
    expect(formatDateTime(0, 'en', 'UTC')).not.toBe(formatDateTime(0, 'ru', 'UTC'))
  })
})
