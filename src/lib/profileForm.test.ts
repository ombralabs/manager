import { describe, expect, it } from 'vitest'
import type { Fingerprint, ProfileDetails, ProfileRecord } from '../api/types'
import {
  EDITOR_TABS,
  bookmarkRowError,
  clearFieldError,
  clientErrors,
  emptyModel,
  fieldSnapshot,
  markManual,
  mergeFingerprintErrors,
  modelFromDetails,
  parseCoordinate,
  parseQuota,
  parseSeed,
  pasteKeyFromEvent,
  secretInput,
  splitLines,
  tabOfField,
  toProfileForm,
  unmarkManual,
} from './profileForm'

const fp: Fingerprint = {
  seed: 48213,
  os: 'macos',
  os_version: '15.2.0',
  brand: 'Chrome',
  gpu: { vendor: 'Google Inc. (Apple)', renderer: 'ANGLE (Apple, ANGLE Metal Renderer: Apple M2, Unspecified Version)' },
  hardware_concurrency: 8,
  device_memory: 16,
  screen: { width: 1512, height: 982 },
  timezone: { mode: 'auto' },
  language: { mode: 'auto' },
  webrtc: { mode: 'auto' },
  noise: true,
  storage_quota_mb: null,
  device_pixel_ratio: 2,
  max_touch_points: 0,
  location: { mode: 'off' },
  manual: [],
}

describe('сохранение подключения профиля', () => {
  it.each(['ip2location', 'ip-api'] as const)('сохраняет выбор %s при открытии и повторном сохранении профиля', (ip_checker) => {
    const model = modelFromDetails(details({ network: { direct: true, ip_checker } }))
    expect(model.connectionMode).toBe('direct')
    expect(model.network.ip_checker).toBe(ip_checker)
    expect(toProfileForm(model).network).toEqual({ direct: true, ip_checker })
  })

  it('прямой режим снимает прокси, ручной передаёт черновик атомарно, сохранённый не отправляет скрытые поля', () => {
    const model = emptyModel(structuredClone(fp))
    model.name = 'Network'
    model.proxyId = model.savedProxyId = 7
    model.connectionMode = 'direct'
    model.network.ip_checker = 'ipapi'
    expect(toProfileForm(model)).toMatchObject({ proxy_id: null, network: { direct: true, ip_checker: 'ipapi' } })
    model.connectionMode = 'manual'
    model.proxyDraft.host = 'manual.example'
    model.proxyDraft.port = '1080'
    model.proxyDraft.username = 'user'
    model.proxyDraft.password.value = 'private-test-password'
    model.manualProxyId = 7
    expect(clientErrors(model)).toEqual([])
    expect(toProfileForm(model)).toMatchObject({ proxy_id: null, network: { direct: false, ip_checker: 'ipapi' }, inline_proxy_source: 7,
      inline_proxy: { host: 'manual.example', port: 1080, username: 'user', password: { action: 'set', value: 'private-test-password' } } })
    model.connectionMode = 'saved'
    model.proxyId = 8
    const saved = toProfileForm(model)
    expect(saved.proxy_id).toBe(8)
    expect(saved).not.toHaveProperty('inline_proxy')
    expect(saved).not.toHaveProperty('inline_proxy_source')
  })
})

function record(patch: Partial<ProfileRecord> = {}): ProfileRecord {
  return {
    id: 'k7f2q9',
    serial: 1,
    name: 'Магазин',
    note: '',
    group_id: null,
    platform_domain: '',
    start_urls: [],
    extra_args: [],
    username: null,
    has_password: false,
    has_totp: false,
    fingerprint_json: '{}',
    proxy_id: null,
    created_at: 1,
    last_opened_at: null,
    restore_tabs: true,
    gateway_replaced_at: null,
    deleted_at: null,
    ext_category_id: 0,
    humanize: { enabled: true, preset: 'default', block_input: true, paste_key: '' },
    humanize_seed: '18446744073709551615',
    ...patch,
  }
}

