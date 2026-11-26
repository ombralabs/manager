import { flushPromises } from '@vue/test-utils'
import { describe, expect, it } from 'vitest'
import type { ExtCategory } from '../../api/types'
import BulkBar from './BulkBar.vue'
import { extCategory } from '../../test/fixtures'
import { mountWithApp } from '../../test/mount'

function bar(props: Partial<{
  count: number
  busy: boolean
  sets: ExtCategory[]
  riskySets: ReadonlySet<number>
}> = {}) {
  return mountWithApp(BulkBar, {
    props: { count: 3, groups: [], sets: [], riskySets: new Set<number>(), busy: false, ...props },
  })
}

const button = (wrapper: ReturnType<typeof bar>, text: string) =>
  wrapper.findAll('button').find((candidate) => candidate.text() === text)

describe('панель массовых действий', () => {
  it('показывает число выбранных', () => {
    expect(bar().text()).toContain('Выбрано: 3')
  })

  it('кнопки действий — со значком и подсказкой: в узкой панели подпись скрыта', () => {
    const actions = bar().findAll('button')
    expect(actions).toHaveLength(11)
    for (const action of actions) {
      expect(action.find('svg').exists()).toBe(true)
      expect(action.attributes('title')).toBe(action.find('.label').text())
    }
  })

  it('без выбора панель на месте, действия неактивны', () => {
    const actions = bar({ count: 0 }).findAll('button')
    expect(actions).toHaveLength(11)
    expect(actions.every((action) => action.attributes('disabled') !== undefined)).toBe(true)
  })

  it('«Синхронизировать» — от двух выбранных, передаётся экрану', async () => {
    expect(button(bar({ count: 1 }), 'Синхронизировать')!.attributes('disabled')).toBeDefined()
    const wrapper = bar({ count: 2 })
    await button(wrapper, 'Синхронизировать')!.trigger('click')
    expect(wrapper.emitted('sync')).toHaveLength(1)
  })

  it('«Запустить сценарий…» передаёт экрану runFlow', async () => {
    const wrapper = bar()
    await button(wrapper, 'Запустить сценарий…')!.trigger('click')
    expect(wrapper.emitted('runFlow')).toHaveLength(1)
  })

  it('«Удалить» и «Снять выделение» передаются экрану', async () => {
    const wrapper = bar()
    await button(wrapper, 'Удалить')!.trigger('click')
    await button(wrapper, 'Снять выделение')!.trigger('click')
    expect(wrapper.emitted('remove')).toHaveLength(1)
    expect(wrapper.emitted('clear')).toHaveLength(1)
  })

  it('«Расширения» — меню наборов с пометкой риска; выбор передаёт набор экрану', async () => {
    const wrapper = bar({ sets: [extCategory(), extCategory({ id: 1, name: 'Работа' })], riskySets: new Set([1]) })
    await button(wrapper, 'Расширения')!.trigger('keydown', { key: 'ArrowDown' })
    await flushPromises()
    expect(wrapper.text()).toContain('Назначить набор')
    const items = wrapper.findAll('[role="menuitem"]')
    expect(items.map((item) => item.find('.name').text())).toEqual(['Без набора', 'Работа'])
    expect(items[0]!.find('.risk').exists()).toBe(false)
    expect(items[1]!.find('.risk .sr-only').text()).toBe('В наборе есть расширение, которое может изменять настройки прокси')
    await items[1]!.trigger('click')
    expect(wrapper.emitted('extSet')).toEqual([[1]])
  })

  it('пока идёт операция, кнопки заняты, но фокус на них остаётся; щелчок не уходит экрану; пункты меню неактивны', async () => {
    const wrapper = bar({ busy: true, sets: [extCategory()] })
    for (const label of ['Запустить', 'Удалить']) {
      const found = button(wrapper, label)!
      expect(found.attributes('disabled')).toBeUndefined()
      expect(found.attributes('aria-disabled')).toBe('true')
    }
    await button(wrapper, 'Запустить')!.trigger('click')
    expect(wrapper.emitted('start')).toBeUndefined()
    await button(wrapper, 'Расширения')!.trigger('keydown', { key: 'ArrowDown' })
    await flushPromises()
    const items = wrapper.findAll('[role="menuitem"]')
    expect(items.length).toBeGreaterThan(0)
    expect(items.every((item) => item.attributes('aria-disabled') === 'true')).toBe(true)
  })
})
