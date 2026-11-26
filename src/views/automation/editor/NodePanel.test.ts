import { flushPromises } from '@vue/test-utils'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { FlowDoc, OpsCatalog } from '../../../api/types'
import { core } from '../../../api/core'
import { mountWithApp } from '../../../test/mount'
import { setConfig } from '../../../lib/flowEdit'

vi.mock('../../../api/core', () => ({ core: { validateFlow: vi.fn(async () => ({ errors: [], warnings: [] })), listFlows: vi.fn(async () => []), testNode: vi.fn() }, events: {} }))

import { useFlowEditorStore } from '../../../stores/flowEditor'
import FieldControl from './FieldControl.vue'
import NodePanel from './NodePanel.vue'
import UiSelect from '../../../components/UiSelect.vue'
import { testCatalog } from './testOps'

const field = (name: string, kind: string, required = false, options: string[] = [], def: unknown = null) => ({ name, kind, options, required, default: def })
const catalog: OpsCatalog = {
  system: ['profile_id'],
  ops: [
    ...testCatalog.ops,
    { id: 'web.goto', adspower: 'Go to URL', group: 'web', inputs: ['in'], outputs: ['next'], fields: [field('url', 'text', true), field('timeout_ms', 'int', false, [], 30000)] },
    { id: 'web.weird', adspower: 'Weird', group: 'web', inputs: ['in'], outputs: ['next'], fields: [field('target', 'mystery', true)] },
    { id: 'web.scroll', adspower: 'Scroll', group: 'web', inputs: ['in'], outputs: ['next'], fields: [
      field('area', 'choice', false, ['page', 'target'], 'page'), field('target', 'target'),
      field('position', 'choice', false, ['top', 'middle', 'bottom', 'pixels'], 'bottom'), field('pixels', 'int', false, [], 0),
      field('kind', 'choice', false, ['smooth', 'auto'], 'smooth'),
      field('step_px', 'int_range', false, [], { min: 100, max: 150 }), field('step_ms', 'int_range', false, [], { min: 200, max: 300 }),
    ] },
  ],
}

function setup(nodes: FlowDoc['nodes'], selected = 'n2', extra: Partial<FlowDoc> = {}) {
  const store = useFlowEditorStore()
  store.catalog = catalog
  store.doc = { format: 'ombra-flow/1', name: 'T', nodes, ...extra }
  store.selected = selected
  return { store, wrapper: mountWithApp(NodePanel) }
}
const goto = [{ id: 'n1', op: 'start' }, { id: 'n2', op: 'web.goto' }]

beforeEach(() => vi.useRealTimers())

