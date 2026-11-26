import { enableAutoUnmount, flushPromises } from '@vue/test-utils'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { createMemoryHistory, createRouter } from 'vue-router'
import { CoreError } from '../api/errors'
import type { FieldError, Fingerprint, FingerprintOptions, ProfileDetails, ProfileForm } from '../api/types'
import { mountWithApp } from '../test/mount'

const fp: Fingerprint = {
  seed: 48213,
  os: 'macos',
  os_version: '15.2.0',
  brand: 'Chrome',
  gpu: { vendor: 'Google Inc. (Apple)', renderer: 'ANGLE (Apple, ANGLE Metal Renderer: Apple M2, Unspecified Version)' },
  hardware_concurrency: 8,
  device_memory: 16,
  screen: { width: 1512, height: 982 },
  timezone: { mode: 'auto' },
  language: { mode: 'auto' },
  webrtc: { mode: 'auto' },
  noise: true,
  storage_quota_mb: null,
  device_pixel_ratio: 2,
  max_touch_points: 0,
  location: { mode: 'off' },
  manual: [],
}

const OPTIONS: FingerprintOptions = {
  screens: [fp.screen],
  gpus: [fp.gpu],
  hardware_concurrency: [8],
  device_memory: [16],
  engine_version: '150.0.7871.114',
}

const api = vi.hoisted(() => ({
  fingerprintOptions: vi.fn(),
  newFingerprint: vi.fn(),
  validateFingerprint: vi.fn(async (): Promise<FieldError[]> => []),
  createProfile: vi.fn(),
  getProfile: vi.fn(),
  updateProfile: vi.fn(),
  totpCode: vi.fn(),
  trafficSummary: vi.fn(),
  connectionLog: vi.fn(),
  listProxies: vi.fn(async () => []),
  listExtensions: vi.fn(async () => []),
  listExtCategories: vi.fn(async () => [{ id: 0, name: 'Общий', remark: '', extension_ids: [], profile_count: 0 }]),
  listGroups: vi.fn(async () => ({ groups: [], ungrouped: 0 })),
  listTags: vi.fn(async () => []),
  listProfiles: vi.fn(async () => ({ rows: [], total: 0, running: 0 })),
}))
vi.mock('../api/core', () => ({ core: api, events: {} }))

import ProfileEditorView from './ProfileEditorView.vue'
import ConfirmDialog from '../components/ConfirmDialog.vue'
enableAutoUnmount(afterEach)

async function mountEditor(path = '/profiles/new') {
  const router = createRouter({
    history: createMemoryHistory(),
    routes: [
      { path: '/profiles', component: { template: '<div />' } },
      { path: '/profiles/new', component: ProfileEditorView },
      { path: '/profiles/:id', component: ProfileEditorView },
    ],
  })
  await router.push(path)
  await router.isReady()
  const wrapper = mountWithApp({ template: '<RouterView />' }, { global: { plugins: [router] } })
  await flushPromises()
  return wrapper.getComponent(ProfileEditorView)
}

type EditorWrapper = Awaited<ReturnType<typeof mountEditor>>

function button(wrapper: EditorWrapper, label: string) {
  const found = wrapper.findAll('button').find((b) => b.text() === label)
  if (!found) throw new Error(`нет кнопки «${label}»`)
  return found
}

async function openTab(wrapper: EditorWrapper, label: string) {
  const trigger = wrapper.findAll('[role="tab"]').find((tab) => tab.text() === label)
  if (!trigger) throw new Error(`нет вкладки «${label}»`)
  await trigger.trigger('mousedown', { button: 0 })
  await flushPromises()
}

/** Точка ошибок на вкладке. */
function tabDot(wrapper: EditorWrapper, label: string): boolean {
  const trigger = wrapper.findAll('[role="tab"]').find((tab) => tab.text() === label)
  if (!trigger) throw new Error(`нет вкладки «${label}»`)
  return trigger.find('.dot').exists()
}

