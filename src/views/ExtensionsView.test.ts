import { flushPromises } from '@vue/test-utils'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { createMemoryHistory, createRouter } from 'vue-router'
import type { ExtCategory } from '../api/types'
import { extCategory, extension, riskyExtension } from '../test/fixtures'
import { mountWithApp } from '../test/mount'

const api = vi.hoisted(() => ({
  listExtensions: vi.fn(),
  listExtCategories: vi.fn(),
  addStoreExtension: vi.fn(),
  addFileExtension: vi.fn(),
  openStoreSearch: vi.fn(),
  openStorePage: vi.fn(),
  updateExtension: vi.fn(),
  removeExtension: vi.fn(),
  createExtCategory: vi.fn(),
  updateExtCategory: vi.fn(),
  setExtCategoryItems: vi.fn(),
  deleteExtCategory: vi.fn(),
}))
const events = vi.hoisted(() => ({ onProfilesChanged: vi.fn() }))
const dialogs = vi.hoisted(() => ({ pickExtensionFile: vi.fn(), onFileDrop: vi.fn() }))
vi.mock('../api/core', () => ({ core: api, events }))
vi.mock('../api/dialogs', () => dialogs)

import { useExtensionsStore } from '../stores/extensions'
import { useToasts } from '../stores/toasts'
import ExtensionsView from './ExtensionsView.vue'

type Wrapper = ReturnType<typeof mountWithApp>

/** В документе — иначе фокус полей не проверить; снимается после теста. */
const mounted: Wrapper[] = []
afterEach(() => mounted.splice(0).forEach((wrapper) => wrapper.unmount()))

async function mountView(path = '/extensions') {
  const router = createRouter({
    history: createMemoryHistory(),
    routes: [
      { path: '/extensions', component: ExtensionsView },
      { path: '/extensions/sets', component: ExtensionsView },
    ],
  })
  await router.push(path)
  const wrapper = mountWithApp(ExtensionsView, { global: { plugins: [router] }, attachTo: document.body })
  mounted.push(wrapper)
  await flushPromises()
  return { wrapper, router }
}

const toasts = () => useToasts().items.map(({ kind, title, body }) => ({ kind, title, body }))
const byText = (wrapper: Wrapper, selector: string, text: string) => {
  const found = wrapper.findAll(selector).find((element) => element.text() === text)
  if (!found) throw new Error(`нет «${text}» в ${selector}`)
  return found
}
const cardOf = (wrapper: Wrapper, name: string) => {
  const found = wrapper.findAll('[data-test="ext-card"]').find((card) => card.find('.name-text').text() === name)
  if (!found) throw new Error(`нет карточки «${name}»`)
  return found
}
const names = (wrapper: Wrapper) => wrapper.findAll('[data-test="ext-card"] .name-text').map((name) => name.text())

async function openMenu(trigger: ReturnType<Wrapper['find']>) {
  await trigger.trigger('keydown', { key: 'ArrowDown' })
  await flushPromises()
}

async function cardAction(wrapper: Wrapper, name: string, item: string) {
  await openMenu(cardOf(wrapper, name).find(`button[aria-label="Ещё действия: ${name}"]`))
  await byText(wrapper, '[role="menuitem"]', item).trigger('click')
  await flushPromises()
}

const CRYPTO = extCategory({ id: 1, name: 'Крипто', extension_ids: [riskyExtension.id], profile_count: 3 })

beforeEach(() => {
  for (const mock of [...Object.values(api), ...Object.values(dialogs)]) mock.mockReset()
  api.listExtensions.mockResolvedValue([extension(), riskyExtension])
  // uBlock включён (в наборе 0), ZeroOmega — только в наборе «Крипто».
  api.listExtCategories.mockResolvedValue([extCategory({ extension_ids: [extension().id] }), CRYPTO])
  api.openStoreSearch.mockResolvedValue(null)
  api.openStorePage.mockResolvedValue(null)
  dialogs.onFileDrop.mockResolvedValue(() => {})
  events.onProfilesChanged.mockReset()
  events.onProfilesChanged.mockResolvedValue(() => {})
})

