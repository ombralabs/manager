import { type VueWrapper, flushPromises } from '@vue/test-utils'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { CoreError } from '../../api/errors'
import type { ApiConfig } from '../../api/types'
import { i18n } from '../../i18n'
import { mountWithApp } from '../../test/mount'

const api = vi.hoisted(() => ({
  setApiConfig: vi.fn(),
  regenerateApiKey: vi.fn(),
}))
const clipboard = vi.hoisted(() => ({ writeText: vi.fn(async () => undefined) }))
vi.mock('../../api/core', () => ({ core: api, events: {} }))
vi.mock('@tauri-apps/plugin-clipboard-manager', () => clipboard)

import { useSettingsStore } from '../../stores/settings'
import { useToasts } from '../../stores/toasts'
import ApiCard from './ApiCard.vue'

const CONFIG: ApiConfig = {
  enabled: true,
  port: 50325,
  require_key: false,
  key: null,
  status: { state: 'listening', port: 50325 },
}

function mountCard(config: ApiConfig = CONFIG) {
  const settings = useSettingsStore()
  settings.api = { ...config }
  settings.setApiStatus(config.status)
  return mountWithApp(ApiCard)
}

function button(wrapper: VueWrapper, label: string) {
  const found = wrapper.findAll('button').find((b) => b.text() === label)
  if (!found) throw new Error(`нет кнопки «${label}»`)
  return found
}

const inputValue = (wrapper: VueWrapper, selector: string) =>
  (wrapper.find(selector).element as HTMLInputElement).value
const CYRILLIC = /[А-Яа-яЁё]/

