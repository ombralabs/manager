import { enableAutoUnmount, flushPromises } from '@vue/test-utils'
import { createMemoryHistory, createRouter } from 'vue-router'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { SyncState } from '../api/types'
import { profileRow } from '../test/fixtures'
import { mountWithApp } from '../test/mount'

const IDLE: SyncState = { active: false, mode: 'full_copy', main: null, windows: [] }
const running = { state: 'running', pid: 1, cdp_ws: '', debug_port: 1, relay_port: 2 }
const handlers = vi.hoisted(() => ({ sync: (_: unknown) => {} }))
const api = vi.hoisted(() => ({
  listProfiles: vi.fn(),
  listGroups: vi.fn(async () => ({ groups: [], ungrouped: 0 })),
  listTags: vi.fn(async () => []),
  syncState: vi.fn(),
  syncStart: vi.fn(),
  syncStop: vi.fn(async () => null),
  syncRelease: vi.fn(async () => null),
  syncResume: vi.fn(async () => null),
  syncOpenLikeMain: vi.fn(async () => null),
  showWindow: vi.fn(async () => null),
  getSyncSettings: vi.fn(),
  setSyncSettings: vi.fn(),
}))
vi.mock('../api/core', () => ({
  core: api,
  events: { onSync: async (handler: (state: unknown) => void) => { handlers.sync = handler; return () => {} } },
}))

import { useGroupsStore } from '../stores/groups'
import { useSyncStore } from '../stores/sync'
import SyncView from './SyncView.vue'

enableAutoUnmount(beforeEach)

const rows = [
  profileRow({ id: 'a', name: 'Альфа', status: running as never }),
  profileRow({ id: 'b', name: 'Бета', status: running as never }),
  profileRow({ id: 'c', name: 'Гамма', status: { state: 'stopped' } }),
]

async function mountView() {
  const router = createRouter({ history: createMemoryHistory(), routes: [{ path: '/sync', component: SyncView }, { path: '/settings', component: { template: '<i />' } }] })
  await router.push('/sync')
  await useSyncStore().init()
  await useSyncStore().load()
  const wrapper = mountWithApp(SyncView, { global: { plugins: [router] } })
  await flushPromises()
  return { wrapper, router }
}
const button = (w: { findAll: (s: string) => { text: () => string }[] }, text: string) => w.findAll('button').find((b) => b.text().includes(text)) as never as { trigger: (e: string) => Promise<void>; attributes: (n: string) => string | undefined }

