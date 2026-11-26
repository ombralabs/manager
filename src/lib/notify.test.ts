import { createPinia, setActivePinia } from 'pinia'
import { afterEach, beforeEach, describe, expect, it } from 'vitest'
import { CoreError, toCoreError } from '../api/errors'
import { i18n } from '../i18n'
import { useToasts } from '../stores/toasts'
import { errorToast, reportError } from './notify'

describe('уведомление об ошибке', () => {
  beforeEach(() => setActivePinia(createPinia()))
  afterEach(() => {
    i18n.global.locale.value = 'ru'
  })

  it('keeps masked historical device diagnostics and offers a browser update', () => {
    for (const lang of ['ru', 'en'] as const) {
      i18n.global.locale.value = lang
      const toast = errorToast(toCoreError({ code: 'FreeDeviceBound', message: '', ownerHint: 'iv***v@example.com' }))
      expect(toast.title).toContain('iv***v@example.com')
      expect(toast.action).toEqual({ label: lang === 'ru' ? 'Обновить браузер' : 'Update browser', to: '/settings#engine' })
    }
  })

  it('never displays an unmasked address or unsafe owner hint', () => {
    for (const ownerHint of ['ivan@example.com', 'iv***v@example.com\n', '<b>iv***v@example.com</b>', 'iv****v@example.com']) {
      const toast = errorToast(toCoreError({ code: 'FreeDeviceBound', message: ownerHint, ownerHint }))
      expect(toast.title).not.toContain(ownerHint)
      expect(toast.body).toBeUndefined()
    }
  })

  it('offers an engine update for old licensing failures', () => {
    for (const code of ['BrowserAccessInvalid', 'BrowserAccessRequired', 'FreeDeviceBound', 'DeviceIdentityUnavailable', 'LicenseWaiting', 'EngineUpdateRequired']) {
      reportError(new CoreError(code, ''))
      expect(errorToast(new CoreError(code, '')).action).toEqual({ label: 'Обновить браузер', to: '/settings#engine' })
      expect(errorToast(new CoreError(code, '')).title).toMatch(/Обновите браузер/)
    }
  })

  it('подробность ядра у LaunchFailed — только в русском интерфейсе', () => {
    const err = new CoreError('LaunchFailed', 'браузер вышел с кодом 1')
    expect(errorToast(err)).toEqual({ title: 'Браузер не запустился', body: 'браузер вышел с кодом 1' })
    i18n.global.locale.value = 'en'
    expect(errorToast(err)).toEqual({ title: 'The browser did not start', body: undefined })
  })

  it('известный код без подробностей и неизвестный код', () => {
    expect(errorToast(new CoreError('Validation', 'поля'))).toEqual({ title: 'Проверьте поля формы', body: undefined })
    expect(errorToast(new CoreError('Weird', 'странное'))).toEqual({ title: 'Неизвестная ошибка', body: 'странное' })
  })

  it('ошибка проверки с полями без места в форме — тексты полей по строке, в обоих языках', () => {
    const err = new CoreError('Validation', 'группа не найдена', [
      { field: 'group_id', code: 'invalid' },
      { field: 'tags', code: 'invalid' },
    ])
    expect(errorToast(err)).toEqual({
      title: 'Проверьте поля формы',
      body: 'Группа не найдена\nТег — не длиннее 30 символов',
    })
    i18n.global.locale.value = 'en'
    expect(errorToast(err)).toEqual({
      title: 'Check the form fields',
      body: 'Group not found\nA tag is at most 30 characters',
    })
  })

  it('одинаковые тексты полей не повторяются; поле без своего текста — общий текст по коду', () => {
    const err = new CoreError('Validation', 'поля', [
      { field: 'group_id', code: 'invalid' },
      { field: 'group_id', code: 'invalid' },
      { field: 'weird_field', code: 'required' },
    ])
    expect(errorToast(err).body).toBe('Группа не найдена\nОбязательное поле')
  })

  it('reportError с полями — тексты полей в теле', () => {
    reportError({ code: 'Validation', message: 'группа не найдена', fields: [{ field: 'group_id', code: 'invalid' }] })
    expect(useToasts().items).toEqual([{ id: 1, kind: 'error', title: 'Проверьте поля формы', body: 'Группа не найдена' }])
  })

  it('reportError показывает уведомление без компонента', () => {
    const err = reportError({ code: 'Storage', message: 'диск полон' })
    expect(err.code).toBe('Storage')
    expect(useToasts().items).toEqual([{ id: 1, kind: 'error', title: 'Ошибка базы данных', body: 'диск полон' }])
  })

  it('ExtensionFormat — причина текстом в обоих языках; неизвестная причина — без тела', () => {
    const err = (code: string) => new CoreError('ExtensionFormat', 'пакет больше допустимого', [{ field: 'package', code }])
    const reasons: Record<string, string> = {
      crx: 'Этот формат CRX не поддерживается. Выберите расширение в формате CRX3 или ZIP.',
      zip: 'Не удалось открыть ZIP-архив. Файл повреждён или имеет другой формат.',
      manifest: 'В архиве отсутствует или повреждено описание расширения.',
      mv2: 'Эта версия расширения не поддерживается. Выберите версию с Manifest V3.',
      too_large: 'Пакет больше 100 МБ или в нём больше 10 000 файлов',
      id_mismatch: 'Скачанное расширение не соответствует выбранному. Повторите загрузку.',
    }
    for (const [code, body] of Object.entries(reasons)) {
      const details = code === 'manifest' ? { details: 'Проверьте manifest.json: в нём должны быть название и версия.' } : {}
      expect(errorToast(err(code))).toEqual({ title: 'Файл не является расширением Chrome или не поддерживается', body, ...details })
    }
    expect(errorToast(err('weird')).body).toBeUndefined()
    i18n.global.locale.value = 'en'
    expect(errorToast(err('too_large')).body).toBe('The package is over 100 MB or has more than 10,000 files')
    expect(errorToast(err('manifest'))).toMatchObject({
      body: 'The extension description is missing or damaged.',
      details: 'Check manifest.json: it must contain the extension name and version.',
    })
  })

  it('ExtensionDownload — подробность ядра только в русском интерфейсе', () => {
    const err = new CoreError('ExtensionDownload', 'сервер ответил 404')
    expect(errorToast(err)).toEqual({ title: 'Не удалось скачать расширение из Chrome Web Store', body: 'сервер ответил 404' })
    i18n.global.locale.value = 'en'
    expect(errorToast(err).body).toBeUndefined()
  })
})
