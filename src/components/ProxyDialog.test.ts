import { flushPromises } from '@vue/test-utils'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { CoreError } from '../api/errors'
import type { ProxyRecord } from '../api/types'
import { type ProxyEditDraft, emptyProxyDraft } from '../lib/proxies'
import { mountWithApp } from '../test/mount'

const api = vi.hoisted(() => ({
  createProxy: vi.fn(),
  updateProxy: vi.fn(),
  listProxies: vi.fn(async () => []),
}))
vi.mock('../api/core', () => ({ core: api, events: {} }))

import ProxyDialog from './ProxyDialog.vue'

const saved: ProxyRecord = {
  id: 7,
  kind: 'socks5',
  host: 'gw1.example.net',
  port: 1080,
  username: 'user',
  has_password: true,
  note: 'основной',
  enabled: true,
  check: null,
}

function mountDialog(proxy: ProxyRecord | null) {
  return mountWithApp(ProxyDialog, {
    props: { open: true, proxy },
  })
}

type Wrapper = ReturnType<typeof mountDialog>
const button = (wrapper: Wrapper, text: string) => wrapper.findAll('button').find((b) => b.text() === text)!
/** Черновик диалога: у закрытого диалога полей в DOM нет, поэтому смотрим состояние компонента. */
const draftOf = (wrapper: Wrapper) => (wrapper.vm as unknown as { draft: ProxyEditDraft }).draft

beforeEach(() => {
  api.createProxy.mockReset()
  api.updateProxy.mockReset()
})

describe('диалог прокси', () => {
  it('новая запись: createProxy с формой, событие saved, диалог закрывается', async () => {
    api.createProxy.mockResolvedValue({ ...saved, id: 9 })
    const wrapper = mountDialog(null)
    await flushPromises()
    expect(wrapper.text()).toContain('Добавить прокси')
    await wrapper.find('#proxy-dialog-host').setValue(' gw9.example.net ')
    await wrapper.find('#proxy-dialog-port').setValue('1080')
    await wrapper.find('#proxy-dialog-username').setValue('u1')
    await wrapper.find('#proxy-dialog-password').setValue('pw')
    await button(wrapper, 'Добавить').trigger('click')
    await flushPromises()
    expect(api.createProxy).toHaveBeenCalledWith({
      kind: 'http',
      host: 'gw9.example.net',
      port: 1080,
      username: 'u1',
      password: { action: 'set', value: 'pw' },
      note: '',
      enabled: true,
    })
    expect(wrapper.emitted('saved')).toEqual([[{ ...saved, id: 9 }]])
    expect(wrapper.emitted('update:open')).toEqual([[false]])
  })

  it('правка с пустым паролем оставляет сохранённый', async () => {
    api.updateProxy.mockResolvedValue(saved)
    const wrapper = mountDialog(saved)
    await flushPromises()
    expect(wrapper.text()).toContain('Изменить прокси')
    expect((wrapper.find('#proxy-dialog-username').element as HTMLInputElement).value).toBe('user')
    await button(wrapper, 'Сохранить').trigger('click')
    await flushPromises()
    expect(api.updateProxy).toHaveBeenCalledWith(7, {
      kind: 'socks5',
      host: 'gw1.example.net',
      port: 1080,
      username: 'user',
      password: { action: 'keep' },
      note: 'основной',
      enabled: true,
    })
  })

  it('ошибка поля от ядра — у поля, диалог остаётся открытым', async () => {
    api.createProxy.mockRejectedValue(
      new CoreError('Validation', 'проверьте поля формы', [{ field: 'proxy_port', code: 'out_of_range' }]),
    )
    const wrapper = mountDialog(null)
    await flushPromises()
    await wrapper.find('#proxy-dialog-host').setValue('gw9.example.net')
    await wrapper.find('#proxy-dialog-port').setValue('1080')
    await button(wrapper, 'Добавить').trigger('click')
    await flushPromises()
    expect(wrapper.text()).toContain('От 1 до 65535')
    expect(wrapper.emitted('update:open')).toBeUndefined()
  })

  it('порт вне 1–65535 в ядро не уходит', async () => {
    const wrapper = mountDialog(null)
    await flushPromises()
    await wrapper.find('#proxy-dialog-host').setValue('gw9.example.net')
    await wrapper.find('#proxy-dialog-port').setValue('70000')
    await button(wrapper, 'Добавить').trigger('click')
    await flushPromises()
    expect(api.createProxy).not.toHaveBeenCalled()
    expect(wrapper.text()).toContain('От 1 до 65535')
  })

  it('закрытие стирает черновик: введённый пароль не остаётся в памяти', async () => {
    const wrapper = mountDialog(null)
    await flushPromises()
    await wrapper.find('#proxy-dialog-host').setValue('gw9.example.net')
    await wrapper.find('#proxy-dialog-password').setValue('secret')
    expect(draftOf(wrapper).password.value).toBe('secret')
    await wrapper.setProps({ open: false })
    await flushPromises()
    expect(draftOf(wrapper)).toEqual(emptyProxyDraft())
    await wrapper.setProps({ open: true })
    await flushPromises()
    expect((wrapper.find('#proxy-dialog-password').element as HTMLInputElement).value).toBe('')
    expect((wrapper.find('#proxy-dialog-host').element as HTMLInputElement).value).toBe('')
  })

  it('правка поля снимает только его ошибку', async () => {
    api.createProxy.mockRejectedValue(
      new CoreError('Validation', 'проверьте поля формы', [
        { field: 'proxy_port', code: 'out_of_range' },
        { field: 'proxy_host', code: 'invalid' },
      ]),
    )
    const wrapper = mountDialog(null)
    await flushPromises()
    await wrapper.find('#proxy-dialog-host').setValue('gw9.example.net')
    await wrapper.find('#proxy-dialog-port').setValue('1080')
    await button(wrapper, 'Добавить').trigger('click')
    await flushPromises()
    expect(wrapper.find('#proxy-dialog-port').classes()).toContain('invalid')
    expect(wrapper.find('#proxy-dialog-host').classes()).toContain('invalid')
    await wrapper.find('#proxy-dialog-port').setValue('1081')
    await flushPromises()
    expect(wrapper.find('#proxy-dialog-port').classes()).not.toContain('invalid')
    expect(wrapper.text()).not.toContain('От 1 до 65535')
    expect(wrapper.find('#proxy-dialog-host').classes()).toContain('invalid')
    expect(wrapper.text()).toContain('Неверное значение')
  })
})