describe('редактор профиля', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    api.fingerprintOptions.mockResolvedValue(OPTIONS)
    api.newFingerprint.mockImplementation(async () => structuredClone(fp))
  })

  it('у нового профиля нет вкладки «Трафик»', async () => {
    const wrapper = await mountEditor()
    expect(wrapper.findAll('[role="tab"]').map((tab) => tab.text())).not.toContain('Трафик')
  })

  it('ошибка поля отпечатка от ядра видна во вкладке и снимается правкой поля', async () => {
    api.createProfile.mockRejectedValueOnce(
      new CoreError('Validation', 'проверьте поля', [{ field: 'os_version', code: 'invalid' }]),
    )
    const wrapper = await mountEditor()
    await wrapper.find('#name').setValue('Магазин')
    await button(wrapper, 'Создать профиль').trigger('click')
    await flushPromises()
    expect(api.createProfile).toHaveBeenCalledTimes(1)
    const osVersion = wrapper.find('#os-version')
    expect(osVersion.exists()).toBe(true)
    expect(osVersion.classes()).toContain('invalid')
    expect(wrapper.text()).toContain('Неверное значение')
    await osVersion.setValue('15.3')
    await flushPromises()
    expect(wrapper.find('#os-version').classes()).not.toContain('invalid')
  })

  it('пустой seed — «Обязательное поле» до ядра, поле остаётся пустым', async () => {
    const wrapper = await mountEditor()
    await wrapper.find('#name').setValue('Магазин')
    await openTab(wrapper, 'Отпечаток')
    await wrapper.find('#seed').setValue('')
    await button(wrapper, 'Создать профиль').trigger('click')
    await flushPromises()
    expect(api.createProfile).not.toHaveBeenCalled()
    const seed = wrapper.find('#seed')
    expect((seed.element as HTMLInputElement).value).toBe('')
    expect(seed.classes()).toContain('invalid')
    expect(wrapper.text()).toContain('Обязательное поле')
    await seed.setValue('77')
    await flushPromises()
    expect(wrapper.find('#seed').classes()).not.toContain('invalid')
  })

  it('ошибка поля, которого нет в форме, остаётся до следующего сохранения', async () => {
    api.createProfile
      .mockRejectedValueOnce(new CoreError('Validation', 'проверьте поля', [{ field: 'unknown_field', code: 'invalid' }]))
      .mockRejectedValueOnce(new CoreError('Validation', 'проверьте поля', [{ field: 'name', code: 'too_long' }]))
    const wrapper = await mountEditor()
    await wrapper.find('#name').setValue('Магазин')
    await button(wrapper, 'Создать профиль').trigger('click')
    await flushPromises()
    expect(tabDot(wrapper, 'Отпечаток')).toBe(true)
    await wrapper.find('#seed').setValue('77')
    await flushPromises()
    expect(tabDot(wrapper, 'Отпечаток')).toBe(true)
    await button(wrapper, 'Создать профиль').trigger('click')
    await flushPromises()
    expect(api.createProfile).toHaveBeenCalledTimes(2)
    expect(tabDot(wrapper, 'Отпечаток')).toBe(false)
    expect(tabDot(wrapper, 'Основное')).toBe(true)
  })

  it('правка одного поля не снимает ошибку другого', async () => {
    api.createProfile.mockRejectedValueOnce(
      new CoreError('Validation', 'проверьте поля', [
        { field: 'name', code: 'too_long' },
        { field: 'tags', code: 'invalid' },
      ]),
    )
    const wrapper = await mountEditor()
    await wrapper.find('#name').setValue('Магазин')
    await button(wrapper, 'Создать профиль').trigger('click')
    await flushPromises()
    expect(wrapper.text()).toContain('Слишком длинное значение')
    expect(wrapper.text()).toContain('Тег — не длиннее 30 символов')
    await wrapper.find('#name').setValue('Магазин 2')
    await flushPromises()
    expect(wrapper.find('#name').classes()).not.toContain('invalid')
    expect(wrapper.text()).not.toContain('Слишком длинное значение')
    expect(wrapper.text()).toContain('Тег — не длиннее 30 символов')
  })

  it('ошибка ядра поля отпечатка ставит точку на «Отпечатке», пока открыта другая вкладка', async () => {
    api.createProfile.mockRejectedValueOnce(
      new CoreError('Validation', 'проверьте поля', [
        { field: 'name', code: 'too_long' },
        { field: 'gpu', code: 'arch_mismatch' },
      ]),
    )
    const wrapper = await mountEditor()
    await wrapper.find('#name').setValue('Магазин')
    await button(wrapper, 'Создать профиль').trigger('click')
    await flushPromises()
    expect(wrapper.find('#name').exists()).toBe(true)
    expect([tabDot(wrapper, 'Основное'), tabDot(wrapper, 'Отпечаток'), tabDot(wrapper, 'Подключение')]).toEqual([
      true,
      true,
      false,
    ])
  })

  it('сохранённый профиль: «Трафик» есть; сохранение без правок после обхода вкладок ничего не теряет', async () => {
    const saved = {
      ...structuredClone(fp),
      location: { mode: 'manual' as const, lat: 52.52, lon: 13.405 },
      storage_quota_mb: 5000,
      device_pixel_ratio: 3,
      manual: ['location', 'storage_quota_mb', 'device_pixel_ratio'] as Fingerprint['manual'],
    }
    const bookmarks = [{ title: 'Shop', url: 'https://shop.example/' }]
    api.getProfile.mockResolvedValue({
      profile: {
        id: 'p1', serial: 1, name: 'Магазин', note: '', group_id: null, platform_domain: '', start_urls: [], extra_args: [],
        username: 'shop', has_password: true, has_totp: true, fingerprint_json: '{}', proxy_id: null, created_at: 1,
        last_opened_at: null, restore_tabs: true, gateway_replaced_at: null, deleted_at: null, ext_category_id: 0,
        humanize: { enabled: true, preset: 'careful', block_input: true, paste_key: 'Ctrl+Alt+V' }, humanize_seed: '42',
      },
      fingerprint: saved,
      proxy: null,
      tags: [],
      bookmarks,
      persona: { generator_version: 1, keys_per_sec: 5, typo_rate: 0.01, dwell_ms: 100, mouse_speed: 1, scroll_device: 'wheel' },
    } satisfies ProfileDetails)
    api.updateProfile.mockResolvedValue({})
    api.totpCode.mockResolvedValue({ code: '287082', seconds_left: 20 })
    api.trafficSummary.mockResolvedValue({ bytes_up: 0, bytes_down: 0, connections: 0, since: null, live: false })
    api.connectionLog.mockResolvedValue([])
    const wrapper = await mountEditor('/profiles/p1')
    expect(wrapper.findAll('[role="tab"]').map((tab) => tab.text())).toContain('Трафик')
    for (const label of ['Отпечаток', 'Запуск', 'Поведение ввода', 'Трафик', 'Основное']) await openTab(wrapper, label)
    await button(wrapper, 'Сохранить').trigger('click')
    await flushPromises()
    expect(api.updateProfile).toHaveBeenCalledTimes(1)
    const [id, form] = api.updateProfile.mock.calls[0] as unknown as [string, ProfileForm]
    expect(id).toBe('p1')
    expect(form).toMatchObject({ totp: { action: 'keep' }, password: { action: 'keep' }, bookmarks })
    expect(form.humanize).toEqual({ enabled: true, preset: 'careful', block_input: true, paste_key: 'Ctrl+Alt+V' })
    expect(form.fingerprint).toMatchObject({
      location: { mode: 'manual', lat: 52.52, lon: 13.405 },
      storage_quota_mb: 5000,
      device_pixel_ratio: 3,
      manual: ['location', 'storage_quota_mb', 'device_pixel_ratio'],
    })
    expect('proxy_id' in form).toBe(false)
  })

  it('набор удалён, пока открыт редактор: ошибка ядра — точка на «Запуске» и текст под полем набора', async () => {
    api.createProfile.mockRejectedValueOnce(new CoreError('Validation', 'проверьте поля', [{ field: 'ext_category_id', code: 'invalid' }]))
    const wrapper = await mountEditor()
    expect(api.listExtCategories).toHaveBeenCalled()
    await wrapper.find('#name').setValue('Магазин')
    await button(wrapper, 'Создать профиль').trigger('click')
    await flushPromises()
    expect(tabDot(wrapper, 'Запуск')).toBe(true)
    await openTab(wrapper, 'Запуск')
    expect(wrapper.text()).toContain('Набор удалён, выберите другой')
  })
})


