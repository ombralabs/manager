import { i18n } from '../i18n'
import { formatDateTime } from './columns'

const UNITS = ['b', 'kb', 'mb', 'gb'] as const

/** Байты в Б/КБ/МБ/ГБ по 1024; дробные единицы — с одной цифрой после запятой, разделитель по языку. */
export function formatBytes(bytes: number): string {
  const { t, locale } = i18n.global
  let value = bytes
  let unit = 0
  // Единица меняется по округлённому значению: 1 048 575 Б — «1,0 МБ», не «1 024,0 КБ».
  while (Math.round(value * 10) / 10 >= 1024 && unit < UNITS.length - 1) {
    value /= 1024
    unit += 1
  }
  const digits = unit === 0 ? 0 : 1
  const number = new Intl.NumberFormat(locale.value, { minimumFractionDigits: digits, maximumFractionDigits: digits }).format(value)
  return `${number} ${t(`editor.traffic.units.${UNITS[unit]}`)}`
}

/** «850 мс», «12,3 с», «4 мин». */
export function formatDuration(ms: number): string {
  const { t, locale } = i18n.global
  if (ms < 1000) return t('editor.traffic.ms', { n: ms })
  if (Math.round(ms / 100) < 600) {
    const seconds = new Intl.NumberFormat(locale.value, { minimumFractionDigits: 1, maximumFractionDigits: 1 }).format(ms / 1000)
    return t('editor.traffic.seconds', { n: seconds })
  }
  return t('editor.traffic.minutes', { n: Math.round(ms / 60_000) })
}

/** Прослойка пишет причину отказа текстом: «ok», «400: …», «502: …» или текст ошибки ОС. */
export function resultText(result: string): string {
  const { t } = i18n.global
  if (result === 'ok') return t('editor.traffic.result.ok')
  if (result.startsWith('400')) return t('editor.traffic.result.badRequest')
  if (result.startsWith('502')) return t('editor.traffic.result.gateway')
  return t('editor.traffic.result.failed')
}

/** Сегодня — только время; иначе дата и время (как колонки «Профилей»). */
export function formatWhen(tsMs: number, now: number = Date.now()): string {
  const locale = i18n.global.locale.value
  const date = new Date(tsMs)
  if (date.toDateString() === new Date(now).toDateString()) {
    return new Intl.DateTimeFormat(locale, { timeStyle: 'short' }).format(date)
  }
  return formatDateTime(Math.floor(tsMs / 1000), locale)
}
