import { flushPromises } from '@vue/test-utils'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { CoreError } from '../../api/errors'
import { profileRow } from '../../test/fixtures'
import { mountWithApp } from '../../test/mount'

const api = vi.hoisted(() => ({ exportProfile: vi.fn() }))
const files = vi.hoisted(() => ({ pickSaveArchive: vi.fn() }))
vi.mock('../../api/core', () => ({ core: api, events: {} }))
vi.mock('../../api/dialogs', () => files)

import { useToasts } from '../../stores/toasts'
import ArchiveExportDialog from './ArchiveExportDialog.vue'

const toastTitles = () => useToasts().items.map((toast) => toast.title)
const mountDialog = () =>
  mountWithApp(ArchiveExportDialog, { props: { open: true, row: profileRow({ id: 'p1', name: 'Shop' }) } })
const save = (wrapper: ReturnType<typeof mountWithApp>) => wrapper.findAll('button').find((b) => b.text() === 'Сохранить…')!

beforeEach(() => {
  api.exportProfile.mockReset()
  files.pickSaveArchive.mockReset()
})

describe('диалог «Экспорт архива»', () => {
  it('без пароля: имя файла по профилю, пароль null', async () => {
    files.pickSaveArchive.mockResolvedValue('/tmp/Shop.tar.gz')
    api.exportProfile.mockResolvedValue(null)
    const wrapper = mountDialog()
    expect(wrapper.text()).toContain('Чтобы включить в архив логин, пароль, ключ 2FA и пароль прокси, задайте пароль архива.')
    await save(wrapper).trigger('click')
    await flushPromises()
    expect(files.pickSaveArchive).toHaveBeenCalledWith('Shop.tar.gz')
    expect(api.exportProfile).toHaveBeenCalledWith('p1', '/tmp/Shop.tar.gz', null)
    expect(toastTitles()).toContain('Архив сохранён')
    expect(wrapper.emitted('update:open')?.at(-1)).toEqual([false])
  })

  it('пароли не совпадают — ошибка у второго поля, кнопка неактивна; совпали — уходит пароль', async () => {
    files.pickSaveArchive.mockResolvedValue('/tmp/Shop.tar.gz')
    api.exportProfile.mockResolvedValue(null)
    const wrapper = mountDialog()
    const [first, second] = wrapper.findAll('input[type="password"]')
    await first!.setValue('pw')
    await second!.setValue('px')
    expect(wrapper.text()).toContain('Пароли не совпадают')
    expect(save(wrapper).attributes('disabled')).toBeDefined()
    await second!.setValue('pw')
    await save(wrapper).trigger('click')
    await flushPromises()
    expect(api.exportProfile).toHaveBeenCalledWith('p1', '/tmp/Shop.tar.gz', 'pw')
  })

  it('отмена окна сохранения — диалог открыт; ошибка ядра — уведомлением', async () => {
    files.pickSaveArchive.mockResolvedValueOnce(null)
    const wrapper = mountDialog()
    await save(wrapper).trigger('click')
    await flushPromises()
    expect(api.exportProfile).not.toHaveBeenCalled()
    expect(wrapper.emitted('update:open')).toBeUndefined()
    files.pickSaveArchive.mockResolvedValueOnce('/tmp/Shop.tar.gz')
    api.exportProfile.mockRejectedValue(new CoreError('ProfileRunning', 'сначала остановите профиль'))
    await save(wrapper).trigger('click')
    await flushPromises()
    expect(toastTitles().length).toBe(1)
  })
})
