import { describe, expect, it } from 'vitest'
import type { ProxyRow } from '../api/types'
import { proxyRow } from '../test/fixtures'
import { proxyOptions } from './proxyOptions'

const proxy = proxyRow({ id: 7, kind: 'socks5', port: 1080, username: 'secret-login', has_password: true, note: 'DE', profiles: 1 })

describe('прокси в выпадающем списке', () => {
  it('тип, адрес и заметка; выключенный помечен; логина нет', () => {
    const options = proxyOptions([proxy, { ...proxy, id: 8, note: '', enabled: false }], 'Выберите прокси', 'выключен')
    expect(options).toEqual([
      { value: '', label: 'Выберите прокси' },
      { value: '7', label: 'SOCKS5 · gw1.example.net:1080 · DE' },
      { value: '8', label: 'SOCKS5 · gw1.example.net:1080 · выключен' },
    ])
    expect(JSON.stringify(options)).not.toContain('secret-login')
  })

  it('страна последней проверки — перед заметкой', () => {
    const checked: ProxyRow = { ...proxy, note: 'основной', check: { at: 1, ok: false, exit_ip: '203.0.113.24', country: 'DE' } }
    expect(proxyOptions([checked], '—', 'выключен')[1]!.label).toBe('SOCKS5 · gw1.example.net:1080 · DE · основной')
  })

  it('занятый другим запущенным профилем помечен, свой — нет', () => {
    const busy: ProxyRow = { ...proxy, note: '', used_by: 'p2', used_by_name: 'Почта 02' }
    const label = (name: string) => `занят: ${name}`
    expect(proxyOptions([busy], '—', 'выключен', label)[1]!.label).toBe('SOCKS5 · gw1.example.net:1080 · занят: Почта 02')
    expect(proxyOptions([busy], '—', 'выключен', label, 'p2')[1]!.label).toBe('SOCKS5 · gw1.example.net:1080')
    expect(proxyOptions([busy], '—', 'выключен')[1]!.label).toBe('SOCKS5 · gw1.example.net:1080')
  })
})
