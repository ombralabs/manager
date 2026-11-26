import { flushPromises } from '@vue/test-utils'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { FileDrop } from '../../api/dialogs'
import { extension } from '../../test/fixtures'
import { mountWithApp } from '../../test/mount'

const api = vi.hoisted(() => ({ openStoreSearch: vi.fn() }))
const dialogs = vi.hoisted(() => ({ pickExtensionFile: vi.fn(), onFileDrop: vi.fn() }))
vi.mock('../../api/core', () => ({ core: api, events: {} }))
vi.mock('../../api/dialogs', () => dialogs)

import { useExtensionsStore } from '../../stores/extensions'
import AddExtensionDialog from './AddExtensionDialog.vue'

type Wrapper = ReturnType<typeof mountWithApp>
let drop: (event: FileDrop) => void
const unlisten = vi.fn()

beforeEach(() => {
  for (const mock of [...Object.values(api), ...Object.values(dialogs)]) mock.mockReset()
  unlisten.mockReset()
  dialogs.onFileDrop.mockImplementation(async (handler: (event: FileDrop) => void) => {
    drop = handler
    return unlisten
  })
  api.openStoreSearch.mockResolvedValue(null)
})

async function mountDialog() {
  const wrapper = mountWithApp(AddExtensionDialog, { props: { open: true }, attachTo: document.body })
  await flushPromises()
  return wrapper
}
const button = (wrapper: Wrapper, text: string) => {
  const found = wrapper.findAll('[role="dialog"] button').find((element) => element.text() === text)
  if (!found) throw new Error(`нет кнопки «${text}»`)
  return found
}

describe('диалог «Добавить расширение»', () => {
  it('неверная ссылка — ошибка у поля с подсказкой, добавления нет, фокус в поле, диалог открыт', async () => {
    const wrapper = await mountDialog()
    const add = vi.spyOn(useExtensionsStore(), 'add').mockResolvedValue()
    await wrapper.find('#ext-url').setValue('ublock origin')
    await button(wrapper, 'Добавить').trigger('click')
    await flushPromises()
    expect(add).not.toHaveBeenCalled()
    expect(wrapper.find('[role="dialog"]').text()).toContain(
      'Нужна ссылка на страницу расширения или его ID. Чтобы найти по названию, нажмите «Открыть магазин»',
    )
    expect(wrapper.find('#ext-url').attributes('aria-invalid')).toBe('true')
    expect(document.activeElement).toBe(wrapper.find('#ext-url').element)
    expect(wrapper.emitted('update:open')).toBeUndefined()
    wrapper.unmount()
  })

  it('«Открыть магазин» — поиск по тексту поля; верная ссылка — добавление по ID с галочкой, диалог закрыт', async () => {
    const wrapper = await mountDialog()
    const add = vi.spyOn(useExtensionsStore(), 'add').mockResolvedValue()
    await button(wrapper, 'Открыть магазин').trigger('click')
    expect(api.openStoreSearch).toHaveBeenLastCalledWith('')
    const link = ` https://chromewebstore.google.com/detail/ublock-origin/${extension().id}?hl=ru `
    await wrapper.find('#ext-url').setValue(link)
    await button(wrapper, 'Открыть магазин').trigger('click')
    expect(api.openStoreSearch).toHaveBeenLastCalledWith(link.trim())
    await wrapper.find('input[type="checkbox"]').setValue(false)
    await button(wrapper, 'Добавить').trigger('click')
    expect(add).toHaveBeenCalledWith({ store: extension().id }, false)
    expect(wrapper.emitted('update:open')).toEqual([[false]])
    wrapper.unmount()
  })

  it('файл: «Выбрать файл», «Убрать», перетаскивание (подсветка, первый путь); ссылка и файл — что-то одно', async () => {
    const wrapper = await mountDialog()
    const add = vi.spyOn(useExtensionsStore(), 'add').mockResolvedValue()
    expect(button(wrapper, 'Добавить').attributes('disabled')).toBeDefined()
    dialogs.pickExtensionFile.mockResolvedValueOnce('/Users/demo/Downloads/team-autologin.zip')
    await button(wrapper, 'Выбрать файл').trigger('click')
    await flushPromises()
    expect(wrapper.find('.drop').text()).toContain('team-autologin.zip')
    expect(button(wrapper, 'Добавить').attributes('disabled')).toBeUndefined()
    await button(wrapper, 'Убрать').trigger('click')
    expect(wrapper.find('.drop').text()).toContain('Перетащите сюда архив .zip или файл .crx')
    dialogs.pickExtensionFile.mockResolvedValueOnce('/Users/demo/Downloads/team-autologin.zip')
    await button(wrapper, 'Выбрать файл').trigger('click')
    await flushPromises()
    await wrapper.find('#ext-url').setValue('x')
    expect(wrapper.find('.drop').text()).toContain('Перетащите сюда архив .zip или файл .crx')
    drop({ type: 'enter' })
    await flushPromises()
    expect(wrapper.find('.drop').classes()).toContain('over')
    drop({ type: 'drop', paths: ['/Users/demo/my-ext', '/Users/demo/other.zip'] })
    await flushPromises()
    expect(wrapper.find('.drop').classes()).not.toContain('over')
    expect((wrapper.find('#ext-url').element as HTMLInputElement).value).toBe('')
    expect(wrapper.find('.drop').text()).toContain('my-ext')
    await button(wrapper, 'Добавить').trigger('click')
    expect(add).toHaveBeenCalledWith({ file: '/Users/demo/my-ext' }, true)
    wrapper.unmount()
  })

  it('перетаскивание слушается, только пока диалог открыт; при новом открытии поля чистые', async () => {
    const wrapper = await mountDialog()
    expect(dialogs.onFileDrop).toHaveBeenCalledTimes(1)
    await wrapper.find('#ext-url').setValue('что-то')
    await wrapper.setProps({ open: false })
    expect(unlisten).toHaveBeenCalledTimes(1)
    await wrapper.setProps({ open: true })
    await flushPromises()
    expect(dialogs.onFileDrop).toHaveBeenCalledTimes(2)
    expect((wrapper.find('#ext-url').element as HTMLInputElement).value).toBe('')
    expect((wrapper.find('input[type="checkbox"]').element as HTMLInputElement).checked).toBe(true)
    wrapper.unmount()
  })
})
