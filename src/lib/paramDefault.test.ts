import { describe, expect, it } from 'vitest'
import type { VarDecl } from '../api/types'
import { formDefault } from './paramDefault'

const decl = (type: VarDecl['type'], def: unknown): VarDecl => ({ name: 'x', type, default: def, description: '' })

describe('formDefault', () => {
  it('булево из true и "true"', () => {
    expect(formDefault(decl('boolean', true))).toBe(true)
    expect(formDefault(decl('boolean', 'true'))).toBe(true)
    expect(formDefault(decl('boolean', 'false'))).toBe(false)
    expect(formDefault(decl('boolean', null))).toBe(false)
  })
  it('число из числа и числовой строки', () => {
    expect(formDefault(decl('number', 5))).toBe('5')
    expect(formDefault(decl('number', ' 2.5 '))).toBe('2.5')
    expect(formDefault(decl('number', 'x'))).toBe('')
  })
  it('прочие типы как текст', () => {
    expect(formDefault(decl('string', 'a'))).toBe('a')
    expect(formDefault(decl('array', [1]))).toBe('[1]')
    expect(formDefault(decl('string', null))).toBe('')
  })
})