function details(patch: Partial<ProfileRecord> = {}, tags: string[] = []): ProfileDetails {
  return {
    profile: record(patch),
    fingerprint: structuredClone(fp),
    proxy: null,
    tags,
    bookmarks: [],
    persona: { generator_version: 1, keys_per_sec: 5, typo_rate: 0.01, dwell_ms: 100, mouse_speed: 1, scroll_device: 'wheel' },
  }
}

describe('модель редактора', () => {
  it('режет строки и отбрасывает пустые', () => {
    expect(splitLines(' https://a.example/ \n\n  --x \n')).toEqual(['https://a.example/', '--x'])
  })

  it('переводит пароль в keep, set и clear', () => {
    expect(secretInput({ value: '', saved: true, clear: false })).toEqual({ action: 'keep' })
    expect(secretInput({ value: 'p', saved: true, clear: false })).toEqual({ action: 'set', value: 'p' })
    expect(secretInput({ value: '', saved: true, clear: true })).toEqual({ action: 'clear' })
    expect(secretInput({ value: '', saved: false, clear: true })).toEqual({ action: 'keep' })
  })

  it('до ядра проверяется только имя', () => {
    const model = emptyModel(fp)
    expect(clientErrors(model)).toEqual([{ field: 'name', code: 'required' }])
    model.name = 'Магазин'
    expect(clientErrors(model)).toEqual([])
  })

  it('вкладки редактора и вкладки ошибок полей', () => {
    expect(EDITOR_TABS).toEqual(['basic', 'fingerprint', 'gateway', 'cookies', 'launch', 'humanize', 'traffic'])
    const fields = ['name', 'group_id', 'tags', 'proxy_id', 'start_urls', 'restore_tabs', 'extra_args', 'gpu']
    expect(fields.map((field) => tabOfField(field))).toEqual([
      'basic',
      'basic',
      'basic',
      'gateway',
      'launch',
      'launch',
      'launch',
      'fingerprint',
    ])
  })

  it('помечает поле «вручную» один раз и снимает пометку', () => {
    const copy = structuredClone(fp)
    markManual(copy, 'gpu')
    markManual(copy, 'gpu')
    expect(copy.manual).toEqual(['gpu'])
    unmarkManual(copy, 'gpu')
    expect(copy.manual).toEqual([])
  })

  it('собирает форму из сохранённого профиля без встроенного прокси', () => {
    const model = modelFromDetails(
      details({
        platform_domain: 'amazon.com',
        start_urls: ['https://a.example/'],
        extra_args: ['--disable-sync'],
        username: 'shop',
        has_password: true,
        proxy_id: 3,
        restore_tabs: false,
      }),
    )
    expect([model.startUrls, model.proxyId, model.savedProxyId, model.restoreTabs]).toEqual([
      'https://a.example/',
      3,
      3,
      false,
    ])
    const form = toProfileForm(model)
    expect(form).toMatchObject({
      username: 'shop',
      password: { action: 'keep' },
      start_urls: ['https://a.example/'],
      extra_args: ['--disable-sync'],
      restore_tabs: false,
    })
    expect(form).not.toHaveProperty('proxy')
    expect(form).not.toHaveProperty('proxy_id')
  })

  it('новый профиль: «Случайный» не шлёт поле, выбранный прокси — числом', () => {
    const model = emptyModel(fp)
    expect([model.proxyId, model.savedProxyId, model.restoreTabs]).toEqual([null, null, true])
    const random = toProfileForm(model)
    expect(random).not.toHaveProperty('proxy_id')
    expect(random.restore_tabs).toBe(true)
    model.proxyId = 7
    expect(toProfileForm(model)).toHaveProperty('proxy_id', 7)
  })

  it('сохранённый профиль: «Случайный» снимает закрепление, возврат к прежнему — без поля', () => {
    const model = modelFromDetails(details({ proxy_id: 3 }))
    model.proxyId = null
    expect(toProfileForm(model)).toHaveProperty('proxy_id', null)
    model.proxyId = 5
    expect(toProfileForm(model)).toHaveProperty('proxy_id', 5)
    model.proxyId = 3
    expect(toProfileForm(model)).not.toHaveProperty('proxy_id')
  })

  it('передаёт группу и теги явно, в том числе «Без группы»', () => {
    const model = emptyModel(fp)
    expect([model.group, model.tags]).toEqual([null, []])
    expect(emptyModel(fp, 7).group).toBe(7)
    const ungrouped = toProfileForm(model)
    expect(ungrouped).toHaveProperty('group_id', null)
    expect(ungrouped.tags).toEqual([])
    model.group = 5
    model.tags = ['Shop', 'vip']
    expect(toProfileForm(model)).toMatchObject({ group_id: 5, tags: ['Shop', 'vip'] })
  })

  it('читает группу и теги сохранённого профиля', () => {
    const source = details({ id: 'r2d2xx', group_id: 4 }, ['vip', 'Shop'])
    const model = modelFromDetails(source)
    expect([model.group, model.tags]).toEqual([4, ['vip', 'Shop']])
    model.tags.push('new')
    expect(source.tags).toEqual(['vip', 'Shop'])
    expect(toProfileForm(model)).toMatchObject({ group_id: 4, tags: ['vip', 'Shop', 'new'] })
  })

  it('ошибка снимается только у правленного поля', () => {
    const errors = [
      { field: 'name', code: 'required' },
      { field: 'seed', code: 'required' },
    ]
    expect(clearFieldError(errors, 'name')).toEqual([{ field: 'seed', code: 'required' }])
    expect(clearFieldError(errors, 'gpu')).toBe(errors)
  })

  it('снимок поля меняется при правке этого поля', () => {
    const model = emptyModel(structuredClone(fp))
    const fields = ['name', 'start_urls', 'proxy_id', 'seed', 'os_version', 'password', 'unknown_field']
    const before = fields.map((field) => fieldSnapshot(model, field))
    model.name = 'Магазин'
    model.startUrls = 'https://a.example/'
    model.proxyId = 3
    model.seedText = ''
    model.fingerprint.os_version = '15.3'
    model.password = { ...model.password, value: 'p' }
    const after = fields.map((field) => fieldSnapshot(model, field))
    expect(after.map((value, index) => value !== before[index])).toEqual([true, true, true, true, true, true, false])
  })

  it('снимок координат учитывает режим: возврат к «Не задана» снимает ошибки скрытых полей', () => {
    const model = emptyModel(structuredClone(fp))
    model.fingerprint.location = { mode: 'manual', lat: 0, lon: 0 }
    const before = [fieldSnapshot(model, 'location.lat'), fieldSnapshot(model, 'location.lon')]
    model.fingerprint.location = { mode: 'off' }
    expect(fieldSnapshot(model, 'location.lat')).not.toBe(before[0])
    expect(fieldSnapshot(model, 'location.lon')).not.toBe(before[1])
  })

  it('пустой или неверный seed — ошибка до ядра, отпечаток не меняется', () => {
    expect(modelFromDetails(details()).seedText).toBe('48213')
    const model = emptyModel(structuredClone(fp))
    model.name = 'Магазин'
    expect(model.seedText).toBe('48213')
    expect(clientErrors(model)).toEqual([])
    model.seedText = ' '
    expect(clientErrors(model)).toEqual([{ field: 'seed', code: 'required' }])
    model.seedText = '4294967296'
    expect(clientErrors(model)).toEqual([{ field: 'seed', code: 'out_of_range' }])
    expect(model.fingerprint.seed).toBe(48213)
    expect([parseSeed('007'), parseSeed(' 5 '), parseSeed('4294967295'), parseSeed('1e3'), parseSeed('')]).toEqual([
      7,
      5,
      4294967295,
      null,
      null,
    ])
  })

  it('ошибки «Отпечатка»: формы — первыми, живой проверки — по остальным полям', () => {
    const live = [
      { field: 'gpu', code: 'arch_mismatch' },
      { field: 'os_version', code: 'invalid' },
    ]
    const form = [
      { field: 'name', code: 'required' },
      { field: 'os_version', code: 'unsupported' },
      { field: 'seed', code: 'required' },
    ]
    expect(mergeFingerprintErrors(live, form)).toEqual([
      { field: 'os_version', code: 'unsupported' },
      { field: 'seed', code: 'required' },
      { field: 'gpu', code: 'arch_mismatch' },
    ])
  })
})

