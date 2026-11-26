import { flushPromises } from '@vue/test-utils'
import { describe, expect, it, vi } from 'vitest'
import type { FlowDoc, OpsCatalog } from '../../../../api/types'
import { mountWithApp } from '../../../../test/mount'

const api = vi.hoisted(() => ({ validateFlow: vi.fn(async () => ({ errors: [], warnings: [] })), listFlows: vi.fn(async (): Promise<unknown[]> => []) }))
vi.mock('../../../../api/core', () => ({ core: api, events: {} }))

import UiSelect from '../../../../components/UiSelect.vue'
import { useAutomationStore } from '../../../../stores/automation'
import { useFlowEditorStore } from '../../../../stores/flowEditor'
import NodePanel from '../NodePanel.vue'
import { testCatalog } from '../testOps'

const f = (name: string, kind: string, required = false, options: string[] = [], def: unknown = null) => ({ name, kind, options, required, default: def })
const op = (id: string, fields: ReturnType<typeof f>[]) => ({ id, adspower: id, group: 'web', inputs: ['in'], outputs: ['next'], fields })
const catalog: OpsCatalog = {
  system: [],
  ops: [
    ...testCatalog.ops,
    op('t.target', [f('target', 'target', true)]),
    op('t.targets', [f('target', 'targets', true)]),
    op('t.range', [f('times', 'int_range')]),
    op('t.choices', [f('modifiers', 'choice_list', false, ['alt', 'ctrl'], ['alt'])]),
    op('t.vars', [f('vars', 'var_rows')]),
    op('t.call', [f('process', 'process', true), f('inputs', 'inputs')]),
    op('t.mouse', [f('use_mouse', 'mouse')]),
    op('t.names', [f('names', 'var_in_list')]),
    op('t.js', [f('inject', 'js_var_list')]),
    op('t.choice', [f('position', 'choice', false, ['top', 'middle', 'bottom', 'pixels', 'zzz_raw'], 'bottom'), f('button', 'choice', false, ['middle']), f('key', 'choice', true, ['Enter'])]),
    op('t.mods', [f('modifiers', 'choice_list', true, ['cmd', 'ctrl', 'alt', 'shift', 'zzz_raw']), f('kind', 'choice', false, ['text', 'object', 'iframe', 'html', 'attribute', 'child'])]),
  ],
}

function setup(opId: string, config?: Record<string, unknown>, extra: Partial<FlowDoc> = {}) {
  const store = useFlowEditorStore()
  store.catalog = catalog
  store.doc = { format: 'ombra-flow/1', name: 'T', nodes: [{ id: 'n1', op: 'start' }, { id: 'n2', op: opId, config }], ...extra }
  store.selected = 'n2'
  return { store, wrapper: mountWithApp(NodePanel) }
}
const cfg = (store: ReturnType<typeof useFlowEditorStore>) => store.doc!.nodes[1]!.config
const byText = (w: ReturnType<typeof setup>['wrapper'], sel: string, text: string) => w.findAll(sel).find((b) => b.text() === text)!
/** Пропсы UiSelect: он обобщённый, типы test-utils их не раскрывают. */
const sel = (s: { props: () => unknown }) => s.props() as { id?: string; modelValue: string; options: { value: string; label: string }[] }
/** Списки полей операции — без политики ошибки узла. */
const selects = (w: ReturnType<typeof setup>['wrapper']) => w.findAllComponents(UiSelect).filter((s) => sel(s).id !== 'node-on-error')

