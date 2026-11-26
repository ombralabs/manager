import { createPinia, setActivePinia } from 'pinia'
import { beforeEach, describe, expect, it, vi } from 'vitest'

vi.mock('../api/core', () => ({ core: {} }))

import { useToasts } from '../stores/toasts'
import { currentReport, showBulkReport } from './bulkReport'

describe('отчёт массовой операции', () => {
  beforeEach(() => {
    localStorage.clear()
    setActivePinia(createPinia())
    currentReport.value = null
  })

  it('всё прошло — уведомление', () => {
    showBulkReport('clone', { ok: ['a', 'b', 'c'], failed: [] })
    expect(useToasts().items).toEqual([{ id: 1, kind: 'info', title: 'Создано 3 копии' }])
    expect(currentReport.value).toBeNull()
  })

  it('массовый запуск: профили, ждущие поток, — своим уведомлением', () => {
    showBulkReport('start', { ok: ['a', 'b'], failed: [], waiting: ['c', 'd', 'e', 'f'] })
    expect(useToasts().items.map((toast) => toast.title)).toEqual(['Запущено 2 профиля', '4 профиля ожидают запуска'])
    useToasts().items.splice(0)
    showBulkReport('start', { ok: [], failed: [], waiting: ['a'] })
    expect(useToasts().items.map((toast) => toast.title)).toEqual(['1 профиль ожидает запуска'])
  })

  it('были ошибки — диалог со строками «профиль — причина»', () => {
    showBulkReport('start', {
      ok: ['a'],
      failed: [
        { id: 'b', error: { code: 'AlreadyRunning', message: '' } },
        { id: 'c', error: { code: 'NotFound', message: '' } },
      ],
    })
    expect(useToasts().items).toEqual([])
    expect(currentReport.value).toEqual({
      title: 'Запущено 1, не запущено 2',
      inList: true,
      lines: [
        { id: 'b', name: 'b', code: 'AlreadyRunning' },
        { id: 'c', name: 'c', code: 'NotFound' },
      ],
    })
  })
})
