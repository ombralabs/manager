import type {
  Bookmark,
  FieldError,
  FieldName,
  Fingerprint,
  Humanize,
  ProfileDetails,
  ProfileForm,
  ProfileNetwork,
} from '../api/types'
import type { SecretDraft } from '../components/types'
import { parseIntegerIn } from './settings'
import { draftFromRow, emptyProxyDraft, proxyDraftErrors, toProxyEditForm, type ProxyEditDraft } from './proxies'
import { secretInput } from './secretInput'
export { secretInput } from './secretInput'

export type EditorTab = 'basic' | 'fingerprint' | 'gateway' | 'cookies' | 'launch' | 'humanize' | 'traffic'
export const EDITOR_TABS: EditorTab[] = ['basic', 'fingerprint', 'gateway', 'cookies', 'launch', 'humanize', 'traffic']

export interface EditorModel {
  name: string
  note: string
  /** Группа профиля; `null` — «Без группы». */
  group: number | null
  tags: string[]
  platformDomain: string
  /** Textarea: по одной ссылке в строке. */
  startUrls: string
  /** «Восстанавливать вкладки прошлой сессии». */
  restoreTabs: boolean
  extraArgs: string
  bookmarks: Bookmark[]
  /** Набор расширений; 0 — «Общий». */
  extCategoryId: number
  username: string
  password: SecretDraft
  totp: SecretDraft
  fingerprint: Fingerprint
  /** Текст поля «Seed»: пустое или неверное значение в отпечаток не попадает. */
  seedText: string
  /** Поля «Широта» и «Долгота»: пустое или неверное значение в отпечаток не попадает, ошибку даст `clientErrors`. */
  latText: string
  lonText: string
  /** Текст поля «Квота хранилища»: пусто — `null`; неверный текст в отпечаток не попадает, ошибку даст `clientErrors`. */
  quotaText: string
  /** Выбранный прокси списка; `null` — «Случайный при первом запуске». */
  proxyId: number | null
  /** Закрепление на момент загрузки: пока выбор с ним совпадает, `proxy_id` в форму не попадает. */
  savedProxyId: number | null
  connectionMode: 'direct' | 'saved' | 'manual'
  network: ProfileNetwork
  proxyDraft: ProxyEditDraft
  manualProxyId: number | null
  humanize: Humanize
}

/** Новый профиль: очеловечивание включено (решение владельца 2026-10-11), реальный ввод блокируется, клавиши нет. */
export const defaultHumanize = (): Humanize => ({ enabled: true, preset: 'default', block_input: true, paste_key: '' })

const secretDraft = (saved: boolean): SecretDraft => ({ value: '', saved, clear: false })

/** Пустая форма; `group` — группа, по которой сейчас отфильтрован список. */
export function emptyModel(fingerprint: Fingerprint, group: number | null = null): EditorModel {
  return {
    name: '',
    note: '',
    group,
    tags: [],
    platformDomain: '',
    startUrls: '',
    restoreTabs: true,
    extraArgs: '',
    bookmarks: [],
    extCategoryId: 0,
    username: '',
    password: secretDraft(false),
    totp: secretDraft(false),
    fingerprint,
    seedText: String(fingerprint.seed),
    ...coordinateTexts(fingerprint),
    quotaText: quotaTextOf(fingerprint),
    proxyId: null,
    savedProxyId: null,
    connectionMode: 'saved',
    network: { direct: false, ip_checker: 'auto' },
    proxyDraft: emptyProxyDraft(),
    manualProxyId: null,
    humanize: defaultHumanize(),
  }
}

/** Текст полей координат по сохранённой ручной геолокации; иначе пусто. */
function coordinateTexts(fingerprint: Fingerprint): { latText: string; lonText: string } {
  const location = fingerprint.location
  return location.mode === 'manual'
    ? { latText: String(location.lat), lonText: String(location.lon) }
    : { latText: '', lonText: '' }
}

/** Широта (`limit` 90) или долгота (180) из поля: точка или запятая; пусто, не число или вне пределов — `null`. */
export function parseCoordinate(text: string, limit: number): number | null {
  const trimmed = text.trim()
  if (trimmed === '') return null
  const value = Number(trimmed.replace(',', '.'))
  return Number.isFinite(value) && Math.abs(value) <= limit ? value : null
}

export function modelFromDetails(details: ProfileDetails): EditorModel {
  const { profile } = details
  return {
    name: profile.name,
    note: profile.note,
    group: profile.group_id,
    tags: [...details.tags],
    platformDomain: profile.platform_domain,
    startUrls: profile.start_urls.join('\n'),
    restoreTabs: profile.restore_tabs,
    extraArgs: profile.extra_args.join('\n'),
    bookmarks: details.bookmarks.map((b) => ({ ...b })),
    extCategoryId: profile.ext_category_id,
    username: profile.username ?? '',
    password: secretDraft(profile.has_password),
    totp: secretDraft(profile.has_totp),
    fingerprint: details.fingerprint,
    seedText: String(details.fingerprint.seed),
    ...coordinateTexts(details.fingerprint),
    quotaText: quotaTextOf(details.fingerprint),
    proxyId: profile.proxy_id,
    savedProxyId: profile.proxy_id,
    connectionMode: profile.network?.direct ? 'direct' : 'saved',
    network: { direct: false, ip_checker: 'auto', ...profile.network },
    proxyDraft: details.proxy ? draftFromRow(details.proxy) : emptyProxyDraft(),
    manualProxyId: details.proxy?.id ?? null,
    humanize: { ...profile.humanize },
  }
}