describe('составные поля', () => {
  it('цель CSS с номером 2', async () => {
    const { wrapper, store } = setup('t.target')
    await wrapper.find('input#f-target').setValue('#a')
    expect(cfg(store)).toEqual({ target: { by: 'css', selector: '#a' } })
    await wrapper.find('.range input').setValue('2')
    expect(cfg(store)).toEqual({ target: { by: 'css', selector: '#a', index: 2 } })
  })

  it('«Элемент из переменной» даёт {element}', async () => {
    const { wrapper, store } = setup('t.target', { target: { by: 'css', selector: '#a' } })
    await wrapper.find('[aria-label="Элемент из переменной"]').trigger('click')
    await wrapper.find('input#f-target').setValue('el')
    expect(cfg(store)).toEqual({ target: { element: '${el}' } })
  })

  it('у targets нет номера и переключателя элемента', () => {
    const { wrapper } = setup('t.targets')
    expect(wrapper.find('[aria-label="Элемент из переменной"]').exists()).toBe(false)
    expect(wrapper.text()).not.toContain('Номер элемента')
  })

  it('интервал {min, max}; целое — черновик до ухода из поля', async () => {
    const { wrapper, store } = setup('t.range')
    await byText(wrapper, 'button', 'Интервал').trigger('click')
    const [lo, hi] = wrapper.findAll<HTMLInputElement>('.pair input')
    await lo!.setValue('1')
    await hi!.setValue('3')
    expect(cfg(store)).toEqual({ times: { min: 1, max: 3 } })
    await byText(wrapper, 'button', 'Число').trigger('click')
    expect(cfg(store)).toEqual({ times: 1 })
    const one = wrapper.find<HTMLInputElement>('.range input')
    await one.setValue('1.')
    expect(one.element.value).toBe('1.')
    expect(cfg(store)).toEqual({ times: 1 })
  })

  it('«Число» → «Интервал» с пустым числом не оставляет старое значение', async () => {
    const { wrapper, store } = setup('t.range', { times: '${n}' })
    const one = wrapper.find<HTMLInputElement>('.range input')
    await one.setValue('')
    await byText(wrapper, 'button', 'Интервал').trigger('click')
    expect(cfg(store)?.times).toBeUndefined()
  })

  it('пустой choice_list — ошибка поля', async () => {
    const { wrapper, store } = setup('t.choices')
    await wrapper.findAll('input[type=checkbox]')[0]!.setValue(false)
    expect(cfg(store)).toEqual({ modifiers: [] })
    expect(wrapper.find('.error-text').text()).toBe('Выберите хотя бы один')
    await wrapper.findAll('input[type=checkbox]')[1]!.setValue(true)
    expect(cfg(store)).toEqual({ modifiers: ['ctrl'] })
  })

  it('var_in_list: неверное имя остаётся тегом с ошибкой поля, в процесс не идёт', async () => {
    const { wrapper, store } = setup('t.names')
    await wrapper.find('input.entry').setValue('a1, 1bad')
    await wrapper.find('input.entry').trigger('keydown', { key: 'Enter' })
    expect(cfg(store)).toEqual({ names: ['a1'] })
    expect(wrapper.findAll('.tag').map((t) => t.text())).toEqual(['a1', '1bad'])
    expect(wrapper.find('.error-text').text()).toBe('Неверное имя переменной: 1bad')
    await wrapper.findAll('.tag')[1]!.find('button').trigger('click')
    expect(wrapper.find('.error-text').exists()).toBe(false)
    expect(cfg(store)).toEqual({ names: ['a1'] })
  })

  it('js_var_list: слово JavaScript — ошибка поля, в процесс не идёт', async () => {
    const { wrapper, store } = setup('t.js')
    await wrapper.find('input.entry').setValue('ok, class')
    await wrapper.find('input.entry').trigger('keydown', { key: 'Enter' })
    expect(cfg(store)).toEqual({ inject: ['ok'] })
    expect(wrapper.findAll('.tag').map((t) => t.text())).toEqual(['ok', 'class'])
    expect(wrapper.find('.error-text').text()).toBe('class — зарезервированное слово JavaScript. Выберите другое имя переменной.')
  })

  it('элемент из переменной: неверное имя — ошибка поля, цель не меняется', async () => {
    const { wrapper, store } = setup('t.target', { target: { element: '${el}' } })
    const input = wrapper.find<HTMLInputElement>('input#f-target')
    await input.setValue('1x')
    expect(cfg(store)).toEqual({ target: { element: '${el}' } })
    expect(input.element.value).toBe('1x')
    expect(wrapper.find('.error-text').text()).toBe('Неверное имя переменной: 1x')
    await input.setValue('el2')
    expect(cfg(store)).toEqual({ target: { element: '${el2}' } })
    expect(wrapper.find('.error-text').exists()).toBe(false)
  })

  it('элемент из переменной: незаписанное неверное имя и его ошибка переживают правку другого поля', async () => {
    const { wrapper } = setup('t.target', { target: { element: '${el}' } })
    const input = wrapper.find<HTMLInputElement>('input#f-target')
    await input.setValue('1x')
    await wrapper.find('input#node-description').setValue('Описание')
    expect(input.element.value).toBe('1x')
    expect(wrapper.find('.error-text').text()).toBe('Неверное имя переменной: 1x')
  })

  it('inputs: пустая строка, уже записанная в процессе, остаётся при правке другой строки', async () => {
    const { wrapper, store } = setup('t.call', { process: 7, inputs: { a: '', b: 'x' } })
    const inputs = wrapper.findAll<HTMLInputElement>('.row input')
    await inputs[3]!.setValue('y')
    expect(cfg(store)).toEqual({ process: 7, inputs: { a: '', b: 'y' } })
  })

  it('js_var_list: отмена возвращает список без черновика отвергнутого и без его ошибки', async () => {
    const { wrapper, store } = setup('t.js', { inject: ['class'] })
    await wrapper.find('input.entry').setValue('ok')
    await wrapper.find('input.entry').trigger('keydown', { key: 'Enter' })
    expect(cfg(store)).toEqual({ inject: ['ok'] })
    expect(wrapper.findAll('.tag').map((t) => t.text())).toEqual(['ok', 'class'])
    store.undo()
    await flushPromises()
    expect(wrapper.findAll('.tag').map((t) => t.text())).toEqual(['class'])
    expect(wrapper.find('.error-text').exists()).toBe(false)
  })

  it('var_in_list: отмена ввода с верным и неверным именем не оставляет ошибки', async () => {
    const { wrapper, store } = setup('t.names')
    await wrapper.find('input.entry').setValue('a1, 1bad')
    await wrapper.find('input.entry').trigger('keydown', { key: 'Enter' })
    expect(wrapper.find('.error-text').exists()).toBe(true)
    store.undo()
    await flushPromises()
    expect(cfg(store)).toBeUndefined()
    expect(wrapper.findAll('.tag')).toHaveLength(0)
    expect(wrapper.find('.error-text').exists()).toBe(false)
  })

  it('inputs: пустое значение не пишется — действует умолчание подпроцесса', async () => {
    const { wrapper, store } = setup('t.call', { process: 7 })
    await wrapper.find('button.add-row').trigger('click')
    const [name, value] = wrapper.findAll<HTMLInputElement>('.row input')
    await name!.setValue('login')
    expect(cfg(store)).toEqual({ process: 7 })
    expect(name!.element.value).toBe('login')
    await value!.setValue('${u}')
    expect(cfg(store)).toEqual({ process: 7, inputs: { login: '${u}' } })
  })

  it('необязательный выбор снимается пунктом «По умолчанию (…)»: поле уходит из конфига', async () => {
    const { wrapper, store } = setup('t.choice', { position: 'top', key: 'Enter' })
    const [position, button, key] = selects(wrapper)
    expect(sel(position!).options[0]).toEqual({ value: '', label: 'По умолчанию (Низ)' })
    expect(sel(button!).options[0]).toEqual({ value: '', label: 'По умолчанию' })
    expect(sel(button!).modelValue).toBe('')
    expect(sel(key!).options.map((o) => o.value)).toEqual(['Enter'])
    position!.vm.$emit('update:modelValue', '')
    await flushPromises()
    expect(cfg(store)).toEqual({ key: 'Enter' })
    expect(sel(position!).modelValue).toBe('')
  })

  it('подписи вариантов: своя у поля, затем общая, иначе значение как есть', () => {
    const { wrapper } = setup('t.choice')
    const [position, button] = selects(wrapper)
    const labels = (s: typeof position) => sel(s!).options.map((o) => o.label)
    expect(labels(position)).toEqual(['По умолчанию (Низ)', 'Верх', 'Середина', 'Низ', 'Пиксели', 'zzz_raw'])
    expect(labels(button)).toEqual(['По умолчанию', 'Средняя'])
  })

  it('подписи модификаторов и видов извлечения', () => {
    const { wrapper } = setup('t.mods')
    expect(wrapper.findAll('.check').map((c) => c.text())).toEqual(['Cmd / Win', 'Ctrl', 'Alt / Option', 'Shift', 'zzz_raw'])
    expect(sel(selects(wrapper)[0]!).options.map((o) => o.label)).toEqual(['По умолчанию', 'Текст', 'Элемент', 'Фрейм', 'HTML', 'Атрибут', 'Дочерний элемент'])
  })

  it('строки proc.vars → [{name,type,value}]', async () => {
    const { wrapper, store } = setup('t.vars')
    await wrapper.find('button.add-row').trigger('click')
    expect(cfg(store)).toEqual({ vars: [{ name: '', type: 'string', value: '' }] })
    const [name, value] = wrapper.findAll<HTMLInputElement>('.row input')
    await name!.setValue('x')
    await value!.setValue('5')
    expect(cfg(store)).toEqual({ vars: [{ name: 'x', type: 'string', value: '5' }] })
    await wrapper.find('button.del-row').trigger('click')
    expect(cfg(store)?.vars).toBeUndefined()
  })

  it('inputs подсказывает переменные подпроцесса', async () => {
    api.listFlows.mockResolvedValue([{ id: 7, name: 'Sub', variables: [{ name: 'login' }, { name: 'pass' }] }])
    const automation = useAutomationStore()
    await automation.refreshFlows()
    const { wrapper, store } = setup('t.call', { process: 7 })
    await flushPromises()
    await wrapper.find('button.add-row').trigger('click')
    expect(wrapper.findAll('datalist option').map((o) => o.attributes('value'))).toEqual(['login', 'pass'])
    const [name, value] = wrapper.findAll<HTMLInputElement>('.row input')
    await value!.setValue('${u}')
    expect(cfg(store)).toEqual({ process: 7 })
    await name!.setValue('login')
    expect(cfg(store)).toEqual({ process: 7, inputs: { login: '${u}' } })
  })

  it('inputs: имя переменной подпроцесса вроде constructor передаётся', async () => {
    const { wrapper, store } = setup('t.call', { process: 7 })
    await wrapper.find('button.add-row').trigger('click')
    const [name, value] = wrapper.findAll<HTMLInputElement>('.row input')
    await value!.setValue('1')
    await name!.setValue('constructor')
    expect(cfg(store)).toEqual({ process: 7, inputs: { constructor: '1' } })
  })

  it('мышь: включение, X/Y и «отпустить через»; выключенная убирает поле', async () => {
    const { wrapper, store } = setup('t.mouse')
    await wrapper.find('[aria-label="Мышью"]').trigger('click')
    expect(cfg(store)).toEqual({ use_mouse: { x: 0, y: 0 } })
    const [x, y] = wrapper.findAll<HTMLInputElement>('.xy input')
    await x!.setValue('10')
    await y!.setValue('${py}')
    expect(cfg(store)).toEqual({ use_mouse: { x: 10, y: '${py}' } })
    await byText(wrapper, 'button', 'Интервал').trigger('click')
    await wrapper.findAll<HTMLInputElement>('.pair input')[0]!.setValue('50')
    await wrapper.findAll<HTMLInputElement>('.pair input')[1]!.setValue('80')
    expect(cfg(store)).toEqual({ use_mouse: { x: 10, y: '${py}', release_ms: { min: 50, max: 80 } } })
    await wrapper.find('[aria-label="Мышью"]').trigger('click')
    expect(cfg(store)?.use_mouse).toBeUndefined()
  })
})
