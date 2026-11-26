import { flushPromises } from '@vue/test-utils'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { CookieStats } from '../../api/types'
import { mountWithApp } from '../../test/mount'

const api = vi.hoisted(() => ({ cookieStats: vi.fn(), exportCookiesTo: vi.fn() }))
const dialogs = vi.hoisted(() => ({ pickSaveFile: vi.fn(), pickFolder: vi.fn() }))
vi.mock('../../api/core', () => ({ core: api, events: {} }))
vi.mock('../../api/dialogs', () => dialogs)

import { useToasts } from '../../stores/toasts'
import CookieExportDialog from './CookieExportDialog.vue'

const STATS: Record<string, CookieStats> = {
  a: { total: 10, unreadable: 3 },
  b: { total: 4, unreadable: 0 },
  c: { total: 7, unreadable: 2 },
}

function mountDialog(ids: string[]) {
  return mountWithApp(CookieExportDialog, {
    props: { open: true, ids, names: { a: 'Магазин', b: 'Почта', c: 'Реклама' } },
  })
}

describe('диалог экспорта cookies', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    api.cookieStats.mockImplementation(async (id: string) => STATS[id])
  })

  it('один профиль с нечитаемыми записями — предупреждение с их числом', async () => {
    const wrapper = mountDialog(['a'])
    await flushPromises()
    expect(api.cookieStats).toHaveBeenCalledWith('a')
    expect(wrapper.text()).toContain('3 записи не удалось прочитать — их нет в файле')
  })

  it('несколько профилей — сумма; без нечитаемых предупреждения нет', async () => {
    const many = mountDialog(['a', 'b', 'c'])
    await flushPromises()
    expect(api.cookieStats.mock.calls.map(([id]) => id)).toEqual(['a', 'b', 'c'])
    expect(many.text()).toContain('5 записей не удалось прочитать — их нет в файле')
    const clean = mountDialog(['b'])
    await flushPromises()
    expect(clean.text()).not.toContain('не удалось прочитать')
  })

  it('ошибка счёта у профиля не мешает: предупреждение по остальным, уведомлений нет', async () => {
    api.cookieStats.mockImplementation(async (id: string) => {
      if (id === 'a') throw { code: 'NotFound', message: 'профиль не найден' }
      return STATS[id]
    })
    const wrapper = mountDialog(['a', 'c'])
    await flushPromises()
    expect(wrapper.text()).toContain('2 записи не удалось прочитать — их нет в файле')
    expect(useToasts().items).toEqual([])
  })

  /** cookieStats с ответами «вручную»: по одному ожидающему вызову на профиль. */
  function manualStats() {
    const pending: Record<string, (stats: CookieStats) => void> = {}
    api.cookieStats.mockImplementation((id: string) => new Promise<CookieStats>((resolve) => (pending[id] = resolve)))
    return pending
  }
  const calls = () => api.cookieStats.mock.calls.map(([id]) => id)

  it('смена профилей во время подсчёта: старый результат отброшен, вызовы по новому набору', async () => {
    const pending = manualStats()
    const wrapper = mountDialog(['a', 'b'])
    await flushPromises()
    expect(calls()).toEqual(['a'])
    await wrapper.setProps({ ids: ['c'] })
    await flushPromises()
    expect(calls()).toEqual(['a', 'c'])
    pending.a({ total: 10, unreadable: 3 })
    await flushPromises()
    expect(calls()).toEqual(['a', 'c'])
    expect(wrapper.text()).not.toContain('не удалось прочитать')
    pending.c({ total: 7, unreadable: 2 })
    await flushPromises()
    expect(wrapper.text()).toContain('2 записи не удалось прочитать')
  })

  it('закрытие диалога во время подсчёта: сумма не показана, остальные профили не считаются', async () => {
    const pending = manualStats()
    const wrapper = mountDialog(['a', 'b'])
    await flushPromises()
    await wrapper.setProps({ open: false })
    pending.a({ total: 10, unreadable: 3 })
    await flushPromises()
    expect(calls()).toEqual(['a'])
    await wrapper.setProps({ open: true })
    await flushPromises()
    expect(wrapper.text()).not.toContain('не удалось прочитать')
  })

  it('размонтирование во время подсчёта: остальные профили не считаются', async () => {
    const pending = manualStats()
    const wrapper = mountDialog(['a', 'b', 'c'])
    await flushPromises()
    wrapper.unmount()
    pending.a({ total: 10, unreadable: 3 })
    await flushPromises()
    expect(calls()).toEqual(['a'])
  })
})
