import { type VueWrapper, flushPromises } from '@vue/test-utils'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { ProxyRecord, ProxyRow } from '../../api/types'
import type { SelectOption } from '../../components/types'
import { fingerprint, proxyRow } from '../../test/fixtures'
import { emptyModel, type EditorModel } from '../../lib/profileForm'
import { mountWithApp } from '../../test/mount'

const api = vi.hoisted(() => ({
  listProxies: vi.fn(async (): Promise<ProxyRow[]> => []),
  checkConnection: vi.fn(async () => ({ ip: '203.0.113.24', country: 'DE', timezone: 'Europe/Berlin' })),
}))
vi.mock('../../api/core', () => ({ core: api, events: {} }))

import ProxyDialog from '../../components/ProxyDialog.vue'
import UiSelect from '../../components/UiSelect.vue'
import { useProxiesStore } from '../../stores/proxies'
import GatewayTab from './GatewayTab.vue'

const proxy = (id: number, patch: Partial<ProxyRow> = {}): ProxyRow =>
  proxyRow({
    id,
    kind: 'socks5',
    host: `gw${id}.example.net`,
    port: 1080,
    username: 'secret-login',
    has_password: true,
    ...patch,
  })

const ROWS: ProxyRow[] = [
  proxy(7, {
    check: { at: 1_790_000_000, ok: true, exit_ip: '203.0.113.24', country: 'DE' },
    profiles: 1,
    used_by: 'self01',
    used_by_name: 'Магазин',
    pinned_to: 'Магазин',
  }),
  proxy(8, { profiles: 1, used_by: 'other1', used_by_name: 'Почта 02', pinned_to: 'Почта 02' }),
  proxy(9, { enabled: false }),
]

interface TabProps {
  modelValue: number | null
  /** По умолчанию — как `modelValue`: выбран сохранённый прокси. */
  savedProxyId?: number | null
  running?: boolean
  replacedAt?: number | null
}

function mountTab(props: TabProps) {
  useProxiesStore().rows = ROWS.map((row) => ({ ...row }))
  api.listProxies.mockResolvedValue(ROWS)
  const model = emptyModel(structuredClone(fingerprint))
  model.proxyId = props.modelValue
  model.savedProxyId = props.savedProxyId === undefined ? props.modelValue : props.savedProxyId
  return mountWithApp(GatewayTab, {
    props: { profileId: 'self01', running: props.running ?? false, replacedAt: props.replacedAt ?? null, errors: [], modelValue: model },
    global: { stubs: { ProxyDialog: true } },
  })
}

function button(wrapper: VueWrapper, label: string) {
  const found = wrapper.findAll('button').find((b) => b.text() === label)
  if (!found) throw new Error(`нет кнопки «${label}»`)
  return found
}

