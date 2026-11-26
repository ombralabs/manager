import { enableAutoUnmount, flushPromises } from '@vue/test-utils'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { mountWithApp } from '../test/mount'
import { profileLabel } from '../test/fixtures'
import { useProfileLabelsStore } from '../stores/profileLabels'
import ProfileStatusFilter from './ProfileStatusFilter.vue'
import UiSelect from './UiSelect.vue'
import type { SelectOption } from './types'

const api = vi.hoisted(() => ({ listProfileLabels: vi.fn(), saveProfileLabel: vi.fn(), deleteProfileLabel: vi.fn() }))
vi.mock('../api/core', () => ({ core: api, events: {} }))
enableAutoUnmount(afterEach)

describe('единый выбор ручного статуса', () => {
  it('создание, переименование и удаление обновляют все селекторы, сохраняя выбор по id', async () => {
    const labels = useProfileLabelsStore()
    labels.labels = [profileLabel()]
    labels.loaded = true
    const first = mountWithApp(ProfileStatusFilter, { props: { modelValue: 2, active: false } })
    const second = mountWithApp(ProfileStatusFilter, { props: { modelValue: 'any', active: false } })
    const options = (wrapper: typeof first) => (wrapper.getComponent(UiSelect).props() as { options: SelectOption<string>[] }).options
    expect(options(first)).toEqual(options(second))
    const custom = profileLabel({ id: 4, name: 'Проверить', preset: null })
    api.saveProfileLabel.mockResolvedValueOnce(custom)
    await labels.save(null, custom.name, custom.color)
    expect(options(first).some((option) => option.label === 'Проверить')).toBe(true)
    expect(options(second)).toEqual(options(first))
    await first.setProps({ modelValue: 4 })
    api.saveProfileLabel.mockResolvedValueOnce({ ...custom, name: 'Готов' })
    await labels.save(4, 'Готов', custom.color)
    expect(first.props('modelValue')).toBe(4)
    expect(options(second).some((option) => option.value === '4' && option.label === 'Готов')).toBe(true)
    api.deleteProfileLabel.mockResolvedValueOnce(null)
    await labels.remove(4)
    await flushPromises()
    expect(first.emitted('update:modelValue')).toEqual([['any']])
    expect(options(second).some((option) => option.value === '4')).toBe(false)
    expect(options(first).map((option) => option.value)).toEqual(['any', 'none', '2'])
  })
})
