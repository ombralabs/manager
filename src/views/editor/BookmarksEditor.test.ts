import { describe, expect, it } from 'vitest'
import type { Bookmark } from '../../api/types'
import { mountWithApp } from '../../test/mount'
import BookmarksEditor from './BookmarksEditor.vue'

function mountEditor(list: Bookmark[], error: string | null = null) {
  const state = { list }
  const wrapper = mountWithApp(BookmarksEditor, {
    props: { modelValue: list, 'onUpdate:modelValue': (value: Bookmark[]) => (state.list = value), error },
  })
  return { wrapper, state }
}

describe('закладки в редакторе', () => {
  it('добавить, удалить, ошибка строки и ошибка ядра', async () => {
    const { wrapper, state } = mountEditor([{ title: 'Shop', url: 'example.com' }], 'Проверьте закладки: название и адрес http(s)://')
    expect(wrapper.text()).toContain('Адрес начинается с http:// или https://')
    expect(wrapper.text()).toContain('Проверьте закладки')
    await wrapper.findAll('button').find((b) => b.text() === 'Добавить закладку')!.trigger('click')
    expect(state.list).toHaveLength(2)
    await wrapper.setProps({ modelValue: state.list })
    await wrapper.find('button[aria-label="Удалить закладку «Shop»"]').trigger('click')
    expect(state.list).toEqual([{ title: '', url: '' }])
  })

  it('ошибка названия подсвечивает название, ошибка адреса — адрес', () => {
    const title = mountEditor([{ title: '', url: 'https://a.test/' }]).wrapper.findAll('input')
    expect(title.map((input) => input.classes().includes('invalid'))).toEqual([true, false])
    const url = mountEditor([{ title: 'A', url: 'a.test' }]).wrapper.findAll('input')
    expect(url.map((input) => input.classes().includes('invalid'))).toEqual([false, true])
  })

  it('500 закладок — «Добавить» неактивна', () => {
    const many = Array.from({ length: 500 }, (_, i) => ({ title: `B${i}`, url: `https://b${i}.test/` }))
    const { wrapper } = mountEditor(many)
    expect(wrapper.findAll('button').find((b) => b.text() === 'Добавить закладку')!.attributes('disabled')).toBeDefined()
  })
})