/** Строки textarea → список без пустых. */
export function splitLines(text: string): string[] {
  return text
    .split('\n')
    .map((line) => line.trim())
    .filter((line) => line !== '')
}

/**
 * Группа передаётся всегда, в том числе `null`: без поля ядро оставило бы прежнюю. Теги заменяют набор профиля.
 * `proxy_id` — только после правки выбора: закрепление, которое сделал запуск, пока редактор был открыт, не затирается.
 */
export function toProfileForm(model: EditorModel): ProfileForm {
  const form: ProfileForm = {
    name: model.name.trim(),
    note: model.note.trim(),
    group_id: model.group,
    tags: [...model.tags],
    platform_domain: model.platformDomain.trim(),
    start_urls: splitLines(model.startUrls),
    restore_tabs: model.restoreTabs,
    extra_args: splitLines(model.extraArgs),
    bookmarks: model.bookmarks
      .map((b) => ({ title: b.title.trim(), url: b.url.trim() }))
      .filter((b) => b.title !== '' || b.url !== ''),
    ext_category_id: model.extCategoryId,
    username: model.username.trim(),
    password: secretInput(model.password),
    totp: secretInput(model.totp),
    fingerprint: model.fingerprint,
    humanize: { ...model.humanize },
    network: { direct: model.connectionMode === 'direct', ip_checker: model.network.ip_checker },
  }
  if (model.proxyId !== model.savedProxyId) form.proxy_id = model.proxyId
  if (model.connectionMode === 'direct') form.proxy_id = null
  if (model.connectionMode === 'manual') {
    form.proxy_id = null
    form.inline_proxy = toProxyEditForm(model.proxyDraft)
    form.inline_proxy_source = model.manualProxyId
  }
  return form
}

/** Пределы ядра (`store::bookmarks`). */
export const BOOKMARKS_MAX = 500
export const BOOKMARK_TITLE_MAX = 200

/** Ошибка строки закладки для подсветки в форме; пустая строка — не ошибка (при сохранении отбрасывается). */
export function bookmarkRowError(bookmark: Bookmark): 'title' | 'url' | null {
  const title = bookmark.title.trim()
  const url = bookmark.url.trim()
  if (title === '' && url === '') return null
  if (title.length === 0 || [...title].length > BOOKMARK_TITLE_MAX) return 'title'
  if (!url.startsWith('http://') && !url.startsWith('https://')) return 'url'
  return null
}

/** Наибольший seed — `u32` ядра. */
const SEED_MAX = 4_294_967_295

export const parseSeed = (text: string): number | null => parseIntegerIn(text, 0, SEED_MAX)

/** Наибольшая квота — `u32` ядра. */
const QUOTA_MAX = 4_294_967_295

/** Квота из поля: целое от 1 до `u32`; иначе `null`. */
export const parseQuota = (text: string): number | null => parseIntegerIn(text, 1, QUOTA_MAX)

/** Текст поля квоты по отпечатку. */
const quotaTextOf = (fingerprint: Fingerprint): string =>
  fingerprint.storage_quota_mb === null ? '' : String(fingerprint.storage_quota_mb)

/** Проверки, которым не нужно ядро; остальное проверит ядро при сохранении. */
export function clientErrors(model: EditorModel): FieldError[] {
  const errors: FieldError[] = []
  if (model.connectionMode === 'manual') errors.push(...proxyDraftErrors(model.proxyDraft))
  if (model.name.trim() === '') errors.push({ field: 'name', code: 'required' })
  if (model.seedText.trim() === '') errors.push({ field: 'seed', code: 'required' })
  else if (parseSeed(model.seedText) === null) errors.push({ field: 'seed', code: 'out_of_range' })
  if (model.quotaText.trim() !== '' && parseQuota(model.quotaText) === null) {
    errors.push({ field: 'storage_quota_mb', code: 'out_of_range' })
  }
  if (model.fingerprint.location.mode === 'manual') {
    for (const [field, text, limit] of [
      ['location.lat', model.latText, 90],
      ['location.lon', model.lonText, 180],
    ] as const) {
      if (text.trim() === '') errors.push({ field, code: 'required' })
      else if (parseCoordinate(text, limit) === null) errors.push({ field, code: 'invalid' })
    }
  }
  return errors
}

