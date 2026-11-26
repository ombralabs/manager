import { enableAutoUnmount, flushPromises } from '@vue/test-utils'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { createMemoryHistory, createRouter, RouterView } from 'vue-router'
import { mountWithApp } from '../../test/mount'
import { i18n } from '../../i18n'
import { flowTemplates } from '../../lib/flowTemplates'
import { useToasts } from '../../stores/toasts'
import FlowLibraryView from './FlowLibraryView.vue'

const api = vi.hoisted(() => ({
  importFlowJson: vi.fn(), listFlows: vi.fn(async () => []), createTask: vi.fn(),
}))
vi.mock('../../api/core', () => ({ core: api, events: {} }))
enableAutoUnmount(afterEach)
afterEach(() => { i18n.global.locale.value = 'ru' })
beforeEach(() => {
  vi.clearAllMocks()
  api.importFlowJson.mockReset()
  api.importFlowJson.mockResolvedValue({ id: 42 })
})

async function mountView(path = '/automation/library') {
  const router = createRouter({ history: createMemoryHistory(), routes: [
    { path: '/automation/library/:templateId?', component: FlowLibraryView },
    { path: '/:p(.*)*', component: { template: '<div />' } },
  ] })
  await router.push(path)
  const w = mountWithApp({ components: { RouterView }, template: '<RouterView />' }, {
    global: { plugins: [router] }, attachTo: document.body,
  })
  await flushPromises()
  return { w, router }
}

