import { enableAutoUnmount, flushPromises } from '@vue/test-utils'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { CoreError } from '../../api/errors'
import type { FlowRow, TaskDetails } from '../../api/types'
import UiSelect from '../../components/UiSelect.vue'
import { group, profileLabel, profileRow } from '../../test/fixtures'
import { mountWithApp } from '../../test/mount'

const api = vi.hoisted(() => ({
  listProfileLabels: vi.fn(async () => [profileLabel()]),
  createTask: vi.fn(),
  previewTable: vi.fn(),
  previewTableData: vi.fn(),
  getTask: vi.fn(),
  updateTask: vi.fn(),
  listFlows: vi.fn(async (): Promise<FlowRow[]> => []),
  listTasks: vi.fn(async () => []),
}))
vi.mock('../../api/core', () => ({ core: api, events: {} }))
const dialogs = vi.hoisted(() => ({ pickTableFile: vi.fn() }))
vi.mock('../../api/dialogs', () => dialogs)

import { useGroupsStore } from '../../stores/groups'
import TaskDialog from './TaskDialog.vue'

enableAutoUnmount(afterEach)

const flow = (patch: Partial<FlowRow> = {}): FlowRow => ({
  id: 1, name: 'Вход', group: '', remark: '', created_at: 1, updated_at: 1, has_errors: false,
  variables: [
    { name: 'login', type: 'string', default: null, description: '' },
    { name: 'n', type: 'number', default: null, description: '' },
    { name: 'on', type: 'boolean', default: true, description: '' },
    { name: 'list', type: 'array', default: [], description: '' },
  ],
  ...patch,
})

const flowList = () => [flow(), flow({ id: 2, name: 'Сломан', has_errors: true })]
function savedTask(patch: Partial<TaskDetails> = {}): TaskDetails {
  return { id: 10, revision: 2, name: 'Сохранённая задача', flow_id: 1, flow: null, process: flow(),
    profiles: ['b', 'a'], distribution: 'per_profile', params: { login: 'saved-login', n: '5', on: false, list: [1, 2], extra: 'keep' },
    data: [{ login: 'row-login', extra: 'row-extra' }], threads: 3, order: 'random', missing_row: 'random', created_at: 1, ...patch }
}

/** Диалог открыт при пустом хранилище процессов: нужное он подгружает сам. */
async function mountDialog(props: { flowId?: number | null; profileIds?: string[] | null; taskId?: number } = {}) {
  const w = mountFresh(props)
  await flushPromises()
  return w
}
function mountFresh(props: { flowId?: number | null; profileIds?: string[] | null; taskId?: number } = {}) {
  const groups = useGroupsStore()
  groups.loaded = true
  groups.groups = [group({ id: 5 })]
  groups.rows = [
    profileRow({ id: 'a', serial: 1, group: { id: 5 } as never }),
    profileRow({ id: 'b', serial: 2 }),
  ]
  return mountWithApp(TaskDialog, { props: { open: true, flowId: 1, profileIds: null, ...props }, attachTo: document.body })
}
type Wrapper = ReturnType<typeof mountFresh>
const button = (w: Wrapper, text: string) => w.findAll('button').find((b) => b.text() === text)!
const select = (w: Wrapper, id: string, value: string) =>
  w.findAllComponents(UiSelect).find((c) => (c.props() as { id?: string }).id === id)!.vm.$emit('update:modelValue', value)

beforeEach(() => {
  api.listFlows.mockReset().mockImplementation(async () => flowList())
  api.createTask.mockReset().mockResolvedValue(11)
  api.previewTable.mockReset()
  api.previewTableData.mockReset().mockResolvedValue({ columns: ['login', 'extra'], rows: 1, key: 'order', matched: 2, skipped: [] })
  api.getTask.mockReset().mockResolvedValue(savedTask())
  api.updateTask.mockReset().mockResolvedValue(null)
  dialogs.pickTableFile.mockReset()
})

