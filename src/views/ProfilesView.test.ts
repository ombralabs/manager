import { flushPromises } from '@vue/test-utils'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { createMemoryHistory, createRouter } from 'vue-router'
import { CoreError } from '../api/errors'
import type { ProfileRow } from '../api/types'
import { profileRow } from '../test/fixtures'
import { mountWithApp } from '../test/mount'
import { useProfilesStore } from '../stores/profiles'
import { useSyncStore } from '../stores/sync'
import { useToasts } from '../stores/toasts'

const api = vi.hoisted(() => ({
  listProxies: vi.fn(async () => []),
  listProfiles: vi.fn(async () => ({ rows: [] as ProfileRow[], total: 0, running: 0 })),
  listGroups: vi.fn(async () => ({ groups: [], ungrouped: 0 })),
  listTags: vi.fn(async () => []),
  deleteProfile: vi.fn(async () => undefined),
  listExtensions: vi.fn(async () => []),
  listExtCategories: vi.fn(async () => []),
  setProfilesExtCategory: vi.fn(),
  startProfiles: vi.fn(async (ids: string[]) => ({ ok: ids, failed: [] as { id: string; error: { code: string; message: string } }[] })),
  syncStart: vi.fn(async (main: string) => ({ active: true, mode: 'full_copy' as const, main, windows: [] })),
  startProfile: vi.fn(),
  stopProfile: vi.fn(async () => null),
}))
vi.mock('../api/core', () => ({ core: api, events: {} }))

import BulkBar from './profiles/BulkBar.vue'
import BulkCreateDialog from './profiles/BulkCreateDialog.vue'
import TaskDialog from './automation/TaskDialog.vue'
import ProfilesView from './ProfilesView.vue'

const profile = (id: string): ProfileRow =>
  profileRow({ id, name: id.toUpperCase(), restore_tabs: false, created_at: 0 })

const running = { status: { state: 'running' as const, pid: 1, cdp_ws: '', debug_port: 1, relay_port: 2, proxy_id: 0, exit_ip: null, country: null, timezone: 'UTC', started_at: 1 } }

async function mountView() {
  const router = createRouter({ history: createMemoryHistory(), routes: ['/', '/sync'].map((path) => ({ path, component: { template: '<div />' } })) })
  const wrapper = mountWithApp(ProfilesView, { global: { plugins: [router] } })
  await flushPromises()
  return wrapper
}

