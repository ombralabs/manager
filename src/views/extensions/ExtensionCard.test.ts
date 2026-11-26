import { flushPromises } from '@vue/test-utils'
import { describe, expect, it } from 'vitest'
import type { ExtensionRecord } from '../../api/types'
import { extension, riskyExtension } from '../../test/fixtures'
import { mountWithApp } from '../../test/mount'
import ExtensionCard from './ExtensionCard.vue'

const mountCard = (props: Record<string, unknown>) => mountWithApp(ExtensionCard, { props, attachTo: document.body })

describe('карточка расширения', () => {
  it('плитка, имя, версия, описание, «Доступно», переключатель с подписью; источник — ссылка на магазин и наборы', async () => {
    const wrapper = mountCard({ ext: extension({ description: 'Блокировщик рекламы' }), enabled: true, sets: ['Крипто', 'Реклама'] })
    // Цвет плитки проверяет тест `tileColor` (задача 3): запись цвета в `style` зависит от happy-dom.
    expect(wrapper.find('.tile').text()).toBe('u')
    expect(wrapper.find('.name-text').text()).toBe('uBlock Origin')
    expect(wrapper.find('.sub').text()).toBe('версия 1.62.0')
    expect(wrapper.find('.description').text()).toBe('Блокировщик рекламы')
    expect(wrapper.find('.status').text()).toBe('Доступно')
    const toggle = wrapper.find('[role="switch"]')
    expect(toggle.attributes('aria-label')).toBe('uBlock Origin: включено')
    expect(toggle.attributes('aria-checked')).toBe('true')
    expect(wrapper.find('.source').text()).toBe('Источник: Chrome Web Store · наборы: Крипто, Реклама')
    expect(wrapper.find('.source').attributes('title')).toBe('Источник: Chrome Web Store · наборы: Крипто, Реклама')
    expect(wrapper.find('.risk').exists()).toBe(false)
    await wrapper.find('.source button').trigger('click')
    expect(wrapper.emitted('openStore')).toHaveLength(1)
    await toggle.trigger('click')
    expect(wrapper.emitted('toggle')).toEqual([[false]])
    wrapper.unmount()
  })

  it('источник: файл с именем, без имени, папка; вне наборов; ⚠ у рискованного; длинное имя и описание — в title', () => {
    const cases: [Partial<ExtensionRecord>, string][] = [
      [{ source: 'file', file_name: 'team-autologin.zip' }, 'Источник: файл team-autologin.zip · вне наборов'],
      [{ source: 'file', file_name: null }, 'Источник: файл · вне наборов'],
      [{ source: 'file', file_name: 'my-ext' }, 'Источник: папка my-ext · вне наборов'],
    ]
    for (const [patch, text] of cases) {
      const wrapper = mountCard({ ext: extension(patch) })
      expect(wrapper.find('.source').text()).toBe(text)
      expect(wrapper.find('.source button').exists()).toBe(false)
      wrapper.unmount()
    }
    const name = 'Очень длинное название расширения '.repeat(6).trim()
    const description = 'Длинное описание. '.repeat(30).trim()
    const risky = mountCard({ ext: { ...riskyExtension, name, description } })
    expect(risky.find('.risk').attributes('title')).toBe('Расширение может изменять настройки прокси')
    expect(risky.find('.risk .sr-only').text()).toBe('Расширение может изменять настройки прокси')
    expect(risky.find('.name-text').attributes('title')).toBe(name)
    expect(risky.find('.description').attributes('title')).toBe(description)
    expect(risky.find('[role="switch"]').attributes('aria-label')).toBe(`${name}: выключено`)
    risky.unmount()
  })

  it('пока сохраняется набор 0, переключатель aria-disabled, но не disabled', () => {
    const wrapper = mountCard({ ext: extension(), saving: true })
    const toggle = wrapper.find('[role="switch"]')
    expect(toggle.attributes('aria-disabled')).toBe('true')
    expect(toggle.attributes('disabled')).toBeUndefined()
    wrapper.unmount()
  })

  it('меню «⋯»: у магазинного — обновление и удаление, у файлового — только удаление', async () => {
    const wrapper = mountCard({ ext: extension() })
    await wrapper.find('button[aria-label="Ещё действия: uBlock Origin"]').trigger('keydown', { key: 'ArrowDown' })
    await flushPromises()
    const items = () => wrapper.findAll('[role="menuitem"]')
    expect(items().map((item) => item.text())).toEqual(['Обновить из Web Store', 'Удалить из библиотеки'])
    await items()[0]!.trigger('click')
    expect(wrapper.emitted('update')).toHaveLength(1)
    wrapper.unmount()
    const file = mountCard({ ext: extension({ source: 'file' }) })
    await file.find('button[aria-label="Ещё действия: uBlock Origin"]').trigger('keydown', { key: 'ArrowDown' })
    await flushPromises()
    expect(file.findAll('[role="menuitem"]').map((item) => item.text())).toEqual(['Удалить из библиотеки'])
    await file.findAll('[role="menuitem"]')[0]!.trigger('click')
    expect(file.emitted('remove')).toHaveLength(1)
    file.unmount()
  })

  it('«Загружается»: имя из ссылки, «загрузка из магазина», переключатель неактивен, без описания и меню', () => {
    const wrapper = mountCard({ pending: { key: extension().id, name: extension().id, from: 'store' } })
    expect(wrapper.find('.sub').text()).toBe('загрузка из магазина')
    expect(wrapper.find('.status').text()).toBe('Загружается')
    const toggle = wrapper.find('[role="switch"]')
    expect(toggle.attributes('aria-label')).toBe(`${extension().id}: загружается`)
    expect(toggle.attributes('disabled')).toBeDefined()
    expect(wrapper.find('.source').text()).toBe('Источник: Chrome Web Store')
    expect(wrapper.find('button[aria-label^="Ещё действия"]').exists()).toBe(false)
    wrapper.unmount()
    const file = mountCard({ pending: { key: '/tmp/team.zip', name: 'team.zip', from: 'file' } })
    expect(file.find('.sub').text()).toBe('загрузка из файла')
    expect(file.find('.source').text()).toBe('Источник: файл')
    file.unmount()
  })
})