describe('workflow library catalog', () => {
  it('shows the Google tile and opens details with parameters and instructions', async () => {
    const { w, router } = await mountView()
    expect(w.findAll('.library-tile')).toHaveLength(1)
    expect(w.get('.library-tile').text()).toContain('CAPTCHA')
    expect(w.find('#task-param-url').exists()).toBe(false)
    await w.get('[data-template-id="google-search"]').trigger('click')
    await flushPromises()
    expect(router.currentRoute.value.path).toBe('/automation/library/google-search')
    expect(w.get('h2').text()).toBe('Google — поиск и переход на сайт')
    expect(w.find('#task-param-url').exists()).toBe(true)
    expect(w.findAll('.template-instructions li').length).toBeGreaterThan(2)
    expect(api.importFlowJson).not.toHaveBeenCalled()
  })

  it('combines case-insensitive search and category, with a working empty reset', async () => {
    const { w } = await mountView()
    await w.get('input[type="search"]').setValue('  GooGLe  ')
    await flushPromises()
    expect(w.findAll('.library-tile')).toHaveLength(1)
    expect(w.get('.library-tile').text()).toContain('Google')
    await w.get('[data-category="search"]').trigger('click')
    await flushPromises()
    await w.get('input[type="search"]').setValue('no match')
    await flushPromises()
    expect(w.findAll('.library-tile')).toHaveLength(0)
    expect(w.text()).toContain('Ничего не найдено')
    await w.get('.library-empty button').trigger('click')
    await flushPromises()
    expect(w.findAll('.library-tile')).toHaveLength(1)
    expect(w.get('input[type="search"]').element).toHaveProperty('value', '')
  })

  it('keeps filters and restores tile focus when returning from details', async () => {
    const { w, router } = await mountView('/automation/library?q=google&category=search')
    await w.get('[data-template-id="google-search"]').trigger('click')
    await flushPromises()
    await w.get('.library-back').trigger('click')
    await flushPromises()
    expect(router.currentRoute.value.query).toEqual({ q: 'google', category: 'search' })
    expect(w.findAll('.library-tile')).toHaveLength(1)
    expect(document.activeElement).toBe(w.get('[data-template-id="google-search"]').element)
  })

  it.each(['ru', 'en'] as const)('imports a localized editable copy, without starting a task (%s)', async (language) => {
    i18n.global.locale.value = language
    const { w, router } = await mountView('/automation/library/google-search')
    expect(w.get('#task-param-page_limit').element).toHaveProperty('value', '20')
    await w.get('#task-param-page_limit').setValue('7')
    await w.get('#task-param-url').setValue('https://example.test')
    await w.get('.template-use').trigger('click')
    await flushPromises()
    const source = JSON.parse(api.importFlowJson.mock.calls[0]![0])
    expect(source.name).toBe(language === 'en' ? 'Google — search and visit' : 'Google — поиск и переход на сайт')
    const inspect = source.nodes.find((node: { id: string }) => node.id === 'inspect')
    expect(inspect.op).toBe('control.group')
    expect(inspect.nodes.find((node: { id: string }) => node.id === 'inspect_loop').config.times).toBe(180)
    expect(source.nodes.find((node: { id: string }) => node.id === 'submit_ready').on_error).toBe('skip')
    expect(source.nodes.find((node: { id: string }) => node.id === 'next_ready').on_error).toBe('skip')
    expect(source.variables.find((v: { name: string }) => v.name === 'url').default).toBe('https://example.test')
    expect(source.variables.find((v: { name: string }) => v.name === 'page_limit').default).toBe(7)
    expect(JSON.parse(flowTemplates[0]!.sources[language]).variables.find((v: { name: string }) => v.name === 'page_limit').default).toBe(20)
    expect(JSON.parse(flowTemplates[0]!.sources[language]).variables.find((v: { name: string }) => v.name === 'url').default).toBe('')
    expect(router.currentRoute.value.path).toBe('/automation/flows/42')
    expect(api.createTask).not.toHaveBeenCalled()
  })

  it('preserves the executable graph when changing global defaults and search depth', async () => {
    const { w } = await mountView('/automation/library/google-search')
    await w.get('#task-param-keyword').setValue('sample query')
    await w.get('#task-param-url').setValue('example.com/docs')
    await w.get('#task-param-page_limit').setValue('3')
    await w.get('.template-use').trigger('click')
    await flushPromises()
    const copy = JSON.parse(api.importFlowJson.mock.calls[0]![0])
    const original = JSON.parse(flowTemplates[0]!.sources.ru)
    expect(copy.variables.map((v: { default: unknown }) => v.default)).toEqual(['sample query', 'example.com/docs', 3])
    expect({ ...copy, variables: [] }).toEqual({ ...original, variables: [] })
  })

  it('blocks duplicate imports and navigation; storage failure retains edits for retry', async () => {
    let reject!: (error: unknown) => void
    api.importFlowJson.mockImplementationOnce(() => new Promise((_, fail) => { reject = fail }))
    const { w, router } = await mountView('/automation/library/google-search')
    await w.get('#task-param-url').setValue('https://example.test')
    const use = w.get('.template-use')
    await use.trigger('click')
    await use.trigger('click')
    expect(api.importFlowJson).toHaveBeenCalledTimes(1)
    expect(use.attributes('aria-disabled')).toBe('true')
    await router.push('/automation/library')
    expect(router.currentRoute.value.path).toBe('/automation/library/google-search')
    reject({ code: 'Storage', message: '' })
    await flushPromises()
    expect(useToasts().items.at(-1)?.kind).toBe('error')
    expect(use.attributes('aria-disabled')).toBeUndefined()
    expect(w.get('#task-param-url').element).toHaveProperty('value', 'https://example.test')
    await use.trigger('click')
    await flushPromises()
    expect(router.currentRoute.value.path).toBe('/automation/flows/42')
  })

  it('keeps edits when changing language and translates detail content', async () => {
    const { w } = await mountView('/automation/library/google-search')
    await w.get('#task-param-url').setValue('https://example.test')
    i18n.global.locale.value = 'en'
    await flushPromises()
    expect(w.get('h2').text()).toBe('Google — search and visit')
    expect(w.get('#task-param-url').element).toHaveProperty('value', 'https://example.test')
    expect(w.get('.template-instructions').text()).not.toMatch(/[А-Яа-яЁё]/)
  })

  it('recovers from an unknown template and ignores unknown category filters', async () => {
    const { w, router } = await mountView('/automation/library/missing?category=unknown')
    expect(w.text()).toContain('Сценарий не найден')
    await w.get('.library-back').trigger('click')
    await flushPromises()
    expect(router.currentRoute.value.path).toBe('/automation/library')
    expect(w.findAll('.library-tile')).toHaveLength(1)
  })
})
