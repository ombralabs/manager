import type { FieldError, LineError, ProxyEditForm, ProxyKind, ProxyRecord, ProxyRow } from '../api/types'
import type { IconName } from '../components/icons'
import type { SecretDraft } from '../components/types'
import { i18n } from '../i18n'
import { secretInput } from './secretInput'
import { PORT_MAX, parseIntegerIn } from './settings'

/** «Проверить все» и проверка после импорта: столько проверок идёт одновременно. */
export const CHECK_PARALLEL = 8

/** `хост:порт`; адрес IPv6 — в квадратных скобках, как его принимает импорт. */
export function proxyAddress(proxy: Pick<ProxyRecord, 'host' | 'port'>): string {
  return proxy.host.includes(':') ? `[${proxy.host}]:${proxy.port}` : `${proxy.host}:${proxy.port}`
}

/** «DE · 203.0.113.24» последней удачной проверки (неудача их не стирает); без неё — `null`. */
export function countryIp(proxy: Pick<ProxyRecord, 'check'>): string | null {
  const check = proxy.check
  return check?.country && check.exit_ip ? `${check.country} · ${check.exit_ip}` : null
}

export type CheckState = 'ok' | 'failed' | 'unchecked'

/** Значок состояния проверки: галочка, треугольник, кольцо. */
export const CHECK_ICON: Record<CheckState, IconName> = { ok: 'tick', failed: 'alert', unchecked: 'ring' }

export function checkState(proxy: Pick<ProxyRecord, 'check'>): CheckState {
  if (proxy.check === null) return 'unchecked'
  return proxy.check.ok ? 'ok' : 'failed'
}

/** «Закреплён за»: имя, если профиль один; «N профилей»; «Свободен». */
export function pinnedLabel(row: Pick<ProxyRow, 'profiles' | 'pinned_to'>): string {
  const { t } = i18n.global
  if (row.profiles === 0) return t('proxies.pinned.free')
  if (row.profiles === 1 && row.pinned_to !== null) return row.pinned_to
  return t('proxies.pinned.profiles', row.profiles)
}

/** Записи импорта, как их считает ядро (`parse_proxy_line`): без пустых строк и комментариев `#`. */
export function importLineCount(text: string): number {
  return text.split('\n').filter((line) => {
    const trimmed = line.trim()
    return trimmed !== '' && !trimmed.startsWith('#')
  }).length
}

const LINE_CODES = new Set(['format', 'kind', 'port'])

/** «Строка 3: неверный порт»; неизвестный код — как `format`. */
export function lineErrorText(error: LineError): string {
  const { t } = i18n.global
  const code = LINE_CODES.has(error.code) ? error.code : 'format'
  return t('proxies.import.lineError', { line: error.line, reason: t(`proxies.import.errors.${code}`) })
}

/**
 * После импорта в поле остаются только строки с ошибками, номера ошибок — по новому тексту.
 * Номер ядра — индекс в `text.lines()` с 1: строки делятся по `\n`, `\r` в конце отбрасывается.
 */
export function keepErrorLines(text: string, errors: LineError[]): { text: string; errors: LineError[] } {
  const lines = text.split('\n').map((line) => line.replace(/\r$/, ''))
  const sorted = [...errors].sort((a, b) => a.line - b.line)
  return {
    text: sorted.map((error) => lines[error.line - 1] ?? '').join('\n'),
    errors: sorted.map((error, index) => ({ line: index + 1, code: error.code })),
  }
}

/** Диалог прокси: значения полей ввода, как их держит форма. */
export interface ProxyEditDraft {
  kind: ProxyKind
  host: string
  /** Строка из поля ввода; число получается при сборке формы. */
  port: string
  username: string
  password: SecretDraft
  note: string
  enabled: boolean
}

export function emptyProxyDraft(): ProxyEditDraft {
  return {
    kind: 'http',
    host: '',
    port: '',
    username: '',
    password: { value: '', saved: false, clear: false },
    note: '',
    enabled: true,
  }
}

/** Сохранённая запись: пароль не приходит в интерфейс, поле пустое с пометкой «Сохранён». */
export function draftFromRow(proxy: ProxyRecord): ProxyEditDraft {
  return {
    kind: proxy.kind,
    host: proxy.host,
    port: String(proxy.port),
    username: proxy.username ?? '',
    password: { value: '', saved: proxy.has_password, clear: false },
    note: proxy.note,
    enabled: proxy.enabled,
  }
}

/** Порт прокси: целое 1–65535, иначе `null`. */
export function parseProxyPort(text: string): number | null {
  return parseIntegerIn(text, 1, PORT_MAX)
}

/** Без логина пароль не хранится (ядро требует их вместе): пустой логин удаляет пароль. */
export function toProxyEditForm(draft: ProxyEditDraft): ProxyEditForm {
  const username = draft.username.trim()
  return {
    kind: draft.kind,
    host: draft.host.trim(),
    port: parseProxyPort(draft.port) ?? 0,
    username,
    password: username === '' ? { action: 'clear' } : secretInput(draft.password),
    note: draft.note.trim(),
    enabled: draft.enabled,
  }
}

/**
 * Проверки до ядра, теми же полями и кодами, что `validate_proxy_form`. Порт вне 1–65535 нельзя отправлять:
 * в форме ядра он `u16`, и такая форма не разобралась бы.
 */
export function proxyDraftErrors(draft: ProxyEditDraft): FieldError[] {
  const errors: FieldError[] = []
  if (draft.host.trim() === '') errors.push({ field: 'proxy_host', code: 'required' })
  if (parseProxyPort(draft.port) === null) errors.push({ field: 'proxy_port', code: 'out_of_range' })
  const hasLogin = draft.username.trim() !== ''
  const hasPassword = draft.password.value !== '' || (draft.password.saved && !draft.password.clear)
  if (!hasLogin && draft.password.value !== '') errors.push({ field: 'proxy_username', code: 'required' })
  if (hasLogin && !hasPassword) errors.push({ field: 'proxy_password', code: 'required' })
  return errors
}
