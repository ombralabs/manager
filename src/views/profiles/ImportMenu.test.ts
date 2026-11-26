import { flushPromises } from '@vue/test-utils'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { CoreError } from '../../api/errors'
import type { CsvImportReport, ProfileRecord } from '../../api/types'
import { mountWithApp } from '../../test/mount'

const api = vi.hoisted(() => ({ importCsvFile: vi.fn(), importProfile: vi.fn() }))
const files = vi.hoisted(() => ({ pickCsvFile: vi.fn(), pickArchiveFile: vi.fn() }))
vi.mock('../../api/core', () => ({ core: api, events: {} }))
vi.mock('../../api/dialogs', () => files)

import { useToasts } from '../../stores/toasts'
import ImportMenu from './ImportMenu.vue'

const record = (name: string) => ({ id: 'n1', name }) as ProfileRecord
const report = (created: number, errors: CsvImportReport['errors'] = []): CsvImportReport => ({
  created: Array.from({ length: created }, (_, i) => record(`P${i}`)),
  errors,
})
const toastTitles = () => useToasts().items.map((toast) => toast.title)

async function choose(wrapper: ReturnType<typeof mountWithApp>, item: string) {
  await wrapper.find('button').trigger('keydown', { key: 'ArrowDown' })
  await flushPromises()
  await wrapper.findAll('[role="menuitem"]').find((el) => el.text() === item)!.trigger('click')
  await flushPromises()
}

beforeEach(() => {
  for (const mock of [...Object.values(api), ...Object.values(files)]) mock.mockReset()
})

