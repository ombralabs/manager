import type { LineError } from '../api/types'
import { i18n } from '../i18n'

/** «Строка 3: неверный прокси»; неизвестный код — «не удалось разобрать строку», как у импорта прокси. */
export function csvLineErrorText(error: LineError): string {
  const { t, te } = i18n.global
  const key = `profiles.importMenu.reasons.${error.code}`
  const reason = te(key) ? t(key) : t('profiles.importMenu.reasons.format')
  return t('profiles.importMenu.line', { line: error.line, reason })
}
