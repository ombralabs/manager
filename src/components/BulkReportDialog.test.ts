import { flushPromises } from '@vue/test-utils'
import { describe, expect, it, vi } from 'vitest'
import { mountWithApp } from '../test/mount'

const push = vi.hoisted(() => vi.fn())
vi.mock('vue-router', () => ({ useRouter: () => ({ push }) }))
vi.mock('../api/core', () => ({ core: { listProfiles: () => new Promise(() => undefined) } }))

import { useProfilesStore } from '../stores/profiles'
import { currentReport } from '../lib/bulkReport'
import BulkReportDialog from './BulkReportDialog.vue'
import { i18n } from '../i18n'
import { reportLines } from '../lib/bulk'

describe('диалог отчёта массовой операции', () => {
  it('shows the first account mask in bulk reports in RU and EN', async () => {
    try {
      for (const lang of ['ru', 'en'] as const) {
        i18n.global.locale.value = lang
        const wrapper = mountWithApp(BulkReportDialog, { attachTo: document.body })
        currentReport.value = { title: 'Bulk', inList: false, lines: reportLines({ ok: [], failed: [
          { id: 'a', error: { code: 'FreeDeviceBound', message: '', ownerHint: 'iv***v@example.com' } },
          { id: 'b', error: { code: 'FreeDeviceBound', message: '', ownerHint: 'ivan@example.com' } },
        ] }, (id) => id) }
        await flushPromises()
        expect(document.body.textContent).toContain('iv***v@example.com')
        expect(document.body.textContent).not.toContain('ivan@example.com')
        wrapper.unmount()
      }
    } finally { i18n.global.locale.value = 'ru'; currentReport.value = null }
  })
  it('показывает «профиль — причина» и ведёт к этим профилям в списке', async () => {
    const wrapper = mountWithApp(BulkReportDialog, { attachTo: document.body })
    currentReport.value = {
      title: 'Запущено 1, не запущено 2',
      inList: true,
      lines: [
        { id: 'a1', name: 'Магазин 001', code: 'AlreadyRunning' },
        { id: 'b2', name: 'Магазин 002', code: 'Weird' },
      ],
    }
    await flushPromises()

    const text = document.body.textContent ?? ''
    expect(text).toContain('Запущено 1, не запущено 2')
    expect(text).toContain('Магазин 001 — Профиль уже запущен')
    expect(text).toContain('Магазин 002 — Неизвестная ошибка')

    const show = [...document.body.querySelectorAll('button')].find((b) => b.textContent?.includes('Показать в списке'))
    show!.click()
    await flushPromises()
    expect(useProfilesStore().only).toEqual(['a1', 'b2'])
    expect(push).toHaveBeenCalledWith('/profiles')
    expect(currentReport.value).toBeNull()
    wrapper.unmount()
  })

  it('без «Показать в списке», если профилей нет в списке (корзина)', async () => {
    const wrapper = mountWithApp(BulkReportDialog, { attachTo: document.body })
    currentReport.value = { title: 'Не удалено 1', inList: false, lines: [{ id: 'a1', name: 'Магазин', code: 'Io' }] }
    await flushPromises()
    const labels = [...document.body.querySelectorAll('button')].map((b) => b.textContent)
    expect(labels.some((text) => text?.includes('Показать в списке'))).toBe(false)
    wrapper.unmount()
  })
})