describe('NodePanel', () => {
  it('прокрутка показывает умолчания без правки документа; зависимые поля меняются вместе с режимом', async () => {
    const { wrapper, store } = setup([{ id: 'n1', op: 'start' }, { id: 'n2', op: 'web.scroll' }])
    expect(wrapper.get<HTMLInputElement>('#f-step_px').element.value).toBe('100')
    expect(wrapper.get<HTMLInputElement>('#f-step_ms').element.value).toBe('200')
    expect(store.doc!.nodes[1]!.config).toBeUndefined()
    expect(store.canUndo).toBe(false)
    expect(wrapper.find('#f-pixels').exists()).toBe(false)
    expect(wrapper.findComponent(FieldControl).exists()).toBe(false)
    await wrapper.findAll('button').find((button) => button.text() === 'Селектор')!.trigger('click')
    expect(wrapper.findComponent(FieldControl).props('field').name).toBe('target')
    const mode = wrapper.findAllComponents(UiSelect).find((select) => (select.props() as { ariaLabel?: string }).ariaLabel === 'Способ задания позиции')!
    mode.vm.$emit('update:modelValue', 'pixels')
    await flushPromises()
    await wrapper.get('#f-pixels').setValue('250')
    expect(store.doc!.nodes[1]!.config).toMatchObject({ area: 'target', position: 'pixels', pixels: 250 })
    await wrapper.get('#f-step_px').setValue('120')
    expect(store.doc!.nodes[1]!.config!.step_px).toEqual({ min: 120, max: 150 })
    await wrapper.get('input[type="radio"][value="auto"]').setValue(true)
    expect(wrapper.find('.scroll-speed').exists()).toBe(false)
    await wrapper.get('input[type="radio"][value="smooth"]').setValue(true)
    expect(wrapper.get<HTMLInputElement>('#f-step_px').element.value).toBe('120')
  })

  it('фиксированная скорость прокрутки остаётся фиксированной до правки; скрытые ошибки видны у узла', async () => {
    const { wrapper, store } = setup([{ id: 'n1', op: 'start' }, { id: 'n2', op: 'web.scroll', config: { step_px: 90, kind: 'auto' } }])
    store.report = { errors: [{ node: 'n2', field: 'step_px', message: 'Шаг должен быть положительным' }], warnings: [] }
    await flushPromises()
    expect(wrapper.text()).toContain('Шаг должен быть положительным')
    await wrapper.get('input[type="radio"][value="smooth"]').setValue(true)
    expect(wrapper.get<HTMLInputElement>('#f-step_px').element.value).toBe('90')
    expect(store.doc!.nodes[1]!.config!.step_px).toBe(90)
    const end = wrapper.get('#f-step_px').element.closest('.pair')!.querySelectorAll('input')[1]!
    end.value = '110'
    end.dispatchEvent(new Event('input', { bubbles: true }))
    expect(store.doc!.nodes[1]!.config!.step_px).toEqual({ min: 90, max: 110 })
  })

  it('«Проверить» — только у Email и Verification Code: результат и ошибка под кнопкой', async () => {
    const cat = { ...catalog, ops: [...catalog.ops, { id: 'data.totp', adspower: 'Verification Code', group: 'data', inputs: ['in'], outputs: ['next'], fields: [field('secret', 'secret')] }] }
    const { wrapper, store } = setup([{ id: 'n1', op: 'start' }, { id: 'n2', op: 'data.totp' }])
    store.catalog = cat
    await flushPromises()
    vi.mocked(core.testNode).mockResolvedValueOnce('123456')
    await wrapper.find('.probe button').trigger('click')
    await flushPromises()
    expect(core.testNode).toHaveBeenCalledWith(store.doc, 'n2')
    expect(wrapper.find('[data-probe=ok]').text()).toBe('123456')
    vi.mocked(core.testNode).mockRejectedValueOnce(new Error('ключ 2FA не в Base32'))
    await wrapper.find('.probe button').trigger('click')
    await flushPromises()
    expect(wrapper.find('[data-probe=error]').text()).toBe('ключ 2FA не в Base32')
    expect(wrapper.find('[data-probe=ok]').exists()).toBe(false)
    const other = setup(goto)
    await flushPromises()
    expect(other.wrapper.find('.probe').exists()).toBe(false)
  })

  it('вид any — текстовое поле: ${x} и текст строкой, JSON объектом', async () => {
    const cat = { ...catalog, ops: [...catalog.ops, { id: 'web.any', adspower: 'Any', group: 'web', inputs: ['in'], outputs: ['next'], fields: [field('value', 'any')] }] }
    const { wrapper, store } = setup([{ id: 'n1', op: 'start' }, { id: 'n2', op: 'web.any' }])
    store.catalog = cat
    await flushPromises()
    const input = wrapper.find('input#f-value')
    await input.setValue('${x}')
    expect(store.doc!.nodes[1]!.config).toEqual({ value: '${x}' })
    await input.setValue('abc')
    expect(store.doc!.nodes[1]!.config).toEqual({ value: 'abc' })
    await input.setValue('{"a":1}')
    expect(store.doc!.nodes[1]!.config).toEqual({ value: { a: 1 } })
  })


  it('«Проверить»: результат гаснет при правке узла, ответ на старый запрос игнорируется', async () => {
    const cat = { ...catalog, ops: [...catalog.ops, { id: 'data.totp', adspower: 'Verification Code', group: 'data', inputs: ['in'], outputs: ['next'], fields: [field('secret', 'secret')] }] }
    const { wrapper, store } = setup([{ id: 'n1', op: 'start' }, { id: 'n2', op: 'data.totp' }, { id: 'n3', op: 'data.totp' }])
    store.catalog = cat
    await flushPromises()
    vi.mocked(core.testNode).mockResolvedValueOnce('111111')
    await wrapper.find('.probe button').trigger('click')
    await flushPromises()
    expect(wrapper.find('[data-probe=ok]').exists()).toBe(true)
    await wrapper.find('input#f-secret').setValue('ABC')
    expect(wrapper.find('[data-probe=ok]').exists()).toBe(false)
    // A -> B -> A, пока ответ первого запроса в пути
    let late!: (v: string) => void
    vi.mocked(core.testNode).mockImplementationOnce(() => new Promise<string>((r) => { late = r }))
    await wrapper.find('.probe button').trigger('click')
    store.selected = 'n3'
    await flushPromises()
    store.selected = 'n2'
    await flushPromises()
    late('222222')
    await flushPromises()
    expect(wrapper.find('[data-probe=ok]').exists()).toBe(false)
  })

  it('«Проверить»: правка другого узла результат не гасит', async () => {
    const cat = { ...catalog, ops: [...catalog.ops, { id: 'data.totp', adspower: 'Verification Code', group: 'data', inputs: ['in'], outputs: ['next'], fields: [field('secret', 'secret')] }] }
    const { wrapper, store } = setup([{ id: 'n1', op: 'start' }, { id: 'n2', op: 'data.totp' }, { id: 'n3', op: 'data.totp' }])
    store.catalog = cat
    await flushPromises()
    vi.mocked(core.testNode).mockResolvedValueOnce('111111')
    await wrapper.find('.probe button').trigger('click')
    await flushPromises()
    store.apply((doc) => setConfig(doc, store.path, 'n3', 'secret', 'ABC'))
    await flushPromises()
    expect(wrapper.find('[data-probe=ok]').text()).toBe('111111')
  })

  it('вид any: набранный JSON не переписывается, пока поле в фокусе; на blur — как в документе', async () => {
    const cat = { ...catalog, ops: [...catalog.ops, { id: 'web.any', adspower: 'Any', group: 'web', inputs: ['in'], outputs: ['next'], fields: [field('value', 'any')] }] }
    const { wrapper, store } = setup([{ id: 'n1', op: 'start' }, { id: 'n2', op: 'web.any' }])
    store.catalog = cat
    await flushPromises()
    const input = wrapper.find<HTMLInputElement>('input#f-value')
    await input.setValue('{"a": 1, "b": 2}')
    expect(input.element.value).toBe('{"a": 1, "b": 2}')
    expect(store.doc!.nodes[1]!.config).toEqual({ value: { a: 1, b: 2 } })
    await input.trigger('blur')
    expect(input.element.value).toBe('{"a":1,"b":2}')
  })

  it('вид any: отмена при фокусе в поле сразу показывает значение документа', async () => {
    const cat = { ...catalog, ops: [...catalog.ops, { id: 'web.any', adspower: 'Any', group: 'web', inputs: ['in'], outputs: ['next'], fields: [field('value', 'any')] }] }
    const { wrapper, store } = setup([{ id: 'n1', op: 'start' }, { id: 'n2', op: 'web.any', config: { value: 'abc' } }])
    store.catalog = cat
    await flushPromises()
    const input = wrapper.find<HTMLInputElement>('input#f-value')
    input.element.focus()
    await input.setValue('{"a": 1}')
    store.undo()
    await flushPromises()
    expect(input.element.value).toBe('abc')
    store.redo()
    await flushPromises()
    expect(input.element.value).toBe('{"a":1}')
  })

  it('поле JSON: отмена при фокусе сразу обновляет черновик, свой равнозначный текст остаётся', async () => {
    const { wrapper, store } = setup([{ id: 'n1', op: 'start' }, { id: 'n2', op: 'web.weird', config: { target: { a: 1 } } }])
    const area = wrapper.find<HTMLTextAreaElement>('textarea#f-target')
    area.element.focus()
    await area.setValue('{ "b" : 2 }')
    expect(area.element.value).toBe('{ "b" : 2 }')
    store.undo()
    await flushPromises()
    expect(JSON.parse(area.element.value)).toEqual({ a: 1 })
    await area.setValue('{oops')
    store.redo()
    await flushPromises()
    expect(JSON.parse(area.element.value)).toEqual({ b: 2 })
    expect(wrapper.find('.error-text').exists()).toBe(false)
  })

  it('без выбора — подсказка', () => {
    const { wrapper } = setup(goto, 'zzz')
    expect(wrapper.text()).toContain('Выберите блок на схеме')
  })

  it('операция без перевода — заголовок из названия AdsPower', () => {
    const { wrapper, store } = setup([{ id: 'n1', op: 'start' }, { id: 'n2', op: 'ext.tool' }])
    store.catalog = { ...catalog, ops: [...catalog.ops, { id: 'ext.tool', adspower: 'Some Tool', group: 'ext', inputs: ['in'], outputs: ['next'], fields: [] }] }
    return flushPromises().then(() => {
      expect(wrapper.find('.title').text()).toBe('Some Tool')
      expect(wrapper.find('.sub').exists()).toBe(false)
    })
  })

  it('поля операции по каталогу', () => {
    const { wrapper } = setup(goto)
    expect(wrapper.text()).toContain('Адрес *')
    expect(wrapper.text()).toContain('Время ожидания, мс')
  })

  it('правка поля попадает в конфиг узла', async () => {
    const { wrapper, store } = setup(goto)
    await wrapper.find('input#f-url').setValue('https://a.b')
    expect(store.doc!.nodes[1]!.config).toEqual({ url: 'https://a.b' })
  })

  it('ошибка проверки показана у поля', async () => {
    const { wrapper, store } = setup(goto)
    store.report = { errors: [{ node: 'n2', field: 'url', message: 'Нужен адрес' }], warnings: [] }
    await flushPromises()
    expect(wrapper.find('.error-text').text()).toBe('Нужен адрес')
  })

  it('{x} вставляет переменную в позицию курсора', async () => {
    const { wrapper, store } = setup(goto, 'n2', {
      variables: [{ name: 'user', type: 'string', default: null, description: '' }],
    })
    const input = wrapper.find<HTMLInputElement>('input#f-url')
    await input.setValue('ab')
    input.element.setSelectionRange(1, 1)
    await wrapper.find('button.var-insert').trigger('click')
    await wrapper.findAll('.var-menu button').find((b) => b.text() === 'user')!.trigger('click')
    expect(store.doc!.nodes[1]!.config!.url).toBe('a${user}b')
  })

  it('политика «Ветка ошибки» даёт узлу выход error', async () => {
    const { wrapper, store } = setup(goto)
    store.apply((d) => ({ ...d, nodes: d.nodes.map((n) => (n.id === 'n2' ? { ...n, on_error: 'branch' as const } : n)) }))
    await flushPromises()
    expect(store.doc!.nodes[1]!.on_error).toBe('branch')
    expect(wrapper.text()).toContain('Ветка ошибки')
  })

  it('описание и отключение пишутся в узел', async () => {
    const { wrapper, store } = setup(goto)
    await wrapper.find('input#node-description').setValue('Открыть')
    expect(store.doc!.nodes[1]!.description).toBe('Открыть')
    await wrapper.find('[role=switch]').trigger('click')
    expect(store.doc!.nodes[1]!.disabled).toBe(true)
  })

  it('переменные Start добавляются и удаляются, подсказка про пароли видна', async () => {
    const { wrapper, store } = setup(goto, 'n1')
    expect(wrapper.text()).toContain('Значения по умолчанию сохраняются без шифрования. Пароли и ключи передавайте в параметрах задачи или в таблице.')
    await wrapper.find('button.add-var').trigger('click')
    expect(store.doc!.variables).toHaveLength(1)
    await wrapper.find('input.var-name').setValue('login')
    expect(store.doc!.variables![0]!.name).toBe('login')
    await wrapper.find('button.del-var').trigger('click')
    expect(store.doc!.variables).toHaveLength(0)
  })

  it('подсказка про пароли — и у ввода текста, и у переменных процесса', async () => {
    const hint = 'Пароли — через ${переменную}, параметр или таблицу'
    const cat = { ...catalog, ops: [...catalog.ops, ...['web.input', 'proc.vars'].map((id) => ({ id, adspower: id, group: 'web', inputs: ['in'], outputs: ['next'], fields: [] }))] }
    const { wrapper, store } = setup([{ id: 'n1', op: 'start' }, { id: 'n2', op: 'web.input' }, { id: 'n3', op: 'proc.vars' }, { id: 'n4', op: 'web.goto' }])
    store.catalog = cat
    await flushPromises()
    expect(wrapper.text()).toContain(hint)
    store.selected = 'n3'
    await flushPromises()
    expect(wrapper.text()).toContain(hint)
    store.selected = 'n4'
    await flushPromises()
    expect(wrapper.text()).not.toContain(hint)
  })

  it('неверный JSON в поле — ошибка, значение не меняется', async () => {
    const { wrapper, store } = setup([{ id: 'n1', op: 'start' }, { id: 'n2', op: 'web.weird', config: { target: { a: 1 } } }])
    const area = wrapper.find('textarea#f-target')
    await area.setValue('{oops')
    expect(wrapper.find('.error-text').text()).toBe('Ожидается JSON')
    expect(store.doc!.nodes[1]!.config).toEqual({ target: { a: 1 } })
    await area.setValue('{"b":2}')
    expect(store.doc!.nodes[1]!.config).toEqual({ target: { b: 2 } })
  })

  it('неверный черновик JSON переживает правку другого поля', async () => {
    const { wrapper } = setup([{ id: 'n1', op: 'start' }, { id: 'n2', op: 'web.weird', config: { target: { a: 1 } } }])
    await wrapper.find('textarea#f-target').setValue('{oops')
    await wrapper.find('input#node-description').setValue('Описание')
    expect(wrapper.find<HTMLTextAreaElement>('textarea#f-target').element.value).toBe('{oops')
    expect(wrapper.find('.error-text').text()).toBe('Ожидается JSON')
  })

  it('черновик JSON и ошибка не переходят на другой узел', async () => {
    const { wrapper, store } = setup([{ id: 'n1', op: 'start' }, { id: 'n2', op: 'web.weird', config: { target: { a: 1 } } }, { id: 'n3', op: 'web.weird', config: { target: { c: 3 } } }])
    await wrapper.find('textarea#f-target').setValue('{oops')
    store.selected = 'n3'
    await flushPromises()
    expect(wrapper.find('.error-text').exists()).toBe(false)
    expect(wrapper.find<HTMLTextAreaElement>('textarea#f-target').element.value).toContain('"c": 3')
  })

  it('набор JSON по символам не переписывается', async () => {
    const { wrapper } = setup([{ id: 'n1', op: 'start' }, { id: 'n2', op: 'web.weird' }])
    const area = wrapper.find<HTMLTextAreaElement>('textarea#f-target')
    for (const text of ['{', '{"a"', '{"a":1', '{"a":1}']) {
      await area.setValue(text)
      await flushPromises()
      expect(area.element.value).toBe(text)
    }
  })

  it('целое принимает ${n}, промежуточный текст не переписывается', async () => {
    const { wrapper, store } = setup(goto)
    const input = wrapper.find<HTMLInputElement>('input#f-timeout_ms')
    await input.setValue('${n}')
    expect(store.doc!.nodes[1]!.config).toEqual({ timeout_ms: '${n}' })
    await input.setValue('007')
    expect(input.element.value).toBe('007')
  })

  it('ошибки процесса без узла видны в панели Start', async () => {
    const { wrapper, store } = setup(goto, 'n1', { variables: [{ name: 'login', type: 'string', default: null, description: '' }] })
    store.report = {
      errors: [
        { node: null, field: 'name', message: 'имя процесса — от 1 до 100 символов' },
        { node: null, field: 'variables.login', message: 'переменная объявлена дважды' },
        { node: null, field: null, message: 'цикл в графе без узла цикла' },
        { node: 'n2', field: 'url', message: 'Нужен адрес' },
      ],
      warnings: [],
    }
    await flushPromises()
    const box = wrapper.find('.flow-issues')
    expect(box.text()).toContain('Ошибки сценария')
    expect(box.findAll('.error-text').map((p) => p.text())).toEqual([
      'имя процесса — от 1 до 100 символов', 'login: переменная объявлена дважды', 'цикл в графе без узла цикла',
    ])
    store.selected = 'n2'
    await flushPromises()
    expect(wrapper.find('.flow-issues').exists()).toBe(false)
  })

  it('предупреждения — у поля и узла приглушённо, не как ошибки', async () => {
    const { wrapper, store } = setup(goto)
    store.report = { errors: [], warnings: [{ node: 'n2', field: 'url', message: 'переменная x нигде не задаётся' }, { node: 'n2', field: null, message: 'узел недостижим из start' }] }
    await flushPromises()
    expect(wrapper.find('.error-text').exists()).toBe(false)
    expect(wrapper.findAll('.hint').map((h) => h.text())).toEqual(expect.arrayContaining(['переменная x нигде не задаётся', 'узел недостижим из start']))
    expect(wrapper.find('input#f-url').classes()).not.toContain('invalid')
  })

  it('ошибка переменной Start — у её строки', async () => {
    const vars = [{ name: 'login', type: 'string' as const, default: null, description: '' }, { name: 'other', type: 'string' as const, default: null, description: '' }]
    const { wrapper, store } = setup(goto, 'n1', { variables: vars })
    store.report = { errors: [{ node: null, field: 'variables.login', message: 'переменная объявлена дважды' }], warnings: [] }
    await flushPromises()
    const rows = wrapper.findAll('.var-row')
    expect(rows[0]!.find('.error-text').text()).toBe('переменная объявлена дважды')
    expect(rows[0]!.find('input.var-name').classes()).toContain('invalid')
    expect(rows[1]!.find('.error-text').exists()).toBe(false)
  })

  it('число в умолчании Start: «1.5» даёт 1.5', async () => {
    const { wrapper, store } = setup(goto, 'n1', { variables: [{ name: 'x', type: 'number', default: null, description: '' }] })
    const input = wrapper.find<HTMLInputElement>('input.var-default')
    for (const text of ['1', '1.', '1.5']) {
      await input.setValue(text)
      expect(input.element.value).toBe(text)
    }
    expect(store.doc!.variables![0]!.default).toBe(1.5)
  })
})

