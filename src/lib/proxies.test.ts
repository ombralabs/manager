import { describe, expect, it } from 'vitest'
import { proxyRow } from '../test/fixtures'
import {
  checkState,
  countryIp,
  draftFromRow,
  emptyProxyDraft,
  importLineCount,
  keepErrorLines,
  lineErrorText,
  pinnedLabel,
  proxyAddress,
  proxyDraftErrors,
  toProxyEditForm,
} from './proxies'


const row = proxyRow({ kind: 'socks5', port: 1080, note: 'DE' })

describe('подписи экрана «Прокси»', () => {
  it('адрес, страна и внешний IP', () => {
    expect(proxyAddress(row)).toBe('gw1.example.net:1080')
    expect(proxyAddress({ host: '2001:db8::1', port: 1080 })).toBe('[2001:db8::1]:1080')
    expect(countryIp(row)).toBeNull()
    expect(countryIp({ check: { at: 1, ok: false, exit_ip: '203.0.113.24', country: 'DE' } })).toBe('DE · 203.0.113.24')
  })

  it('итог проверки', () => {
    expect(checkState({ check: null })).toBe('unchecked')
    expect(checkState({ check: { at: 1, ok: true, exit_ip: '203.0.113.24', country: 'DE' } })).toBe('ok')
    expect(checkState({ check: { at: 1, ok: false, exit_ip: null, country: null } })).toBe('failed')
  })

  it('«Закреплён за» для 0, 1 и 3 профилей', () => {
    expect(pinnedLabel({ profiles: 0, pinned_to: null })).toBe('Свободен')
    expect(pinnedLabel({ profiles: 1, pinned_to: 'Почта 02' })).toBe('Почта 02')
    expect(pinnedLabel({ profiles: 3, pinned_to: null })).toBe('3 профиля')
    expect(pinnedLabel({ profiles: 5, pinned_to: null })).toBe('5 профилей')
  })
})

describe('импорт списком', () => {
  it('считает строки, как ядро: без пустых и комментариев', () => {
    const text = 'socks5://u1:pw@gw9.example.net:1080\n\n   \n# запасные\ngw10.example.net:8080:u2:pw\r\nhttps://u3:pw@gw11.example.net:443\n'
    expect(importLineCount(text)).toBe(3)
    expect(importLineCount('')).toBe(0)
  })

  it('текст ошибки строки', () => {
    expect(lineErrorText({ line: 3, code: 'port' })).toBe('Строка 3: неверный порт')
    expect(lineErrorText({ line: 1, code: 'kind' })).toBe('Строка 1: неизвестный тип прокси')
    expect(lineErrorText({ line: 2, code: 'weird' })).toBe('Строка 2: неверный формат строки')
  })

  it('в поле остаются строки с ошибками, номера — по новому тексту', () => {
    const text = 'gw1.example.net:1080\r\ngw2.example.net:abc\ngw3.example.net:8080\nftp://gw4.example.net:21'
    expect(keepErrorLines(text, [{ line: 4, code: 'kind' }, { line: 2, code: 'port' }])).toEqual({
      text: 'gw2.example.net:abc\nftp://gw4.example.net:21',
      errors: [
        { line: 1, code: 'port' },
        { line: 2, code: 'kind' },
      ],
    })
  })
})

describe('черновик диалога прокси', () => {
  it('проверки до ядра: хост, порт, логин и пароль вместе', () => {
    const draft = emptyProxyDraft()
    expect(proxyDraftErrors(draft)).toEqual([
      { field: 'proxy_host', code: 'required' },
      { field: 'proxy_port', code: 'out_of_range' },
    ])
    draft.host = 'gw.example.net'
    draft.port = '70000'
    expect(proxyDraftErrors(draft)).toEqual([{ field: 'proxy_port', code: 'out_of_range' }])
    draft.port = '8080'
    draft.password.value = 'pw'
    expect(proxyDraftErrors(draft)).toEqual([{ field: 'proxy_username', code: 'required' }])
    draft.password.value = ''
    draft.username = 'user'
    expect(proxyDraftErrors(draft)).toEqual([{ field: 'proxy_password', code: 'required' }])
  })

  it('сохранённая запись: пустой пароль оставляет прежний', () => {
    const draft = draftFromRow({ ...row, username: 'user', has_password: true })
    expect(proxyDraftErrors(draft)).toEqual([])
    expect(toProxyEditForm(draft)).toEqual({
      kind: 'socks5',
      host: 'gw1.example.net',
      port: 1080,
      username: 'user',
      password: { action: 'keep' },
      note: 'DE',
      enabled: true,
    })
  })

  it('без логина пароль удаляется; поля обрезаются', () => {
    const draft = draftFromRow({ ...row, username: 'user', has_password: true })
    draft.username = '  '
    draft.host = ' gw2.example.net '
    draft.note = ' запасной '
    expect(toProxyEditForm(draft)).toMatchObject({
      host: 'gw2.example.net',
      username: '',
      password: { action: 'clear' },
      note: 'запасной',
    })
  })
})
