import { flushPromises } from '@vue/test-utils'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { mountWithApp } from '../../test/mount'

const api = vi.hoisted(() => ({
  listProxies: vi.fn(async () => [
    { id: 4, kind: 'socks5', host: 'p.example', port: 1080 },
    { id: 9, kind: 'http', host: 'q.example', port: 3128 },
  ]),
  setServicesProxy: vi.fn(async () => null),
  setGoogleKey: vi.fn(async () => 'bot@p.iam.gserviceaccount.com'),
  clearGoogleKey: vi.fn(async () => null),
}))
const dialogs = vi.hoisted(() => ({ pickGoogleKeyFile: vi.fn(async () => '/tmp/key.json') }))
vi.mock('../../api/dialogs', () => dialogs)
vi.mock('../../api/core', () => ({ core: api, events: {} }))

import UiSelect from '../../components/UiSelect.vue'
import { useSettingsStore } from '../../stores/settings'
import AutomationCard from './AutomationCard.vue'

async function mountCard(saved: number | null) {
  useSettingsStore().servicesProxy = saved
  const wrapper = mountWithApp(AutomationCard)
  await flushPromises()
  return wrapper
}

describe('карточка «Автоматизация»', () => {
  beforeEach(() => vi.clearAllMocks())

  it('показывает выбранный прокси, выбор сохраняет через стор, «Нет» сбрасывает', async () => {
    const wrapper = await mountCard(4)
    const select = wrapper.findComponent(UiSelect)
    expect(wrapper.get('#services-proxy').text()).toContain('socks5://p.example:1080')
    expect(wrapper.text()).toContain('почты, OpenAI, 2Captcha и Google Sheets')
    expect(wrapper.text()).not.toContain('Прокси удалён')
    select.vm.$emit('update:modelValue', '9')
    await flushPromises()
    expect(api.setServicesProxy).toHaveBeenLastCalledWith(9)
    expect(useSettingsStore().servicesProxy).toBe(9)
    select.vm.$emit('update:modelValue', 'none')
    expect(api.setServicesProxy).toHaveBeenLastCalledWith(null)
  })

  it('удалённый прокси: предупреждение, поле на «Нет», ничего не сохранено', async () => {
    const wrapper = await mountCard(77)
    expect(wrapper.text()).toContain('Прокси удалён — выберите другой')
    expect(wrapper.get('#services-proxy').text()).toContain('— выберите —')
    expect(api.setServicesProxy).not.toHaveBeenCalled()
  })

  it('удалённый прокси и пустой список: «Нет» сохраняет null, предупреждение исчезает', async () => {
    api.listProxies.mockResolvedValueOnce([])
    const wrapper = await mountCard(77)
    expect(wrapper.text()).toContain('Прокси удалён')
    wrapper.findComponent(UiSelect).vm.$emit('update:modelValue', 'none')
    await flushPromises()
    expect(api.setServicesProxy).toHaveBeenCalledWith(null)
    expect(wrapper.text()).not.toContain('Прокси удалён')
  })

  it('повреждённая настройка: ошибка у поля, выбор «Нет» её снимает', async () => {
    useSettingsStore().servicesProxyError = 'повреждена'
    const wrapper = await mountCard(null)
    expect(wrapper.text()).toContain('Настройка прокси для сервисов повреждена — выберите заново')
    expect(wrapper.get('#services-proxy').text()).toContain('— выберите —')
    wrapper.findComponent(UiSelect).vm.$emit('update:modelValue', 'none')
    await flushPromises()
    expect(api.setServicesProxy).toHaveBeenCalledWith(null)
    expect(wrapper.text()).not.toContain('повреждена')
  })

  it('ключ Google не расшифровался: так и сказано, «Удалить» доступно', async () => {
    const settings = useSettingsStore()
    settings.googleEmail = null
    settings.googleKeyBroken = true
    const wrapper = await mountCard(null)
    expect(wrapper.text()).toContain('Не удалось прочитать ключ Google. Загрузите файл ключа заново.')
    await wrapper.findAll('button').find((b) => b.text() === 'Удалить')!.trigger('click')
    await flushPromises()
    expect(api.clearGoogleKey).toHaveBeenCalled()
    expect(wrapper.text()).not.toContain('не расшифровался')
  })

  it('кнопка загрузки ключа занята, пока ключ загружается', async () => {
    let done: (email: string) => void = () => {}
    api.setGoogleKey.mockImplementationOnce(() => new Promise<string>((resolve) => { done = resolve }))
    useSettingsStore().googleEmail = null
    const wrapper = await mountCard(null)
    const load = () => wrapper.findAll('button').find((b) => b.text().includes('Загрузить ключ JSON'))!
    await load().trigger('click')
    await flushPromises()
    expect(load().attributes('aria-disabled')).toBe('true')
    await load().trigger('click')
    expect(dialogs.pickGoogleKeyFile).toHaveBeenCalledTimes(1)
    done('bot@p.iam.gserviceaccount.com')
    await flushPromises()
    expect(load().attributes('aria-disabled')).toBeUndefined()
  })

  it('ключ Google: загрузка показывает адрес, «Удалить» очищает', async () => {
    useSettingsStore().googleEmail = null
    const wrapper = await mountCard(null)
    expect(wrapper.text()).not.toContain('Удалить')
    const load = wrapper.findAll('button').find((b) => b.text().includes('Загрузить ключ JSON'))!
    await load.trigger('click')
    await flushPromises()
    expect(api.setGoogleKey).toHaveBeenCalledWith('/tmp/key.json')
    expect(wrapper.text()).toContain('bot@p.iam.gserviceaccount.com')
    await wrapper.findAll('button').find((b) => b.text() === 'Удалить')!.trigger('click')
    await flushPromises()
    expect(api.clearGoogleKey).toHaveBeenCalled()
    expect(wrapper.text()).not.toContain('bot@p.iam')
  })
})
