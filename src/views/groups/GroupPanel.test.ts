import { type VueWrapper, flushPromises } from '@vue/test-utils'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { nextTick } from 'vue'
import type { GroupRecord, ProfileRow, ProfileStatus } from '../../api/types'
import { i18n } from '../../i18n'
import en from '../../i18n/en.json'
import ru from '../../i18n/ru.json'
import { extCategory, extension, group, profileRow, riskyExtension } from '../../test/fixtures'
import { mountWithApp } from '../../test/mount'

const core = vi.hoisted(() => ({
  createGroup: vi.fn(),
  updateGroup: vi.fn(),
  deleteGroup: vi.fn(),
  moveProfiles: vi.fn(),
  listGroups: vi.fn(),
  listTags: vi.fn(),
  listProfiles: vi.fn(),
}))
vi.mock('../../api/core', () => ({ core }))
vi.mock('vue-router', () => ({ useRouter: () => ({ push: vi.fn() }) }))

import { useExtensionsStore } from '../../stores/extensions'
import { useGroupsStore } from '../../stores/groups'
import GroupPanel from './GroupPanel.vue'

const shops = group({ id: 1, name: 'Магазины', color: '#0a6b66', quick_access: true })

function status(state: 'stopped' | 'paused' | 'crashed'): ProfileStatus {
  if (state === 'crashed') return { state: 'crashed', code: 1, signal: null, at: 0 }
  if (state === 'paused') {
    return {
      state: 'paused',
      pid: 1,
      cdp_ws: 'ws://127.0.0.1:1/devtools/browser/x',
      debug_port: 1,
      relay_port: 2,
      proxy_id: 1,
      exit_ip: null,
      country: null,
      timezone: 'UTC',
      started_at: 0,
    }
  }
  return { state: 'stopped' }
}

const row = (serial: number, state: 'stopped' | 'paused' | 'crashed'): ProfileRow =>
  profileRow({
    id: `p${serial}`,
    serial,
    name: `Реклама ${serial}`,
    group: { id: 2, name: 'Реклама', color: '#b02a6b' },
    seed: serial,
    status: status(state),
    created_at: 0,
  })

function mountPanel(group: GroupRecord | null, rows: ProfileRow[] = []) {
  return mountWithApp(GroupPanel, { props: { group, rows }, attachTo: document.body })
}

function button(wrapper: VueWrapper, label: string) {
  const found = wrapper.findAll('button').find((b) => b.text() === label)
  if (!found) throw new Error(`нет кнопки «${label}»`)
  return found
}

