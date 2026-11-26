import { flushPromises } from '@vue/test-utils'
import { SelectRoot } from 'reka-ui'
import { describe, expect, it } from 'vitest'
import type { Fingerprint, FingerprintOptions } from '../../api/types'
import { fingerprint as base } from '../../test/fixtures'
import { mountWithApp } from '../../test/mount'
import UiSegmented from '../../components/UiSegmented.vue'
import FingerprintTab from './FingerprintTab.vue'

const OPTIONS: FingerprintOptions = {
  screens: [base.screen],
  gpus: [base.gpu],
  hardware_concurrency: [8],
  device_memory: [16],
  engine_version: '150.0.7871.114',
}

function mountTab(fp: Fingerprint = structuredClone(base), errors = [] as { field: string; code: string }[]) {
  const state = { fp, latText: '', lonText: '', quotaText: fp.storage_quota_mb === null ? '' : String(fp.storage_quota_mb) }
  const wrapper = mountWithApp(FingerprintTab, {
    props: {
      modelValue: state.fp,
      'onUpdate:modelValue': (value: Fingerprint) => (state.fp = value),
      seedText: String(fp.seed),
      latText: state.latText,
      'onUpdate:latText': (value: string) => (state.latText = value),
      lonText: state.lonText,
      'onUpdate:lonText': (value: string) => (state.lonText = value),
      quotaText: state.quotaText,
      'onUpdate:quotaText': (value: string) => {
        state.quotaText = value
        void wrapper.setProps({ quotaText: value })
      },
      options: OPTIONS,
      errors,
    },
  })
  return { wrapper, state }
}

describe('вкладка «Отпечаток»: новые поля', () => {
  it('масштаб экрана: выбор 3× ставит значение и пометку «вручную»', async () => {
    const { wrapper, state } = mountTab()
    const select = wrapper.findAllComponents(SelectRoot).find((s) => (s.props() as { modelValue: string }).modelValue === '2')!
    select.vm.$emit('update:modelValue', '3')
    await flushPromises()
    expect(state.fp.device_pixel_ratio).toBe(3)
    expect(state.fp.manual).toContain('device_pixel_ratio')
  })

  it('квота: неверный символ остаётся в поле и не обнуляет значение отпечатка', async () => {
    const { wrapper, state } = mountTab()
    const quota = wrapper.find('#storage-quota')
    await quota.setValue('5000')
    expect(state.fp.storage_quota_mb).toBe(5000)
    await quota.setValue('5000,')
    await flushPromises()
    expect(state.quotaText).toBe('5000,')
    expect((quota.element as HTMLInputElement).value).toBe('5000,')
    expect(state.fp.storage_quota_mb).toBe(5000)
    await quota.setValue('')
    expect(state.fp.storage_quota_mb).toBeNull()
  })

  it('квота: пусто — null, число — значение, пометка «вручную»', async () => {
    const { wrapper, state } = mountTab()
    const quota = wrapper.find('#storage-quota')
    await quota.setValue('5000')
    await quota.trigger('change')
    expect(state.fp.storage_quota_mb).toBe(5000)
    expect(state.fp.manual).toContain('storage_quota_mb')
    await quota.setValue('')
    await quota.trigger('change')
    expect(state.fp.storage_quota_mb).toBeNull()
  })

  it('квота и координаты принимаются без blur; квота до u32', async () => {
    const { wrapper, state } = mountTab()
    // Без `change`: прямой клик по другой вкладке не даёт полю потерять текст.
    await wrapper.find('#storage-quota').setValue('2000000')
    expect(state.fp.storage_quota_mb).toBe(2_000_000)
    await wrapper.find('#storage-quota').setValue('4294967295')
    expect(state.fp.storage_quota_mb).toBe(4_294_967_295)
    const segmented = wrapper.findAllComponents(UiSegmented).find((c) => (c.props() as { ariaLabel?: string }).ariaLabel === 'Геолокация')!
    segmented.vm.$emit('update:modelValue', 'manual')
    await flushPromises()
    await wrapper.find('#latitude').setValue('52.52')
    await wrapper.find('#longitude').setValue('13.4')
    expect([state.latText, state.lonText]).toEqual(['52.52', '13.4'])
  })

  it('геолокация: «Вручную» показывает поля, координаты из текста попадают в отпечаток; «Не задана» снимает', async () => {
    const { wrapper, state } = mountTab()
    expect(wrapper.find('#latitude').exists()).toBe(false)
    const segmented = wrapper.findAllComponents(UiSegmented).find((c) => (c.props() as { ariaLabel?: string }).ariaLabel === 'Геолокация')!
    segmented.vm.$emit('update:modelValue', 'manual')
    await flushPromises()
    expect(state.fp.location.mode).toBe('manual')
    expect(state.fp.manual).toContain('location')
    await wrapper.setProps({ latText: '55.75', lonText: '37.62' })
    await flushPromises()
    expect(state.fp.location).toEqual({ mode: 'manual', lat: 55.75, lon: 37.62 })
    segmented.vm.$emit('update:modelValue', 'off')
    await flushPromises()
    expect(state.fp.location).toEqual({ mode: 'off' })
    expect(state.fp.manual).not.toContain('location')
  })

  it('ошибки ядра — под своими полями', () => {
    const manual: Fingerprint = { ...structuredClone(base), location: { mode: 'manual', lat: 91, lon: 0 } }
    const { wrapper } = mountTab(manual, [
      { field: 'location.lat', code: 'invalid' },
      { field: 'storage_quota_mb', code: 'out_of_range' },
    ])
    expect(wrapper.text()).toContain('Широта — число от −90 до 90')
    expect(wrapper.text()).toContain('Целое число МБ, не меньше 1')
  })
})