describe('экран «Синхронизатор»', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    api.listProfiles.mockResolvedValue({ rows, total: 3, running: 2 })
    api.syncState.mockResolvedValue(IDLE)
    api.getSyncSettings.mockResolvedValue({ mode: 'full_copy', clicks: true, typing: true, scroll: true, mouse: true, click_delay: [100, 600], typing_delay: [100, 600] })
  })

  it('«Начать» включается после двух галочек; главный — первый отмеченный; остановленные не показаны', async () => {
    const { wrapper } = await mountView()
    expect(wrapper.text()).not.toContain('Гамма')
    const start = () => button(wrapper, 'Начать синхронизацию')
    expect(start().attributes('disabled')).toBeDefined()
    const boxes = wrapper.findAll('input[type=checkbox]')
    await boxes[1].setValue(true)
    expect(start().attributes('disabled')).toBeDefined()
    await boxes[0].setValue(true)
    expect(start().attributes('disabled')).toBeUndefined()
    api.syncStart.mockResolvedValue({ ...IDLE, active: true, main: 'b', windows: [] })
    await start().trigger('click')
    expect(api.syncStart).not.toHaveBeenCalled()
    expect(wrapper.get('[role=dialog]').text()).toContain('перезапущены')
    await wrapper.get('input[type=radio][value=b]').setValue(true)
    await wrapper.get('button.sync-confirm').trigger('click')
    await flushPromises()
    expect(api.syncStart).toHaveBeenCalledWith('b', ['a'])
  })

  it('«Начать» выключена, пока сессия стартует', async () => {
    const { wrapper } = await mountView()
    const boxes = wrapper.findAll('input[type=checkbox]')
    await boxes[0].setValue(true)
    await boxes[1].setValue(true)
    let started = (_: SyncState) => {}
    api.syncStart.mockImplementationOnce(() => new Promise<SyncState>((resolve) => { started = resolve }))
    const start = () => button(wrapper, 'Начать синхронизацию')
    await start().trigger('click')
    await wrapper.get('input[type=radio][value=a]').setValue(true)
    await wrapper.get('button.sync-confirm').trigger('click')
    expect(wrapper.get('button.sync-confirm').attributes('aria-disabled')).toBe('true')
    started(IDLE)
    await flushPromises()
    expect(start().attributes('disabled')).toBeUndefined()
    expect(api.syncStart).toHaveBeenCalledTimes(1)
  })

  it('пауза из события показывает текст причины; «Приостановить синхронизацию» и «Возобновить синхронизацию» зовут ядро', async () => {
    const { wrapper } = await mountView()
    handlers.sync({
      active: true, mode: 'full_copy', main: 'a',
      windows: [
        { profileId: 'a', main: true, status: { state: 'synced' }, plain: false },
        { profileId: 'b', main: false, status: { state: 'synced' }, plain: false },
      ],
    })
    await flushPromises()
    await button(wrapper, 'Приостановить синхронизацию').trigger('click')
    expect(api.syncRelease).toHaveBeenCalledWith('b')
    handlers.sync({
      active: true, mode: 'full_copy', main: 'a',
      windows: [
        { profileId: 'a', main: true, status: { state: 'synced' }, plain: false },
        { profileId: 'b', main: false, status: { state: 'paused', code: 'element', detail: 'ссылка «Подробнее» вместо кнопки «Купить»' }, plain: true },
      ],
    })
    await flushPromises()
    expect(wrapper.text()).toContain('На паузе: в выбранной точке находится другой элемент: ссылка «Подробнее» вместо кнопки «Купить»')
    expect(wrapper.text()).toContain('без имитации естественного ввода')
    await button(wrapper, 'Возобновить синхронизацию').trigger('click')
    expect(api.syncResume).toHaveBeenCalledWith('b')
    await button(wrapper, 'Открыть страницу главного окна').trigger('click')
    expect(api.syncOpenLikeMain).toHaveBeenCalledWith('b')
  })

  it('выпавшее окно: «Отключено от синхронизации: окно закрыто»', async () => {
    const { wrapper } = await mountView()
    handlers.sync({ active: true, mode: 'full_copy', main: 'a', windows: [{ profileId: 'b', main: false, status: { state: 'dropped', code: 'closed' }, plain: false }] })
    await flushPromises()
    expect(wrapper.text()).toContain('Отключено от синхронизации: окно закрыто')
  })

  it('«Настройки» ведёт на /sync/settings', async () => {
    const { wrapper, router } = await mountView()
    await button(wrapper, 'Настройки').trigger('click')
    await flushPromises()
    expect(router.currentRoute.value.fullPath).toBe('/sync/settings')
  })

  it('остановленный отмеченный профиль не считается: «Начать» выключается, главный заменяется', async () => {
    const { wrapper } = await mountView()
    const boxes = wrapper.findAll('input[type=checkbox]')
    await boxes[0].setValue(true)
    await boxes[1].setValue(true)
    expect(useSyncStore().mainId).toBe('a')
    api.listProfiles.mockResolvedValue({ rows: [{ ...rows[0], status: { state: 'stopped' } }, rows[1], rows[2]], total: 3, running: 1 })
    await useGroupsStore().refresh()
    await flushPromises()
    expect(useSyncStore().mainId).toBe('b')
    expect([...useSyncStore().checked]).toEqual(['b'])
    expect(button(wrapper, 'Начать синхронизацию').attributes('disabled')).toBeDefined()
  })

  it('во время сессии окно остановленного профиля остаётся в списке с причиной', async () => {
    const { wrapper } = await mountView()
    api.listProfiles.mockResolvedValue({ rows: [rows[0], { ...rows[1], status: { state: 'stopped' } }], total: 2, running: 1 })
    await useGroupsStore().refresh()
    handlers.sync({ active: true, mode: 'full_copy', main: 'a', windows: [{ profileId: 'b', main: false, status: { state: 'dropped', code: 'closed' }, plain: false }] })
    await flushPromises()
    expect(wrapper.text()).toContain('Бета')
    expect(wrapper.text()).toContain('Отключено от синхронизации: окно закрыто')
  })

  it('завершение сеанса во время запуска новее ответа на start', async () => {
    const { wrapper } = await mountView()
    const sync = useSyncStore()
    sync.pick(['a', 'b'])
    let answer!: (state: SyncState) => void
    api.syncStart.mockImplementationOnce(() => new Promise<SyncState>((resolve) => { answer = resolve }))
    const pending = sync.start()
    handlers.sync(IDLE)
    answer({ ...IDLE, active: true, main: 'a' })
    await pending
    expect(sync.state.active).toBe(false)
    expect(wrapper.text()).not.toContain('Синхронно')
  })

  it('load: ошибка первого чтения — отказом, подписка остаётся; событие во время чтения новее ответа', async () => {
    const sync = useSyncStore()
    await sync.init()
    api.syncState.mockRejectedValueOnce(new Error('x'))
    await expect(sync.load()).rejects.toThrow()
    let answer: (s: SyncState) => void = () => {}
    api.syncState.mockImplementationOnce(() => new Promise<SyncState>((r) => { answer = r }))
    const pending = sync.load()
    await flushPromises()
    const live = { ...IDLE, active: true, main: 'a' }
    handlers.sync(live)
    answer(IDLE)
    await pending
    expect(useSyncStore().state).toEqual(live)
  })
})
