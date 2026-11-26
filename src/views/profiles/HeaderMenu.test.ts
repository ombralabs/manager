import { flushPromises } from '@vue/test-utils'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { profileRow } from '../../test/fixtures'
import { mountWithApp } from '../../test/mount'

const api = vi.hoisted(() => ({ exportCsvFile: vi.fn(), arrangeWindows: vi.fn(), listProfiles: vi.fn() }))
const files = vi.hoisted(() => ({ pickSaveCsv: vi.fn() }))
vi.mock('../../api/core', () => ({ core: api, events: {} }))
vi.mock('../../api/dialogs', () => files)

import { currentReport } from '../../lib/bulkReport'
import { useProfilesStore } from '../../stores/profiles'
import { useToasts } from '../../stores/toasts'
import HeaderMenu from './HeaderMenu.vue'

const toastTitles = () => useToasts().items.map((toast) => toast.title)

async function openMenu(wrapper: ReturnType<typeof mountWithApp>) {
  await wrapper.find('button[aria-label="Ещё действия"]').trigger('keydown', { key: 'ArrowDown' })
  await flushPromises()
}

const item = (wrapper: ReturnType<typeof mountWithApp>, text: string) =>
  wrapper.findAll('[role="menuitem"]').find((el) => el.text() === text)!

beforeEach(() => {
  for (const mock of [...Object.values(api), ...Object.values(files)]) mock.mockReset()
  currentReport.value = null
  const profiles = useProfilesStore()
  profiles.total = 10
  profiles.rows = [1, 2, 3].map((n) => profileRow({ id: `p${n}`, serial: n }))
  profiles.only = null
  profiles.running = 0
})

describe('меню «⋯» на «Профилях»', () => {
  it('«Расставить окна» неактивен без запущенных профилей', async () => {
    const wrapper = mountWithApp(HeaderMenu)
    await openMenu(wrapper)
    expect(item(wrapper, 'Расставить окна').attributes('data-disabled')).toBeDefined()
  })

  it('«Расставить окна»: итог уведомлением, неудачи — отчётом', async () => {
    useProfilesStore().running = 2
    api.arrangeWindows.mockResolvedValueOnce({ ok: ['a', 'b'], failed: [] })
    const wrapper = mountWithApp(HeaderMenu)
    await openMenu(wrapper)
    await item(wrapper, 'Расставить окна').trigger('click')
    await flushPromises()
    expect(toastTitles()).toContain('Расставлено окон: 2')
    api.arrangeWindows.mockResolvedValueOnce({ ok: ['a'], failed: [{ id: 'b', error: { code: 'Cdp', message: 'cdp' } }] })
    await openMenu(wrapper)
    await item(wrapper, 'Расставить окна').trigger('click')
    await flushPromises()
    expect(currentReport.value?.title).toBe('Расставлено 1, не расставлено 1')
  })

  it('экспорт CSV: число профилей, флажок секретов с предупреждением, запись файла', async () => {
    files.pickSaveCsv.mockResolvedValue('/tmp/out.csv')
    api.exportCsvFile.mockResolvedValue(3)
    const wrapper = mountWithApp(HeaderMenu)
    await openMenu(wrapper)
    await item(wrapper, 'Экспорт в CSV…').trigger('click')
    await flushPromises()
    expect(wrapper.text()).toContain('В файл попадут 3 профиля по текущим фильтрам и поиску.')
    expect(wrapper.text()).not.toContain('Логины, пароли и ключи 2FA будут сохранены без шифрования.')
    await wrapper.find('input[type="checkbox"]').setValue(true)
    expect(wrapper.text()).toContain('Логины, пароли и ключи 2FA будут сохранены без шифрования.')
    await wrapper.findAll('button').find((b) => b.text() === 'Сохранить…')!.trigger('click')
    await flushPromises()
    expect(files.pickSaveCsv.mock.calls[0]![0]).toMatch(/^ombra-profiles-\d{4}-\d{2}-\d{2}\.csv$/)
    expect(api.exportCsvFile).toHaveBeenCalledWith(useProfilesStore().listQuery(), true, '/tmp/out.csv')
    expect(toastTitles()).toContain('Сохранено 3 профиля')
    expect(wrapper.text()).not.toContain('Экспорт в CSV')
  })

  it('«Экспорт в CSV…» неактивен при фильтре «эти профили», с подсказкой', async () => {
    useProfilesStore().only = ['p1']
    const wrapper = mountWithApp(HeaderMenu)
    await openMenu(wrapper)
    const exportItem = item(wrapper, 'Экспорт в CSV…')
    expect(exportItem.attributes('data-disabled')).toBeDefined()
    expect(exportItem.attributes('title')).toBe('Снимите фильтр «эти профили», чтобы экспортировать')
  })

  it('отмена окна сохранения — диалог остаётся, файл не пишется', async () => {
    files.pickSaveCsv.mockResolvedValue(null)
    const wrapper = mountWithApp(HeaderMenu)
    await openMenu(wrapper)
    await item(wrapper, 'Экспорт в CSV…').trigger('click')
    await flushPromises()
    await wrapper.findAll('button').find((b) => b.text() === 'Сохранить…')!.trigger('click')
    await flushPromises()
    expect(api.exportCsvFile).not.toHaveBeenCalled()
    expect(wrapper.text()).toContain('Экспорт в CSV')
    expect(toastTitles()).toEqual([])
  })

  it('при фильтре «эти профили» пояснение к экспорту видно текстом в меню', async () => {
    useProfilesStore().only = ['p1']
    const wrapper = mountWithApp(HeaderMenu)
    await openMenu(wrapper)
    expect(wrapper.text()).toContain('Снимите фильтр «эти профили», чтобы экспортировать')
  })
})
