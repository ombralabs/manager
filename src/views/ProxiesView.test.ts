import { flushPromises } from '@vue/test-utils'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { CoreError } from '../api/errors'
import type { ProxyRow } from '../api/types'
import { proxyRow } from '../test/fixtures'
import { mountWithApp } from '../test/mount'

const api = vi.hoisted(() => ({
  listProxies: vi.fn(),
  deleteProxy: vi.fn(),
  checkProxy: vi.fn(),
  importProxies: vi.fn(),
}))
vi.mock('../api/core', () => ({ core: api, events: {} }))

import { useToasts } from '../stores/toasts'
import ProxiesView from './ProxiesView.vue'

const row = (id: number, profiles = 0): ProxyRow =>
  proxyRow({ id, host: `gw${id}.example.net`, profiles })

async function mountView() {
  const wrapper = mountWithApp(ProxiesView)
  await flushPromises()
  return wrapper
}

const toastTitles = () => useToasts().items.map((toast) => toast.title)

beforeEach(() => {
  for (const mock of Object.values(api)) mock.mockReset()
  api.listProxies.mockResolvedValue([row(1, 2), row(2)])
})

describe('экран «Прокси»', () => {
  it('шапка: число записей и закреплённых', async () => {
    const wrapper = await mountView()
    expect(wrapper.find('.head').text()).toContain('2 записи · 1 закреплена за профилями')
  })

  it('удаление спрашивает подтверждение; занятый прокси — уведомлением', async () => {
    api.deleteProxy.mockRejectedValue(new CoreError('ProxyInUse', 'прокси используется'))
    const wrapper = await mountView()
    await wrapper.findAll('.body .row')[0]!.find('button[aria-label="Ещё действия"]').trigger('keydown', { key: 'ArrowDown' })
    await flushPromises()
    await wrapper.findAll('[role="menuitem"]').find((item) => item.text() === 'Удалить')!.trigger('click')
    await flushPromises()
    expect(wrapper.text()).toContain('Удалить прокси gw1.example.net:8080?')
    expect(wrapper.text()).toContain('Профили, закреплённые за ним (2), получат случайный прокси при следующем запуске.')
    await wrapper.findAll('button').find((button) => button.text() === 'Удалить')!.trigger('click')
    await flushPromises()
    expect(api.deleteProxy).toHaveBeenCalledWith(1)
    expect(toastTitles()).toContain('Прокси используется запущенным профилем')
  })

  it('«Проверить все» — итог уведомлением', async () => {
    api.checkProxy.mockImplementation(async (id: number) => {
      if (id === 2) throw new CoreError('ProxyUnavailable', 'прокси недоступен')
      return { ip: '203.0.113.24', country: 'DE', timezone: 'Europe/Berlin' }
    })
    const wrapper = await mountView()
    await wrapper.findAll('button').find((button) => button.text() === 'Проверить все')!.trigger('click')
    await vi.waitFor(() => expect(toastTitles()).toContain('Доступны 1 из 2'))
    expect(api.checkProxy).toHaveBeenCalledTimes(2)
  })
  it('пока запись проверяется, её «Проверить» и «Проверить все» неактивны', async () => {
    let finish: (value: { ip: string; country: string; timezone: string }) => void = () => undefined
    api.checkProxy.mockImplementation(() => new Promise((resolve) => (finish = resolve)))
    const wrapper = await mountView()
    const checkAllButton = () => wrapper.findAll('button').find((button) => button.text() === 'Проверить все')!
    expect(checkAllButton().attributes('disabled')).toBeUndefined()
    const menuButton = (index: number) => wrapper.findAll('.body .row')[index]!.find('button[aria-label="Ещё действия"]')
    await menuButton(0).trigger('keydown', { key: 'ArrowDown' })
    await flushPromises()
    await wrapper.findAll('[role="menuitem"]').find((item) => item.text() === 'Проверить')!.trigger('click')
    await flushPromises()
    expect(checkAllButton().attributes('disabled')).toBeDefined()
    await menuButton(0).trigger('keydown', { key: 'ArrowDown' })
    await flushPromises()
    const item = wrapper.findAll('[role="menuitem"]').find((entry) => entry.text() === 'Проверить')!
    expect(item.attributes('data-disabled')).toBeDefined()
    finish({ ip: '203.0.113.24', country: 'DE', timezone: 'Europe/Berlin' })
    await flushPromises()
    expect(checkAllButton().attributes('disabled')).toBeUndefined()
  })
})