/** Ошибки без поля `field`; если их не было — тот же массив (наблюдатели не срабатывают зря). */
export function clearFieldError(errors: FieldError[], field: string): FieldError[] {
  return errors.some((error) => error.field === field) ? errors.filter((error) => error.field !== field) : errors
}

/** Поле ошибки → значение в модели редактора. Поля отпечатка читаются из `fingerprint`. */
const MODEL_FIELDS: Record<string, (model: EditorModel) => unknown> = {
  name: (model) => model.name,
  note: (model) => model.note,
  group_id: (model) => model.group,
  tags: (model) => model.tags,
  platform_domain: (model) => model.platformDomain,
  start_urls: (model) => model.startUrls,
  restore_tabs: (model) => model.restoreTabs,
  extra_args: (model) => model.extraArgs,
  bookmarks: (model) => model.bookmarks,
  ext_category_id: (model) => model.extCategoryId,
  username: (model) => model.username,
  password: (model) => model.password,
  totp: (model) => model.totp,
  proxy_id: (model) => [model.proxyId, model.connectionMode],
  proxy_host: (model) => model.proxyDraft.host,
  proxy_port: (model) => model.proxyDraft.port,
  proxy_kind: (model) => model.proxyDraft.kind,
  proxy_username: (model) => model.proxyDraft.username,
  proxy_password: (model) => model.proxyDraft.password,
  seed: (model) => model.seedText,
  storage_quota_mb: (model) => model.quotaText,
  // Режим — в снимке: возврат к «Не задана» снимает ошибки скрытых полей.
  'location.lat': (model) => [model.fingerprint.location.mode, model.latText],
  'location.lon': (model) => [model.fingerprint.location.mode, model.lonText],
  'humanize.paste_key': (model) => model.humanize.paste_key,
}

/** Значение поля ошибки строкой: снимок изменился — поле правили. Неизвестное поле — всегда `''`. */
export function fieldSnapshot(model: EditorModel, field: string): string {
  const read = MODEL_FIELDS[field]
  if (read) return JSON.stringify(read(model) ?? null)
  if (field in model.fingerprint) return JSON.stringify(model.fingerprint[field as keyof Fingerprint] ?? null)
  return ''
}

/** Ошибки вкладки «Отпечаток»: формы (клиент и ядро при сохранении) — первыми, живой проверки — по остальным полям. */
export function mergeFingerprintErrors(live: FieldError[], form: FieldError[]): FieldError[] {
  const own = form.filter((error) => tabOfField(error.field) === 'fingerprint')
  const fields = new Set(own.map((error) => error.field))
  return [...own, ...live.filter((error) => !fields.has(error.field))]
}

const BASIC_FIELDS = new Set(['name', 'note', 'group_id', 'tags', 'platform_domain', 'username', 'password', 'totp'])
const LAUNCH_FIELDS = new Set(['start_urls', 'restore_tabs', 'extra_args', 'bookmarks', 'ext_category_id'])

export function tabOfField(field: string): EditorTab {
  if (field.startsWith('proxy_')) return 'gateway'
  if (field.startsWith('humanize.')) return 'humanize'
  if (LAUNCH_FIELDS.has(field)) return 'launch'
  return BASIC_FIELDS.has(field) ? 'basic' : 'fingerprint'
}

/**
 * Горячая клавиша «Вставить как человек» из нажатия — в форме, которую разбирает движок: ⌃ и/или ⌥, по желанию ⇧, и
 * латинская буква или цифра по физической клавише (`code`), чтобы сочетание не зависело от раскладки; то же правило, что
 * у `paste_key_valid` ядра. `''` — Backspace или Delete без модификаторов (снять клавишу); `null` — нажатие не годится:
 * модификатор сам по себе, без модификатора, Esc, с ⌘ (его сочетания занимает меню Chrome) или без ⌃ и ⌥ (один ⇧ отнял
 * бы у страниц заглавную букву).
 */
export function pasteKeyFromEvent(event: Pick<KeyboardEvent, 'code' | 'key' | 'metaKey' | 'ctrlKey' | 'altKey' | 'shiftKey'>): string | null {
  if (!event.metaKey && !event.ctrlKey && !event.altKey && !event.shiftKey) {
    return event.key === 'Backspace' || event.key === 'Delete' ? '' : null
  }
  if (event.metaKey || !(event.ctrlKey || event.altKey)) return null
  const mods = [event.ctrlKey && 'Ctrl', event.altKey && 'Alt', event.shiftKey && 'Shift'].filter(
    (m): m is string => typeof m === 'string',
  )
  const key = /^(?:Key([A-Z])|Digit([0-9]))$/.exec(event.code)
  return key ? [...mods, key[1] ?? key[2]].join('+') : null
}

export function markManual(fingerprint: Fingerprint, field: FieldName): void {
  if (!fingerprint.manual.includes(field)) fingerprint.manual.push(field)
}

export function unmarkManual(fingerprint: Fingerprint, field: FieldName): void {
  fingerprint.manual = fingerprint.manual.filter((f) => f !== field)
}