describe('экран «Расширения»: вкладки', () => {
  it('вкладки — ссылки с aria-current; подзаголовок и кнопка — по вкладке; счётчики без набора 0', async () => {
    const { wrapper, router } = await mountView()
    const tabs = () => wrapper.findAll('nav[aria-label="Разделы расширений"] a')
    expect(tabs().map((tab) => tab.text())).toEqual(['Все расширения2', 'Наборы1'])
    expect(tabs().map((tab) => tab.attributes('aria-current'))).toEqual(['page', undefined])
    expect(wrapper.find('.head .hint').text()).toBe('2 расширения · 1 включено')
    expect(wrapper.find('.head button').text()).toBe('Добавить расширение')
    await tabs()[1]!.trigger('click')
    await flushPromises()
    expect(router.currentRoute.value.path).toBe('/extensions/sets')
    expect(tabs().map((tab) => tab.attributes('aria-current'))).toEqual([undefined, 'page'])
    expect(wrapper.find('.head .hint').text()).toBe('1 набор · 3 профиля с набором')
    expect(wrapper.find('.head button').text()).toBe('Новый набор')
  })

  it('перечитывается по событию profiles-changed и отписывается при закрытии', async () => {
    const unlisten = vi.fn()
    events.onProfilesChanged.mockResolvedValue(unlisten)
    const { wrapper } = await mountView()
    expect(api.listExtCategories).toHaveBeenCalledTimes(1)
    vi.useFakeTimers()
    try {
      events.onProfilesChanged.mock.calls[0]![0]()
      await vi.advanceTimersByTimeAsync(500)
    } finally {
      vi.useRealTimers()
    }
    expect(api.listExtCategories).toHaveBeenCalledTimes(2)
    mounted.splice(mounted.indexOf(wrapper), 1)
    wrapper.unmount()
    expect(unlisten).toHaveBeenCalled()
  })
})

