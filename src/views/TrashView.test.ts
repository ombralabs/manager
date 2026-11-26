import { flushPromises } from '@vue/test-utils'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { ProfileRow } from '../api/types'
import { profileRow } from '../test/fixtures'
import { mountWithApp } from '../test/mount'

const api = vi.hoisted(() => ({
  listTrash: vi.fn(),
  restoreProfiles: vi.fn(),
  purgeProfiles: vi.fn(),
  emptyTrash: vi.fn(),
  listGroups: vi.fn(),
  listTags: vi.fn(),
  listProfiles: vi.fn(),
}))
const events = vi.hoisted(() => ({ onProfilesChanged: vi.fn() }))
vi.mock('../api/core', () => ({ core: api, events }))

import { currentReport } from '../lib/bulkReport'
import { useToasts } from '../stores/toasts'
import TrashView from './TrashView.vue'

const trashed = (n: number): ProfileRow =>
  profileRow({ id: `p${n}`, serial: n, name: `Профиль ${n}`, deleted_at: 1_700_000_000 + n })

async function mountView() {
  const wrapper = mountWithApp(TrashView)
  await flushPromises()
  return wrapper
}

const toastTitles = () => useToasts().items.map((toast) => toast.title)
const button = (wrapper: ReturnType<typeof mountWithApp>, text: string) =>
  wrapper.findAll('button').find((b) => b.text() === text)!

async function openRowMenu(wrapper: ReturnType<typeof mountWithApp>, index: number) {
  await wrapper.findAll('.body .row')[index]!.find('button[aria-label="Ещё действия"]').trigger('keydown', { key: 'ArrowDown' })
  await flushPromises()
}

const menuItem = (wrapper: ReturnType<typeof mountWithApp>, text: string) =>
  wrapper.findAll('[role="menuitem"]').find((item) => item.text() === text)!

beforeEach(() => {
  for (const mock of Object.values(api)) mock.mockReset()
  currentReport.value = null
  api.listTrash.mockResolvedValue([trashed(1), trashed(2)])
  api.listGroups.mockResolvedValue({ groups: [], ungrouped: 0 })
  api.listTags.mockResolvedValue([])
  api.listProfiles.mockResolvedValue({ rows: [], total: 0 })
  events.onProfilesChanged.mockReset()
  events.onProfilesChanged.mockResolvedValue(() => {})
})

