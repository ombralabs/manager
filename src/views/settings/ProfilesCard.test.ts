import { flushPromises } from '@vue/test-utils'
import { describe, expect, it, vi } from 'vitest'
import { mountWithApp } from '../../test/mount'

const api = vi.hoisted(() => ({
  setOnQuit: vi.fn(async () => null),
}))
vi.mock('../../api/core', () => ({ core: api, events: {} }))

import UiSegmented from '../../components/UiSegmented.vue'
import { useSettingsStore } from '../../stores/settings'
import ProfilesCard from './ProfilesCard.vue'

function mountCard() {
  useSettingsStore().profile = { on_quit: 'ask' }
  return mountWithApp(ProfilesCard)
}

describe('profile launch settings without a product cap', () => {
  it('does not offer a maximum simultaneous profile count', () => {
    expect(mountCard().find('#max-running').exists()).toBe(false)
  })
  it('«Остановить все» сохраняется сразу', async () => {
    const wrapper = mountCard()
    wrapper.findComponent(UiSegmented).vm.$emit('update:modelValue', 'stop_all')
    await flushPromises()
    expect(api.setOnQuit).toHaveBeenCalledWith('stop_all')
  })

  it('правила шлюзов без переключателей, у каждого «Всегда включено»', () => {
    const wrapper = mountCard()
    expect(wrapper.find('[role="switch"]').exists()).toBe(false)
    const rules = wrapper.findAll('.rule')
    expect(rules).toHaveLength(2)
    for (const rule of rules) expect(rule.text()).toContain('Всегда включено')
  })

})
