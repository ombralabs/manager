import { describe, expect, it } from 'vitest'
import { apiAddress, parsePort } from './settings'

describe('поля «Настроек»', () => {
  it('порт Local API — от 1024 до 65535', () => {
    expect([parsePort('50325'), parsePort(' 1024 '), parsePort('65535')]).toEqual([50325, 1024, 65535])
    expect([parsePort('80'), parsePort('65536'), parsePort(''), parsePort('50 325'), parsePort('5e4')]).toEqual([
      null,
      null,
      null,
      null,
      null,
    ])
  })

  it('адрес Local API — только 127.0.0.1', () => {
    expect(apiAddress(50325)).toBe('http://127.0.0.1:50325')
  })
})
