import { flushPromises } from '@vue/test-utils'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { CoreError } from '../../api/errors'
import { mountWithApp } from '../../test/mount'

const api = vi.hoisted(() => ({
  createProfilesBulk: vi.fn(async () => [{}, {}]),
  listProxies: vi.fn(async () => []),
  listProfiles: vi.fn(async (_query: { group_id: number | null }) => ({ rows: [], total: 0, running: 0 })),
  listGroups: vi.fn(async () => ({ groups: [], ungrouped: 0 })),
  listTags: vi.fn(async () => []),
}))
vi.mock('../../api/core', () => ({ core: api, events: {} }))

import { useProfilesStore } from '../../stores/profiles'
import BulkCreateDialog from './BulkCreateDialog.vue'

async function createWith(group: number | null) {
  const profiles = useProfilesStore()
  if (group !== null) profiles.filters.group = group
  const wrapper = mountWithApp(BulkCreateDialog, {
    props: { open: true },
  })
  await flushPromises()
  await wrapper.find('#bulk-create-prefix').setValue('Магазин')
  api.listProfiles.mockClear()
  const submit = wrapper.findAll('button').find((button) => button.text().startsWith('Создать'))
  await submit!.trigger('click')
  await flushPromises()
}

describe('ошибка группы', () => {
  it('ядро не нашло группу — ошибка видна у поля «Группа»', async () => {
    api.createProfilesBulk.mockRejectedValueOnce(
      new CoreError('Validation', 'группа не найдена', [{ field: 'group_id', code: 'invalid' }]),
    )
    const wrapper = mountWithApp(BulkCreateDialog, {
      props: { open: true },
    })
    await flushPromises()
    await wrapper.find('#bulk-create-prefix').setValue('Магазин')
    const submit = wrapper.findAll('button').find((button) => button.text().startsWith('Создать'))
    await submit!.trigger('click')
    await flushPromises()
    expect(wrapper.text()).toContain('Группа не найдена')
  })
})

describe('обновление списка после создания', () => {
  beforeEach(() => {
    api.createProfilesBulk.mockClear()
    api.listProfiles.mockClear()
  })

  // Хранилище групп тоже читает полный список (без фильтров), поэтому считаем запросы списка экрана отдельно.
  const screenCalls = () => api.listProfiles.mock.calls.filter(([query]) => query.group_id === 3)

  it('с выбранной группой список экрана запрашивается один раз', async () => {
    await createWith(3)
    expect(api.createProfilesBulk).toHaveBeenCalledTimes(1)
    expect(screenCalls()).toHaveLength(1)
  })

  it('без группы список экрана и полный список групп — по одному запросу', async () => {
    await createWith(null)
    expect(api.createProfilesBulk).toHaveBeenCalledTimes(1)
    expect(api.listProfiles).toHaveBeenCalledTimes(2)
  })
})