describe('геолокация в модели редактора', () => {
  it('текст координат: точка и запятая, пределы', () => {
    expect(parseCoordinate('55.75', 90)).toBe(55.75)
    expect(parseCoordinate(' 37,62 ', 180)).toBe(37.62)
    expect(parseCoordinate('', 90)).toBeNull()
    expect(parseCoordinate('91', 90)).toBeNull()
    expect(parseCoordinate('abc', 180)).toBeNull()
  })

  it('ручная геолокация без чисел — ошибки полей; сохранённые координаты попадают в текст', () => {
    const model = emptyModel({ ...fp, location: { mode: 'manual', lat: 0, lon: 0 } })
    model.name = 'p'
    model.latText = ''
    model.lonText = '200'
    expect(clientErrors(model)).toEqual([
      { field: 'location.lat', code: 'required' },
      { field: 'location.lon', code: 'invalid' },
    ])
    const loaded = modelFromDetails({ ...details(), fingerprint: { ...fp, location: { mode: 'manual', lat: 55.75, lon: 37.62 } } })
    expect([loaded.latText, loaded.lonText]).toEqual(['55.75', '37.62'])
    expect(toProfileForm(loaded).fingerprint.location).toEqual({ mode: 'manual', lat: 55.75, lon: 37.62 })
  })

  it('ключ 2FA: по умолчанию «не менять», новый — set, удаление сохранённого — clear; поле на «Основном»', () => {
    const model = modelFromDetails(details({ has_totp: true }))
    expect(toProfileForm(model).totp).toEqual({ action: 'keep' })
    model.totp.value = 'JBSWY3DPEHPK3PXP'
    expect(toProfileForm(model).totp).toEqual({ action: 'set', value: 'JBSWY3DPEHPK3PXP' })
    model.totp = { value: '', saved: true, clear: true }
    expect(toProfileForm(model).totp).toEqual({ action: 'clear' })
    expect(tabOfField('totp')).toBe('basic')
  })
})

