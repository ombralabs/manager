import { describe, expect, it } from 'vitest'
import { mountWithApp } from '../test/mount'
import StatusPill from './StatusPill.vue'

describe('StatusPill', () => {
  it('identifies the first account with a masked tooltip and never forwards a raw address to HTML', () => {
    const masked = mountWithApp(StatusPill, { props: { state: 'waiting', license: 'device_bound', ownerHint: 'iv***v@example.com' } })
    expect(masked.attributes('title')).toContain('iv***v@example.com')
    const raw = mountWithApp(StatusPill, { props: { state: 'waiting', license: 'device_bound', ownerHint: 'ivan@example.com' } })
    expect(raw.html()).not.toContain('ivan@example.com')
  })
  it('«Ожидает запуска»: подпись и причина в подсказке', () => {
    const wrapper = mountWithApp(StatusPill, { props: { state: 'waiting', license: 'no_slot' } })
    expect(wrapper.text()).toBe('Ожидает запуска')
    expect(wrapper.attributes('title')).toBe('Прежний браузер: Достигнут лимит одновременных профилей. Обновите браузер.')
  })

  it('неизвестное состояние — подсказки нет', () => {
    const wrapper = mountWithApp(StatusPill, { props: { state: 'waiting', license: 'brand_new' } })
    expect(wrapper.text()).toBe('Ожидает запуска')
    expect(wrapper.attributes('title')).toBeUndefined()
  })
})