describe('вкладка «Шлюз»', () => {
  it.each([['ip2location', 'IP2Location'], ['ip-api', 'ip-api']])('новый IP-чекер %s доступен и передаётся при проверке', async (value, label) => {
    const wrapper = mountTab({ modelValue: 7 })
    const select = wrapper.findAllComponents(UiSelect).at(-1)!
    expect((select.vm.$props as { options: SelectOption[] }).options).toContainEqual({ value, label })
    select.vm.$emit('update:modelValue', value)
    await flushPromises()
    await button(wrapper, 'Проверить соединение').trigger('click')
    await flushPromises()
    expect(api.checkConnection).toHaveBeenCalledWith({ mode: 'saved', id: 7 }, value)
    expect((wrapper.props('modelValue') as EditorModel).network.ip_checker).toBe(value)
  })

  it('прямое соединение и выбранный сервис используются в реальном запросе проверки', async () => {
    const wrapper = mountTab({ modelValue: 7 })
    await button(wrapper, 'Без прокси').trigger('click')
    expect(wrapper.find('#gateway-proxy').exists()).toBe(false)
    wrapper.findAllComponents(UiSelect).at(-1)!.vm.$emit('update:modelValue', 'ipapi')
    await flushPromises()
    await button(wrapper, 'Проверить соединение').trigger('click')
    await flushPromises()
    expect(api.checkConnection).toHaveBeenCalledWith({ mode: 'direct' }, 'ipapi')
    expect(wrapper.get('[role="status"]').text()).toContain('203.0.113.24')
    expect((wrapper.props('modelValue') as EditorModel).network).toEqual({ direct: true, ip_checker: 'ipapi' })
  })

  it('ручной прокси проверяется из формы без записи в список, пароль сохраняется по ссылке на исходный прокси', async () => {
    const wrapper = mountTab({ modelValue: 7 })
    await button(wrapper, 'Настроить').trigger('click')
    expect(wrapper.get<HTMLInputElement>('#profile-proxy-host').element.value).toBe('gw7.example.net')
    await wrapper.get('#profile-proxy-host').setValue('new.proxy.example')
    await wrapper.get('#profile-proxy-port').setValue('8080')
    await button(wrapper, 'Проверить соединение').trigger('click')
    await flushPromises()
    expect(api.checkConnection).toHaveBeenCalledWith({ mode: 'manual', source_id: 7, proxy: expect.objectContaining({ host: 'new.proxy.example', port: 8080, password: { action: 'keep' } }) }, 'auto')
    expect((wrapper.props('modelValue') as EditorModel).proxyId).toBe(7)
  })

  beforeEach(() => {
    vi.clearAllMocks()
  })

  it('подписи списка: «Случайный», занятый другим профилем, выключенный; логина нет', () => {
    const wrapper = mountTab({ modelValue: null })
    const labels = (wrapper.findComponent(UiSelect).vm.$props as { options: SelectOption[] }).options.map((option) => option.label)
    expect(labels[0]).toBe('Случайный при первом запуске')
    expect(labels.find((label) => label.includes('gw7.example.net'))).not.toContain('занят')
    expect(labels.find((label) => label.includes('gw8.example.net'))).toContain('занят: Почта 02')
    expect(labels.find((label) => label.includes('gw9.example.net'))).toContain('выключен')
    expect(wrapper.html()).not.toContain('secret-login')
    expect(wrapper.text()).not.toContain('Закреплённый прокси')
  })

  it('подсказка про случайный прокси — только когда прокси не выбран', () => {
    const hint = 'получит случайный включённый прокси'
    expect(mountTab({ modelValue: null }).text()).toContain(hint)
    expect(mountTab({ modelValue: 7 }).text()).not.toContain(hint)
  })

  it('выбор пункта и «Сбросить» меняют прокси профиля', async () => {
    const wrapper = mountTab({ modelValue: 7 })
    wrapper.findComponent(UiSelect).vm.$emit('update:modelValue', '8')
    await button(wrapper, 'Сбросить').trigger('click')
    expect((wrapper.props('modelValue') as EditorModel).proxyId).toBeNull()
  })

  it('карточка закреплённого прокси и «Проверить» через список прокси', async () => {
    const wrapper = mountTab({ modelValue: 7 })
    expect(wrapper.text()).toContain('Закреплённый прокси')
    expect(wrapper.text()).toContain('DE · 203.0.113.24')
    expect(wrapper.text()).toContain('Доступен')
    await button(wrapper, 'Проверить соединение').trigger('click')
    await flushPromises()
    expect(api.checkConnection).toHaveBeenCalledWith({ mode: 'saved', id: 7 }, 'auto')
  })

  it('работающий профиль и заменённый шлюз — предупреждения', () => {
    const busy = mountTab({ modelValue: 7, running: true, replacedAt: 1_790_000_000 })
    expect(busy.text()).toContain('Изменения применятся при следующем запуске профиля.')
    expect(busy.text()).toContain('При последнем запуске недоступный прокси был заменён.')
    const idle = mountTab({ modelValue: 7 })
    expect(idle.text()).not.toContain('Изменения применятся при следующем запуске профиля.')
    expect(idle.text()).not.toContain('Шлюз заменён')
  })

  it('«Добавить прокси…» открывает общий диалог, новая запись выбирается', async () => {
    const wrapper = mountTab({ modelValue: null })
    const dialog = wrapper.findComponent(ProxyDialog)
    expect(dialog.props('open')).toBe(false)
    await button(wrapper, 'Добавить прокси…').trigger('click')
    expect(dialog.props('open')).toBe(true)
    const created: ProxyRecord = {
      id: 12,
      kind: 'http',
      host: 'gw12.example.net',
      port: 8080,
      username: null,
      has_password: false,
      note: '',
      enabled: true,
      check: null,
    }
    dialog.vm.$emit('saved', created)
    await flushPromises()
    expect((wrapper.props('modelValue') as EditorModel).proxyId).toBe(12)
    expect(api.listProxies).toHaveBeenCalled()
  })

  it('до сохранения карточка — «Выбранный прокси», плашка о замене — только у закреплённого', () => {
    const changed = mountTab({ modelValue: 8, savedProxyId: 7, replacedAt: 1_790_000_000 })
    expect(changed.text()).toContain('Выбранный прокси')
    expect(changed.text()).not.toContain('Закреплённый прокси')
    expect(changed.text()).not.toContain('Шлюз заменён')
    const random = mountTab({ modelValue: null, savedProxyId: 7, replacedAt: 1_790_000_000 })
    expect(random.text()).not.toContain('Шлюз заменён')
    const fresh = mountTab({ modelValue: 7, savedProxyId: null })
    expect(fresh.text()).toContain('Выбранный прокси')
    const kept = mountTab({ modelValue: 7, savedProxyId: 7, replacedAt: 1_790_000_000 })
    expect(kept.text()).toContain('Закреплённый прокси')
    expect(kept.text()).toContain('При последнем запуске недоступный прокси был заменён.')
  })
})
