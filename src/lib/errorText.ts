import type { FieldError } from '../api/types'
import { maskedOwnerHint } from '../api/errors'
import { i18n } from '../i18n'

const { t, te } = i18n.global

/** Перевод кода из словаря: `<prefix>.<код>`; неизвестный код — `<prefix>.<fallback>`, а не текст ядра. */
export function codeText(prefix: string, code: string, fallback: string): string {
  const key = `${prefix}.${code}`
  return te(key) ? t(key) : t(`${prefix}.${fallback}`)
}

/** Заголовок ошибки ядра: `errors.<код>`, неизвестный код — «Неизвестная ошибка». */
export function errorTitle(code: string, ownerHint?: string): string {
  const account = maskedOwnerHint(ownerHint)
  if (code === 'FreeDeviceBound' && account) return t('errors.FreeDeviceBoundAccount', { account })
  return codeText('errors', code, 'Unknown')
}

/** Почему Local API не запустился (`ApiStatus.failed.code`); неизвестный код — «системная ошибка». */
export function apiFailureText(code: string): string {
  return codeText('settings.api.failure', code, 'io')
}

/** Почему строка импорта cookies пропущена (`Skipped.code`); неизвестный код — «запись не разобрана». */
export function skipText(code: string): string {
  return codeText('editor.cookies.skip', code, 'unparsed')
}

/** Текст ошибки поля: свой у поля (`validation.fields.<поле>.<код>`), иначе общий по коду, иначе «Неверное значение». */
export function fieldErrorText(errors: FieldError[], field: string): string | null {
  const code = errors.find((error) => error.field === field)?.code
  if (code === undefined) return null
  for (const key of [`validation.fields.${field}.${code}`, `validation.${code}`]) {
    if (te(key)) return t(key)
  }
  return t('validation.invalid')
}

/** Переведённый текст ошибки поля формы или `null`. */
export function useFieldErrors(errors: () => FieldError[]) {
  return (field: string): string | null => fieldErrorText(errors(), field)
}
