import { describe, expect, it } from 'vitest'
import { csvLineErrorText } from './csvImport'

describe('csvLineErrorText', () => {
  it('известные коды и запасной текст', () => {
    expect(csvLineErrorText({ line: 3, code: 'proxy_invalid' })).toBe('Строка 3: неверный прокси')
    expect(csvLineErrorText({ line: 2, code: 'totp_invalid' })).toBe('Строка 2: неверный ключ 2FA')
    expect(csvLineErrorText({ line: 4, code: 'weird' })).toBe('Строка 4: неверный формат строки')
  })
})
