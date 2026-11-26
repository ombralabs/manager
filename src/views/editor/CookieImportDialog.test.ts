import { type VueWrapper, flushPromises } from '@vue/test-utils'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { CookieImportReport } from '../../api/types'
import { i18n } from '../../i18n'
import { mountWithApp } from '../../test/mount'

const api = vi.hoisted(() => ({ importCookies: vi.fn() }))
const dialogs = vi.hoisted(() => ({ pickCookieFile: vi.fn() }))
vi.mock('../../api/core', () => ({ core: api, events: {} }))
vi.mock('../../api/dialogs', () => dialogs)

import CookieImportDialog from './CookieImportDialog.vue'

const REPORT: CookieImportReport = {
  added: 2,
  replaced: 1,
  skipped: [{ line: 3, code: 'unparsed', reason: 'нужно 7 полей через табуляцию' }],
  sent_to_browser: false,
}

function dialog() {
  return mountWithApp(CookieImportDialog, {
    props: { open: true, profileId: 'k7f2q9' },
  })
}

function button(wrapper: VueWrapper, label: string) {
  const found = wrapper.findAll('button').find((b) => b.text() === label)
  if (!found) throw new Error(`нет кнопки «${label}»`)
  return found
}

describe('импорт cookies', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    api.importCookies.mockResolvedValue(REPORT)
  })

  afterEach(() => {
    i18n.global.locale.value = 'ru'
  })

  it('в английском интерфейсе пропуски — переводом кода, без русского текста ядра', async () => {
    i18n.global.locale.value = 'en'
    api.importCookies.mockResolvedValue({
      ...REPORT,
      skipped: [
        { line: 3, code: 'bad_expires', reason: 'срок — не число секунд' },
        { line: 5, code: 'no_domain', reason: 'нет домена' },
        { line: 6, code: 'bad_domain', reason: 'неверный домен' },
        { line: 8, code: 'control', reason: 'управляющие символы' },
      ],
    })
    const wrapper = dialog()
    await flushPromises()
    await wrapper.find('#cookie-import-text').setValue('.example.com\tTRUE\t/\tFALSE\tsoon\tsid\t1')
    await button(wrapper, 'Import').trigger('click')
    await flushPromises()
    const text = wrapper.text()
    expect(text).toContain('Skipped lines: 4')
    expect(text).toContain('Line 3: invalid expiration date')
    expect(text).toContain('Line 5: missing domain')
    expect(text).toContain('Line 6: invalid domain')
    expect(text).toContain('Line 8: control characters in the record')
    expect(text).not.toMatch(/[А-Яа-яЁё]/)
  })

  it('пустой текст не импортируется', async () => {
    const wrapper = dialog()
    await flushPromises()
    expect(button(wrapper, 'Импортировать').attributes('disabled')).toBeDefined()
  })

  it('текст уходит ядру; итог и пропущенные строки видны', async () => {
    const wrapper = dialog()
    await flushPromises()
    await wrapper.find('#cookie-import-text').setValue('[{"domain":".example.com"}]')
    await button(wrapper, 'Импортировать').trigger('click')
    await flushPromises()
    expect(api.importCookies).toHaveBeenCalledWith('k7f2q9', { kind: 'text', text: '[{"domain":".example.com"}]' })
    expect(wrapper.text()).toContain('Добавлено 2, заменено 1')
    expect(wrapper.text()).toContain('Строка 3: неверный формат записи')
    expect(wrapper.text()).not.toContain('нужно 7 полей')
    expect(wrapper.text()).not.toContain('Переданы в работающий браузер')
    expect(wrapper.emitted('imported')).toEqual([[REPORT]])
  })

  it('«Из файла…» отправляет путь; работающему браузеру — отметка', async () => {
    dialogs.pickCookieFile.mockResolvedValue('/Users/me/cookies.txt')
    api.importCookies.mockResolvedValue({ ...REPORT, skipped: [], sent_to_browser: true })
    const wrapper = dialog()
    await flushPromises()
    await button(wrapper, 'Из файла…').trigger('click')
    await flushPromises()
    expect(api.importCookies).toHaveBeenCalledWith('k7f2q9', { kind: 'file', path: '/Users/me/cookies.txt' })
    expect(wrapper.text()).toContain('Переданы в работающий браузер')
  })

  it('отмена окна выбора файла ничего не делает', async () => {
    dialogs.pickCookieFile.mockResolvedValue(null)
    const wrapper = dialog()
    await flushPromises()
    await button(wrapper, 'Из файла…').trigger('click')
    await flushPromises()
    expect(api.importCookies).not.toHaveBeenCalled()
    expect(wrapper.emitted('imported')).toBeUndefined()
  })

  it('пока открыто окно выбора файла, кнопки неактивны и второе окно не открывается', async () => {
    let answer: (path: string | null) => void = () => {}
    dialogs.pickCookieFile.mockImplementationOnce(() => new Promise<string | null>((resolve) => (answer = resolve)))
    const wrapper = dialog()
    await flushPromises()
    await wrapper.find('#cookie-import-text').setValue('[]')
    await button(wrapper, 'Из файла…').trigger('click')
    expect(button(wrapper, 'Из файла…').attributes('disabled')).toBeDefined()
    expect(button(wrapper, 'Импортировать').attributes('disabled')).toBeDefined()
    expect(button(wrapper, 'Отмена').attributes('disabled')).toBeDefined()
    await button(wrapper, 'Из файла…').trigger('click')
    expect(dialogs.pickCookieFile).toHaveBeenCalledTimes(1)
    answer('/Users/me/cookies.txt')
    await flushPromises()
    expect(api.importCookies).toHaveBeenCalledTimes(1)
    expect(api.importCookies).toHaveBeenCalledWith('k7f2q9', { kind: 'file', path: '/Users/me/cookies.txt' })
  })
})