async function toProfiles(w: Wrapper, perProfile = false) {
  await button(w, 'К профилям').trigger('click')
  await flushPromises()
  if (perProfile) {
    await w.find('input[value="per_profile"]').setValue(true)
    await flushPromises()
  }
}
async function toReview(w: Wrapper) {
  await button(w, 'Далее').trigger('click')
  await flushPromises()
}
async function launch(w: Wrapper) {
  await button(w, 'Создать и запустить').trigger('click')
  await flushPromises()
}
async function attachTable(w: Wrapper) {
  dialogs.pickTableFile.mockResolvedValue('/data/links.csv')
  await button(w, 'Выбрать файл').trigger('click')
  await flushPromises()
}

describe('полное редактирование задачи', () => {
  it('загружает все сохранённые значения и сохраняет их без запуска и повторного чтения файла', async () => {
    const w = await mountDialog({ taskId: 10 })
    expect(w.text()).toContain('Изменить задачу')
    expect(w.find('#task-param-login').element).toHaveProperty('value', 'saved-login')
    expect(w.find('#task-param-n').element).toHaveProperty('value', '5')
    expect(w.find('#task-param-extra').element).toHaveProperty('value', 'keep')
    expect(w.find('[role="switch"]').attributes('data-state')).toBe('unchecked')
    expect(w.text()).toContain('Сохранённая таблица · строк: 1')
    await toProfiles(w)
    expect(button(w, 'Выбраны').attributes('data-state')).toBe('on')
    expect(w.text()).toContain('Профилей: 2')
    expect(api.previewTable).not.toHaveBeenCalled()
    expect(api.previewTableData.mock.calls[0][0]).toMatchObject({ data: savedTask().data, profile_ids: ['b', 'a'] })
    await toReview(w)
    expect(w.find('#task-name').element).toHaveProperty('value', 'Сохранённая задача')
    expect(w.find('#task-threads').element).toHaveProperty('value', '3')
    expect(button(w, 'Создать и запустить')).toBeUndefined()
    await button(w, 'Сохранить').trigger('click')
    await flushPromises()
    expect(api.updateTask).toHaveBeenCalledWith(10, expect.objectContaining({ revision: 2, name: 'Сохранённая задача', flow_id: 1,
      profile_ids: ['b', 'a'], data_path: null, data: savedTask().data, params: savedTask().params,
      order: 'random', missing_row: 'random', threads: 3 }))
    expect(api.createTask).not.toHaveBeenCalled()
    expect(w.emitted('updated')).toEqual([[10]])
  })

  it('меняет процесс, параметры, таблицу, профили и порядок', async () => {
    api.listFlows.mockResolvedValue([flow(), flow({ id: 3, name: 'Другой процесс', variables: [{ name: 'url', type: 'string', default: '', description: '' }] })])
    api.previewTable.mockResolvedValue({ columns: ['url'], rows: 3, key: 'order', matched: 1, skipped: [] })
    const w = await mountDialog({ taskId: 10 })
    w.findComponent(UiSelect).vm.$emit('update:modelValue', '3')
    await flushPromises()
    await w.find('#task-param-url').setValue('https://new.example')
    await attachTable(w)
    await toProfiles(w)
    await button(w, 'Группа').trigger('click')
    await flushPromises()
    expect(w.text()).toContain('Профилей: 1')
    await toReview(w)
    await w.find('#task-name').setValue('Новый запуск')
    await w.find('#task-threads').setValue('4')
    await w.find('.advanced summary').trigger('click')
    await button(w, 'По порядку').trigger('click')
    await button(w, 'Пропустить').trigger('click')
    await button(w, 'Сохранить').trigger('click')
    await flushPromises()
    expect(api.updateTask).toHaveBeenCalledWith(10, expect.objectContaining({ name: 'Новый запуск', flow_id: 3,
      params: { url: 'https://new.example' }, data_path: '/data/links.csv', data: null,
      profile_ids: ['a'], threads: 4, order: 'ordered', missing_row: 'skip', distribution: 'per_profile' }))
  })

  it('объясняет новую очередь при смене распределения и сохраняет новое распределение', async () => {
    const w = await mountDialog({ taskId: 10 })
    await toProfiles(w)
    await w.find('input[value="queue"]').setValue(true)
    await flushPromises()
    await toReview(w)
    expect(w.text()).toContain('очередь будет сформирована заново')
    await button(w, 'Сохранить').trigger('click')
    await flushPromises()
    expect(api.updateTask.mock.calls[0][1].distribution).toBe('queue')
  })

  it('сохраняет точные параметры очереди при изменении только имени и потоков', async () => {
    const saved = savedTask({ distribution: 'queue', params: { login: '', n: '5' } })
    api.getTask.mockResolvedValue(saved)
    const w = await mountDialog({ taskId: 10 })
    await toProfiles(w)
    await toReview(w)
    expect(w.text()).not.toContain('очередь будет сформирована заново')
    await w.find('#task-name').setValue('Другое имя')
    await w.find('#task-threads').setValue('7')
    await button(w, 'Сохранить').trigger('click')
    await flushPromises()
    expect(api.updateTask.mock.calls[0][1].params).toEqual({ login: '', n: '5' })
  })

  it('позволяет удалить сохранённую таблицу и изменить номера профилей', async () => {
    const w = await mountDialog({ taskId: 10 })
    await button(w, 'Убрать таблицу').trigger('click')
    await toProfiles(w)
    await button(w, 'Номера').trigger('click')
    expect(w.find('#task-serials').element).toHaveProperty('value', '2, 1')
    await w.find('#task-serials').setValue('1')
    await toReview(w)
    await button(w, 'Сохранить').trigger('click')
    await flushPromises()
    expect(api.updateTask.mock.calls[0][1]).toMatchObject({ data: null, data_path: null, profile_ids: ['a'] })
  })

  it('сохраняет процесс из файла без подмены первым процессом из базы', async () => {
    api.getTask.mockResolvedValue(savedTask({ flow_id: null, flow: { format: 'ombra-flow/1', name: 'Из файла', variables: flow().variables, nodes: [] },
      process: flow({ id: -1, name: 'Из файла' }) }))
    const w = await mountDialog({ taskId: 10 })
    expect(w.findComponent(UiSelect).props()).toHaveProperty('modelValue', '-1')
    await toProfiles(w)
    await toReview(w)
    await button(w, 'Сохранить').trigger('click')
    await flushPromises()
    expect(api.updateTask.mock.calls[0][1].flow_id).toBeNull()
  })

  it('при ошибке загрузки не даёт сохранить пустую форму, повтор загружает задачу', async () => {
    api.getTask.mockRejectedValueOnce(new Error('load failed')).mockResolvedValueOnce(savedTask())
    const w = await mountDialog({ taskId: 10 })
    expect(w.text()).toContain('Не удалось загрузить настройки задачи')
    expect(button(w, 'К профилям').attributes('disabled')).toBeDefined()
    expect(w.find('#task-param-login').exists()).toBe(false)
    await button(w, 'Загрузить ещё раз').trigger('click')
    await flushPromises()
    expect(w.find('#task-param-login').element).toHaveProperty('value', 'saved-login')
    expect(api.updateTask).not.toHaveBeenCalled()
  })
})