describe('меню «Импорт»', () => {
  it('пока открыто окно выбора файла, «Импорт» занят без потери фокуса, пункты меню неактивны', async () => {
    files.pickCsvFile.mockReturnValueOnce(new Promise(() => {}))
    const wrapper = mountWithApp(ImportMenu)
    await choose(wrapper, 'Таблица CSV…')
    const trigger = wrapper.find('button')
    expect(trigger.attributes('disabled')).toBeUndefined()
    expect(trigger.attributes('aria-disabled')).toBe('true')
    await trigger.trigger('keydown', { key: 'ArrowDown' })
    await flushPromises()
    const items = wrapper.findAll('[role="menuitem"]')
    expect(items.every((item) => item.attributes('aria-disabled') === 'true')).toBe(true)
    await items.find((item) => item.text() === 'Архив профиля…')!.trigger('click')
    expect(files.pickArchiveFile).not.toHaveBeenCalled()
  })

  it('CSV без ошибок — уведомление и событие', async () => {
    files.pickCsvFile.mockResolvedValue('/tmp/p.csv')
    api.importCsvFile.mockResolvedValue(report(2))
    const wrapper = mountWithApp(ImportMenu)
    await choose(wrapper, 'Таблица CSV…')
    expect(api.importCsvFile).toHaveBeenCalledWith('/tmp/p.csv')
    expect(toastTitles()).toContain('Создано 2 профиля')
    expect(wrapper.emitted('imported')).toHaveLength(1)
  })

  it('CSV с ошибками — отчёт, первые 50 строк и «и ещё»', async () => {
    files.pickCsvFile.mockResolvedValue('/tmp/p.csv')
    const errors = Array.from({ length: 60 }, (_, i) => ({ line: i + 2, code: i === 0 ? 'proxy_invalid' : 'weird' }))
    api.importCsvFile.mockResolvedValue(report(1, errors))
    const wrapper = mountWithApp(ImportMenu)
    await choose(wrapper, 'Таблица CSV…')
    expect(wrapper.text()).toContain('Импорт из таблицы')
    expect(wrapper.text()).toContain('Создан 1 профиль')
    expect(wrapper.text()).toContain('Строка 2: неверный прокси')
    expect(wrapper.findAll('.report-line')).toHaveLength(50)
    expect(wrapper.text()).toContain('и ещё 10')
    expect(wrapper.emitted('imported')).toHaveLength(1)
  })

  it('отмена окна выбора — ничего не происходит', async () => {
    files.pickCsvFile.mockResolvedValue(null)
    const wrapper = mountWithApp(ImportMenu)
    await choose(wrapper, 'Таблица CSV…')
    expect(api.importCsvFile).not.toHaveBeenCalled()
    expect(toastTitles()).toEqual([])
  })

  it('архив без пароля импортируется сразу', async () => {
    files.pickArchiveFile.mockResolvedValue('/tmp/a.tar.gz')
    api.importProfile.mockResolvedValue(record('Магазин'))
    const wrapper = mountWithApp(ImportMenu)
    await choose(wrapper, 'Архив профиля…')
    expect(api.importProfile).toHaveBeenCalledWith('/tmp/a.tar.gz', null)
    expect(toastTitles()).toContain('Профиль «Магазин» импортирован')
  })

  it('архив с паролем: спросить, неверный — ошибка у поля, верный — импорт', async () => {
    files.pickArchiveFile.mockResolvedValue('/tmp/a.tar.gz')
    api.importProfile
      .mockRejectedValueOnce(new CoreError('ArchivePassword', 'неверный пароль архива'))
      .mockRejectedValueOnce(new CoreError('ArchivePassword', 'неверный пароль архива'))
      .mockResolvedValueOnce(record('Магазин'))
    const wrapper = mountWithApp(ImportMenu)
    await choose(wrapper, 'Архив профиля…')
    expect(wrapper.text()).toContain('Пароль архива')
    const input = wrapper.find('input[type="password"]')
    await input.setValue('wrong')
    await wrapper.findAll('button').find((b) => b.text() === 'Импортировать')!.trigger('click')
    await flushPromises()
    expect(api.importProfile).toHaveBeenLastCalledWith('/tmp/a.tar.gz', 'wrong')
    expect(wrapper.text()).toContain('Неверный пароль')
    expect(input.attributes('disabled')).toBeUndefined()
    await input.setValue('pw')
    await wrapper.findAll('button').find((b) => b.text() === 'Импортировать')!.trigger('click')
    await flushPromises()
    expect(toastTitles()).toContain('Профиль «Магазин» импортирован')
    expect(wrapper.text()).not.toContain('Пароль архива')
  })

  it('пока архив проверяется, поле пароля только для чтения', async () => {
    files.pickArchiveFile.mockResolvedValue('/tmp/a.tar.gz')
    api.importProfile
      .mockRejectedValueOnce(new CoreError('ArchivePassword', 'неверный пароль архива'))
      .mockReturnValueOnce(new Promise(() => {}))
    const wrapper = mountWithApp(ImportMenu)
    await choose(wrapper, 'Архив профиля…')
    const input = wrapper.find('input[type="password"]')
    await input.setValue('pw')
    await wrapper.findAll('button').find((b) => b.text() === 'Импортировать')!.trigger('click')
    await flushPromises()
    expect(input.attributes('readonly')).toBeDefined()
    expect(input.attributes('disabled')).toBeUndefined()
  })

  it('не архив Ombra — уведомление с ошибкой', async () => {
    files.pickArchiveFile.mockResolvedValue('/tmp/x.tar.gz')
    api.importProfile.mockRejectedValue(new CoreError('ArchiveFormat', 'не тот формат'))
    const wrapper = mountWithApp(ImportMenu)
    await choose(wrapper, 'Архив профиля…')
    expect(toastTitles()).toContain('Это не архив профиля Ombra')
  })

  it('пустой пароль архива и повторный Enter во время импорта не запускают импорт', async () => {
    files.pickArchiveFile.mockResolvedValue('/tmp/a.tar.gz')
    let finish!: (value: ProfileRecord) => void
    api.importProfile
      .mockRejectedValueOnce(new CoreError('ArchivePassword', 'нужен пароль'))
      .mockReturnValueOnce(new Promise<ProfileRecord>((resolve) => (finish = resolve)))
    const wrapper = mountWithApp(ImportMenu)
    await choose(wrapper, 'Архив профиля…')
    const input = wrapper.find('input[type="password"]')
    await input.trigger('keydown', { key: 'Enter' })
    expect(api.importProfile).toHaveBeenCalledTimes(1)
    await input.setValue('pw')
    await input.trigger('keydown', { key: 'Enter' })
    await input.trigger('keydown', { key: 'Enter' })
    expect(api.importProfile).toHaveBeenCalledTimes(2)
    finish(record('Магазин'))
    await flushPromises()
  })

  it('в таблице нет колонки name — своё уведомление', async () => {
    files.pickCsvFile.mockResolvedValue('/tmp/p.csv')
    api.importCsvFile.mockRejectedValue(new CoreError('Validation', 'name', [{ field: 'name', code: 'required' }]))
    const wrapper = mountWithApp(ImportMenu)
    await choose(wrapper, 'Таблица CSV…')
    expect(toastTitles()).toContain('В таблице нет колонки name')
  })
})
