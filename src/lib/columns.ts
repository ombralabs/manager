import type { ProfileRow, ProxyKind } from '../api/types'
import { gatewayTitle } from './format'
import { PREF_COLUMNS, readPref, writePref } from './prefs'

/** Имя, статус и действие видны всегда; остальные колонки настраиваются. */
export const OPTIONAL_COLUMNS = ['serial', 'gateway', 'tags', 'fingerprint', 'created', 'last_opened'] as const
export type OptionalColumn = (typeof OPTIONAL_COLUMNS)[number]

/** По умолчанию — компактный список: имя, статус, прокси, теги и действие. */
export const DEFAULT_HIDDEN: OptionalColumn[] = ['serial', 'fingerprint', 'created', 'last_opened']

const WIDTH: Record<OptionalColumn, string> = {
  serial: '44px',
  tags: 'minmax(90px, .7fr)',
  gateway: 'minmax(130px, 1fr)',
  fingerprint: '184px',
  created: '136px',
  last_opened: '136px',
}

function isOptionalColumn(value: unknown): value is OptionalColumn {
  return typeof value === 'string' && (OPTIONAL_COLUMNS as readonly string[]).includes(value)
}

/** Скрытые колонки из `localStorage`: неизвестные значения отбрасываются, испорченное значение — как по умолчанию. */
export function loadHidden(): OptionalColumn[] {
  return readPref(PREF_COLUMNS, (raw) => (Array.isArray(raw) ? raw.filter(isOptionalColumn) : null), [
    ...DEFAULT_HIDDEN,
  ])
}

export function saveHidden(hidden: OptionalColumn[]): void {
  writePref(PREF_COLUMNS, hidden)
}

/** Порядок совпадает с заголовками и ячейками ProfilesTable. */
export function gridTemplate(hidden: readonly OptionalColumn[]): string {
  const optional = OPTIONAL_COLUMNS.filter((column) => column !== 'serial' && !hidden.includes(column)).map((column) => WIDTH[column])
  return ['28px', ...(!hidden.includes('serial') ? [WIDTH.serial] : []), 'minmax(160px, 1.5fr)', '132px', ...optional, '160px'].join(' ')
}

type WithStatus = Pick<ProfileRow, 'status'>

/** Браузер открыт: «Работает», «Пауза» или «Ждёт поток» с `info` (браузер работает, но потерял поток). */
export function isLive(row: WithStatus): boolean {
  const { status } = row
  return status.state === 'running' || status.state === 'paused' || (status.state === 'waiting' && status.info !== null)
}

/** Профиль занят — открыт или запускается: удалить его и очистить кэш нельзя. */
export function isActive(row: WithStatus): boolean {
  return isLive(row) || row.status.state === 'starting' || row.status.state === 'waiting'
}

/** Вторая строка колонки «Имя»: теги через запятую, без тегов — заметка. */
export function nameSubline(row: Pick<ProfileRow, 'tags' | 'note'>): string {
  return row.tags.length > 0 ? row.tags.join(', ') : row.note
}

export type GatewayNote = 'pinned' | 'replaced' | 'unreachable' | 'unassigned'

/** Колонка «Шлюз»: `title` — страна и IP последней проверки или адрес прокси, `null` — прокси ещё не назначен. */
export interface GatewayCell {
  title: string | null
  kind: ProxyKind | null
  note: GatewayNote
}

export function gatewayCell(row: Pick<ProfileRow, 'gateway' | 'gateway_replaced_at' | 'status'>): GatewayCell {
  const title = row.gateway ? gatewayTitle(row.gateway) : null
  const kind = row.gateway ? row.gateway.kind : null
  if (row.status.state === 'paused') return { title, kind, note: 'unreachable' }
  if (!row.gateway) return { title, kind, note: 'unassigned' }
  if (row.gateway_replaced_at !== null) return { title, kind, note: 'replaced' }
  return { title, kind, note: 'pinned' }
}

/** Форматтеры дат по языку и часовому поясу: создание `Intl.DateTimeFormat` дорого, а ячеек сотни. */
const dateFormats = new Map<string, Intl.DateTimeFormat>()

/** Дата и время по языку интерфейса; `timeZone` задают только тесты. */
export function formatDateTime(seconds: number, locale: string, timeZone?: string): string {
  const key = `${locale}|${timeZone ?? ''}`
  let format = dateFormats.get(key)
  if (!format) {
    const options: Intl.DateTimeFormatOptions = { dateStyle: 'short', timeStyle: 'short' }
    if (timeZone) options.timeZone = timeZone
    format = new Intl.DateTimeFormat(locale, options)
    dateFormats.set(key, format)
  }
  return format.format(new Date(seconds * 1000))
}
