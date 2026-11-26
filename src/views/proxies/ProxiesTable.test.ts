import { describe, expect, it } from 'vitest'
import type { ProxyRow } from '../../api/types'
import ProxiesTable from './ProxiesTable.vue'
import { proxyRow } from '../../test/fixtures'
import { mountWithApp } from '../../test/mount'

const base = proxyRow({ username: 'secret-login', has_password: true })

const rows: ProxyRow[] = [
  base,
  {
    ...base,
    id: 2,
    host: 'gw2.example.net',
    note: 'почта',
    profiles: 1,
    pinned_to: 'Почта 02',
    used_by: 'p2',
    used_by_name: 'Почта 02',
    check: { at: 1, ok: false, exit_ip: '203.0.113.24', country: 'DE' },
  },
  {
    ...base,
    id: 3,
    kind: 'socks5',
    host: 'gw3.example.net',
    profiles: 2,
    check: { at: 1, ok: true, exit_ip: '198.51.100.7', country: 'NL' },
  },
]

function mountTable(checking: number[] = []) {
  return mountWithApp(ProxiesTable, {
    props: { rows, checking: new Set(checking) },
  })
}

type Wrapper = ReturnType<typeof mountTable>
const rowText = (wrapper: Wrapper, index: number) => wrapper.findAll('.body .row')[index]!.text()

describe('таблица прокси', () => {
  it('«Закреплён за»: «Свободен», имя, «2 профиля»; работающий профиль второй строкой', () => {
    const wrapper = mountTable()
    expect(rowText(wrapper, 0)).toContain('Свободен')
    expect(rowText(wrapper, 1)).toContain('Почта 02')
    expect(rowText(wrapper, 1)).toContain('используется профилем: Почта 02')
    expect(rowText(wrapper, 2)).toContain('2 профиля')
  })

  it('«Проверка» и «Страна и внешний IP»', () => {
    const wrapper = mountTable()
    expect(rowText(wrapper, 0)).toContain('Не проверен')
    expect(rowText(wrapper, 0)).toContain('Неизвестно')
    expect(rowText(wrapper, 1)).toContain('Нет связи')
    expect(rowText(wrapper, 1)).toContain('DE · 203.0.113.24')
    expect(rowText(wrapper, 2)).toContain('Доступен')
    expect(rowText(wrapper, 2)).toContain('NL · 198.51.100.7')
    expect(rowText(wrapper, 2)).toContain('SOCKS5')
    expect(rowText(wrapper, 1)).toContain('gw2.example.net:8080')
    expect(rowText(wrapper, 1)).toContain('почта')
  })

  it('во время проверки — «Проверяется» вместо итога', () => {
    const wrapper = mountTable([3])
    expect(rowText(wrapper, 2)).toContain('Проверяется')
    expect(rowText(wrapper, 2)).not.toContain('Доступен')
  })

  it('логина нет в разметке', () => {
    expect(mountTable().html()).not.toContain('secret-login')
  })

  it('переключатель «Вкл.» сообщает новое значение', async () => {
    const wrapper = mountTable()
    await wrapper.find('[aria-label="Прокси gw1.example.net:8080 включён"]').trigger('click')
    expect(wrapper.emitted('toggle')).toEqual([[rows[0], false]])
  })
})
