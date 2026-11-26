import { afterEach, describe, expect, it } from 'vitest'
import { i18n } from '../i18n'
import { apiFailureText, errorTitle, fieldErrorText, skipText } from './errorText'

describe('перевод кодов ядра', () => {
  afterEach(() => {
    i18n.global.locale.value = 'ru'
  })

  it('заголовок ошибки: известный и неизвестный код', () => {
    expect(errorTitle('AlreadyRunning')).toBe('Профиль уже запущен')
    expect(errorTitle('Weird')).toBe('Неизвестная ошибка')
    i18n.global.locale.value = 'en'
    expect(errorTitle('AlreadyRunning')).toBe('The profile is already running')
    expect(errorTitle('Weird')).toBe('Unknown error')
  })

  it('причина отказа Local API; неизвестный код — системная ошибка', () => {
    expect(['port_busy', 'settings_unreadable', 'io', 'weird'].map((code) => apiFailureText(code))).toEqual([
      'порт занят другой программой',
      'настройки не прочитаны — подробности в журнале',
      'системная ошибка — подробности в журнале',
      'системная ошибка — подробности в журнале',
    ])
    i18n.global.locale.value = 'en'
    expect(apiFailureText('port_busy')).toBe('the port is used by another program')
  })

  it('причина пропуска строки cookies; неизвестный код — «неверный формат записи»', () => {
    const codes = ['no_domain', 'bad_domain', 'no_name', 'bad_path', 'control', 'bad_expires', 'unparsed', 'weird']
    expect(codes.map((code) => skipText(code))).toEqual([
      'нет домена',
      'неверный домен',
      'нет имени cookie',
      'неверный путь',
      'управляющие символы в записи',
      'неверный срок действия',
      'неверный формат записи',
      'неверный формат записи',
    ])
    i18n.global.locale.value = 'en'
    expect(codes.map((code) => skipText(code))).toEqual([
      'missing domain',
      'invalid domain',
      'missing cookie name',
      'invalid path',
      'control characters in the record',
      'invalid expiration date',
      'invalid entry format',
      'invalid entry format',
    ])
  })
})

describe('текст ошибки поля', () => {
  it('сначала свой текст поля, затем общий по коду', () => {
    expect(fieldErrorText([{ field: 'count', code: 'out_of_range' }], 'count')).toBe('От 1 до 1000')
    expect(fieldErrorText([{ field: 'tags', code: 'invalid' }], 'tags')).toBe('Тег — не длиннее 30 символов')
    expect(fieldErrorText([{ field: 'name', code: 'required' }], 'name')).toBe('Обязательное поле')
  })

  it('тексты полей 2В-2', () => {
    expect(fieldErrorText([{ field: 'port', code: 'busy' }], 'port')).toBe('Порт занят другой программой')
    expect(fieldErrorText([{ field: 'port', code: 'out_of_range' }], 'port')).toBe('От 1024 до 65535')
    expect(fieldErrorText([{ field: 'proxy_port', code: 'out_of_range' }], 'proxy_port')).toBe('От 1 до 65535')
    expect(fieldErrorText([{ field: 'require_key', code: 'invalid' }], 'require_key')).toBe(
      'Сначала включите ключ API',
    )
  })

  it('неизвестный код даёт общий текст поля', () => {
    expect(fieldErrorText([{ field: 'name', code: 'weird_code' }], 'name')).toBe('Неверное значение')
  })

  it('нет ошибки у поля — null', () => {
    expect(fieldErrorText([{ field: 'note', code: 'required' }], 'name')).toBeNull()
    expect(fieldErrorText([], 'name')).toBeNull()
  })
})
