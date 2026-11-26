import { flushPromises } from '@vue/test-utils'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { ProxyImportReport } from '../../api/types'
import { mountWithApp } from '../../test/mount'

const api = vi.hoisted(() => ({
  importProxies: vi.fn(),
  listProxies: vi.fn(async () => []),
  checkProxy: vi.fn(),
}))
vi.mock('../../api/core', () => ({ core: api, events: {} }))

import { useToasts } from '../../stores/toasts'
import ImportPanel from './ImportPanel.vue'

function mountPanel() {
  return mountWithApp(ImportPanel)
}

type Wrapper = ReturnType<typeof mountPanel>
const submit = (wrapper: Wrapper) => wrapper.findAll('button').find((b) => b.text().startsWith('Импортировать'))!
const field = (wrapper: Wrapper) => wrapper.find('#proxy-import-text')

const TEXT = 'gw1.example.net:1080\ngw2.example.net:abc\ngw3.example.net:8080'

beforeEach(() => {
  api.importProxies.mockReset()
  api.checkProxy.mockReset().mockResolvedValue({ ip: '203.0.113.24', country: 'DE', timezone: 'Europe/Berlin' })
})

describe('панель «Импорт списком»', () => {
  it('кнопка считает непустые строки; без строк неактивна', async () => {
    const wrapper = mountPanel()
    expect(field(wrapper).attributes('placeholder')).toContain('socks5://u1:pw@gw9.example.net:1080')
    expect(submit(wrapper).attributes('disabled')).toBeDefined()
    await field(wrapper).setValue(
      'socks5://u1:pw@gw9.example.net:1080\n\ngw10.example.net:8080:u2:pw\nhttps://u3:pw@gw11.example.net:443\n',
    )
    expect(submit(wrapper).text()).toBe('Импортировать 3 записи')
    expect(submit(wrapper).attributes('disabled')).toBeUndefined()
  })

  it('после отчёта в поле остаются строки с ошибками, ниже — их тексты', async () => {
    api.importProxies.mockResolvedValue({ added: 2, duplicates: 1, errors: [{ line: 2, code: 'port' }], ids: [1, 2] })
    const wrapper = mountPanel()
    await field(wrapper).setValue(TEXT)
    await submit(wrapper).trigger('click')
    await flushPromises()
    expect(api.importProxies).toHaveBeenCalledWith(TEXT, 'http')
    expect((field(wrapper).element as HTMLTextAreaElement).value).toBe('gw2.example.net:abc')
    expect(wrapper.text()).toContain('Строка 1: неверный порт')
    expect(useToasts().items.map((toast) => [toast.title, toast.body])).toEqual([
      ['Добавлено 2 записи', 'Повторов: 1 — они пропущены'],
    ])
    expect(api.checkProxy.mock.calls.map(([id]) => id)).toEqual([1, 2])
  })

  it('без «Проверить после импорта» записи не проверяются, поле очищается', async () => {
    api.importProxies.mockResolvedValue({ added: 3, duplicates: 0, errors: [], ids: [1, 2, 3] })
    const wrapper = mountPanel()
    await wrapper.find('input[type="checkbox"]').setValue(false)
    await field(wrapper).setValue('gw1.example.net:1080')
    await submit(wrapper).trigger('click')
    await flushPromises()
    expect(api.checkProxy).not.toHaveBeenCalled()
    expect((field(wrapper).element as HTMLTextAreaElement).value).toBe('')
  })

  it('поле заблокировано на время импорта, ошибки — по отправленному тексту', async () => {
    let answer: (report: ProxyImportReport) => void = () => {}
    api.importProxies.mockImplementation(() => new Promise<ProxyImportReport>((resolve) => (answer = resolve)))
    const wrapper = mountPanel()
    await wrapper.find('input[type="checkbox"]').setValue(false)
    await field(wrapper).setValue(TEXT)
    await submit(wrapper).trigger('click')
    expect(field(wrapper).attributes('disabled')).toBeDefined()
    await field(wrapper).setValue(`gw0.example.net:1080\n${TEXT}`)
    answer({ added: 2, duplicates: 0, errors: [{ line: 2, code: 'port' }], ids: [1, 3] })
    await flushPromises()
    expect(api.importProxies).toHaveBeenCalledTimes(1)
    expect(api.importProxies).toHaveBeenCalledWith(TEXT, 'http')
    expect(field(wrapper).attributes('disabled')).toBeUndefined()
    expect((field(wrapper).element as HTMLTextAreaElement).value).toBe('gw2.example.net:abc')
    expect(wrapper.find('.line-errors').text()).toBe('Строка 1: неверный порт')
  })
})
