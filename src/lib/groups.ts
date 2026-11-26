import type { ProfileRow, ProfileStatus } from '../api/types'

/** Палитра цветов групп (холст заказчика). */
export const GROUP_PALETTE = ['#0A6B66', '#B02A6B', '#6B46C1', '#2447C9', '#B4451F', '#2F7D32', '#9A6A00', '#5A5F6B'] as const

/** Первый цвет палитры, не занятый другой группой; все заняты — первый. Ядро хранит цвет строчными. */
export function nextGroupColor(groups: { color: string }[]): string {
  const used = new Set(groups.map((group) => group.color.toLowerCase()))
  return GROUP_PALETTE.find((color) => !used.has(color.toLowerCase())) ?? GROUP_PALETTE[0]
}

export type StatusState = ProfileStatus['state']

export interface StatusCounts {
  stopped: number
  starting: number
  waiting: number
  running: number
  paused: number
  crashed: number
  total: number
}

export function countStatuses(rows: { status: ProfileStatus }[]): StatusCounts {
  const counts: StatusCounts = { stopped: 0, starting: 0, waiting: 0, running: 0, paused: 0, crashed: 0, total: rows.length }
  for (const row of rows) counts[row.status.state] += 1
  return counts
}

/** Важность статуса в боковой панели группы: проблемные и активные — сверху. */
export const STATUS_ORDER: StatusState[] = ['crashed', 'paused', 'starting', 'waiting', 'running', 'stopped']

export function panelProfiles(rows: ProfileRow[], limit = 5): { shown: ProfileRow[]; rest: ProfileRow[] } {
  const rank = (row: ProfileRow) => STATUS_ORDER.indexOf(row.status.state)
  const sorted = [...rows].sort((a, b) => rank(a) - rank(b) || a.serial - b.serial)
  return { shown: sorted.slice(0, limit), rest: sorted.slice(limit) }
}

/** Наборы расширений у профилей: сначала у большинства, при равенстве — по id. */
export function extSetCounts(rows: { ext_category_id: number }[]): { id: number; n: number }[] {
  const counts = new Map<number, number>()
  for (const row of rows) counts.set(row.ext_category_id, (counts.get(row.ext_category_id) ?? 0) + 1)
  return [...counts].map(([id, n]) => ({ id, n })).sort((a, b) => b.n - a.n || a.id - b.id)
}

export type SummaryState = 'running' | 'starting' | 'waiting' | 'paused' | 'crashed'
const SUMMARY_ORDER: SummaryState[] = ['running', 'starting', 'waiting', 'paused', 'crashed']

/**
 * Части сводки «Состояние» без остановленных. Пустой список — «все остановлены» (`total > 0`)
 * или «нет профилей» (`total === 0`): это решает компонент.
 */
export function summaryParts(counts: StatusCounts): { state: SummaryState; n: number }[] {
  return SUMMARY_ORDER.filter((state) => counts[state] > 0).map((state) => ({ state, n: counts[state] }))
}
