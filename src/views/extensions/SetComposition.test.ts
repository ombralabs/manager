import { describe, expect, it } from 'vitest'
import { extCategory, extension, riskyExtension } from '../../test/fixtures'
import { mountWithApp } from '../../test/mount'
import SetComposition from './SetComposition.vue'

const HELPER = extension({ id: 'kfmnhoepiablgmcdbjlpkgofdjbcnhia', name: 'Helper', source: 'file', file_name: 'team.zip', version: '0.4.1' })

describe('состав набора', () => {
  it('шапка и «в наборе N из M»; строки — вся библиотека; переключатель — «в наборе»; «только в наборах» у выключенных', async () => {
    const set = extCategory({ id: 1, name: 'Крипто', extension_ids: [riskyExtension.id] })
    const wrapper = mountWithApp(SetComposition, {
      props: { set, library: [extension(), riskyExtension, HELPER], enabled: new Set([extension().id]), pending: [], saving: false },
    })
    expect(wrapper.find('.card-head').text()).toContain('Состав набора «Крипто»')
    expect(wrapper.find('.card-head .hint').text()).toBe('в наборе 1 из 3')
    const rows = wrapper.findAll('[data-test="composition-row"]')
    expect(rows.map((row) => row.find('.line').text())).toEqual([
      'Chrome Web Store · версия 1.62.0',
      'Chrome Web Store · версия 3.3.23',
      'файл team.zip · версия 0.4.1',
    ])
    expect(rows.map((row) => row.find('[role="switch"]').attributes('aria-label'))).toEqual([
      'uBlock Origin: не в наборе',
      'ZeroOmega: в наборе',
      'Helper: не в наборе',
    ])
    expect(rows.map((row) => row.find('.name-text').attributes('title'))).toEqual(['uBlock Origin', 'ZeroOmega', 'Helper'])
    expect(rows.map((row) => row.find('.badge').exists())).toEqual([false, true, true])
    expect(rows[1]!.find('.badge').text()).toBe('только в наборах')
    expect(rows[1]!.find('.badge').attributes('title')).toBe('Расширение доступно только профилям с назначенным набором')
    expect(rows[1]!.find('.risk').exists()).toBe(true)
    await rows[0]!.find('[role="switch"]').trigger('click')
    expect(wrapper.emitted('toggle')).toEqual([[extension().id, true]])
    expect(wrapper.find('.foot').text()).toBe('Добавьте расширения в набор. Изменения сохраняются автоматически.')
  })

  it('пока сохраняется — переключатели aria-disabled; загружающееся — неактивно, «Загружается»', () => {
    const wrapper = mountWithApp(SetComposition, {
      props: {
        set: extCategory({ id: 1, name: 'Крипто' }),
        library: [extension()],
        enabled: new Set<string>(),
        pending: [{ key: 'hlkenndednhfkekhgcdicdfddnkalmdm', name: 'hlkenndednhfkekhgcdicdfddnkalmdm', from: 'store' }],
        saving: true,
      },
    })
    const rows = wrapper.findAll('[data-test="composition-row"]')
    expect(rows[0]!.find('[role="switch"]').attributes('aria-disabled')).toBe('true')
    expect(rows[0]!.find('[role="switch"]').attributes('disabled')).toBeUndefined()
    const loading = rows[1]!.find('[role="switch"]')
    expect(loading.attributes('aria-label')).toBe('hlkenndednhfkekhgcdicdfddnkalmdm: загружается, добавить в набор пока нельзя')
    expect(loading.attributes('disabled')).toBeDefined()
    expect(rows[1]!.find('.status').text()).toBe('Загружается')
    expect(rows[1]!.find('.line').text()).toBe('Chrome Web Store')
  })
})
