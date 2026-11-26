import { describe, expect, it } from 'vitest'
import { CoreError, toCoreError } from './errors'

describe('ошибки ядра', () => {
  it('keeps only a masked owner hint attached to a device conflict', () => {
    expect(toCoreError({ code: 'FreeDeviceBound', ownerHint: 'iv***v@example.com' }))
      .toMatchObject({ ownerHint: 'iv***v@example.com' })
    expect(toCoreError({ code: 'FreeDeviceBound', ownerHint: 'ivan@example.com' }))
      .not.toMatchObject({ ownerHint: 'ivan@example.com' })
  })
  it('сохраняют код, сообщение и поля', () => {
    const error = toCoreError({
      code: 'Validation',
      message: 'проверьте поля формы',
      fields: [{ field: 'name', code: 'required' }],
    })
    expect(error).toBeInstanceOf(CoreError)
    expect(error.code).toBe('Validation')
    expect(error.message).toBe('проверьте поля формы')
    expect(error.fields).toEqual([{ field: 'name', code: 'required' }])
  })

  it('Error становится Internal, прочее — Unknown без [object Object]', () => {
    expect(toCoreError(new Error('сбой'))).toMatchObject({ code: 'Internal', message: 'сбой' })
    expect(toCoreError('command not found')).toMatchObject({ code: 'Unknown', message: 'command not found' })
    expect(toCoreError({})).toMatchObject({ code: 'Unknown', message: '' })
    expect(toCoreError({ message: 'без кода' })).toMatchObject({ code: 'Unknown', message: 'без кода' })
    expect(toCoreError({ code: 7, message: { nested: true } })).toMatchObject({ code: 'Unknown', message: '' })
    expect(toCoreError(null)).toMatchObject({ code: 'Unknown', message: '' })
    const known = new CoreError('NoProxy', 'нет прокси')
    expect(toCoreError(known)).toBe(known)
  })
})