describe('карточка Local API', () => {
  beforeEach(() => {
    vi.clearAllMocks()
  })

  it('отклонённый порт и ошибка остаются после перемонтирования раздела', async () => {
    const wrapper = mountCard()
    await wrapper.get('#api-port').setValue('0')
    await wrapper.get('#api-port').trigger('keydown.enter')
    const message = wrapper.get('.error-text').text()
    wrapper.unmount()
    const reopened = mountWithApp(ApiCard)
    expect(inputValue(reopened, '#api-port')).toBe('0')
    expect(reopened.get('.error-text').text()).toBe(message)
    await reopened.get('#api-port').setValue(String(CONFIG.port))
    await reopened.get('#api-port').trigger('blur')
    expect(reopened.find('.error-text').exists()).toBe(false)
  })

  afterEach(() => {
    i18n.global.locale.value = 'ru'
  })

  it('причина отказа — переводом кода; в английском интерфейсе нет русского текста ядра', () => {
    const failed: ApiConfig = {
      ...CONFIG,
      status: { state: 'failed', port: 50325, code: 'port_busy', reason: 'порт 50325 занят' },
    }
    const ru = mountCard(failed)
    expect(ru.text()).toContain('Не удалось запустить: порт занят другой программой')
    expect(ru.text()).not.toContain('порт 50325 занят')
    ru.unmount()

    i18n.global.locale.value = 'en'
    const en = mountCard(failed)
    expect(en.text()).toContain('Could not start: the port is used by another program')
    expect(en.text()).not.toMatch(CYRILLIC)
  })

  it('адрес, состояние, пример с текущим портом и «Скопировать адрес»', async () => {
    const wrapper = mountCard()
    expect(inputValue(wrapper, '#api-address')).toBe('http://127.0.0.1:50325')
    expect(wrapper.text()).toContain('Доступен на 127.0.0.1:50325')
    expect(wrapper.text()).toContain('GET http://127.0.0.1:50325/api/v1/browser/start?user_id=k7f2q9')
    // Адрес — поле только для чтения, копирование — кнопка-значок внутри него.
    expect(wrapper.get('#api-address').attributes('readonly')).toBeDefined()
    const copy = wrapper.get('.copy-field button[aria-label="Скопировать адрес"]')
    expect(copy.text()).toBe('')
    await copy.trigger('click')
    await flushPromises()
    expect(clipboard.writeText).toHaveBeenCalledWith('http://127.0.0.1:50325')
  })

  it('порт 80 по Enter — ошибка у поля, ядро не вызывается', async () => {
    const wrapper = mountCard()
    const port = wrapper.find('#api-port')
    await port.setValue('80')
    await port.trigger('keydown.enter')
    await flushPromises()
    expect(wrapper.text()).toContain('От 1024 до 65535')
    expect(port.classes()).toContain('invalid')
    expect(api.setApiConfig).not.toHaveBeenCalled()
  })

  it('занятый порт — «Порт занят» у поля, прежний адрес остаётся, повтора при уходе из поля нет', async () => {
    api.setApiConfig.mockRejectedValue(new CoreError('Validation', 'порт занят', [{ field: 'port', code: 'busy' }]))
    const wrapper = mountCard()
    const port = wrapper.find('#api-port')
    await port.setValue('50326')
    await port.trigger('keydown.enter')
    await flushPromises()
    expect(api.setApiConfig).toHaveBeenCalledWith({ enabled: true, port: 50326, require_key: false })
    expect(wrapper.text()).toContain('Порт занят другой программой')
    expect(inputValue(wrapper, '#api-address')).toBe('http://127.0.0.1:50325')
    await port.trigger('blur')
    await flushPromises()
    expect(api.setApiConfig).toHaveBeenCalledTimes(1)
  })

  it('после «Порт занят» удачная правка другого поля не снимает пометку занятого порта', async () => {
    api.setApiConfig.mockRejectedValueOnce(new CoreError('Validation', 'порт занят', [{ field: 'port', code: 'busy' }]))
    const wrapper = mountCard()
    const port = wrapper.find('#api-port')
    await port.setValue('50326')
    await port.trigger('keydown.enter')
    await flushPromises()
    expect(wrapper.text()).toContain('Порт занят другой программой')

    api.setApiConfig.mockResolvedValueOnce({ ...CONFIG, require_key: true, key: 'k', status: CONFIG.status })
    await wrapper.find('[role="switch"][aria-label="Требовать ключ API"]').trigger('click')
    await flushPromises()
    const stillMarked = wrapper.text().includes('Порт занят другой программой')
    const showsReal = inputValue(wrapper, '#api-port') === '50325'
    expect(stillMarked || showsReal, 'занятый порт без пометки').toBe(true)
    expect(stillMarked).toBe(true)
    expect(inputValue(wrapper, '#api-port')).toBe('50326')
  })

  it('выключатель Local API сохраняется сразу', async () => {
    api.setApiConfig.mockResolvedValue({ ...CONFIG, enabled: false, status: { state: 'off' } })
    const wrapper = mountCard()
    await wrapper.find('[role="switch"][aria-label="Локальный API включён"]').trigger('click')
    await flushPromises()
    expect(api.setApiConfig).toHaveBeenCalledWith({ enabled: false, port: 50325, require_key: false })
  })

  it('«Новый ключ» — только после подтверждения', async () => {
    const withKey: ApiConfig = { ...CONFIG, require_key: true, key: 'K'.repeat(32) }
    api.regenerateApiKey.mockResolvedValue({ ...withKey, key: 'N'.repeat(32) })
    const wrapper = mountCard(withKey)
    expect(wrapper.html()).toContain('K'.repeat(32))
    await button(wrapper, 'Новый ключ').trigger('click')
    await flushPromises()
    expect(wrapper.text()).toContain('Скрипты со старым ключом перестанут работать')
    expect(api.regenerateApiKey).not.toHaveBeenCalled()
    await button(wrapper, 'Заменить ключ').trigger('click')
    await flushPromises()
    expect(api.regenerateApiKey).toHaveBeenCalledTimes(1)
  })

  it('повторный Enter на занятом порте снова уходит ядру, уход из поля — нет', async () => {
    api.setApiConfig.mockRejectedValue(new CoreError('Validation', 'порт занят', [{ field: 'port', code: 'busy' }]))
    const wrapper = mountCard()
    const port = wrapper.find('#api-port')
    await port.setValue('50326')
    await port.trigger('keydown.enter')
    await flushPromises()
    await port.trigger('blur')
    await flushPromises()
    expect(api.setApiConfig).toHaveBeenCalledTimes(1)
    api.setApiConfig.mockResolvedValue({ ...CONFIG, port: 50326, status: { state: 'listening', port: 50326 } })
    await port.trigger('keydown.enter')
    await flushPromises()
    expect(api.setApiConfig).toHaveBeenCalledTimes(2)
    expect(inputValue(wrapper, '#api-address')).toBe('http://127.0.0.1:50326')
  })

  it('нечитаемый ключ — предупреждение и «Новый ключ» с подтверждением', async () => {
    const unreadable: ApiConfig = { ...CONFIG, require_key: true, key: null }
    api.regenerateApiKey.mockResolvedValue({ ...unreadable, key: 'N'.repeat(32) })
    const wrapper = mountCard(unreadable)
    expect(wrapper.text()).toContain('Ключ API не удалось прочитать — создайте новый')
    await button(wrapper, 'Новый ключ').trigger('click')
    await flushPromises()
    expect(api.regenerateApiKey).not.toHaveBeenCalled()
    await button(wrapper, 'Заменить ключ').trigger('click')
    await flushPromises()
    expect(api.regenerateApiKey).toHaveBeenCalledTimes(1)
  })

  it('уход из «Порта» и сразу щелчок «Требовать ключ API» — оба запроса уходят по очереди', async () => {
    api.setApiConfig.mockImplementation(async (form: { port: number; require_key: boolean; enabled: boolean }) => ({
      ...CONFIG,
      ...form,
      key: form.require_key ? 'K'.repeat(32) : null,
      status: { state: 'listening', port: form.port },
    }))
    const wrapper = mountCard()
    const port = wrapper.find('#api-port')
    await port.setValue('50326')
    await port.trigger('blur')
    await wrapper.find('[role="switch"][aria-label="Требовать ключ API"]').trigger('click')
    await flushPromises()
    expect(api.setApiConfig).toHaveBeenCalledTimes(2)
    expect(api.setApiConfig).toHaveBeenNthCalledWith(1, { enabled: true, port: 50326, require_key: false })
    expect(api.setApiConfig).toHaveBeenNthCalledWith(2, { enabled: true, port: 50326, require_key: true })
  })

  it('«Порт занят» от переключателя снимается удачным повтором, когда в поле реальный порт', async () => {
    const off: ApiConfig = { ...CONFIG, enabled: false, status: { state: 'off' } }
    api.setApiConfig
      .mockRejectedValueOnce(new CoreError('Validation', 'порт занят', [{ field: 'port', code: 'busy' }]))
      .mockResolvedValueOnce(CONFIG)
    const wrapper = mountCard(off)
    const toggle = wrapper.find('[role="switch"][aria-label="Локальный API включён"]')
    await toggle.trigger('click')
    await flushPromises()
    expect(wrapper.text()).toContain('Порт занят другой программой')
    expect(inputValue(wrapper, '#api-port')).toBe('50325')

    await toggle.trigger('click')
    await flushPromises()
    expect(api.setApiConfig).toHaveBeenNthCalledWith(2, { enabled: true, port: 50325, require_key: false })
    expect(wrapper.text()).not.toContain('Порт занят другой программой')
    expect(wrapper.find('#api-port').classes()).not.toContain('invalid')
    expect(wrapper.text()).toContain('Доступен на 127.0.0.1:50325')
  })

  it('«Скопировать» копирует ключ API и сообщает об этом', async () => {
    const key = 'K'.repeat(32)
    const wrapper = mountCard({ ...CONFIG, require_key: true, key })
    await button(wrapper, 'Скопировать').trigger('click')
    await flushPromises()
    expect(clipboard.writeText).toHaveBeenCalledTimes(1)
    expect(clipboard.writeText).toHaveBeenCalledWith(key)
    expect(useToasts().items.map((toast) => toast.title)).toEqual(['Ключ скопирован'])
  })
})
