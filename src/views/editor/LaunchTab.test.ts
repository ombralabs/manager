import { flushPromises } from '@vue/test-utils'
import { SelectRoot } from 'reka-ui'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { reactive } from 'vue'
import { i18n } from '../../i18n'
import { type EditorModel, emptyModel } from '../../lib/profileForm'
import { extCategory, extension, fingerprint, riskyExtension } from '../../test/fixtures'
import { mountWithApp } from '../../test/mount'

vi.mock('../../api/core', () => ({ core: {}, events: {} }))

import { useExtensionsStore } from '../../stores/extensions'
import LaunchTab from './LaunchTab.vue'

function fillStore() {
  const store = useExtensionsStore()
  store.extensions = [extension(), riskyExtension]
  store.categories = [
    extCategory({ extension_ids: [] }),
    extCategory({ id: 1, name: 'Работа', extension_ids: [extension().id, riskyExtension.id] }),
  ]
  store.loaded = true
}

/** Модель реактивна, как у редактора: вкладка правит её поля на месте. */
function mountTab(patch: Partial<EditorModel> = {}, errors: { field: string; code: string }[] = []) {
  const model = reactive({ ...emptyModel(structuredClone(fingerprint)), ...patch })
  const wrapper = mountWithApp(LaunchTab, { props: { modelValue: model, errors } })
  return { wrapper, model }
}

describe('вкладка «Запуск»: набор расширений', () => {
  afterEach(() => {
    i18n.global.locale.value = 'ru'
  })

  it('по-английски набор 0 — “No set”, кириллицы в пунктах нет', () => {
    fillStore()
    i18n.global.locale.value = 'en'
    const { wrapper } = mountTab()
    expect(wrapper.find('#ext-set').text()).toContain('No set')
    expect(wrapper.find('#ext-set').text()).not.toMatch(/[А-Яа-я]/)
  })

  it('пункты — имя и число расширений; выбор меняет набор модели; имена набора и предупреждение о риске', async () => {
    fillStore()
    const { wrapper, model } = mountTab()
    expect(wrapper.find('#ext-set').text()).toContain('Без набора · нет расширений')
    expect(wrapper.text()).toContain('В наборе нет расширений')
    expect(wrapper.find('.risk-note').exists()).toBe(false)
    // Выбор набора — единственный выпадающий список вкладки.
    wrapper.findAllComponents(SelectRoot)[0]!.vm.$emit('update:modelValue', '1')
    await flushPromises()
    expect(model.extCategoryId).toBe(1)
    expect(wrapper.find('#ext-set').text()).toContain('Работа · 2 расширения')
    expect(wrapper.text()).toContain('uBlock Origin, ZeroOmega')
    expect(wrapper.find('.risk-note').text()).toContain('ZeroOmega может изменить настройки прокси и направить соединения в обход прокси профиля')
    expect(wrapper.text()).toContain('Применяется при следующем запуске профиля.')
  })

  it('список не загружен — «…» и поле неактивно; набор удалён — «Набор удалён», имён нет, ошибка ядра под полем', () => {
    const loading = mountTab({ extCategoryId: 7 }).wrapper
    expect(loading.find('#ext-set').attributes('disabled')).toBeDefined()
    expect(loading.find('#ext-set').text()).toBe('…')
    fillStore()
    const deleted = mountTab({ extCategoryId: 7 }, [{ field: 'ext_category_id', code: 'invalid' }]).wrapper
    expect(deleted.find('#ext-set').attributes('disabled')).toBeUndefined()
    expect(deleted.find('#ext-set').text()).toBe('Набор удалён')
    expect(deleted.text()).not.toContain('В наборе нет расширений')
    expect(deleted.text()).toContain('Набор удалён, выберите другой')
  })
})