const drawerDetails = (): ProfileDetails => ({
  profile: {
    id: 'p1', serial: 1, name: 'Магазин', note: 'Заметка', group_id: null, platform_domain: 'example.com',
    start_urls: ['https://example.com'], extra_args: [], username: 'shop', has_password: true, has_totp: false,
    fingerprint_json: '{}', proxy_id: 7, created_at: 1, last_opened_at: null, restore_tabs: true,
    gateway_replaced_at: null, deleted_at: null, ext_category_id: 0,
    humanize: { enabled: true, preset: 'careful', block_input: true, paste_key: 'Ctrl+Alt+V' }, humanize_seed: '42',
  },
  fingerprint: structuredClone(fp), proxy: null, tags: ['Работа'],
  bookmarks: [{ title: 'Shop', url: 'https://example.com' }],
  persona: { generator_version: 1, keys_per_sec: 5, typo_rate: 0.01, dwell_ms: 100, mouse_speed: 1, scroll_device: 'wheel' },
})

describe('отдельная страница редактирования', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    api.fingerprintOptions.mockResolvedValue(OPTIONS)
    api.getProfile.mockImplementation(async () => drawerDetails())
    api.updateProfile.mockResolvedValue({})
  })

  const drawer = () => mountEditor('/profiles/p1')

  it('сохраняет основные поля, сохраняя скрытые настройки и пароли', async () => {
    const wrapper = await drawer()
    expect(wrapper.findAll('[role="tab"]').map(el => el.text())).toEqual(['Основное', 'Подключение', 'Отпечаток', 'Запуск', 'Cookies', 'Поведение ввода', 'Трафик'])
    expect(wrapper.find('.summary').exists()).toBe(true)
    await wrapper.get('#name').setValue('Рабочий · 01')
    await wrapper.get('#note').setValue('Новая заметка')
    expect(wrapper.text()).toContain('Изменения не сохранены')
    await button(wrapper, 'Сохранить').trigger('click')
    await flushPromises()
    expect(api.updateProfile).toHaveBeenCalledWith('p1', expect.objectContaining({
      name: 'Рабочий · 01', note: 'Новая заметка', tags: ['Работа'],
      password: { action: 'keep' }, totp: { action: 'keep' },
      fingerprint: fp, bookmarks: drawerDetails().bookmarks,
      humanize: drawerDetails().profile.humanize,
    }))
    expect(api.updateProfile.mock.calls[0]![1]).not.toHaveProperty('proxy_id')
    expect(wrapper.vm.$route.path).toBe('/profiles')
  })

  it('при ошибке сохранения оставляет редактор и исправляемое поле открытыми', async () => {
    api.updateProfile.mockRejectedValueOnce(new CoreError('Validation', 'проверьте поля', [{ field: 'name', code: 'required' }]))
    const wrapper = await drawer()
    await wrapper.get('#name').setValue('Другое имя')
    await button(wrapper, 'Сохранить').trigger('click')
    await flushPromises()
    expect(wrapper.get('#name').classes()).toContain('invalid')
    expect(wrapper.vm.$route.path).toBe('/profiles/p1')
    await wrapper.get('#name').setValue('Исправлено')
    expect(wrapper.get('#name').classes()).not.toContain('invalid')
  })

  it('отмена выхода сохраняет черновик, подтверждение разрешает смену профиля', async () => {
    const wrapper = await drawer()
    await wrapper.get('#name').setValue('Черновик')
    const changeProfile = vi.fn()
    const cancel = vi.fn()
    wrapper.vm.beforeClose(changeProfile, cancel)
    await flushPromises()
    expect(wrapper.findComponent(ConfirmDialog).props('open')).toBe(true)
    wrapper.findComponent(ConfirmDialog).vm.$emit('update:open', false)
    await flushPromises()
    expect(cancel).toHaveBeenCalledTimes(1)
    expect(changeProfile).not.toHaveBeenCalled()
    expect((wrapper.get('#name').element as HTMLInputElement).value).toBe('Черновик')
    wrapper.vm.beforeClose(changeProfile, cancel)
    await flushPromises()
    wrapper.findComponent(ConfirmDialog).vm.$emit('confirm')
    await flushPromises()
    expect(changeProfile).toHaveBeenCalledTimes(1)
    expect(cancel).toHaveBeenCalledTimes(1)
  })

  it('закрывается без подтверждения, если поля не менялись', async () => {
    const wrapper = await drawer()
    await wrapper.get('button[aria-label="Закрыть редактирование"]').trigger('click')
    await flushPromises()
    expect(wrapper.vm.$route.path).toBe('/profiles')
    expect(api.updateProfile).not.toHaveBeenCalled()
  })

  it('кнопка назад и переход к другому профилю защищают несохранённый черновик', async () => {
    const wrapper = await drawer()
    await wrapper.get('#name').setValue('Черновик')
    await wrapper.get('button[aria-label="К списку профилей"]').trigger('click')
    await flushPromises()
    expect(wrapper.findComponent(ConfirmDialog).props('open')).toBe(true)
    wrapper.findComponent(ConfirmDialog).vm.$emit('update:open', false)
    await flushPromises()
    expect(wrapper.vm.$route.path).toBe('/profiles/p1')
    expect(wrapper.get<HTMLInputElement>('#name').element.value).toBe('Черновик')
    const navigate = wrapper.vm.$router.push('/profiles/p2')
    await flushPromises()
    expect(wrapper.findComponent(ConfirmDialog).props('open')).toBe(true)
    wrapper.findComponent(ConfirmDialog).vm.$emit('confirm')
    await navigate
    expect(wrapper.vm.$route.path).toBe('/profiles/p2')
    expect(api.updateProfile).not.toHaveBeenCalled()
  })
})
