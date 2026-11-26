import { flushPromises } from '@vue/test-utils'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { mountWithApp } from '../../test/mount'

const api = vi.hoisted(() => ({
  editProfiles: vi.fn(async () => ({ ok: ['a', 'b'], failed: [] })),
  listProxies: vi.fn(async () => []),
  listProfiles: vi.fn(async () => ({ rows: [], total: 0, running: 0 })),
  listGroups: vi.fn(async () => ({ groups: [], ungrouped: 0 })),
  listTags: vi.fn(async () => []),
}))
vi.mock('../../api/core', () => ({ core: api, events: {} }))

import BulkEditDialog from './BulkEditDialog.vue'

function dialog() {
  return mountWithApp(BulkEditDialog, {
    props: { open: true, ids: ['a', 'b'] },
  })
}

function applyButton(wrapper: ReturnType<typeof dialog>) {
  const found = wrapper.findAll('button').find((button) => button.text() === 'Применить к 2 профилям')
  if (!found) throw new Error('нет кнопки «Применить к 2 профилям»')
  return found
}

describe('массовая правка', () => {
  beforeEach(() => {
    api.editProfiles.mockClear()
  })

  it('пока ни одно поле не отмечено, применить нельзя', async () => {
    const wrapper = dialog()
    await flushPromises()
    expect(applyButton(wrapper).attributes('disabled')).toBeDefined()
  })

  it('передаёт ядру только отмеченные поля', async () => {
    const wrapper = dialog()
    await flushPromises()
    await wrapper.find('input[type="checkbox"][aria-label="Заметка"]').setValue(true)
    await wrapper.find('#bulk-edit-note').setValue('новая')
    await applyButton(wrapper).trigger('click')
    await flushPromises()
    expect(api.editProfiles).toHaveBeenCalledWith(['a', 'b'], { note: 'новая' })
    expect(wrapper.emitted('update:open')).toEqual([[false]])
  })
})
