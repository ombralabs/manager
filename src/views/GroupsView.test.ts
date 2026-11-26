import { flushPromises } from '@vue/test-utils'
import { describe, expect, it, vi } from 'vitest'
import { createMemoryHistory, createRouter } from 'vue-router'
import { extCategory, group } from '../test/fixtures'
import { mountWithApp } from '../test/mount'

const api = vi.hoisted(() => ({
  listGroups: vi.fn(async () => ({ groups: [group({ id: 1, name: 'Amazon', profile_count: 3 })], ungrouped: 5 })),
  listTags: vi.fn(async () => []),
  listProfiles: vi.fn(async () => ({ rows: [], total: 0, running: 0 })),
  listExtensions: vi.fn(async () => []),
  listExtCategories: vi.fn(async () => [extCategory()]),
}))
vi.mock('../api/core', () => ({ core: api, events: {} }))

import GroupExtSetDialog from './groups/GroupExtSetDialog.vue'
import GroupsView from './GroupsView.vue'

async function mountView() {
  const router = createRouter({ history: createMemoryHistory(), routes: [{ path: '/', component: { template: '<div />' } }] })
  const wrapper = mountWithApp(GroupsView, { global: { plugins: [router] } })
  await flushPromises()
  return wrapper
}

async function chooseExtSet(wrapper: ReturnType<typeof mountWithApp>, rowIndex: number) {
  await wrapper.findAll('.body .row.item')[rowIndex]!.find('button[aria-label="Ещё действия"]').trigger('keydown', { key: 'ArrowDown' })
  await flushPromises()
  await wrapper.findAll('[role="menuitem"]').find((item) => item.text() === 'Назначить набор расширений…')!.trigger('click')
  await flushPromises()
}

describe('экран «Группы»: набор расширений', () => {
  it('при открытии экран перечитывает библиотеку расширений — для раздела «Расширения» в панели группы', async () => {
    api.listExtensions.mockClear()
    await mountView()
    expect(api.listExtensions).toHaveBeenCalled()
  })

  it('пункт меню у группы и у «Без группы» открывает диалог с числом профилей', async () => {
    const wrapper = await mountView()
    const dialog = () => wrapper.findComponent(GroupExtSetDialog)
    await chooseExtSet(wrapper, 0)
    expect(dialog().props()).toMatchObject({ open: true, count: 3 })
    expect(dialog().props('group')).toMatchObject({ id: 1, name: 'Amazon' })
    dialog().vm.$emit('update:open', false)
    await flushPromises()
    await chooseExtSet(wrapper, 1)
    expect(dialog().props()).toMatchObject({ open: true, group: null, count: 5 })
  })
})
