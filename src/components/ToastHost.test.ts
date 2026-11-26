import { flushPromises } from '@vue/test-utils'
import { afterEach, describe, expect, it } from 'vitest'
import { useToasts } from '../stores/toasts'
import groupPanelSource from '../views/groups/GroupPanel.vue?raw'
import ToastHost from './ToastHost.vue'
import toastHostSource from './ToastHost.vue?raw'
import { mountWithApp } from '../test/mount'

/**
 * CSS блоков `<style>` файла: `scoped` — с атрибутом, `global` — без. Корни, которые reka-ui рисует в портале
 * или с `inheritAttrs: false`, не получают атрибут области `data-v-…`: их правила обязаны быть в `global`.
 */
function styles(source: string) {
  const blocks = [...source.matchAll(/<style(\s+scoped)?>([\s\S]*?)<\/style>/g)]
  return {
    scoped: blocks.filter((block) => block[1]).map((block) => block[2]).join('\n'),
    global: blocks.filter((block) => !block[1]).map((block) => block[2]).join('\n'),
  }
}

describe('стили корней reka-ui в порталах', () => {
  afterEach(() => {
    document.body.innerHTML = ''
  })

  it('уведомление лежит в области `ol.toast-viewport`, её правила — без scoped', async () => {
    mountWithApp(ToastHost, { attachTo: document.body, global: { stubs: { teleport: false } } })
    useToasts().push({ kind: 'error', title: 'Проверьте поля формы' })
    await flushPromises()

    const viewport = document.querySelector('ol.toast-viewport')
    expect(viewport).not.toBeNull()
    expect(viewport?.querySelector('li.ombra-toast.error')?.textContent).toContain('Проверьте поля формы')

    const css = styles(toastHostSource)
    expect(css.global).toMatch(/\.toast-viewport \{[^}]*position: fixed;[^}]*bottom: 24px;/)
    expect(css.global).toMatch(/\.ombra-toast \{[^}]*border-left-width: 4px;/)
    expect(css.global).toMatch(/\.ombra-toast\.error \{/)
    expect(css.scoped).not.toMatch(/\.ombra-toast|\.viewport/)
  })

  it('палитра группы: отступы содержимого `PopoverContent` — без scoped', () => {
    const css = styles(groupPanelSource)
    expect(css.global).toMatch(/\.popover\.palette-popover \{[^}]*padding: 12px;/)
    expect(css.scoped).not.toMatch(/\.palette-popover/)
  })

  it('пояснение ошибки видно сразу, техническая справка свёрнута', async () => {
    const wrapper = mountWithApp(ToastHost, { attachTo: document.body, global: { stubs: { teleport: false } } })
    useToasts().push({ kind: 'error', title: 'Не удалось открыть расширение', body: 'Описание расширения повреждено.', details: 'Проверьте manifest.json.' })
    await flushPromises()
    const toast = document.querySelector('li.ombra-toast.error')!
    expect(toast.querySelector('.body')?.textContent).toBe('Описание расширения повреждено.')
    const details = toast.querySelector('details')!
    expect(details.open).toBe(false)
    expect(details.querySelector('summary')?.textContent).toBe('Подробности')
    details.open = true
    expect(details.textContent).toContain('Проверьте manifest.json.')
    wrapper.unmount()
  })
})
