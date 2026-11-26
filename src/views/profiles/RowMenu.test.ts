import { flushPromises } from '@vue/test-utils'
import { DropdownMenuRoot } from 'reka-ui'
import { afterEach, describe, expect, it, vi } from 'vitest'
import type { ProfileRow } from '../../api/types'
import { profileRow } from '../../test/fixtures'
import { mountWithApp } from '../../test/mount'

vi.mock('../../api/core', () => ({ core: {}, events: {} }))

import ProfilesTable from './ProfilesTable.vue'

const profile = (index: number): ProfileRow =>
  profileRow({
    id: `p${index}`,
    serial: index,
    name: `Профиль ${index}`,
    restore_tabs: false,
    seed: index,
    created_at: 0,
  })

const ROWS = [1, 2, 3].map(profile)
const MORE = 'button[aria-label="Ещё действия"]'

function mountTable() {
  return mountWithApp(ProfilesTable, {
    props: { rows: ROWS, hidden: [], groups: [] },
    attachTo: document.body,
  })
}

type Table = ReturnType<typeof mountTable>

const more = (wrapper: Table, row: number) => wrapper.findAll('.body .row')[row]!.find(MORE)
const items = (wrapper: Table) => wrapper.findAll('[role="menuitem"]').map((item) => item.text())

describe('меню строки «Профилей»', () => {
  afterEach(() => {
    document.body.innerHTML = ''
  })

  it('пока меню не просили, в таблице нет ни одного меню reka; кнопка уже объявляет меню', () => {
    const wrapper = mountTable()
    expect(wrapper.findAllComponents(DropdownMenuRoot)).toHaveLength(0)
    for (const button of wrapper.findAll(MORE)) {
      expect(button.attributes()).toMatchObject({ 'aria-haspopup': 'menu', 'aria-expanded': 'false', title: 'Ещё действия' })
    }
  })

  it('наведение готовит меню одной строки, щелчок открывает его, пункт уходит событием таблицы с профилем', async () => {
    const wrapper = mountTable()
    await more(wrapper, 1).trigger('pointerenter')
    expect(wrapper.findAllComponents(DropdownMenuRoot)).toHaveLength(1)
    expect(items(wrapper)).toEqual([])

    await more(wrapper, 1).trigger('click')
    await flushPromises()
    expect(items(wrapper)).toEqual([
      'Изменить',
      'Создать копию',
      'Перенести в группу…',
      'Экспорт cookies…',
      'Экспорт архива…',
      'Очистить кэш',
      'Удалить',
    ])
    await wrapper.findAll('[role="menuitem"]')[0]!.trigger('click')
    await flushPromises()
    expect(wrapper.emitted('edit')).toEqual([[ROWS[1]]])
  })

  it('«Экспорт архива…» — у остановленного профиля; у запущенного неактивен', async () => {
    const itemOf = async (row: ProfileRow) => {
      const wrapper = mountWithApp(ProfilesTable, { props: { rows: [row], hidden: [], groups: [] }, attachTo: document.body })
      await more(wrapper, 0).trigger('click')
      await flushPromises()
      const item = wrapper.findAll('[role="menuitem"]').find((i) => i.text() === 'Экспорт архива…')!
      wrapper.unmount()
      document.body.innerHTML = ''
      return item
    }
    expect((await itemOf(profileRow())).attributes('data-disabled')).toBeUndefined()
    const running = profileRow({ status: { state: 'running', pid: 1, cdp_ws: '', debug_port: 1, relay_port: 2, proxy_id: 1, exit_ip: null, country: null, timezone: 'UTC', started_at: 1 } })
    expect((await itemOf(running)).attributes('data-disabled')).toBeDefined()
  })

  it('с клавиатуры: фокус переходит на настоящую кнопку меню, Enter открывает, Esc закрывает и возвращает фокус', async () => {
    const wrapper = mountTable()
    ;(more(wrapper, 0).element as HTMLElement).focus()
    await flushPromises()
    const trigger = more(wrapper, 0).element as HTMLElement
    expect(document.activeElement).toBe(trigger)
    expect(trigger.getAttribute('data-state')).toBe('closed')

    await more(wrapper, 0).trigger('keydown', { key: 'Enter' })
    await flushPromises()
    expect(items(wrapper)).toContain('Изменить')
    expect(trigger.getAttribute('aria-expanded')).toBe('true')

    document.activeElement?.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }))
    await flushPromises()
    expect(items(wrapper)).toEqual([])
    // reka возвращает фокус таймером после закрытия.
    await vi.waitFor(() => expect(document.activeElement).toBe(trigger))
  })

  it('стрелка вниз и щелчок без наведения (диктор) открывают меню сразу', async () => {
    const wrapper = mountTable()
    await more(wrapper, 2).trigger('keydown', { key: 'ArrowDown' })
    await flushPromises()
    expect(items(wrapper)).toContain('Удалить')
    expect(wrapper.findAllComponents(DropdownMenuRoot)).toHaveLength(1)
  })

  it('щелчок без наведения и фокуса открывает меню; нажатие, пришедшее вместе с подменой кнопки, тоже', async () => {
    const wrapper = mountTable()
    await more(wrapper, 0).trigger('click')
    await flushPromises()
    expect(items(wrapper)).toContain('Изменить')

    // pointerenter подменил кнопку, pointerdown ушёл на простую (отсоединённую) кнопку.
    const second = more(wrapper, 1)
    await second.trigger('pointerenter')
    await second.trigger('pointerdown')
    await flushPromises()
    expect(wrapper.findAll('[role="menuitem"]').length).toBeGreaterThan(6)
  })

  it('правый щелчок и Ctrl+щелчок по кнопке не открывают меню, левый — открывает', async () => {
    const wrapper = mountTable()
    // Нажатие приходит на простую кнопку вместе с подменой, как в тесте выше.
    const button = more(wrapper, 0)
    await button.trigger('pointerenter')
    await button.trigger('pointerdown', { button: 2 })
    await button.trigger('pointerdown', { button: 0, ctrlKey: true })
    await flushPromises()
    expect(items(wrapper)).toEqual([])
    await button.trigger('pointerdown', { button: 0 })
    await flushPromises()
    expect(items(wrapper)).toContain('Изменить')
  })
})
