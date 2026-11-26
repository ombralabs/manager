import { defineStore } from 'pinia'
import { computed, ref } from 'vue'
import { core } from '../api/core'
import type { ExtCategory, ExtensionRecord } from '../api/types'
import { i18n } from '../i18n'
import { addOutcome, baseName } from '../lib/extensions'
import { toCoreError } from '../api/errors'
import { errorToast, notify, reportError } from '../lib/notify'
import { createRefresher } from '../lib/refresher'
import { useToasts } from './toasts'

export type AddSource = { store: string } | { file: string }

/** Добавление, которое ещё идёт: карточка «Загружается». `key` — ID из ссылки или путь файла. */
export interface PendingExtension {
  key: string
  name: string
  from: 'store' | 'file'
}

/** Библиотека расширений и наборы: экран «Расширения», редактор, «Профили» и «Группы». */
export const useExtensionsStore = defineStore('extensions', () => {
  const extensions = ref<ExtensionRecord[]>([])
  const categories = ref<ExtCategory[]>([])
  const loaded = ref(false)
  const pending = ref<PendingExtension[]>([])

  const refresher = createRefresher(
    () => Promise.all([core.listExtensions(), core.listExtCategories()]),
    ([extensionList, categoryList]) => {
      extensions.value = extensionList
      categories.value = categoryList
      loaded.value = true
    },
  )

  function categoryById(id: number): ExtCategory | undefined {
    return categories.value.find((category) => category.id === id)
  }

  /** Расширения набора в порядке библиотеки; id, которых нет в библиотеке, пропускаются. */
  function extensionsOf(category: ExtCategory | undefined): ExtensionRecord[] {
    if (!category) return []
    const ids = new Set(category.extension_ids)
    return extensions.value.filter((ext) => ids.has(ext.id))
  }

  /** Расширения набора с правом `proxy` или `privacy`. */
  const risky = (category: ExtCategory | undefined) => extensionsOf(category).filter((ext) => ext.gateway_risk)

  /** id наборов, где есть расширение с `gateway_risk`: пометка ⚠ на «Расширениях» и в меню «Профилей». */
  const riskyIds = computed(() => new Set(categories.value.filter((set) => risky(set).length > 0).map((set) => set.id)))

  /** ID в наборе 0: расширение «включено» — его получают профили без своего набора. */
  const enabledIds = computed(() => new Set(categoryById(0)?.extension_ids ?? []))

  /**
   * Состав набора — сразу в списке: галочка уже переключена. Ошибка возвращает прежний (флажок Vue перерисуется
   * только при смене значения) и всплывает; успех перечитывает списки.
   */
  async function setItems(id: number, ids: string[]): Promise<void> {
    const category = categoryById(id)
    if (!category) return
    const before = category.extension_ids
    category.extension_ids = ids
    try {
      await core.setExtCategoryItems(id, ids)
    } catch (error) {
      category.extension_ids = before
      throw error
    }
    await refresher.refresh().catch(reportError)
  }

  /**
   * Добавление в фоне: карточка в `pending`, пока ядро скачивает или распаковывает; тот же `key` во время работы —
   * без второго запроса. Итог — уведомлением; `enable` — новое расширение сразу в набор 0 (включённое не трогается).
   */
  async function add(source: AddSource, enable: boolean): Promise<void> {
    const key = 'store' in source ? source.store : source.file
    if (pending.value.some((item) => item.key === key)) return
    pending.value.push('store' in source ? { key, name: key, from: 'store' } : { key, name: baseName(key), from: 'file' })
    try {
      const record = await ('store' in source ? core.addStoreExtension(source.store) : core.addFileExtension(source.file))
      const title = i18n.global.t(`extensions.${addOutcome(extensions.value, record)}`, { name: record.name, version: record.version })
      if (record.gateway_risk) notify.warning(title, i18n.global.t('extensions.riskText'))
      else notify.info(title)
      const general = categoryById(0)
      if (enable && general && !general.extension_ids.includes(record.id)) {
        // Библиотека ещё без новой записи: `toggledItems` её бы отбросил — id в конец состава.
        try {
          await setItems(0, [...general.extension_ids, record.id])
        } catch (error) {
          notify.error(error)
          await refresher.refresh().catch(reportError)
        }
      } else {
        await refresher.refresh().catch(reportError)
      }
    } catch (error) {
      // Диалог уже закрыт, добавлений может идти несколько — в заголовке имя того, что не удалось.
      const toast = errorToast(toCoreError(error))
      useToasts().push({ kind: 'error', ...toast, title: `${pending.value.find((item) => item.key === key)?.name ?? key}: ${toast.title}` })
    } finally {
      pending.value = pending.value.filter((item) => item.key !== key)
    }
  }

  return {
    extensions,
    categories,
    loaded,
    pending,
    riskyIds,
    enabledIds,
    refresh: refresher.refresh,
    scheduleRefresh: refresher.schedule,
    cancelRefresh: refresher.cancel,
    categoryById,
    extensionsOf,
    risky,
    setItems,
    add,
  }
})