describe('создание задачи по шагам', () => {
  it('показывает режим процесса и сохраняет headless только у новой задачи', async () => {
    api.listFlows.mockResolvedValue([flow({ headless: true })])
    const w = await mountDialog({ profileIds: ['a'] })
    await toProfiles(w)
    await toReview(w)
    const picker = w.get('[data-test="browser-mode"]').findComponent(UiSelect)
    expect(w.get('[data-test="browser-mode"]').text()).toContain('Как в сценарии: Без окна')
    picker.vm.$emit('update:modelValue', 'headless')
    await button(w, 'Назад').trigger('click')
    await toReview(w)
    expect(w.get('[data-test="browser-mode"]').text()).toContain('Без окна')
    await launch(w)
    expect(api.createTask.mock.calls[0][0].headless).toBe(true)
  })

  it.each([true, false, null])('восстанавливает сохранённый режим %s и позволяет вернуться к режиму процесса', async (headless) => {
    api.getTask.mockResolvedValue(savedTask({ headless }))
    const w = await mountDialog({ taskId: 10 })
    await toProfiles(w)
    await toReview(w)
    const picker = w.get('[data-test="browser-mode"]').findComponent(UiSelect)
    expect(w.get('[data-test="browser-mode"] [role="combobox"]').text()).toBe(headless === null ? 'Как в сценарии: С окном' : headless ? 'Без окна' : 'С окном')
    picker.vm.$emit('update:modelValue', 'inherit')
    await button(w, 'Сохранить').trigger('click')
    await flushPromises()
    expect(api.updateTask.mock.calls[0][1].headless).toBeNull()
    expect(api.createTask).not.toHaveBeenCalled()
  })

  it('не запускает при переходах и показывает одно задание для очереди без таблицы', async () => {
    const w = await mountDialog({ profileIds: ['a', 'b'] })
    expect(w.find('#task-param-login').exists()).toBe(true)
    expect(button(w, 'Создать и запустить')).toBeUndefined()
    expect(w.find('#task-threads').exists()).toBe(false)
    await toProfiles(w)
    expect(w.find('input[value="queue"]').element).toHaveProperty('checked', true)
    expect(w.text()).toContain('Без таблицы будет только одно задание')
    expect(w.text()).toContain('Профилей: 2')
    expect(api.createTask).not.toHaveBeenCalled()
    await toReview(w)
    expect(w.find('[data-test="task-run-summary"] .metrics').text()).toContain('1Заданий')
    expect(w.find('#task-threads').element).toHaveProperty('value', '1')
    expect(w.find('label[for="task-threads"]').text()).toBe('Одновременно профилей')
    expect(w.find('details').exists()).toBe(true)
    await launch(w)
    expect(api.createTask.mock.calls[0][0]).toMatchObject({ distribution: 'queue', profile_ids: ['a', 'b'], threads: 1, data_path: null })
    expect(w.emitted('created')).toEqual([[11]])
  })

  it('сохраняет выбранный лимит потоков и объясняет реальную параллельность для одного задания', async () => {
    const w = await mountDialog({ profileIds: ['a', 'b'] })
    await toProfiles(w)
    await toReview(w)
    await w.find('#task-threads').setValue('3')
    expect(w.text()).toContain('Сейчас задание одно, поэтому работать будет один профиль')
    expect(w.find('[data-test="task-run-summary"]').text()).toContain('Одновременно работает не больше 1')
    await launch(w)
    expect(api.createTask.mock.calls[0][0]).toMatchObject({ threads: 3, distribution: 'queue' })
  })

  it('сохраняет параметры при переходе назад и отправляет типизированные значения', async () => {
    const w = await mountDialog({ profileIds: ['a', 'b'] })
    await w.find('#task-param-n').setValue('5')
    await toProfiles(w, true)
    await toReview(w)
    await w.find('#task-name').setValue('Моя задача')
    await w.find('#task-threads').setValue('2')
    await button(w, 'Назад').trigger('click')
    await button(w, 'Назад').trigger('click')
    expect(w.find('#task-param-n').element).toHaveProperty('value', '5')
    await toProfiles(w)
    await toReview(w)
    expect(w.find('#task-name').element).toHaveProperty('value', 'Моя задача')
    await launch(w)
    expect(api.createTask.mock.calls[0][0]).toMatchObject({ name: 'Моя задача', threads: 2, params: { n: 5, on: true } })
    expect(api.createTask.mock.calls[0][0].params).not.toHaveProperty('login')
  })

  it('не пропускает неверные числа и JSON на следующий шаг', async () => {
    const w = await mountDialog({ profileIds: ['a'] })
    for (const bad of ['0x10', 'Infinity', '1e3']) {
      await w.find('#task-param-n').setValue(bad)
      await button(w, 'К профилям').trigger('click')
      expect(w.text()).toContain('Ожидается число')
      expect(w.find('#task-param-n').exists()).toBe(true)
    }
    await w.find('#task-param-n').setValue('-2.5')
    await w.find('#task-param-list').setValue('[1,')
    await button(w, 'К профилям').trigger('click')
    expect(w.text()).toContain('Ожидается JSON')
    await w.find('#task-param-list').setValue('[1,2]')
    await toProfiles(w)
    await toReview(w)
    await launch(w)
    expect(api.createTask.mock.calls[0][0].params).toEqual({ n: -2.5, list: [1, 2], on: true })
  })

  it('считает уникальные номера и блокирует неизвестные профили', async () => {
    const w = await mountDialog()
    await toProfiles(w, true)
    await button(w, 'Номера').trigger('click')
    await w.find('#task-serials').setValue('99')
    expect(w.text()).toContain('Нет профиля с номером 99')
    expect(button(w, 'Далее').attributes('disabled')).toBeDefined()
    await w.find('#task-serials').setValue('2, 1, 2')
    expect(w.text()).toContain('Профилей: 2')
    await toReview(w)
    await launch(w)
    expect(api.createTask.mock.calls[0][0].profile_ids).toEqual(['b', 'a'])
  })

  it('соединяет условия фильтра и передаёт совпадения по номеру', async () => {
    const w = await mountDialog()
    const groups = useGroupsStore()
    const selectedGroup = { id: 5, name: 'Shop', color: '#2447c9' }
    groups.tags = [{ id: 1, name: 'vip', profile_count: 4 }, { id: 2, name: 'shop', profile_count: 3 }]
    groups.rows = [
      profileRow({ id: 'a', serial: 5, name: 'Ёжик 5', group: selectedGroup, tags: ['VIP', 'Shop'], label: profileLabel(), status: { state: 'starting' } }),
      profileRow({ id: 'b', serial: 2, name: 'Ёжик 2', group: selectedGroup, tags: ['vip', 'shop'], label: profileLabel() }),
      profileRow({ id: 'c', serial: 3, name: 'Ёжик 3', tags: ['vip', 'shop'] }),
      profileRow({ id: 'd', serial: 4, name: 'Ёжик 4', group: selectedGroup, tags: ['vip'], status: { state: 'starting' } }),
    ]
    await toProfiles(w, true)
    await button(w, 'Фильтр').trigger('click')
    select(w, 'task-filter-group', '5')
    await flushPromises()
    select(w, 'task-filter-status', '2')
    await w.find('#task-filter-tags').setValue('vip, shop')
    await w.find('#task-filter-search').setValue('ЕЖИК')
    expect(w.find('[data-test="task-profile-matches"]').text()).toBe('Подходит профилей: 2')
    await toReview(w)
    await launch(w)
    expect(api.createTask.mock.calls[0][0].profile_ids).toEqual(['b', 'a'])
  })

  it('пустой фильтр запрещает продолжение, сброс возвращает профили', async () => {
    const w = await mountDialog()
    await toProfiles(w)
    await button(w, 'Фильтр').trigger('click')
    await w.find('#task-filter-search').setValue('неизвестный')
    expect(w.text()).toContain('Нет подходящих профилей')
    expect(button(w, 'Далее').attributes('disabled')).toBeDefined()
    await button(w, 'Сбросить фильтр').trigger('click')
    expect(w.find('[data-test="task-profile-matches"]').text()).toBe('Подходит профилей: 2')
    expect(button(w, 'Далее').attributes('disabled')).toBeUndefined()
  })

  it('пустая группа и пустой контекстный выбор запрещают продолжение', async () => {
    const w = await mountDialog()
    useGroupsStore().rows = []
    await toProfiles(w)
    expect(w.text()).toContain('В группе нет профилей')
    expect(button(w, 'Далее').attributes('disabled')).toBeDefined()
    await w.setProps({ profileIds: [] })
    expect(button(w, 'Далее').attributes('disabled')).toBeDefined()
    expect(api.createTask).not.toHaveBeenCalled()
  })

  it('проверяет таблицу после выбора профилей и ждёт окончания проверки', async () => {
    let finish: (value: unknown) => void = () => {}
    api.previewTable.mockReturnValue(new Promise((resolve) => { finish = resolve }))
    const w = await mountDialog({ profileIds: ['a', 'b'] })
    await attachTable(w)
    expect(api.previewTable).not.toHaveBeenCalled()
    await toProfiles(w)
    expect(button(w, 'Далее').attributes('disabled')).toBeDefined()
    expect(w.text()).toContain('Проверяем таблицу')
    finish({ columns: ['url'], rows: 100, key: 'url', matched: 98, skipped: [], duplicates: 2 })
    await flushPromises()
    expect(w.text()).toContain('заданий в очереди: 98')
    expect(w.text()).toContain('повторов убрано: 2')
    await toReview(w)
    expect(w.find('[data-test="task-run-summary"] .metrics').text()).toContain('98Заданий')
    await w.find('#task-threads').setValue('2')
    await launch(w)
    expect(api.createTask.mock.calls[0][0]).toMatchObject({ data_path: '/data/links.csv', threads: 2 })
  })

  it('пересчитывает строки при смене распределения и показывает их приоритет у параметра', async () => {
    api.previewTable.mockResolvedValue({ columns: ['login'], rows: 3, key: 'order', matched: 2, skipped: [7], duplicates: 0 })
    const w = await mountDialog({ profileIds: ['a', 'b'] })
    await attachTable(w)
    await toProfiles(w, true)
    expect(api.previewTable.mock.calls.at(-1)![0].distribution).toBe('per_profile')
    expect(w.text()).toContain('профилей с данными: 2')
    expect(w.text()).toContain('без строки: 7')
    await button(w, 'Назад').trigger('click')
    expect(w.text()).toContain('Из таблицы')
    expect(w.text()).toContain('Данные из таблицы заменяют')
    await toProfiles(w)
    await toReview(w)
    expect(w.find('details').exists()).toBe(true)
    expect(button(w, 'Случайная строка')).toBeDefined()
  })

  it('отбрасывает устаревшие ответы таблицы после смены режима', async () => {
    let resolveSlow: (value: unknown) => void = () => {}
    api.previewTable.mockReturnValueOnce(new Promise((resolve) => { resolveSlow = resolve }))
      .mockResolvedValueOnce({ columns: ['login'], rows: 9, key: 'order', matched: 2, skipped: [], duplicates: 0 })
    const w = await mountDialog({ profileIds: ['a', 'b'] })
    await attachTable(w)
    await toProfiles(w)
    await w.find('input[value="per_profile"]').setValue(true)
    await flushPromises()
    resolveSlow({ columns: [], rows: 1, key: 'order', matched: 1, skipped: [] })
    await flushPromises()
    expect(w.text()).toContain('Строк: 9')
    expect(w.text()).not.toContain('Строк: 1 ')
  })

  it('обновляет предпросмотр по фильтру и очищает его без совпадений', async () => {
    api.previewTable.mockResolvedValue({ columns: [], rows: 2, key: 'order', matched: 2, skipped: [] })
    const w = await mountDialog()
    await attachTable(w)
    await toProfiles(w, true)
    await button(w, 'Фильтр').trigger('click')
    await flushPromises()
    expect(api.previewTable.mock.calls.at(-1)![0].profile_ids).toEqual(['a', 'b'])
    await w.find('#task-filter-search').setValue('2')
    await flushPromises()
    expect(api.previewTable.mock.calls.at(-1)![0].profile_ids).toEqual(['b'])
    const calls = api.previewTable.mock.calls.length
    await w.find('#task-filter-search').setValue('99')
    await flushPromises()
    expect(api.previewTable).toHaveBeenCalledTimes(calls)
    expect(w.text()).not.toContain('Строк: 2')
  })

  it('ошибка таблицы запрещает продолжение, файл можно убрать на первом шаге', async () => {
    api.previewTable.mockRejectedValue(new CoreError('BadParams', 'Сохраните таблицу как CSV UTF-8'))
    const w = await mountDialog({ profileIds: ['a'] })
    await attachTable(w)
    await toProfiles(w)
    expect(w.text()).toContain('CSV UTF-8')
    expect(button(w, 'Далее').attributes('disabled')).toBeDefined()
    await button(w, 'Изменить данные').trigger('click')
    await button(w, 'Убрать таблицу').trigger('click')
    await toProfiles(w)
    expect(button(w, 'Далее').attributes('disabled')).toBeUndefined()
  })

  it('не исправляет неверную параллельность молча', async () => {
    const w = await mountDialog({ profileIds: ['a', 'b'] })
    await toProfiles(w, true)
    await toReview(w)
    for (const value of ['0', '4294967296', '1.5']) {
      await w.find('#task-threads').setValue(value)
      await launch(w)
      expect(w.text()).toContain('Укажите целое положительное число профилей.')
      expect(api.createTask).not.toHaveBeenCalled()
    }
    await w.find('#task-threads').setValue('2')
    await launch(w)
    expect(api.createTask.mock.calls[0][0].threads).toBe(2)
  })

  it('ошибка имени остаётся до правки имени, ошибка данных возвращает на первый шаг', async () => {
    api.createTask.mockRejectedValueOnce(new CoreError('Validation', 'x', [{ field: 'name', code: 'invalid' }]))
      .mockRejectedValueOnce(new CoreError('Validation', 'x', [{ field: 'data_path', code: 'invalid' }]))
    const w = await mountDialog({ profileIds: ['a', 'b'] })
    await toProfiles(w, true)
    await toReview(w)
    await launch(w)
    expect(w.text()).toContain('Неверное значение')
    await w.find('#task-threads').setValue('2')
    expect(w.text()).toContain('Неверное значение')
    await w.find('#task-name').setValue('новое')
    expect(w.text()).not.toContain('Неверное значение')
    await launch(w)
    expect(w.find('#task-param-login').exists()).toBe(true)
    expect(w.text()).toContain('Неверное значение')
  })

  it('до загрузки продолжение закрыто, процесс с ошибками недоступен', async () => {
    let resolveFlows: (value: FlowRow[]) => void = () => {}
    api.listFlows.mockReturnValue(new Promise((resolve) => { resolveFlows = resolve }))
    const w = mountFresh({ flowId: null, profileIds: ['a'] })
    await flushPromises()
    expect(button(w, 'К профилям').attributes('disabled')).toBeDefined()
    resolveFlows(flowList())
    await flushPromises()
    const options = (w.findComponent(UiSelect).props() as unknown as { options: { value: string; disabled?: boolean }[] }).options
    expect(options.find((option) => option.value === '2')?.disabled).toBe(true)
    await toProfiles(w)
    await toReview(w)
    await launch(w)
    expect(api.createTask.mock.calls[0][0].flow_id).toBe(1)
  })

  it('позволяет исправить отсутствие строк перед запуском', async () => {
    api.previewTable.mockImplementation(async (form) => ({ columns: ['login'], rows: 1, key: 'id', matched: form.missing_row === 'random' ? 2 : 0, skipped: form.missing_row === 'random' ? [] : [1, 2] }))
    const w = await mountDialog({ profileIds: ['a', 'b'] })
    await attachTable(w)
    await toProfiles(w, true)
    await toReview(w)
    expect(w.text()).toContain('Нет заданий для запуска')
    expect(button(w, 'Создать и запустить').attributes('disabled')).toBeDefined()
    await button(w, 'Случайная строка').trigger('click')
    await flushPromises()
    expect(button(w, 'Создать и запустить').attributes('disabled')).toBeUndefined()
    expect(w.text()).not.toContain('Нет заданий для запуска')
    await launch(w)
    expect(api.createTask.mock.calls[0][0].missing_row).toBe('random')
  })

  it('повторное открытие сбрасывает шаг, фильтр и параметры', async () => {
    api.listFlows.mockResolvedValue([flow({ variables: [{ name: 'on', type: 'boolean', default: 'true', description: '' }] })])
    const w = await mountDialog()
    expect(w.find('[role="switch"]').attributes('data-state')).toBe('checked')
    await toProfiles(w)
    await button(w, 'Фильтр').trigger('click')
    await w.find('#task-filter-search').setValue('99')
    await w.setProps({ open: false })
    await w.setProps({ open: true })
    await flushPromises()
    expect(w.find('[role="switch"]').attributes('data-state')).toBe('checked')
    await toProfiles(w)
    expect(button(w, 'Группа').attributes('data-state')).toBe('on')
  })
})

it('accepts 101 parallel profiles without clamping the saved choice', async () => {
  const w = await mountDialog({ profileIds: ['a', 'b'] })
  await toProfiles(w)
  await toReview(w)
  await w.get('#task-threads').setValue('101')
  expect(w.get('#task-threads').attributes('max')).toBeUndefined()
  await launch(w)
  expect(api.createTask.mock.calls[0][0]).toMatchObject({ threads: 101 })
})

it.each(['0', '-1', '1.5', '4294967296', 'not-a-number'])('rejects invalid u32 parallelism %s', async value => {
  const w = await mountDialog({ profileIds: ['a', 'b'] })
  await toProfiles(w)
  await toReview(w)
  await w.get('#task-threads').setValue(value)
  await launch(w)
  expect(api.createTask).not.toHaveBeenCalled()
})
