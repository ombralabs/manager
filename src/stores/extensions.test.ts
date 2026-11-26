import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { ExtensionRecord } from '../api/types'
import { extCategory, extension, riskyExtension } from '../test/fixtures'

const api = vi.hoisted(() => ({
  listExtensions: vi.fn(),
  listExtCategories: vi.fn(),
  setExtCategoryItems: vi.fn(),
  addStoreExtension: vi.fn(),
  addFileExtension: vi.fn(),
}))
vi.mock('../api/core', () => ({ core: api, events: {} }))

import { useExtensionsStore } from './extensions'
import { useToasts } from './toasts'

beforeEach(() => {
  for (const mock of Object.values(api)) mock.mockReset()
})

describe('хранилище расширений', () => {
  it('refresh читает оба списка; расширения набора — в порядке библиотеки, без неизвестных; рискованные', async () => {
    api.listExtensions.mockResolvedValue([extension(), riskyExtension])
    api.listExtCategories.mockResolvedValue([extCategory({ extension_ids: [riskyExtension.id, 'gone', extension().id] })])
    const store = useExtensionsStore()
    expect(store.loaded).toBe(false)
    await store.refresh()
    expect(store.loaded).toBe(true)
    const general = store.categoryById(0)
    expect(store.extensionsOf(general).map((ext) => ext.name)).toEqual(['uBlock Origin', 'ZeroOmega'])
    expect(store.risky(general).map((ext) => ext.name)).toEqual(['ZeroOmega'])
    expect(store.categoryById(9)).toBeUndefined()
    expect(store.extensionsOf(undefined)).toEqual([])
    expect(store.risky(undefined)).toEqual([])
  })

  it('riskyIds — наборы с рискованным расширением', async () => {
    api.listExtensions.mockResolvedValue([extension(), riskyExtension])
    api.listExtCategories.mockResolvedValue([extCategory(), extCategory({ id: 1, name: 'Работа', extension_ids: [riskyExtension.id] })])
    const store = useExtensionsStore()
    await store.refresh()
    expect([...store.riskyIds]).toEqual([1])
  })

  it('setItems: состав сразу в списке; ошибка возвращает прежний и всплывает; успех перечитывает', async () => {
    api.listExtensions.mockResolvedValue([extension(), riskyExtension])
    api.listExtCategories.mockResolvedValue([extCategory({ extension_ids: [riskyExtension.id] })])
    const store = useExtensionsStore()
    await store.refresh()
    let fail!: (error: unknown) => void
    api.setExtCategoryItems.mockReturnValueOnce(new Promise((_, reject) => (fail = reject)))
    const pending = store.setItems(0, [extension().id, riskyExtension.id])
    expect(store.categoryById(0)!.extension_ids).toEqual([extension().id, riskyExtension.id])
    fail({ code: 'Storage', message: 'диск полон' })
    await expect(pending).rejects.toMatchObject({ code: 'Storage' })
    expect(store.categoryById(0)!.extension_ids).toEqual([riskyExtension.id])

    api.setExtCategoryItems.mockResolvedValueOnce(extCategory({ extension_ids: [] }))
    api.listExtCategories.mockClear()
    await store.setItems(0, [])
    expect(api.setExtCategoryItems).toHaveBeenLastCalledWith(0, [])
    expect(api.listExtCategories).toHaveBeenCalledTimes(1)
  })

  it('enabledIds — состав набора 0', async () => {
    api.listExtensions.mockResolvedValue([extension(), riskyExtension])
    api.listExtCategories.mockResolvedValue([extCategory({ extension_ids: [riskyExtension.id] }), extCategory({ id: 1, name: 'Крипто', extension_ids: [extension().id] })])
    const store = useExtensionsStore()
    expect([...store.enabledIds]).toEqual([])
    await store.refresh()
    expect([...store.enabledIds]).toEqual([riskyExtension.id])
  })

  it('add из магазина: карточка «Загружается», повтор того же ID — без запроса; итог — уведомлением; enable — в набор 0', async () => {
    api.listExtensions.mockResolvedValue([riskyExtension])
    api.listExtCategories.mockResolvedValue([extCategory({ extension_ids: [riskyExtension.id] })])
    const store = useExtensionsStore()
    await store.refresh()
    let resolve!: (record: ExtensionRecord) => void
    api.addStoreExtension.mockReturnValueOnce(new Promise((done) => (resolve = done)))
    api.setExtCategoryItems.mockResolvedValueOnce(extCategory({ extension_ids: [riskyExtension.id, extension().id] }))
    const adding = store.add({ store: extension().id }, true)
    expect(store.pending).toEqual([{ key: extension().id, name: extension().id, from: 'store' }])
    await store.add({ store: extension().id }, true)
    expect(api.addStoreExtension).toHaveBeenCalledTimes(1)
    resolve(extension())
    await adding
    expect(store.pending).toEqual([])
    expect(api.setExtCategoryItems).toHaveBeenCalledWith(0, [riskyExtension.id, extension().id])
    expect(useToasts().items.map((toast) => toast.title)).toContain('Добавлено: uBlock Origin 1.62.0')
  })

  it('add: уже включённое и enable=false набор 0 не трогают; ошибка — уведомление, карточка уходит', async () => {
    api.listExtensions.mockResolvedValue([extension()])
    api.listExtCategories.mockResolvedValue([extCategory({ extension_ids: [extension().id] })])
    const store = useExtensionsStore()
    await store.refresh()
    api.addFileExtension.mockResolvedValueOnce(extension())
    const adding = store.add({ file: '/Users/demo/Downloads/ublock.zip' }, true)
    expect(store.pending).toEqual([{ key: '/Users/demo/Downloads/ublock.zip', name: 'ublock.zip', from: 'file' }])
    await adding
    api.addFileExtension.mockResolvedValueOnce(riskyExtension)
    await store.add({ file: '/tmp/omega.crx' }, false)
    expect(api.setExtCategoryItems).not.toHaveBeenCalled()
    api.addStoreExtension.mockRejectedValueOnce({ code: 'ExtensionDownload', message: 'Chrome Web Store ответил 404' })
    await store.add({ store: riskyExtension.id }, true)
    expect(store.pending).toEqual([])
    const failed = useToasts().items.find((toast) => toast.kind === 'error')
    expect(failed?.title).toContain(riskyExtension.id)
    expect(failed?.body).toBe('Chrome Web Store ответил 404')
  })
})