describe('экран «Расширения»: все расширения', () => {
  it('карточки: включённое и выключенное, наборы без набора 0; фильтр со счётчиками; поиск без учёта регистра и «ё»', async () => {
    api.listExtensions.mockResolvedValue([extension(), riskyExtension, extension({ id: 'nkbihfbeogaeaoehlefnkodbefgpgknn', name: 'Ёжик Тестов' })])
    const { wrapper } = await mountView()
    expect(names(wrapper)).toEqual(['uBlock Origin', 'ZeroOmega', 'Ёжик Тестов'])
    expect(cardOf(wrapper, 'uBlock Origin').find('[role="switch"]').attributes('aria-checked')).toBe('true')
    expect(cardOf(wrapper, 'ZeroOmega').find('[role="switch"]').attributes('aria-checked')).toBe('false')
    expect(cardOf(wrapper, 'uBlock Origin').find('.source').text()).toBe('Источник: Chrome Web Store · вне наборов')
    expect(cardOf(wrapper, 'ZeroOmega').find('.source').text()).toBe('Источник: Chrome Web Store · наборы: Крипто')
    const segments = wrapper.findAll('[aria-label="Показывать"] button')
    expect(segments.map((segment) => segment.text())).toEqual(['Все: 3', 'Включённые: 1', 'Выключенные: 2'])
    await segments[2]!.trigger('click')
    expect(names(wrapper)).toEqual(['ZeroOmega', 'Ёжик Тестов'])
    await wrapper.find('input[type="search"]').setValue('ЕЖИК')
    expect(names(wrapper)).toEqual(['Ёжик Тестов'])
    await wrapper.find('input[type="search"]').setValue('нет такого')
    expect(wrapper.text()).toContain('Ничего не найдено')
  })

  it('переключатель шлёт состав набора 0 в порядке библиотеки; пока идёт запрос — aria-disabled и без второго запроса; ошибка — назад', async () => {
    let resolve!: (set: ExtCategory) => void
    api.setExtCategoryItems.mockReturnValueOnce(new Promise((done) => (resolve = done)))
    const { wrapper } = await mountView()
    const toggle = (name: string) => cardOf(wrapper, name).find('[role="switch"]')
    await toggle('ZeroOmega').trigger('click')
    await flushPromises()
    expect(api.setExtCategoryItems).toHaveBeenCalledWith(0, [extension().id, riskyExtension.id])
    expect(toggle('ZeroOmega').attributes('aria-disabled')).toBe('true')
    await toggle('uBlock Origin').trigger('click')
    expect(api.setExtCategoryItems).toHaveBeenCalledTimes(1)
    resolve(extCategory({ extension_ids: [extension().id, riskyExtension.id] }))
    await flushPromises()
    api.setExtCategoryItems.mockRejectedValueOnce({ code: 'Storage', message: 'диск полон' })
    await toggle('uBlock Origin').trigger('click')
    await flushPromises()
    expect(toggle('uBlock Origin').attributes('aria-checked')).toBe('true')
    expect(toasts()).toContainEqual({ kind: 'error', title: 'Ошибка базы данных', body: 'диск полон' })
  })

  it('«Chrome Web Store» на карточке — страница расширения; «Открыть Chrome Web Store» — главная магазина', async () => {
    const { wrapper } = await mountView()
    await cardOf(wrapper, 'ZeroOmega').find('.source button').trigger('click')
    expect(api.openStorePage).toHaveBeenCalledWith(riskyExtension.id)
    await byText(wrapper, 'button', 'Открыть Chrome Web Store').trigger('click')
    expect(api.openStoreSearch).toHaveBeenCalledWith('')
  })

  it('обновление из Web Store: нет новой версии, новая версия, новое право proxy — предупреждение', async () => {
    const { wrapper } = await mountView()
    api.updateExtension.mockResolvedValueOnce(extension())
    await cardAction(wrapper, 'uBlock Origin', 'Обновить из Web Store')
    expect(api.updateExtension).toHaveBeenCalledWith(extension().id)
    expect(toasts()).toContainEqual({ kind: 'info', title: 'Новой версии нет: uBlock Origin 1.62.0', body: undefined })
    api.updateExtension.mockResolvedValueOnce(extension({ version: '1.63.0', gateway_risk: true }))
    await cardAction(wrapper, 'uBlock Origin', 'Обновить из Web Store')
    expect(toasts()).toContainEqual({
      kind: 'warning',
      title: 'Обновлено: uBlock Origin 1.63.0',
      body: 'Расширение может изменить настройки прокси и направить соединения в обход прокси профиля.',
    })
  })

  it('удаление: подтверждение называет наборы кроме набора 0; только в наборе 0 — про данные в профилях', async () => {
    api.listExtCategories.mockResolvedValue([extCategory({ extension_ids: [extension().id, riskyExtension.id] }), CRYPTO])
    api.removeExtension.mockResolvedValue(null)
    const { wrapper } = await mountView()
    await cardAction(wrapper, 'ZeroOmega', 'Удалить из библиотеки')
    const confirm = () => wrapper.find('[role="alertdialog"]')
    expect(confirm().text()).toContain('Удалить «ZeroOmega» из библиотеки?')
    expect(confirm().text()).toContain('Расширение будет удалено из наборов: Крипто. Его данные в профилях сохранятся.')
    api.listExtensions.mockClear()
    await byText(wrapper, '[role="alertdialog"] button', 'Удалить').trigger('click')
    await flushPromises()
    expect(api.removeExtension).toHaveBeenCalledWith(riskyExtension.id)
    expect(toasts()).toContainEqual({ kind: 'info', title: 'Удалено из библиотеки: ZeroOmega', body: undefined })
    expect(api.listExtensions).toHaveBeenCalled()
    await cardAction(wrapper, 'uBlock Origin', 'Удалить из библиотеки')
    expect(confirm().text()).toContain('Данные расширения в профилях останутся.')
    expect(confirm().text()).not.toContain('уйдёт из наборов')
  })

  it('пустая библиотека — пояснение и кнопка добавления', async () => {
    api.listExtensions.mockResolvedValue([])
    const { wrapper } = await mountView()
    expect(wrapper.find('.empty').text()).toContain('Библиотека пуста. Добавьте расширение из Chrome Web Store или из файла.')
    expect(wrapper.find('.empty button').text()).toBe('Добавить расширение')
    expect(wrapper.find('[data-test="ext-card"]').exists()).toBe(false)
  })

  it('добавление: диалог закрывается сразу, карточка «Загружается», затем карточка, «Добавлено» и включено', async () => {
    let resolve!: (record: ReturnType<typeof extension>) => void
    const lite = extension({ id: 'ddkjiahejlhfcafbddmgiahcphecmpfh', name: 'uBlock Origin Lite', version: '2025.1.1' })
    api.addStoreExtension.mockReturnValueOnce(new Promise((done) => (resolve = done)))
    api.setExtCategoryItems.mockResolvedValueOnce(extCategory({ extension_ids: [extension().id, lite.id] }))
    const { wrapper } = await mountView()
    await byText(wrapper, '.head button', 'Добавить расширение').trigger('click')
    await flushPromises()
    await wrapper.find('#ext-url').setValue(lite.id)
    await byText(wrapper, '[role="dialog"] button', 'Добавить').trigger('click')
    await flushPromises()
    expect(wrapper.find('#ext-url').exists()).toBe(false)
    expect(cardOf(wrapper, lite.id).find('.status').text()).toBe('Загружается')
    api.listExtensions.mockResolvedValue([extension(), riskyExtension, lite])
    api.listExtCategories.mockResolvedValue([extCategory({ extension_ids: [extension().id, lite.id] }), CRYPTO])
    resolve(lite)
    await flushPromises()
    expect(api.setExtCategoryItems).toHaveBeenCalledWith(0, [extension().id, lite.id])
    expect(cardOf(wrapper, 'uBlock Origin Lite').find('.status').text()).toBe('Доступно')
    expect(cardOf(wrapper, 'uBlock Origin Lite').find('[role="switch"]').attributes('aria-checked')).toBe('true')
    expect(wrapper.findAll('[data-test="ext-card"] .status').some((status) => status.text() === 'Загружается')).toBe(false)
    expect(toasts()).toContainEqual({ kind: 'info', title: 'Добавлено: uBlock Origin Lite 2025.1.1', body: undefined })
  })

  it('не расширение из файла — уведомление с причиной после закрытия диалога, карточки «Загружается» нет', async () => {
    dialogs.pickExtensionFile.mockResolvedValueOnce('/tmp/big.zip')
    api.addFileExtension.mockRejectedValueOnce({
      code: 'ExtensionFormat',
      message: 'пакет больше допустимого',
      fields: [{ field: 'package', code: 'too_large' }],
    })
    const { wrapper } = await mountView()
    await byText(wrapper, '.head button', 'Добавить расширение').trigger('click')
    await flushPromises()
    await byText(wrapper, '[role="dialog"] button', 'Выбрать файл').trigger('click')
    await flushPromises()
    await byText(wrapper, '[role="dialog"] button', 'Добавить').trigger('click')
    await flushPromises()
    expect(api.addFileExtension).toHaveBeenCalledWith('/tmp/big.zip')
    expect(toasts()).toContainEqual({
      kind: 'error',
      title: 'big.zip: Файл не является расширением Chrome или не поддерживается',
      body: 'Пакет больше 100 МБ или в нём больше 10 000 файлов',
    })
    expect(wrapper.findAll('[data-test="ext-card"] .status').some((status) => status.text() === 'Загружается')).toBe(false)
  })
})

