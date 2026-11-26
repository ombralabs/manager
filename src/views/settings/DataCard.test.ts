import { describe, expect, it } from 'vitest'
import { mountWithApp } from '../../test/mount'
import { useSecurityStore } from '../../stores/security'
import DataCard from './DataCard.vue'

describe('данные на компьютере', () => {
  it('не обещает включённую защиту без настройки и обновляет текст после включения', async () => {
    const security = useSecurityStore()
    security.status = { configured: false, unlocked: false } as never
    const wrapper = mountWithApp(DataCard)
    expect(wrapper.text()).toContain('Защита паролем и 2FA не настроена')
    security.status = { configured: true, unlocked: true } as never
    await wrapper.vm.$nextTick()
    expect(wrapper.text()).toContain('Защита паролем и 2FA включена')
    expect(wrapper.text()).not.toContain('не настроена')
    expect(wrapper.text()).toContain('Открыть папку')
  })
})
