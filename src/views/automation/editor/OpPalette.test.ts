import { beforeEach, describe, expect, it, vi } from 'vitest'
import { i18n } from '../../../i18n'
import { PREF_OP_GROUPS_COLLAPSED } from '../../../lib/prefs'
import { mountWithApp } from '../../../test/mount'

vi.mock('../../../api/core', () => ({ core: {}, events: {} }))

import { useFlowEditorStore } from '../../../stores/flowEditor'
import OpPalette from './OpPalette.vue'
import { testCatalog } from './testOps'

function mountPalette() {
  useFlowEditorStore().catalog = testCatalog
  return mountWithApp(OpPalette)
}

describe('OpPalette', () => {
  beforeEach(() => localStorage.clear())

  it('группы по порядку, Start нет', () => {
    const wrapper = mountPalette()
    const titles = wrapper.findAll('.group-title').map((g) => g.text())
    expect(titles).toEqual(['Управление сценарием', 'Действия на странице', 'Клавиатура', 'Ожидания'])
    expect(wrapper.text()).not.toContain('Начало')
    expect(wrapper.text()).toContain('Условие')
  })

  it('поиск по названию без учёта регистра; английских названий нет ни в списке, ни в поиске', async () => {
    const wrapper = mountPalette()
    expect(wrapper.text()).not.toContain('Click Element')
    const input = wrapper.find('input')
    await input.setValue('ЩЕЛЧОК')
    expect(wrapper.findAll('.op-item .name').map((i) => i.text())).toEqual(['Щелчок мышью'])
    await input.setValue('click')
    expect(wrapper.findAll('.op-item')).toHaveLength(0)
    await input.setValue('zzz')
    expect(wrapper.text()).toContain('Ничего не найдено')
  })

  it('без перевода — название AdsPower; ключ перевода — со всеми точками id', () => {
    i18n.global.mergeLocaleMessage('ru', { automation: { ops: { ext_a_b: 'Две точки' } } })
    const ext = (id: string, adspower: string) => ({ id, adspower, group: 'ext', inputs: ['in'], outputs: ['next'], fields: [] })
    useFlowEditorStore().catalog = { system: [], ops: [ext('ext.a.b', 'A B'), ext('ext.tool', 'Some Tool')] }
    const items = mountWithApp(OpPalette).findAll('.op-item')
    expect(items.map((i) => i.find('.name').text())).toEqual(['Две точки', 'Some Tool'])
  })

  it('щелчок по значку добавляет соответствующую операцию', async () => {
    const wrapper = mountPalette()
    await wrapper.get('[data-op-icon="web.click"]').trigger('click')
    expect(wrapper.emitted('add')![0]![0]).toMatchObject({ id: 'web.click' })
  })

  it('заголовок сворачивает только свою группу, после раскрытия операции доступны', async () => {
    const wrapper = mountPalette()
    const control = wrapper.get('[data-group="control"]')
    const toggle = control.get('.group-toggle')
    expect(toggle.attributes('aria-controls')).toBe(control.get('.group-ops').attributes('id'))
    expect(control.get('.group-count').text()).toBe('3')
    await toggle.trigger('click')
    expect(toggle.attributes('aria-expanded')).toBe('false')
    expect(control.get('.group-ops').isVisible()).toBe(false)
    expect(wrapper.get('[data-group="web"] .group-ops').isVisible()).toBe(true)
    expect(wrapper.emitted('add')).toBeUndefined()
    await toggle.trigger('click')
    expect(toggle.attributes('aria-expanded')).toBe('true')
    expect(control.get('.group-ops').isVisible()).toBe(true)
    await control.get('.op-item').trigger('click')
    expect(wrapper.emitted('add')![0]![0]).toMatchObject({ id: 'control.if' })
  })

  it('сохраняет свёрнутые группы при повторном открытии', async () => {
    const wrapper = mountPalette()
    await wrapper.get('[data-group="control"] .group-toggle').trigger('click')
    await wrapper.get('[data-group="web"] .group-toggle').trigger('click')
    wrapper.unmount()
    const reopened = mountPalette()
    expect(reopened.get('[data-group="control"] .group-ops').isVisible()).toBe(false)
    expect(reopened.get('[data-group="web"] .group-ops').isVisible()).toBe(false)
    expect(reopened.get('[data-group="wait"] .group-ops').isVisible()).toBe(true)
    await reopened.get('[data-group="web"] .group-toggle').trigger('click')
    expect(JSON.parse(localStorage.getItem(PREF_OP_GROUPS_COLLAPSED)!)).toEqual(['control'])
  })

  it('поиск раскрывает совпадения, а очистка восстанавливает состояние групп', async () => {
    const wrapper = mountPalette()
    await wrapper.get('[data-group="web"] .group-toggle').trigger('click')
    const input = wrapper.get('input')
    await input.setValue('щел')
    expect(wrapper.get('[data-group="web"] .group-ops').isVisible()).toBe(true)
    await wrapper.get('[data-group="web"] .group-toggle').trigger('click')
    expect(wrapper.get('[data-group="web"] .group-ops').isVisible()).toBe(false)
    await input.setValue('щелчок')
    expect(wrapper.get('[data-group="web"] .group-ops').isVisible()).toBe(true)
    await input.setValue('')
    expect(wrapper.get('[data-group="web"] .group-ops').isVisible()).toBe(false)
    expect(wrapper.get('[data-group="control"] .group-ops').isVisible()).toBe(true)
    expect(JSON.parse(localStorage.getItem(PREF_OP_GROUPS_COLLAPSED)!)).toEqual(['web'])
  })

  it('перенос за значок отдаёт координаты указателя и не добавляет второй узел по последующему click', async () => {
    const wrapper = mountPalette()
    const item = wrapper.get('[data-group="web"] .op-item')
    await item.get('[data-op-icon]').trigger('pointerdown', { pointerId: 1, button: 0, clientX: 20, clientY: 30 })
    window.dispatchEvent(new PointerEvent('pointermove', { pointerId: 1, clientX: 400, clientY: 250 }))
    expect(wrapper.emitted('drag')!.at(-1)![0]).toMatchObject({ op: { id: 'web.click' }, x: 400, y: 250 })
    expect(wrapper.emitted('add')).toBeUndefined()
    window.dispatchEvent(new PointerEvent('pointerup', { pointerId: 1, clientX: 430, clientY: 270 }))
    await item.trigger('click', { detail: 1 })
    expect(wrapper.emitted('drop')).toHaveLength(1)
    expect(wrapper.emitted('drop')![0]![0]).toMatchObject({ op: { id: 'web.click' }, x: 430, y: 270 })
    expect(wrapper.emitted('drag')!.at(-1)).toEqual([null])
    expect(wrapper.emitted('add')).toBeUndefined()
    wrapper.unmount()
  })

  it('обычный щелчок с небольшим движением остаётся щелчком', async () => {
    const wrapper = mountPalette()
    const item = wrapper.get('[data-group="web"] .op-item')
    await item.trigger('pointerdown', { pointerId: 2, button: 0, clientX: 20, clientY: 30 })
    window.dispatchEvent(new PointerEvent('pointermove', { pointerId: 2, clientX: 22, clientY: 31 }))
    window.dispatchEvent(new PointerEvent('pointerup', { pointerId: 2, clientX: 22, clientY: 31 }))
    await item.trigger('click', { detail: 1 })
    expect(wrapper.emitted('drop')).toBeUndefined()
    expect(wrapper.emitted('drag')).toEqual([[null]])
    expect(wrapper.emitted('add')).toHaveLength(1)
    wrapper.unmount()
  })

  it.each(['escape', 'pointercancel', 'blur', 'lostpointercapture'])('%s отменяет перенос, не блокируя последующую активацию с клавиатуры', async (cancel) => {
    const wrapper = mountPalette()
    const item = wrapper.get('[data-group="web"] .op-item')
    await item.trigger('pointerdown', { pointerId: 3, button: 0, clientX: 20, clientY: 30 })
    window.dispatchEvent(new PointerEvent('pointermove', { pointerId: 3, clientX: 400, clientY: 250 }))
    if (cancel === 'escape') window.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape' }))
    else if (cancel === 'blur') window.dispatchEvent(new Event('blur'))
    else if (cancel === 'lostpointercapture') await item.trigger('lostpointercapture', { pointerId: 3 })
    else window.dispatchEvent(new PointerEvent('pointercancel', { pointerId: 3 }))
    window.dispatchEvent(new PointerEvent('pointerup', { pointerId: 3, clientX: 400, clientY: 250 }))
    await item.trigger('click', { detail: 1 })
    expect(wrapper.emitted('drop')).toBeUndefined()
    expect(wrapper.emitted('add')).toBeUndefined()
    expect(wrapper.emitted('drag')!.at(-1)).toEqual([null])
    await item.trigger('click', { detail: 0 })
    expect(wrapper.emitted('add')).toHaveLength(1)
    wrapper.unmount()
  })

  it('уход с экрана снимает обработчики незавершённого переноса', async () => {
    const onDrag = vi.fn()
    useFlowEditorStore().catalog = testCatalog
    const wrapper = mountWithApp(OpPalette, { props: { onDrag } })
    await wrapper.get('[data-group="web"] .op-item').trigger('pointerdown', { pointerId: 4, button: 0, clientX: 20, clientY: 30 })
    window.dispatchEvent(new PointerEvent('pointermove', { pointerId: 4, clientX: 200, clientY: 100 }))
    expect(onDrag).toHaveBeenCalledTimes(1)
    wrapper.unmount()
    window.dispatchEvent(new PointerEvent('pointermove', { pointerId: 4, clientX: 300, clientY: 200 }))
    window.dispatchEvent(new PointerEvent('pointerup', { pointerId: 4, clientX: 300, clientY: 200 }))
    expect(onDrag).toHaveBeenCalledTimes(2)
    expect(onDrag).toHaveBeenLastCalledWith(null)
    expect(wrapper.emitted('drop')).toBeUndefined()
  })
})
