import { describe, expect, it, vi } from 'vitest'
import { mountWithApp } from '../../test/mount'

vi.mock('../../api/core', () => ({ core: { setLanguage: vi.fn(async () => undefined), revealDataDir: vi.fn() }, events: {} }))

import UiSelect from '../../components/UiSelect.vue'
import { useAppStore } from '../../stores/app'
import GeneralCard from './GeneralCard.vue'

function mountCard() {
  useAppStore().info = { version: '0.1.0', data_dir: '/Users/me/Ombra', language: 'ru', engine: null, init_error: null }
  return mountWithApp(GeneralCard, { attachTo: document.body })
}

describe('общие настройки', () => {
  it('«Язык интерфейса» — выпадающий выбор с подписью, выбор English меняет язык', async () => {
    const wrapper = mountCard()
    const select = wrapper.get('#ui-language')
    expect(select.attributes('role')).toBe('combobox')
    expect(wrapper.get('label[for="ui-language"]').text()).toBe('Язык интерфейса')
    const spy = vi.spyOn(useAppStore(), 'setLanguage')
    wrapper.findComponent(UiSelect).vm.$emit('update:modelValue', 'en')
    expect(spy).toHaveBeenCalledWith('en')
  })
})