describe('экран «Профили»', () => {
  const warn = vi.spyOn(console, 'warn').mockImplementation(() => {})

  beforeEach(() => warn.mockClear())
  afterEach(() => warn.mockClear())

  it('все компоненты шапки разрешены, предупреждений Vue нет', async () => {
    const wrapper = await mountView()
    expect(wrapper.html()).not.toMatch(/<dropdownmenu/i)
    const unresolved = warn.mock.calls.filter((call) => String(call[0]).includes('Failed to resolve component'))
    expect(unresolved).toEqual([])
  })

  it('«Новый профиль ▾» → «Создать несколько…» открывает диалог', async () => {
    const wrapper = await mountView()
    const dialog = () => wrapper.findComponent(BulkCreateDialog)
    expect(dialog().props('open')).toBe(false)
    await wrapper.find('button.split-arrow').trigger('keydown', { key: 'ArrowDown' })
    await flushPromises()
    const item = wrapper.findAll('[role="menuitem"]').find((el) => el.text().includes('Создать несколько'))
    expect(item).toBeDefined()
    await item!.trigger('click')
    await flushPromises()
    expect(dialog().props('open')).toBe(true)
  })

  it('плашка отчёта считает только видимые строки, а не удалённые id', async () => {
    const wrapper = await mountView()
    const profiles = useProfilesStore()
    profiles.rows = [profile('a'), profile('b')]
    profiles.only = ['a', 'gone-1', 'gone-2']
    await flushPromises()
    expect(wrapper.find('.only span').text()).toBe('Показаны профили из отчёта: 1')
  })

  it('пока экран закрыт, события не перечитывают список; при открытии — один запрос', async () => {
    const first = await mountView()
    first.unmount()
    api.listProfiles.mockClear()
    const profiles = useProfilesStore()
    vi.useFakeTimers()
    try {
      for (let i = 0; i < 20; i++) {
        profiles.applyEvent({ kind: 'status', profile_id: `p${i}`, status: { state: 'starting' } })
        profiles.scheduleRefresh()
      }
      vi.advanceTimersByTime(2000)
    } finally {
      vi.useRealTimers()
    }
    expect(api.listProfiles).not.toHaveBeenCalled()
    // Группы уже загружены первым открытием: при повторном читается только список экрана.
    await mountView()
    expect(api.listProfiles).toHaveBeenCalledTimes(1)
    expect(profiles.active).toBe(true)
  })

  it('«Расширения» на панели: набор выбранным профилям, итог уведомлением', async () => {
    api.setProfilesExtCategory.mockResolvedValue({ ok: ['a', 'b'], failed: [] })
    const wrapper = await mountView()
    expect(api.listExtCategories).toHaveBeenCalled()
    const profiles = useProfilesStore()
    profiles.rows = [profile('a'), profile('b')]
    profiles.toggle('a')
    profiles.toggle('b')
    await flushPromises()
    wrapper.findComponent(BulkBar).vm.$emit('extSet', 1)
    await flushPromises()
    expect(api.setProfilesExtCategory).toHaveBeenCalledWith(['a', 'b'], 1)
    expect(useToasts().items.map((toast) => toast.title)).toContain('Набор назначен 2 профилям')
  })

  async function selectForSync() {
    const wrapper = await mountView()
    const profiles = useProfilesStore()
    profiles.rows = [profileRow({ id: 'a', name: 'Альфа', ...running }), profile('b')]
    profiles.toggle('a')
    profiles.toggle('b')
    await flushPromises()
    wrapper.findComponent(BulkBar).vm.$emit('sync')
    await flushPromises()
    return wrapper
  }

  it('сначала выбор мастера и предупреждение; отмена ничего не запускает', async () => {
    const wrapper = await selectForSync()
    const dialog = wrapper.get('[role="dialog"]')
    expect(dialog.text()).toContain('Выберите мастер-профиль')
    expect(dialog.text()).toContain('перезапущены')
    expect(dialog.findAll('input[type=radio]')).toHaveLength(2)
    expect(dialog.findAll('input:checked')).toHaveLength(0)
    expect(dialog.get('button.sync-confirm').attributes('disabled')).toBeDefined()
    expect(api.startProfiles).not.toHaveBeenCalled()
    expect(api.syncStart).not.toHaveBeenCalled()
    await dialog.findAll('button').find((b) => b.text() === 'Отмена')!.trigger('click')
    expect(wrapper.find('[role="dialog"]').exists()).toBe(false)
    expect(api.syncStart).not.toHaveBeenCalled()
  })

  it('выбранный в попапе мастер запускает ровно выбранные профили', async () => {
    const wrapper = await selectForSync()
    await wrapper.get('input[type=radio][value=b]').setValue(true)
    // Выбор таблицы может измениться; попап хранит исходный набор.
    useProfilesStore().clearSelection()
    await wrapper.get('button.sync-confirm').trigger('click')
    await flushPromises()
    expect(api.startProfiles).not.toHaveBeenCalled()
    expect(api.syncStart).toHaveBeenCalledExactlyOnceWith('b', ['a'])
    expect(useSyncStore().state.main).toBe('b')
    expect(wrapper.vm.$router.currentRoute.value.path).toBe('/sync')
  })

  it('текущий мастер отмечен в таблице и снимает отметку по завершении', async () => {
    const wrapper = await mountView()
    useProfilesStore().rows = [profile('a'), profile('b')]
    useSyncStore().state = { active: true, mode: 'full_copy', main: 'b', windows: [] }
    await flushPromises()
    expect(wrapper.findAll('.sync-master')).toHaveLength(1)
    expect(wrapper.get('.sync-master').text()).toContain('Мастер')
    expect(wrapper.get('.sync-master').element.closest('.row')!.textContent).toContain('B')
    useSyncStore().state.active = false
    await flushPromises()
    expect(wrapper.find('.sync-master').exists()).toBe(false)
  })

  it('«Остановить» у строки, которая ждёт поток, нажимается, пока её запуск ещё идёт', async () => {
    let failStart!: (e: unknown) => void
    api.startProfile.mockReturnValue(new Promise((_, reject) => (failStart = reject)))
    const wrapper = await mountView()
    const profiles = useProfilesStore()
    profiles.rows = [profile('a')]
    await flushPromises()
    await wrapper.find('.body .row .start-action').trigger('click')
    expect(api.startProfile).toHaveBeenCalledWith('a')
    profiles.rows = [profileRow({ id: 'a', status: { state: 'waiting', license: 'no_slot', info: null } })]
    await flushPromises()
    await wrapper.find('.body .row .run-action').trigger('click')
    await flushPromises()
    expect(api.stopProfile).toHaveBeenCalledWith('a')
    // Отменённый запуск отвечает NotRunning — это не ошибка.
    failStart(new CoreError('NotRunning', ''))
    await flushPromises()
    expect(useToasts().items).toEqual([])
  })

  it('ошибка запуска остаётся в попапе, повтор возможен без повторного выбора', async () => {
    const wrapper = await selectForSync()
    api.syncStart.mockRejectedValueOnce({ code: 'ProfileBusy', message: 'профиль «B» занят процессом автоматизации' })
    await wrapper.get('input[type=radio][value=b]').setValue(true)
    await wrapper.get('button.sync-confirm').trigger('click')
    await flushPromises()
    expect(wrapper.find('[role="dialog"]').exists()).toBe(true)
    expect(wrapper.vm.$router.currentRoute.value.path).toBe('/')
    expect(useToasts().items.length).toBeGreaterThan(0)
    await wrapper.get('button.sync-confirm').trigger('click')
    await flushPromises()
    expect(api.syncStart).toHaveBeenCalledTimes(2)
    expect(wrapper.vm.$router.currentRoute.value.path).toBe('/sync')
  })

  it('повторное подтверждение не запускает второй сеанс', async () => {
    const wrapper = await selectForSync()
    let resolve!: (value: Awaited<ReturnType<typeof api.syncStart>>) => void
    api.syncStart.mockImplementationOnce(() => new Promise((done) => { resolve = done }))
    await wrapper.get('input[type=radio][value=b]').setValue(true)
    await wrapper.get('button.sync-confirm').trigger('click')
    await wrapper.get('button.sync-confirm').trigger('click')
    expect(api.syncStart).toHaveBeenCalledTimes(1)
    expect(wrapper.get('button.sync-confirm').attributes('aria-disabled')).toBe('true')
    resolve({ active: true, mode: 'full_copy', main: 'b', windows: [] })
    await flushPromises()
  })

  it('действующая сессия открывается без перезапуска выбранных профилей', async () => {
    const wrapper = await mountView()
    useSyncStore().state = { active: true, mode: 'full_copy', main: 'existing', windows: [] }
    wrapper.findComponent(BulkBar).vm.$emit('sync')
    await flushPromises()
    expect(wrapper.find('[role="dialog"]').exists()).toBe(false)
    expect(api.syncStart).not.toHaveBeenCalled()
    expect(wrapper.vm.$router.currentRoute.value.path).toBe('/sync')
  })

  it('«Запустить процесс…» открывает диалог задачи с выбранными профилями', async () => {
    const wrapper = await mountView()
    const profiles = useProfilesStore()
    profiles.rows = [profile('a'), profile('b')]
    profiles.toggle('a')
    profiles.toggle('b')
    await flushPromises()
    wrapper.findComponent(BulkBar).vm.$emit('runFlow')
    await flushPromises()
    const dialog = wrapper.findComponent(TaskDialog)
    expect(dialog.props('open')).toBe(true)
    expect(dialog.props('flowId')).toBeNull()
    expect(dialog.props('profileIds')).toEqual(['a', 'b'])
    expect(dialog.text()).toContain('Профилей: 2')
  })

  it('удаление профиля: текст про корзину и уведомление «Профиль в корзине»', async () => {
    const wrapper = await mountView()
    useProfilesStore().rows = [profile('a')]
    await flushPromises()
    await wrapper.find('.body .row button[aria-label="Ещё действия"]').trigger('keydown', { key: 'ArrowDown' })
    await flushPromises()
    await wrapper.findAll('[role="menuitem"]').find((el) => el.text() === 'Удалить')!.trigger('click')
    await flushPromises()
    expect(wrapper.text()).toContain('Профиль перейдёт в корзину. Восстановить его можно там.')
    await wrapper.findAll('button').filter((b) => b.text() === 'Удалить').at(-1)!.trigger('click')
    await flushPromises()
    expect(api.deleteProfile).toHaveBeenCalledWith('A'.toLowerCase())
    expect(useToasts().items.map((toast) => toast.title)).toContain('Профиль в корзине')
  })
})