describe('закладки в модели редактора', () => {
  it('закладки: из профиля, пустые строки отбрасываются, поле на «Запуске», проверка строки', () => {
    const model = modelFromDetails({ ...details(), bookmarks: [{ title: 'Shop', url: 'https://shop.test/' }] })
    expect(model.bookmarks).toEqual([{ title: 'Shop', url: 'https://shop.test/' }])
    model.bookmarks.push({ title: '  ', url: '' }, { title: ' Docs ', url: ' http://docs.test/ ' })
    expect(toProfileForm(model).bookmarks).toEqual([
      { title: 'Shop', url: 'https://shop.test/' },
      { title: 'Docs', url: 'http://docs.test/' },
    ])
    expect(tabOfField('bookmarks')).toBe('launch')
    expect(bookmarkRowError({ title: 'A', url: 'example.com' })).toBe('url')
    expect(bookmarkRowError({ title: '', url: 'https://a.test/' })).toBe('title')
    expect(bookmarkRowError({ title: 'x'.repeat(201), url: 'https://a.test/' })).toBe('title')
    expect(bookmarkRowError({ title: '', url: '' })).toBeNull()
    expect(bookmarkRowError({ title: 'A', url: 'https://a.test/' })).toBeNull()
  })
})

describe('набор расширений в модели редактора', () => {
  it('новый профиль — «Общий», сохранённый — свой набор, форма передаёт набор, ошибка поля — вкладка «Запуск»', () => {
    expect(emptyModel(structuredClone(fp)).extCategoryId).toBe(0)
    const model = modelFromDetails(details({ ext_category_id: 3 }))
    expect(model.extCategoryId).toBe(3)
    expect(toProfileForm(model).ext_category_id).toBe(3)
    expect(toProfileForm({ ...model, extCategoryId: 0 }).ext_category_id).toBe(0)
    expect(tabOfField('ext_category_id')).toBe('launch')
    expect(fieldSnapshot({ ...model, extCategoryId: 1 }, 'ext_category_id')).not.toBe(fieldSnapshot(model, 'ext_category_id'))
  })
})