describe('экран «Корзина»', () => {
  it('шапка, строки и пустая корзина', async () => {
    const wrapper = await mountView()
    expect(wrapper.find('.head').text()).toContain('2 профиля')
    expect(wrapper.findAll('.body .row')).toHaveLength(2)
    api.listTrash.mockResolvedValue([])
    const empty = await mountView()
    expect(empty.text()).toContain('Корзина пуста')
    expect(button(empty, 'Очистить корзину').attributes('disabled')).toBeDefined()
  })

  it('«Восстановить» в меню строки — без подтверждения, итог уведомлением, список перечитан', async () => {
    api.restoreProfiles.mockResolvedValue({ ok: ['p1'], failed: [] })
    const wrapper = await mountView()
    api.listTrash.mockResolvedValue([trashed(2)])
    await openRowMenu(wrapper, 0)
    await menuItem(wrapper, 'Восстановить').trigger('click')
    await flushPromises()
    expect(api.restoreProfiles).toHaveBeenCalledWith(['p1'])
    expect(toastTitles()).toContain('Восстановлен 1 профиль')
    expect(wrapper.findAll('.body .row')).toHaveLength(1)
  })

  it('после восстановления и удаления перечитываются группы и теги «Профилей»', async () => {
    api.restoreProfiles.mockResolvedValue({ ok: ['p1'], failed: [] })
    const wrapper = await mountView()
    expect(api.listGroups).not.toHaveBeenCalled()
    await openRowMenu(wrapper, 0)
    await menuItem(wrapper, 'Восстановить').trigger('click')
    await flushPromises()
    expect(api.listGroups).toHaveBeenCalledTimes(1)
    expect(api.listTags).toHaveBeenCalledTimes(1)
  })

  it('«Корзина» перечитывается по событию profiles-changed и отписывается при закрытии', async () => {
    const unlisten = vi.fn()
    events.onProfilesChanged.mockResolvedValue(unlisten)
    const wrapper = await mountView()
    expect(api.listTrash).toHaveBeenCalledTimes(1)
    vi.useFakeTimers()
    try {
      events.onProfilesChanged.mock.calls[0]![0]()
      await vi.advanceTimersByTimeAsync(500)
    } finally {
      vi.useRealTimers()
    }
    expect(api.listTrash).toHaveBeenCalledTimes(2)
    wrapper.unmount()
    expect(unlisten).toHaveBeenCalled()
  })

  it('«Удалить навсегда» спрашивает подтверждение; отчёт называет профили по именам и после перечитывания', async () => {
    api.purgeProfiles.mockResolvedValue({ ok: [], failed: [{ id: 'p1', error: { code: 'Io', message: 'диск' } }] })
    const wrapper = await mountView()
    await openRowMenu(wrapper, 0)
    await menuItem(wrapper, 'Удалить навсегда').trigger('click')
    await flushPromises()
    expect(wrapper.text()).toContain('Удалить профиль «Профиль 1» навсегда?')
    expect(wrapper.text()).toContain('Папка браузера с cookies и историей будет удалена. Отменить это нельзя.')
    api.listTrash.mockResolvedValue([])
    await button(wrapper, 'Удалить навсегда').trigger('click')
    await flushPromises()
    expect(api.purgeProfiles).toHaveBeenCalledWith(['p1'])
    expect(currentReport.value?.lines.map((line) => line.name)).toEqual(['Профиль 1'])
    expect(currentReport.value?.inList).toBe(false)
  })

  it('пачка: выделение, «Восстановить», «Снять выделение»', async () => {
    api.restoreProfiles.mockResolvedValue({ ok: ['p1', 'p2'], failed: [] })
    const wrapper = await mountView()
    for (const box of wrapper.findAll('.body input[type="checkbox"]')) await box.setValue(true)
    expect(wrapper.text()).toContain('Выбрано: 2')
    await button(wrapper, 'Восстановить').trigger('click')
    await flushPromises()
    expect(api.restoreProfiles).toHaveBeenCalledWith(['p1', 'p2'])
    expect(wrapper.text()).not.toContain('Выбрано:')
  })

  it('«Очистить корзину» — через подтверждение', async () => {
    api.emptyTrash.mockResolvedValue({ ok: ['p1', 'p2'], failed: [] })
    const wrapper = await mountView()
    await button(wrapper, 'Очистить корзину').trigger('click')
    await flushPromises()
    expect(wrapper.text()).toContain('Очистить корзину?')
    api.listTrash.mockResolvedValue([])
    await wrapper.findAll('button').filter((b) => b.text() === 'Очистить корзину').at(-1)!.trigger('click')
    await flushPromises()
    expect(api.emptyTrash).toHaveBeenCalled()
    expect(wrapper.text()).toContain('Корзина пуста')
  })

  it('страницы: 120 профилей по 50', async () => {
    api.listTrash.mockResolvedValue(Array.from({ length: 120 }, (_, i) => trashed(i + 1)))
    const wrapper = await mountView()
    expect(wrapper.findAll('.body .row')).toHaveLength(50)
    expect(wrapper.text()).toContain('1–50 из 120')
  })

  it('два события profiles-changed подряд дают один запрос корзины', async () => {
    vi.useFakeTimers()
    try {
      let emit = () => {}
      events.onProfilesChanged.mockImplementation(async (cb: () => void) => {
        emit = cb
        return () => {}
      })
      await mountView()
      api.listTrash.mockClear()
      emit()
      emit()
      await vi.advanceTimersByTimeAsync(500)
      expect(api.listTrash).toHaveBeenCalledTimes(1)
    } finally {
      vi.useRealTimers()
    }
  })
})
