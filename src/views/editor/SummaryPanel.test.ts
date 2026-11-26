import { describe, expect, it } from 'vitest'
import { emptyModel } from '../../lib/profileForm'
import { useExtensionsStore } from '../../stores/extensions'
import { extCategory, extension, fingerprint as fp } from '../../test/fixtures'
import { mountWithApp } from '../../test/mount'
import SummaryPanel from './SummaryPanel.vue'

const mountPanel = (errors: { field: string; code: string }[], patch = {}) =>
  mountWithApp(SummaryPanel, { props: { model: { ...emptyModel(structuredClone(fp)), ...patch }, errors, engineVersion: null } })

describe('сводка профиля', () => {
  it('поля новых ошибок отпечатка — словами, не идентификаторами', () => {
    const fields = ['device_pixel_ratio', 'max_touch_points', 'storage_quota_mb', 'location', 'location.lat', 'location.lon']
    const text = mountPanel(fields.map((field) => ({ field, code: 'required' }))).text()
    for (const field of fields) expect(text).not.toContain(field)
    expect(text).toContain('Широта: Обязательное поле')
  })

  it('геолокация — только когда координаты введены', () => {
    const manual = { ...fp, location: { mode: 'manual' as const, lat: 0, lon: 0 } }
    const empty = mountPanel([], { fingerprint: manual, latText: '', lonText: '' })
    expect(empty.text()).not.toContain('Геолокация')
    const filled = mountPanel([], { fingerprint: { ...manual, location: { mode: 'manual', lat: 52.5, lon: 13.4 } }, latText: '52.5', lonText: '13.4' })
    expect(filled.text()).toContain('52.5, 13.4')
  })

  it('строка набора расширений: имя и число; набора нет в списке — «—»', () => {
    const store = useExtensionsStore()
    store.extensions = [extension()]
    store.categories = [extCategory({ id: 1, name: 'Работа', extension_ids: [extension().id] })]
    expect(mountPanel([], { extCategoryId: 1 }).text()).toContain('Работа (1)')
    expect(mountPanel([], { extCategoryId: 9 }).text()).toContain('Расширения—')
  })
})