describe('квота хранилища в модели редактора', () => {
  it('текст квоты: пусто — без ошибки; не целое, меньше 1 или больше u32 — ошибка до ядра; сохранённая квота — в тексте', () => {
    const model = { ...emptyModel(structuredClone(fp)), name: 'Магазин' }
    expect(model.quotaText).toBe('')
    expect(clientErrors(model)).toEqual([])
    expect(clientErrors({ ...model, quotaText: '5000' })).toEqual([])
    for (const quotaText of ['5000,', '0', '5 000 МБ', '4294967296']) {
      expect(clientErrors({ ...model, quotaText })).toEqual([{ field: 'storage_quota_mb', code: 'out_of_range' }])
    }
    expect(parseQuota('4294967295')).toBe(4_294_967_295)
    const saved = details()
    saved.fingerprint.storage_quota_mb = 2048
    expect(modelFromDetails(saved).quotaText).toBe('2048')
    expect(fieldSnapshot({ ...model, quotaText: 'x' }, 'storage_quota_mb')).not.toBe(fieldSnapshot(model, 'storage_quota_mb'))
  })
})

describe('очеловечивание в модели редактора', () => {
  it('из профиля в форму целиком; ошибка клавиши — на своей вкладке и снимается правкой', () => {
    const humanize = { enabled: false, preset: 'careful' as const, block_input: false, paste_key: 'Ctrl+Alt+V' }
    const model = modelFromDetails(details({ humanize }))
    expect(model.humanize).toEqual(humanize)
    expect(model.humanize).not.toBe(details({ humanize }).profile.humanize)
    expect(toProfileForm(model).humanize).toEqual(humanize)
    expect(emptyModel(structuredClone(fp)).humanize).toEqual({ enabled: true, preset: 'default', block_input: true, paste_key: '' })
    expect(tabOfField('humanize.paste_key')).toBe('humanize')
    const edited = { ...model, humanize: { ...humanize, paste_key: '' } }
    expect(fieldSnapshot(edited, 'humanize.paste_key')).not.toBe(fieldSnapshot(model, 'humanize.paste_key'))
  })

  it('горячая клавиша из нажатия — как её разбирает движок', () => {
    const key = (code: string, mods: Partial<Record<'metaKey' | 'ctrlKey' | 'altKey' | 'shiftKey', boolean>> = {}, k = 'x') =>
      pasteKeyFromEvent({ code, key: k, metaKey: false, ctrlKey: false, altKey: false, shiftKey: false, ...mods })
    expect(key('KeyV', { ctrlKey: true, altKey: true }, 'µ')).toBe('Ctrl+Alt+V')
    expect(key('Digit1', { altKey: true })).toBe('Alt+1')
    expect(key('KeyK', { ctrlKey: true, shiftKey: true })).toBe('Ctrl+Shift+K')
    // ⌘ занимает меню Chrome, один ⇧ — набор заглавной: такие нажатия поле не меняют.
    expect(key('KeyV', { metaKey: true, shiftKey: true })).toBeNull()
    expect(key('KeyV', { metaKey: true })).toBeNull()
    expect(key('KeyV', { metaKey: true, ctrlKey: true })).toBeNull()
    expect(key('KeyV', { shiftKey: true }, 'V')).toBeNull()
    expect(key('KeyV')).toBeNull()
    expect(key('F1', { metaKey: true })).toBeNull()
    expect(key('MetaLeft', { metaKey: true }, 'Meta')).toBeNull()
    expect(key('Backspace', {}, 'Backspace')).toBe('')
  })
})
