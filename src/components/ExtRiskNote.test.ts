import { describe, expect, it } from 'vitest'
import { extension, riskyExtension } from '../test/fixtures'
import { mountWithApp } from '../test/mount'
import ExtRiskNote from './ExtRiskNote.vue'

describe('пометка риска расширений', () => {
  it('пусто — ничего; одно — «может»; несколько — «могут» и имена через запятую', () => {
    expect(mountWithApp(ExtRiskNote, { props: { extensions: [] } }).find('.risk-note').exists()).toBe(false)
    expect(mountWithApp(ExtRiskNote, { props: { extensions: [riskyExtension] } }).text()).toBe(
      'ZeroOmega может изменить настройки прокси и направить соединения в обход прокси профиля.',
    )
    const two = mountWithApp(ExtRiskNote, { props: { extensions: [riskyExtension, extension({ name: 'Proxy Helper' })] } })
    expect(two.text()).toBe('ZeroOmega, Proxy Helper могут изменить настройки прокси и направить соединения в обход прокси профиля.')
  })
})