describe('боковая панель группы', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    core.listGroups.mockResolvedValue({ groups: [shops], ungrouped: 0 })
    core.listTags.mockResolvedValue([])
    core.listProfiles.mockResolvedValue({ rows: [], total: 0, running: 0 })
  })

  it('заголовок панели с длинным именем — полный текст в title', async () => {
    const wrapper = mountPanel({ ...shops, id: 7, name: 'Американские магазины электроники и бытовой техники' })
    const title = wrapper.find('h2')
    expect(title.attributes('title')).toBe(title.text())
  })

  it('новая группа сохраняется с первым свободным цветом и без быстрого доступа', async () => {
    useGroupsStore().groups = [shops]
    const created: GroupRecord = { ...shops, id: 2, name: 'Реклама', color: '#b02a6b', quick_access: false }
    core.createGroup.mockResolvedValue(created)
    const wrapper = mountPanel(null)
    await wrapper.find('#group-name').setValue('  Реклама ')
    await button(wrapper, 'Сохранить').trigger('click')
    await flushPromises()
    expect(core.createGroup).toHaveBeenCalledWith({ name: 'Реклама', color: '#B02A6B', note: '', quick_access: false })
    expect(wrapper.emitted('saved')).toEqual([[created]])
    wrapper.unmount()
  })

  it('без имени ядро не вызывается, занятое имя — ошибка у поля имени', async () => {
    const wrapper = mountPanel(null)
    await button(wrapper, 'Сохранить').trigger('click')
    await flushPromises()
    expect(core.createGroup).not.toHaveBeenCalled()
    expect(wrapper.text()).toContain('Обязательное поле')

    core.createGroup.mockRejectedValue({ code: 'GroupExists', message: 'группа уже есть' })
    await wrapper.find('#group-name').setValue('Магазины')
    await button(wrapper, 'Сохранить').trigger('click')
    await flushPromises()
    expect(wrapper.text()).toContain('Группа с таким названием уже есть')
    expect(wrapper.find('#group-name').classes()).toContain('invalid')
    expect(wrapper.emitted('saved')).toBeUndefined()
    wrapper.unmount()
  })

  it('показывает пять профилей по важности статуса и итог по остальным', () => {
    const states = ['stopped', 'crashed', 'stopped', 'paused', 'crashed', 'stopped', 'stopped'] as const
    const rows = states.map((state, index) => row(index + 1, state))
    const wrapper = mountPanel({ ...shops, id: 2, name: 'Реклама', color: '#b02a6b', profile_count: 7 }, rows)
    const names = wrapper.findAll('[data-test="panel-profile"]').map((item) => item.find('.member-name').text())
    expect(names).toEqual(['Реклама 2', 'Реклама 5', 'Реклама 4', 'Реклама 1', 'Реклама 3'])
    expect(wrapper.text()).toContain('Группа «Реклама»')
    expect(wrapper.text()).toContain('Профили в группе: 7')
    expect(wrapper.text()).toContain('Ещё 2 профиля, все остановлены')
    wrapper.unmount()
  })

  it('«Удалить группу» шлёт событие с группой и сама ничего не удаляет', async () => {
    const group = { ...shops, id: 2, name: 'Реклама', color: '#b02a6b', profile_count: 1 }
    const wrapper = mountPanel(group, [row(1, 'stopped')])
    await button(wrapper, 'Удалить группу').trigger('click')
    await flushPromises()
    expect(wrapper.emitted('requestDelete')).toEqual([[group]])
    expect(core.deleteGroup).not.toHaveBeenCalled()
    expect(document.body.textContent).not.toContain('Удалить группу «Реклама»?')
    wrapper.unmount()
  })

  it('«Ещё K профилей, <сводка>» — одна строка словаря, в английском тоже', async () => {
    expect(ru.groups.panel.more).toContain('{summary}')
    expect(en.groups.panel.more).toContain('{summary}')
    const states = ['stopped', 'crashed', 'stopped', 'paused', 'crashed', 'stopped', 'stopped'] as const
    const rows = states.map((state, index) => row(index + 1, state))
    const wrapper = mountPanel({ ...shops, id: 2, name: 'Реклама', color: '#b02a6b', profile_count: 7 }, rows)
    expect(wrapper.find('.more').text()).toBe('Ещё 2 профиля, все остановлены')
    i18n.global.locale.value = 'en'
    try {
      await nextTick()
      expect(wrapper.find('.more').text()).toBe('2 more profiles, all stopped')
    } finally {
      i18n.global.locale.value = 'ru'
      wrapper.unmount()
    }
  })
  it('«Быстрый доступ» — кнопка в шапке панели с aria-pressed, «Сохранить» отправляет новое значение', async () => {
    useGroupsStore().groups = [shops]
    core.updateGroup.mockResolvedValue({ ...shops, quick_access: false })
    const wrapper = mountPanel(shops)
    expect(wrapper.find('[role="switch"]').exists()).toBe(false)
    const toggle = wrapper.get('header button[aria-pressed]')
    expect(toggle.text()).toBe('Быстрый доступ')
    expect(toggle.attributes('aria-pressed')).toBe('true')
    await toggle.trigger('click')
    expect(toggle.attributes('aria-pressed')).toBe('false')
    await button(wrapper, 'Сохранить').trigger('click')
    await flushPromises()
    expect(core.updateGroup).toHaveBeenCalledWith(1, { name: 'Магазины', color: '#0a6b66', note: '', quick_access: false })
  })

  it('«Заметка» остаётся textarea (переводы строк сохраняются), высота — одна строка', async () => {
    useGroupsStore().groups = [shops]
    const wrapper = mountPanel({ ...shops, note: 'первая\nвторая' })
    const note = wrapper.get('#group-note')
    expect(note.element.tagName).toBe('TEXTAREA')
    expect((note.element as HTMLTextAreaElement).value).toBe('первая\nвторая')
    expect(note.attributes('rows')).toBe('1')
  })

  it('пока идёт перенос, «Перенести» занята, пункты меню неактивны', async () => {
    useGroupsStore().groups = [shops, { ...shops, id: 2, name: 'Реклама' }]
    core.moveProfiles.mockReturnValueOnce(new Promise(() => {}))
    const wrapper = mountPanel(shops, [row(1, 'stopped')])
    const trigger = button(wrapper, 'Перенести профили…')
    await trigger.trigger('keydown', { key: 'ArrowDown' })
    await flushPromises()
    await wrapper.findAll('[role="menuitem"]').find((el) => el.text() === 'Реклама')!.trigger('click')
    await flushPromises()
    expect(core.moveProfiles).toHaveBeenCalledTimes(1)
    await trigger.trigger('keydown', { key: 'ArrowDown' })
    await flushPromises()
    const items = wrapper.findAll('[role="menuitem"]')
    expect(items.length).toBeGreaterThan(0)
    expect(items.every((item) => item.attributes('aria-disabled') === 'true')).toBe(true)
    wrapper.unmount()
  })

  it('«Расширения»: наборы профилей группы по числу профилей, состав, ⚠ у рискованных, «Набор пуст»; без профилей раздела нет', async () => {
    const store = useExtensionsStore()
    store.extensions = [extension(), riskyExtension]
    store.categories = [extCategory(), extCategory({ id: 4, name: 'Работа', extension_ids: [riskyExtension.id, extension().id] })]
    store.loaded = true
    const rows = [row(1, 'stopped'), row(2, 'stopped'), row(3, 'stopped')].map((r, i) => ({ ...r, ext_category_id: i === 0 ? 0 : 4 }))
    const wrapper = mountPanel(shops, rows)
    const sets = wrapper.findAll('[data-test="panel-ext-set"]')
    expect(sets.map((set) => set.find('.ext-set-head').text())).toEqual(['Работа · 2 профиля', 'Без набора · 1 профиль'])
    expect(sets[0]!.findAll('.ext').map((ext) => ext.text())).toEqual(['uBlock Origin1.62.0', 'ZeroOmegaРасширение может изменять настройки прокси3.3.23'])
    expect(sets[0]!.find('.risk').attributes('title')).toBe('Расширение может изменять настройки прокси')
    expect(sets[1]!.text()).toContain('Нет включённых расширений')
    expect(sets[1]!.text()).not.toContain('Набор пуст')
    wrapper.unmount()

    const empty = mountPanel(shops, [])
    expect(empty.text()).not.toContain('Расширения')
    empty.unmount()
  })
})
