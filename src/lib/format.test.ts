import { describe, expect, it } from 'vitest'
import type { GatewayView } from '../api/types'
import {
  engineMajor,
  fingerprintTitle,
  gatewayTitle,
  gpuName,
  localDate,
  kindLabel,
  osMajor,
} from './format'

describe('форматирование', () => {
  it('localDate — ГГГГ-ММ-ДД по местному времени', () => {
    expect(localDate(new Date(2026, 9, 6, 23, 59))).toBe('2026-10-06')
    expect(localDate(new Date(2026, 0, 2))).toBe('2026-01-02')
  })

  it('версии ОС и движка', () => {
    expect(osMajor('15.2.0')).toBe('15')
    expect(engineMajor('145.0.7632.109.2')).toBe('145')
    expect(engineMajor(null)).toBeNull()
    expect(engineMajor('garbage')).toBeNull()
    expect(fingerprintTitle('15.2.0', 'Chrome', '145.0.7632.109.2')).toBe('macOS 15 · Chrome 145')
    expect(fingerprintTitle('26.4.1', 'Chrome', null)).toBe('macOS 26 · Chrome')
  })

  it('короткое имя видеокарты', () => {
    expect(gpuName('ANGLE (Apple, ANGLE Metal Renderer: Apple M2, Unspecified Version)')).toBe('Apple M2')
    expect(gpuName('ANGLE (Intel Inc., Intel Iris OpenGL Engine, OpenGL 4.1)')).toBe('Intel Iris')
    expect(gpuName('ANGLE (ATI Technologies Inc., AMD Radeon Pro 570X OpenGL Engine, OpenGL 4.1)')).toBe(
      'AMD Radeon Pro 570X',
    )
    expect(gpuName('Something else')).toBe('Something else')
  })

  it('шлюз в списке профилей', () => {
    const known: GatewayView = { kind: 'socks5', host: 'gw.example.net', port: 1080, ip: '203.0.113.24', country: 'DE', ok: true }
    expect(gatewayTitle(known)).toBe('DE · 203.0.113.24')
    const unknown: GatewayView = { ...known, ip: null, country: null }
    expect(gatewayTitle(unknown)).toBe('gw.example.net:1080')
    expect(kindLabel('https')).toBe('HTTPS')
  })
})