describe('NodePanel: формы Excel и Email', () => {
  const cat = (op: OpsCatalog['ops'][number]): OpsCatalog => ({ ...catalog, ops: [...catalog.ops, op] })
  it('«6 цифр» вставляет правило, секрет подсказывает про ${переменную}, параметр или таблицу', async () => {
    const { wrapper, store } = setup([{ id: 'n1', op: 'start' }, { id: 'n2', op: 'data.email' }])
    store.catalog = cat({ id: 'data.email', adspower: 'Email address', group: 'data', inputs: ['in'], outputs: ['next'], fields: [field('password', 'secret'), field('extract', 'text')] })
    await flushPromises()
    expect(wrapper.text()).toContain('Пароли — через ${переменную}, параметр или таблицу')
    await wrapper.get('[data-preset="digits6"]').trigger('click')
    expect(store.doc!.nodes[1].config?.extract).toBe('\\b\\d{6}\\b')
  })
  it('у секрета с предупреждением видны и предупреждение, и подсказка', async () => {
    const { wrapper, store } = setup([{ id: 'n1', op: 'start' }, { id: 'n2', op: 'data.email' }])
    store.catalog = cat({ id: 'data.email', adspower: 'Email address', group: 'data', inputs: ['in'], outputs: ['next'], fields: [field('password', 'secret')] })
    store.report = { errors: [], warnings: [{ node: 'n2', field: 'password', message: 'неверная ссылка на переменную' }] }
    await flushPromises()
    const text = wrapper.text()
    expect(text).toContain('неверная ссылка на переменную')
    expect(text).toContain('параметр или таблицу')
  })
  it('«6 цифр» — только у extract узла Email', async () => {
    const { wrapper, store } = setup([{ id: 'n1', op: 'start' }, { id: 'n2', op: 'data.other' }])
    store.catalog = cat({ id: 'data.other', adspower: 'Other', group: 'data', inputs: ['in'], outputs: ['next'], fields: [field('extract', 'text')] })
    await flushPromises()
    expect(wrapper.find('input#f-extract').exists()).toBe(true)
    expect(wrapper.find('[data-preset="digits6"]').exists()).toBe(false)
  })
  describe('поля варианта операции', () => {
    const ext = (id: string, fields: OpsCatalog['ops'][number]['fields']) => ({ id, adspower: id, group: 'ext', inputs: ['in'], outputs: ['next'], fields })
    const captcha = ext('ext.2captcha', [
      field('api_key', 'secret', true), field('kind', 'choice', false, ['normal', 'recaptcha_v2', 'recaptcha_v3', 'hcaptcha', 'turnstile'], 'recaptcha_v2'),
      field('image', 'target'), field('image_path', 'text'), field('answer_target', 'target'), field('phrase', 'bool', false, [], false),
      field('case_sensitive', 'bool', false, [], false), field('math', 'bool', false, [], false), field('numeric', 'choice', false, ['any'], 'any'),
      field('min_len', 'int', false, [], 0), field('max_len', 'int', false, [], 0), field('sitekey', 'text'), field('action', 'text'),
      field('min_score', 'text', false, [], '0.3'), field('answer_to', 'var_out'), field('save_to', 'var_out', true),
    ])
    const openai = ext('ext.openai', [
      field('api_key', 'secret', true), field('output', 'choice', false, ['text', 'image'], 'text'), field('model', 'text'), field('prompt', 'text', true),
      field('max_tokens', 'int', false, [], 2000), field('size', 'text', false, [], '1024x1024'), field('format', 'choice', false, ['file', 'b64_json'], 'file'),
      field('quality', 'choice', false, ['auto', 'low', 'medium', 'high', 'xhigh', 'max'], 'auto'), field('save_to', 'var_out', true),
    ])
    const sheet = ext('ext.google_sheet', [
      field('operation', 'choice', false, ['read', 'write', 'clear'], 'read'), field('spreadsheet_id', 'text', true), field('sheet', 'text', false, [], 'Sheet1'),
      field('range', 'text'), field('header', 'bool', false, [], true), field('data', 'any'), field('save_to', 'var_out'),
    ])
    async function open(op: OpsCatalog['ops'][number], config: Record<string, unknown> = {}) {
      const mounted = setup([{ id: 'n1', op: 'start' }, { id: 'n2', op: op.id, config }])
      mounted.store.catalog = { ...catalog, ops: [...catalog.ops, op] }
      await flushPromises()
      const shown = () => mounted.wrapper.findAllComponents(FieldControl).map((c) => (c.props('field') as { name: string }).name)
      const choose = async (name: string, value: string) => {
        mounted.store.apply((doc) => setConfig(doc, [], 'n2', name, value))
        await flushPromises()
      }
      return { ...mounted, shown, choose }
    }

    it('2Captcha: поля обычной капчи — только для normal, action и min_score — только для v3, sitekey — не для normal', async () => {
      const { shown, choose, store } = await open(captcha, { image_path: '/tmp/c.png' })
      expect(shown()).toEqual(['api_key', 'kind', 'sitekey', 'answer_to', 'save_to'])
      await choose('kind', 'recaptcha_v3')
      expect(shown()).toEqual(['api_key', 'kind', 'sitekey', 'action', 'min_score', 'answer_to', 'save_to'])
      await choose('kind', 'normal')
      expect(shown()).toEqual(['api_key', 'kind', 'image', 'image_path', 'answer_target', 'phrase', 'case_sensitive', 'math', 'numeric', 'min_len', 'max_len', 'answer_to', 'save_to'])
      expect(store.doc!.nodes[1]!.config?.image_path).toBe('/tmp/c.png')
    })

    it('OpenAI: поля картинки — только для image, max_tokens — только для text', async () => {
      const { shown, choose } = await open(openai)
      expect(shown()).toEqual(['api_key', 'output', 'model', 'prompt', 'max_tokens', 'save_to'])
      await choose('output', 'image')
      expect(shown()).toEqual(['api_key', 'output', 'model', 'prompt', 'size', 'format', 'quality', 'save_to'])
    })

    it('Google Sheet: data — для write, save_to — для read; «Проверить» — только у чтения', async () => {
      const { shown, choose, wrapper } = await open(sheet)
      expect(shown()).toEqual(['operation', 'spreadsheet_id', 'sheet', 'range', 'header', 'save_to'])
      expect(wrapper.find('.probe').exists()).toBe(true)
      await choose('operation', 'write')
      expect(shown()).toEqual(['operation', 'spreadsheet_id', 'sheet', 'range', 'header', 'data'])
      expect(wrapper.find('.probe').exists()).toBe(false)
      await choose('operation', 'clear')
      expect(shown()).toEqual(['operation', 'spreadsheet_id', 'sheet', 'range', 'header'])
      expect(wrapper.find('.probe').exists()).toBe(false)
    })

    it('ошибка скрытого поля видна у узла', async () => {
      const { wrapper, store } = await open(sheet, { data: 5 })
      store.report = { errors: [{ node: 'n2', field: 'data', message: 'ожидается массив' }], warnings: [] }
      await flushPromises()
      expect(wrapper.text()).toContain('Данные: ожидается массив')
    })
  })

  it('columns Save to Excel — список переменных (теги), а не строка', async () => {
    const { wrapper, store } = setup([{ id: 'n1', op: 'start' }, { id: 'n2', op: 'data.save_excel', config: { columns: ['a', 'b'] } }])
    store.catalog = cat({ id: 'data.save_excel', adspower: 'Save to Excel', group: 'data', inputs: ['in'], outputs: ['next'], fields: [field('columns', 'var_in_list', true)] })
    await flushPromises()
    expect(wrapper.findAll('.tags .tag').map((tag) => tag.text())).toEqual(['a', 'b'])
    expect(wrapper.find('input[type="password"]').exists()).toBe(false)
  })
})
