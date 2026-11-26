import { describe, expect, it } from 'vitest'
import { extCategory, extension, riskyExtension } from '../../test/fixtures'
import { mountWithApp } from '../../test/mount'
import SetsList from './SetsList.vue'

describe('список наборов', () => {
  it('строка: имя, «N расширений · M профилей» по библиотеке, ⚠, выбранная — aria-current; карандаш и корзина', async () => {
    const crypto = extCategory({ id: 1, name: 'Крипто', extension_ids: [extension().id, riskyExtension.id, 'gone'], profile_count: 3 })
    const ads = extCategory({ id: 2, name: 'Реклама', extension_ids: [], profile_count: 0 })
    const wrapper = mountWithApp(SetsList, {
      props: { sets: [crypto, ads], selected: 1, library: [extension(), riskyExtension], risky: new Set([1]) },
    })
    const rows = wrapper.findAll('[data-test="set-row"]')
    expect(rows.map((row) => row.find('.name-text').text())).toEqual(['Крипто', 'Реклама'])
    expect(rows.map((row) => row.find('.meta').text())).toEqual(['2 расширения · 3 профиля', 'нет расширений · нет профилей'])
    expect(rows.map((row) => row.find('.pick').attributes('aria-current'))).toEqual(['true', undefined])
    expect(rows[0]!.find('.risk .sr-only').text()).toBe('В наборе есть расширение, которое может изменять настройки прокси')
    expect(rows[1]!.find('.risk').exists()).toBe(false)
    const rename = rows[1]!.find('button[aria-label="Переименовать набор Реклама"]')
    expect(rename.attributes('title')).toBe('Переименовать')
    expect(rows[1]!.find('button[aria-label="Удалить набор Реклама"]').attributes('title')).toBe('Удалить набор')
    await rows[1]!.find('.pick').trigger('click')
    await rename.trigger('click')
    await rows[1]!.find('button[aria-label="Удалить набор Реклама"]').trigger('click')
    expect(wrapper.emitted('select')).toEqual([[2]])
    expect(wrapper.emitted('rename')).toEqual([[ads]])
    expect(wrapper.emitted('remove')).toEqual([[ads]])
  })

  it('нет наборов — «Наборов пока нет»', () => {
    const wrapper = mountWithApp(SetsList, { props: { sets: [], selected: null, library: [], risky: new Set() } })
    expect(wrapper.text()).toContain('Наборов пока нет')
    expect(wrapper.find('[data-test="set-row"]').exists()).toBe(false)
  })
})
