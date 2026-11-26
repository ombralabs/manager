import { flushPromises } from '@vue/test-utils'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { CoreError } from '../../api/errors'
import { mountWithApp } from '../../test/mount'

const api = vi.hoisted(() => ({
  syncState: vi.fn(async () => ({ active: false, mode: 'full_copy', main: null, windows: [] })),
  getSyncSettings: vi.fn(async () => ({ mode: 'full_copy', clicks: true, typing: true, scroll: true, mouse: true, click_delay: [100, 600], typing_delay: [100, 600] })),
  setSyncSettings: vi.fn(async () => null),
}))
vi.mock('../../api/core', () => ({ core: api, events: { onSync: async () => () => {} } }))

import UiSegmented from '../../components/UiSegmented.vue'
import { useSyncStore } from '../../stores/sync'
import SyncCard from './SyncCard.vue'

async function mountCard() {
  await useSyncStore().init()
  await useSyncStore().load()
  const wrapper = mountWithApp(SyncCard)
  await flushPromises()
  return wrapper
}

describe('карточка «Синхронизатор»', () => {
  beforeEach(() => vi.clearAllMocks())

  it('смена режима сохраняет varied; задержки видны только в нём', async () => {
    const wrapper = await mountCard()
    expect(wrapper.find('#sync-click-from').exists()).toBe(false)
    wrapper.findComponent(UiSegmented).vm.$emit('update:modelValue', 'varied')
    await flushPromises()
    expect(api.setSyncSettings).toHaveBeenCalledWith(expect.objectContaining({ mode: 'varied' }))
    expect(wrapper.find('#sync-click-from').exists()).toBe(true)
    expect(wrapper.text()).toContain('при следующем запуске синхронизации')
  })

  it('ошибка click_delay от ядра подсвечивает поле', async () => {
    const wrapper = await mountCard()
    wrapper.findComponent(UiSegmented).vm.$emit('update:modelValue', 'varied')
    await flushPromises()
    api.setSyncSettings.mockRejectedValueOnce(new CoreError('Validation', 'x', [{ field: 'click_delay', code: 'out_of_range' }]))
    const from = wrapper.get('#sync-click-from')
    await from.setValue('900')
    await flushPromises()
    expect(from.classes()).toContain('invalid')
    expect(wrapper.text()).toContain('Задержка щелчка')
    expect(wrapper.get('.error-text').text()).not.toBe('')
  })

  it('два быстрых переключателя не отменяют друг друга', async () => {
    const wrapper = await mountCard()
    let saved = () => {}
    api.setSyncSettings.mockImplementationOnce(() => new Promise<null>((resolve) => { saved = () => resolve(null) }))
    const switches = wrapper.findAll('button[role=switch]')
    await switches[0].trigger('click')
    await switches[1].trigger('click')
    saved()
    await flushPromises()
    expect(api.setSyncSettings).toHaveBeenCalledTimes(2)
    expect(api.setSyncSettings).toHaveBeenLastCalledWith(expect.objectContaining({ clicks: false, typing: false }))
  })

  it('подсказка: настройки применяются при следующем запуске синхронизации в обоих режимах', async () => {
    const wrapper = await mountCard()
    for (const mode of ['full_copy', 'varied']) {
      wrapper.findComponent(UiSegmented).vm.$emit('update:modelValue', mode)
      await flushPromises()
      expect(wrapper.text()).toContain('Изменения режима, действий и задержек')
    }
  })

  it('переключатель не стирает набранную, но не принятую задержку', async () => {
    const wrapper = await mountCard()
    wrapper.findComponent(UiSegmented).vm.$emit('update:modelValue', 'varied')
    await flushPromises()
    await wrapper.get('#sync-click-from').setValue('abc')
    await wrapper.get('button[role=switch]').trigger('click')
    await flushPromises()
    expect(api.setSyncSettings).toHaveBeenCalledTimes(2)
    expect((wrapper.get('#sync-click-from').element as HTMLInputElement).value).toBe('abc')
  })
})
