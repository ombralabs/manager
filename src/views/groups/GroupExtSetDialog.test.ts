import { flushPromises } from '@vue/test-utils'
import { SelectRoot } from 'reka-ui'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { GroupRecord } from '../../api/types'
import { extCategory, extension, group, riskyExtension } from '../../test/fixtures'
import { mountWithApp } from '../../test/mount'

const api = vi.hoisted(() => ({
  listExtensions: vi.fn(),
  listExtCategories: vi.fn(),
  setGroupExtCategory: vi.fn(),
  listGroups: vi.fn(),
  listTags: vi.fn(),
  listProfiles: vi.fn(),
}))
vi.mock('../../api/core', () => ({ core: api, events: {} }))

import { useToasts } from '../../stores/toasts'
import GroupExtSetDialog from './GroupExtSetDialog.vue'

async function mountDialog(target: GroupRecord | null, count: number) {
  const wrapper = mountWithApp(GroupExtSetDialog, { props: { open: true, group: target, count } })
  await flushPromises()
  return wrapper
}

const submit = (wrapper: ReturnType<typeof mountWithApp>) =>
  wrapper.findAll('[role="dialog"] button').find((button) => button.text() === 'Назначить')!.trigger('click')
const titles = () => useToasts().items.map((toast) => toast.title)

beforeEach(() => {
  for (const mock of Object.values(api)) mock.mockReset()
  api.listExtensions.mockResolvedValue([extension(), riskyExtension])
  api.listExtCategories.mockResolvedValue([
    extCategory(),
    extCategory({ id: 1, name: 'Работа', extension_ids: [riskyExtension.id] }),
  ])
  api.listGroups.mockResolvedValue({ groups: [], ungrouped: 0 })
  api.listTags.mockResolvedValue([])
  api.listProfiles.mockResolvedValue({ rows: [], total: 0, running: 0 })
})

describe('диалог набора расширений группы', () => {
  it('заголовок, число профилей, «Общий» по умолчанию; рискованный набор — предупреждение; «Назначить» — id группы и набор', async () => {
    api.setGroupExtCategory.mockResolvedValue({ ok: ['p1', 'p2'], failed: [] })
    const wrapper = await mountDialog(group({ id: 7, name: 'Amazon', profile_count: 2 }), 2)
    const text = wrapper.find('[role="dialog"]').text()
    expect(text).toContain('Набор расширений для «Amazon»')
    expect(text).toContain('Набор будет назначен всем текущим профилям группы (2). Для профилей, добавленных позже, настройки расширений сохранятся.')
    expect(wrapper.find('.risk-note').exists()).toBe(false)
    wrapper.findComponent(SelectRoot).vm.$emit('update:modelValue', '1')
    await flushPromises()
    expect(wrapper.find('.risk-note').text()).toContain('ZeroOmega может изменить настройки прокси и направить соединения в обход прокси профиля')
    await submit(wrapper)
    await flushPromises()
    expect(api.setGroupExtCategory).toHaveBeenCalledWith(7, 1)
    expect(titles()).toContain('Набор назначен 2 профилям')
    // Панель группы показывает наборы профилей — строки перечитаны.
    expect(api.listProfiles).toHaveBeenCalled()
    expect(wrapper.emitted('update:open')).toEqual([[false]])
  })

  it('пока список наборов не загружен, «Назначить» неактивно', async () => {
    api.listExtCategories.mockReturnValue(new Promise(() => {}))
    const wrapper = await mountDialog(group({ id: 7 }), 2)
    const button = wrapper.findAll('[role="dialog"] button').find((item) => item.text() === 'Назначить')!
    expect(button.attributes('disabled')).toBeDefined()
  })

  it('«Без группы» передаёт null; в группе нет профилей — «Набор не назначен ни одному профилю»', async () => {
    api.setGroupExtCategory.mockResolvedValue({ ok: [], failed: [] })
    const wrapper = await mountDialog(null, 0)
    expect(wrapper.find('[role="dialog"]').text()).toContain('Набор расширений для «Без группы»')
    await submit(wrapper)
    await flushPromises()
    expect(api.setGroupExtCategory).toHaveBeenCalledWith(null, 0)
    expect(titles()).toContain('Набор не назначен ни одному профилю')
  })
})