const rows = (wrapper: Wrapper) => wrapper.findAll('[data-test="composition-row"]')
const setNames = (wrapper: Wrapper) => wrapper.findAll('[data-test="set-row"] .name-text').map((name) => name.text())
const compositionTitle = (wrapper: Wrapper) => wrapper.find('[aria-label="Состав набора"] .card-head h2').text()

describe('экран «Расширения»: наборы', () => {
  it('список без набора 0, выбран первый; переключатели состава; «только в наборах» у не включённых', async () => {
    const { wrapper } = await mountView('/extensions/sets')
    expect(setNames(wrapper)).toEqual(['Крипто'])
    expect(wrapper.find('[data-test="set-row"] .pick').attributes('aria-current')).toBe('true')
    expect(compositionTitle(wrapper)).toBe('Состав набора «Крипто»')
    expect(rows(wrapper).map((row) => row.find('[role="switch"]').attributes('aria-checked'))).toEqual(['false', 'true'])
    expect(rows(wrapper).map((row) => row.find('.badge').exists())).toEqual([false, true])
  })

  it('переключатель шлёт полный состав набора; ошибка — откат и уведомление', async () => {
    api.setExtCategoryItems.mockRejectedValueOnce({ code: 'Storage', message: 'диск полон' })
    const { wrapper } = await mountView('/extensions/sets')
    await rows(wrapper)[0]!.find('[role="switch"]').trigger('click')
    await flushPromises()
    expect(api.setExtCategoryItems).toHaveBeenCalledWith(1, [extension().id, riskyExtension.id])
    expect(rows(wrapper)[0]!.find('[role="switch"]').attributes('aria-checked')).toBe('false')
    expect(toasts()).toContainEqual({ kind: 'error', title: 'Ошибка базы данных', body: 'диск полон' })
  })

  it('«Новый набор» из шапки: пустое название — ошибка до ядра; «занято» — у поля; создание выбирает новый набор', async () => {
    const { wrapper } = await mountView('/extensions/sets')
    await byText(wrapper, '.head button', 'Новый набор').trigger('click')
    await flushPromises()
    const dialog = () => wrapper.find('[role="dialog"]')
    await byText(wrapper, '[role="dialog"] button', 'Создать').trigger('click')
    await flushPromises()
    expect(dialog().text()).toContain('Укажите название')
    expect(api.createExtCategory).not.toHaveBeenCalled()
    for (const reserved of ['Без набора', 'no set', 'общий']) {
      await wrapper.find('#ext-set-name').setValue(reserved)
      await byText(wrapper, '[role="dialog"] button', 'Создать').trigger('click')
      await flushPromises()
      expect(dialog().text()).toContain('Это название занято встроенным набором «Без набора»')
      expect(document.activeElement).toBe(wrapper.find('#ext-set-name').element)
    }
    expect(api.createExtCategory).not.toHaveBeenCalled()
    api.createExtCategory.mockRejectedValueOnce({ code: 'Validation', message: 'поля', fields: [{ field: 'name', code: 'taken' }] })
    await wrapper.find('#ext-set-name').setValue('Крипто')
    await byText(wrapper, '[role="dialog"] button', 'Создать').trigger('click')
    await flushPromises()
    expect(dialog().text()).toContain('Набор с таким названием уже есть')
    expect(document.activeElement).toBe(wrapper.find('#ext-set-name').element)
    const social = extCategory({ id: 5, name: 'Соцсети', remark: 'Для TikTok' })
    api.createExtCategory.mockResolvedValueOnce(social)
    api.listExtCategories.mockResolvedValue([extCategory(), CRYPTO, social])
    await wrapper.find('#ext-set-name').setValue('Соцсети')
    await wrapper.find('#ext-set-remark').setValue('Для TikTok')
    await byText(wrapper, '[role="dialog"] button', 'Создать').trigger('click')
    await flushPromises()
    expect(api.createExtCategory).toHaveBeenLastCalledWith('Соцсети', 'Для TikTok')
    expect(dialog().exists()).toBe(false)
    expect(compositionTitle(wrapper)).toBe('Состав набора «Соцсети»')
  })

  it('ошибка чужого поля в диалоге набора — уведомление', async () => {
    const { wrapper } = await mountView('/extensions/sets')
    await byText(wrapper, '.head button', 'Новый набор').trigger('click')
    await flushPromises()
    api.createExtCategory.mockRejectedValueOnce({ code: 'Validation', message: 'чужое', fields: [{ field: 'other', code: 'invalid' }] })
    await wrapper.find('#ext-set-name').setValue('Новый')
    await byText(wrapper, '[role="dialog"] button', 'Создать').trigger('click')
    await flushPromises()
    expect(toasts().some((toast) => toast.kind === 'error')).toBe(true)
  })

  it('карандаш: поля заполнены; ошибка примечания — у поля; сохранение', async () => {
    api.listExtCategories.mockResolvedValue([extCategory(), { ...CRYPTO, remark: 'старое' }])
    const { wrapper } = await mountView('/extensions/sets')
    await wrapper.find('button[aria-label="Переименовать набор Крипто"]').trigger('click')
    await flushPromises()
    expect((wrapper.find('#ext-set-name').element as HTMLInputElement).value).toBe('Крипто')
    expect((wrapper.find('#ext-set-remark').element as HTMLTextAreaElement).value).toBe('старое')
    api.updateExtCategory.mockRejectedValueOnce({ code: 'Validation', message: 'поля', fields: [{ field: 'remark', code: 'invalid' }] })
    await byText(wrapper, '[role="dialog"] button', 'Сохранить').trigger('click')
    await flushPromises()
    expect(wrapper.find('[role="dialog"]').text()).toContain('Примечание до 500 символов')
    api.updateExtCategory.mockResolvedValueOnce({ ...CRYPTO, name: 'Магазины', remark: 'старое' })
    await wrapper.find('#ext-set-name').setValue('Магазины')
    await byText(wrapper, '[role="dialog"] button', 'Сохранить').trigger('click')
    await flushPromises()
    expect(api.updateExtCategory).toHaveBeenLastCalledWith(1, 'Магазины', 'старое')
    expect(wrapper.find('[role="dialog"]').exists()).toBe(false)
  })

  it('удаление выбранного: текст с числом профилей; после — первый оставшийся; набор без профилей — «Профилей с этим набором нет»', async () => {
    const ads = extCategory({ id: 2, name: 'Реклама', profile_count: 0 })
    api.listExtCategories.mockResolvedValue([extCategory(), CRYPTO, ads])
    api.deleteExtCategory.mockResolvedValue(null)
    const { wrapper } = await mountView('/extensions/sets')
    await wrapper.find('button[aria-label="Удалить набор Реклама"]').trigger('click')
    await flushPromises()
    expect(wrapper.find('[role="alertdialog"]').text()).toContain('Профилей с этим набором нет. Сами расширения останутся в общем списке.')
    await byText(wrapper, '[role="alertdialog"] button', 'Отмена').trigger('click')
    await flushPromises()
    // «Крипто» выбран — первый в списке.
    await wrapper.find('button[aria-label="Удалить набор Крипто"]').trigger('click')
    await flushPromises()
    const confirm = wrapper.find('[role="alertdialog"]')
    expect(confirm.text()).toContain('Удалить набор «Крипто»?')
    expect(confirm.text()).toContain('3 профиля с этим набором снова будут получать все включённые расширения. Сами расширения останутся в общем списке.')
    api.listExtCategories.mockResolvedValue([extCategory(), ads])
    await byText(wrapper, '[role="alertdialog"] button', 'Удалить набор').trigger('click')
    await flushPromises()
    expect(api.deleteExtCategory).toHaveBeenCalledWith(1)
    expect(toasts()).toContainEqual({ kind: 'info', title: 'Набор «Крипто» удалён', body: undefined })
    expect(compositionTitle(wrapper)).toBe('Состав набора «Реклама»')
  })

  it('нет наборов — «Наборов пока нет» и «Новый набор» справа; библиотека пуста — «Библиотека пуста.» и ссылка', async () => {
    api.listExtCategories.mockResolvedValue([extCategory()])
    const { wrapper } = await mountView('/extensions/sets')
    expect(wrapper.text()).toContain('Наборов пока нет')
    expect(wrapper.find('.placeholder').text()).toContain('Создайте набор, чтобы назначить отдельные расширения выбранным профилям.')
    expect(wrapper.find('.placeholder button').text()).toBe('Новый набор')
    api.listExtensions.mockResolvedValue([])
    api.listExtCategories.mockResolvedValue([extCategory(), CRYPTO])
    await useExtensionsStore().refresh()
    await flushPromises()
    expect(wrapper.find('.placeholder').text()).toContain('Библиотека пуста.')
    expect(wrapper.find('.placeholder a').attributes('href')).toBe('/extensions')
  })

  it('выбранный набор пропал после перечитывания — выбран первый', async () => {
    const ads = extCategory({ id: 2, name: 'Реклама' })
    api.listExtCategories.mockResolvedValue([extCategory(), CRYPTO, ads])
    const { wrapper } = await mountView('/extensions/sets')
    await wrapper.findAll('[data-test="set-row"] .pick')[1]!.trigger('click')
    expect(compositionTitle(wrapper)).toBe('Состав набора «Реклама»')
    api.listExtCategories.mockResolvedValue([extCategory(), CRYPTO])
    await useExtensionsStore().refresh()
    await flushPromises()
    expect(compositionTitle(wrapper)).toBe('Состав набора «Крипто»')
  })
})
