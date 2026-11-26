import { type VueWrapper, flushPromises } from '@vue/test-utils'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { CoreError } from '../../api/errors'
import type { EngineBuild, EngineCatalog } from '../../api/types'
import { mountWithApp } from '../../test/mount'

const api = vi.hoisted(() => ({ importEngine: vi.fn(), engineCatalog: vi.fn(), updateEngine: vi.fn(), selectEngine: vi.fn(), setEngineAutomatic: vi.fn(), cancelEngineUpdate: vi.fn() }))
vi.mock('../../api/core', () => ({ core: api, events: {} }))
const dialogs = vi.hoisted(() => ({ pickEngineArchive: vi.fn() }))
vi.mock('../../api/dialogs', () => dialogs)

import { notify } from '../../lib/notify'
import { useAppStore } from '../../stores/app'
import { useEnginesStore } from '../../stores/engines'
import EngineCard from './EngineCard.vue'

const BUILD: EngineBuild = { version: '150.0.7871.114', binary: '/tmp/ombra/engines/Chromium.app/Contents/MacOS/Chromium' }
const installed = { ...BUILD, id: 'legacy', chromium: BUILD.version, active: true }
const newer = { ...installed, id: '151.0.7922.108-3', version: '151.0.7922.108-3', chromium: '151.0.7922.108', binary: '/tmp/ombra/engines/versions/151.0.7922.108-3/Chromium.app/Contents/MacOS/Chromium' }
let catalog: EngineCatalog

async function mountCard(engine: EngineBuild | null) {
  catalog = { automatic: true, versions: engine ? [{ ...installed }] : [] }
  api.engineCatalog.mockImplementation(async () => structuredClone(catalog))
  useAppStore().info = { version: '1.1.23', data_dir: '/tmp/ombra', language: 'ru', engine, init_error: null }
  const wrapper = mountWithApp(EngineCard)
  await flushPromises()
  return wrapper
}

function button(wrapper: VueWrapper, label: string) {
  const found = wrapper.findAll('button').find((b) => b.text() === label)
  if (!found) throw new Error(`нет кнопки «${label}»`)
  return found
}

describe('карточка «Браузер»', () => {
  beforeEach(() => {
    vi.resetAllMocks()
    vi.spyOn(notify, 'info').mockImplementation(() => {})
    vi.spyOn(notify, 'error').mockImplementation(() => undefined as never)
  })

  it('показывает установленную версию и разрешает проверить обновление', async () => {
    const wrapper = await mountCard(BUILD)
    expect(wrapper.text()).toContain('Chromium 150')
    expect(wrapper.text()).toContain('Версия 150.0.7871.114')
    api.updateEngine.mockResolvedValue({ state: 'current', catalog })
    await button(wrapper, 'Проверить и обновить').trigger('click')
    await flushPromises()
    expect(api.updateEngine).toHaveBeenCalledWith(false)
    expect(wrapper.text()).toContain('более новой стабильной версии нет')
  })

  it('без браузера предлагает скачивание и ручной импорт', async () => {
    const wrapper = await mountCard(null)
    expect(wrapper.text()).toContain('не установлен')
    expect(wrapper.text()).toContain('Скачайте стабильную версию')
    expect(button(wrapper, 'Скачать браузер').attributes('aria-disabled')).not.toBe('true')
    expect(button(wrapper, 'Выбрать архив…').exists()).toBe(true)
  })

  it('отмена диалога ничего не меняет', async () => {
    dialogs.pickEngineArchive.mockResolvedValue(null)
    const wrapper = await mountCard(null)
    await button(wrapper, 'Выбрать архив…').trigger('click')
    await flushPromises()
    expect(api.importEngine).not.toHaveBeenCalled()
    expect(notify.info).not.toHaveBeenCalled()
  })

  it('импорт добавляет браузер в список, сохраняя предыдущий', async () => {
    dialogs.pickEngineArchive.mockResolvedValue('/tmp/browser.tar.gz')
    const wrapper = await mountCard(BUILD)
    api.importEngine.mockImplementation(async () => {
      catalog = { automatic: false, versions: [{ ...newer }, { ...installed, active: false }] }
      return { version: newer.chromium, binary: newer.binary }
    })
    await button(wrapper, 'Выбрать архив…').trigger('click')
    await flushPromises()
    expect(api.importEngine).toHaveBeenCalledWith('/tmp/browser.tar.gz')
    expect(wrapper.text()).toContain('Chromium 151')
    expect(wrapper.findAll('.versions li')).toHaveLength(2)
    expect(notify.info).toHaveBeenCalledWith('Браузер установлен')
  })

  it('ошибка загрузки показана рядом с действием, установленная версия остаётся', async () => {
    const wrapper = await mountCard(BUILD)
    api.updateEngine.mockRejectedValue(new CoreError('EngineUpdateUnavailable', 'network'))
    await button(wrapper, 'Проверить и обновить').trigger('click')
    await flushPromises()
    expect(wrapper.get('[role="alert"]').text()).toContain('Не удалось')
    expect(wrapper.text()).toContain('Версия 150.0.7871.114')
    expect(button(wrapper, 'Проверить и обновить').attributes('aria-disabled')).not.toBe('true')
  })

  it('выбор предыдущей версии выключает автоматическое обновление', async () => {
    const wrapper = await mountCard(BUILD)
    catalog = { automatic: true, versions: [{ ...newer }, { ...installed, active: false }] }
    await useEnginesStore().load()
    await flushPromises()
    api.selectEngine.mockImplementation(async () => {
      catalog = { automatic: false, versions: [{ ...newer, active: false }, { ...installed }] }
      return BUILD
    })
    await wrapper.get('[aria-label="Использовать версию 150.0.7871.114"]').trigger('click')
    await flushPromises()
    expect(api.selectEngine).toHaveBeenCalledWith('legacy')
    expect(wrapper.get('[role="switch"]').attributes('aria-checked')).toBe('false')
    expect(wrapper.text()).toContain('Chromium 150')
    expect(wrapper.findAll('.versions li')).toHaveLength(2)
  })

  it('показывает прогресс, блокирует смену версии и позволяет отменить скачивание', async () => {
    const wrapper = await mountCard(BUILD)
    let rejectDownload!: (error: CoreError) => void
    api.updateEngine.mockImplementation(() => new Promise((_, reject) => { rejectDownload = reject }))
    api.cancelEngineUpdate.mockImplementation(async () => { rejectDownload(new CoreError('EngineUpdateCancelled', 'cancelled')) })
    await button(wrapper, 'Проверить и обновить').trigger('click')
    useEnginesStore().onProgress({ phase: 'downloading', version: newer.version, downloaded: 250, total: 1000 })
    await flushPromises()
    expect(wrapper.get('progress').attributes('value')).toBe('25')
    expect(button(wrapper, 'Выбрать архив…').attributes('aria-disabled')).toBe('true')
    await button(wrapper, 'Отмена').trigger('click')
    await flushPromises()
    expect(api.cancelEngineUpdate).toHaveBeenCalledOnce()
    expect(wrapper.find('progress').exists()).toBe(false)
    expect(wrapper.text()).toContain('Скачивание отменено')
    expect(wrapper.text()).toContain('Chromium 150')
  })
})
