import { type CoreError, toCoreError } from '../api/errors'
import type { FieldError } from '../api/types'
import type { Ref } from 'vue'
import { i18n } from '../i18n'
import { useToasts } from '../stores/toasts'
import { errorTitle, fieldErrorText } from './errorText'

/** Коды, у которых сообщение ядра добавляет подробности (код выхода, текст ошибки ОС). */
const DETAILED = new Set(['LaunchFailed', 'StopFailed', 'EngineInstall', 'Engine', 'Storage', 'Io', 'ImportFailed', 'Keychain', 'Internal', 'ExtensionDownload', 'BadParams'])

export interface ErrorToast {
  title: string
  body?: string
  details?: string
  action?: { label: string; to: string }
}

/**
 * Заголовок — перевод кода. Ошибка проверки с полями дошла до уведомления, когда этих полей нет в форме
 * (перенос в удалённую группу): тело — тексты полей, по строке, без повторов. Подробность ядра написана
 * по-русски, поэтому только в русском интерфейсе.
 */
export function errorToast(err: CoreError): ErrorToast {
  const { t, te, locale } = i18n.global
  const title = errorTitle(err.code, err.ownerHint)
  if (['BrowserAccessRequired', 'BrowserAccessInvalid', 'FreeDeviceBound', 'DeviceIdentityUnavailable', 'LicenseWaiting', 'EngineUpdateRequired'].includes(err.code)) {
    return { title, action: { label: t('settings.engine.updateBrowser'), to: '/settings#engine' } }
  }
  if (err.code === 'Validation' && err.fields.length > 0) {
    const lines = err.fields.map((field) => fieldErrorText([field], field.field) ?? t('validation.invalid'))
    return { title, body: [...new Set(lines)].join('\n') }
  }
  // Причина — в поле `package`: ядро не передаёт содержимое файла, только код.
  if (err.code === 'ExtensionFormat') {
    const reason = err.fields.find((field) => field.field === 'package')?.code
    const key = `extensions.format.${reason}`
    if (reason === 'manifest') return { title, body: t(key), details: t('extensions.format.manifestDetails') }
    return { title, body: reason !== undefined && te(key) ? t(key) : undefined }
  }
  const detailed = !te(`errors.${err.code}`) || DETAILED.has(err.code)
  return { title, body: detailed && locale.value === 'ru' && err.message ? err.message : undefined }
}

/** Ошибка из любого места, в том числе из хранилища: уведомление на языке интерфейса. */
export function reportError(e: unknown): CoreError {
  const err = toCoreError(e)
  useToasts().push({ kind: 'error', ...errorToast(err) })
  return err
}

/** Ошибка проверки с полями — к полям формы, любая другая — уведомлением. */
export function fieldsOrToast(error: unknown, errors: Ref<FieldError[]>): void {
  const err = toCoreError(error)
  if (err.fields.length > 0) errors.value = err.fields
  else notify.error(err)
}

/** Уведомления. */
export const notify = {
  error: reportError,
  info(title: string, body?: string) {
    useToasts().push({ kind: 'info', title, body })
  },
  warning(title: string, body?: string) {
    useToasts().push({ kind: 'warning', title, body })
  },
}

/** Фоновый опрос: одно уведомление на серию неудач, следующее — после успешного ответа. */
export function pollReporter() {
  let failed = false
  return {
    ok: () => void (failed = false),
    fail(error: unknown) {
      if (!failed) reportError(error)
      failed = true
    },
  }
}
