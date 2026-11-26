import { beforeEach, describe, expect, it, vi } from 'vitest'
import { nextTick } from 'vue'
import { profileRow } from '../../test/fixtures'
import { mountWithApp } from '../../test/mount'

vi.mock('../../api/core', () => ({ core: {}, events: {} }))

import { useProfilesStore } from '../../stores/profiles'
import ProfilesTable from './ProfilesTable.vue'

const row = profileRow({ name: 'Магазин 1', restore_tabs: false, created_at: 0 })

it('shows the masked account beside a blocked running profile', () => {
  const blocked = profileRow({ status: { state: 'waiting', license: 'device_bound', ownerHint: 'iv***v@example.com', info: null } })
  const wrapper = mountWithApp(ProfilesTable, { props: { rows: [blocked], hidden: [], groups: [] } })
  expect(wrapper.find('.wait-reason').text()).toContain('iv***v@example.com')
})

it('показывает текущее подключение до перезапуска после смены режима', async () => {
  const wrapper = mountWithApp(ProfilesTable, { props: { rows: [profileRow({ direct: true })], hidden: [], groups: [] } })
  expect(wrapper.find('.gateway-title').text()).toBe('Без прокси')
  const running = { state: 'running' as const, pid: 1, cdp_ws: '', debug_port: 2, relay_port: 3, proxy_id: 7, exit_ip: null, country: null, timezone: 'UTC', started_at: 4 }
  await wrapper.setProps({ rows: [profileRow({ direct: true, status: running, gateway: { kind: 'socks5', host: 'proxy.example', port: 1080, ip: null, country: 'DE', ok: true } })] })
  expect(wrapper.find('.gateway-title').text()).toBe('DE · SOCKS5')
  await wrapper.setProps({ rows: [profileRow({ direct: false, status: { ...running, proxy_id: 0 } })] })
  expect(wrapper.find('.gateway-title').text()).toBe('Без прокси')
})

it('«Ожидает запуска» — причина видна текстом; неизвестное состояние — без подписи', async () => {
  const waiting = (license: string) => profileRow({ status: { state: 'waiting', license, info: null } })
  const wrapper = mountWithApp(ProfilesTable, { props: { rows: [waiting('no_slot')], hidden: [], groups: [] } })
  expect(wrapper.find('.profile-meta').text()).toContain('Достигнут лимит одновременных профилей')
  await wrapper.setProps({ rows: [waiting('mystery')] })
  expect(wrapper.find('.profile-meta').text()).toContain('Ожидает запуска')
  expect(wrapper.find('.wait-reason').exists()).toBe(false)
})

function setup() {
  const profiles = useProfilesStore()
  const wrapper = mountWithApp(ProfilesTable, {
    props: { rows: [row], hidden: [], groups: [] },
  })
  const body = wrapper.find('.body').element as HTMLElement
  return { profiles, body }
}

describe('шапка таблицы', () => {
  it('прокручивается по горизонтали вместе со строками', async () => {
    const wrapper = mountWithApp(ProfilesTable, { props: { rows: [row], hidden: [], groups: [] } })
    const body = wrapper.find('.body').element as HTMLElement
    body.scrollLeft = 120
    await wrapper.find('.body').trigger('scroll')
    expect((wrapper.find('.row.header').element as HTMLElement).scrollLeft).toBe(120)
  })
})

describe('шапка таблицы с клавиатуры', () => {
  it('прокрутка шапки (фокус с клавиатуры) сдвигает строки', async () => {
    const wrapper = mountWithApp(ProfilesTable, { props: { rows: [row], hidden: [], groups: [] } })
    const header = wrapper.find('.row.header').element as HTMLElement
    header.scrollLeft = 80
    await wrapper.find('.row.header').trigger('scroll')
    expect((wrapper.find('.body').element as HTMLElement).scrollLeft).toBe(80)
  })
})

describe('эхо прокрутки шапки', () => {
  it('отложенное событие шапки от нашей записи не откатывает ушедшее вперёд тело', async () => {
    const wrapper = mountWithApp(ProfilesTable, { props: { rows: [row], hidden: [], groups: [] } })
    const body = wrapper.find('.body').element as HTMLElement
    body.scrollLeft = 120
    await wrapper.find('.body').trigger('scroll')
    // Тело ушло дальше до того, как пришло событие шапки.
    body.scrollLeft = 200
    await wrapper.find('.row.header').trigger('scroll')
    expect(body.scrollLeft).toBe(200)
  })

  it('после прокрутки тачпадом туда и обратно Shift+Tab по шапке возвращает и строки', async () => {
    const wrapper = mountWithApp(ProfilesTable, { props: { rows: [row], hidden: [], groups: [] } })
    const body = wrapper.find('.body').element as HTMLElement
    const header = wrapper.find('.row.header').element as HTMLElement
    const scroll = async (target: 'body' | 'header', left: number) => {
      const element = target === 'body' ? body : header
      element.scrollLeft = left
      await wrapper.find(target === 'body' ? '.body' : '.row.header').trigger('scroll')
    }
    // Тачпад: вправо и обратно к левому краю; каждое событие тела порождает эхо шапки.
    await scroll('body', 120)
    await wrapper.find('.row.header').trigger('scroll')
    await scroll('body', 0)
    await wrapper.find('.row.header').trigger('scroll')
    // Tab по кнопкам сортировки: шапка уходит вправо, тело догоняет, его событие приходит следом.
    await scroll('header', 250)
    expect(body.scrollLeft).toBe(250)
    await wrapper.find('.body').trigger('scroll')
    // Shift+Tab к флажку «выбрать все»: шапка у левого края — тело тоже.
    await scroll('header', 0)
    expect(body.scrollLeft).toBe(0)
  })
})

describe('прокрутка таблицы', () => {
  beforeEach(() => localStorage.clear())

  const cases: [string, (profiles: ReturnType<typeof useProfilesStore>) => void][] = [
    ['страница', (p) => (p.page = 2)],
    ['размер страницы', (p) => (p.pageSize = 100)],
    ['поиск', (p) => (p.filters.search = 'а')],
    ['статус', (p) => (p.filters.label = 2)],
    ['группа', (p) => (p.filters.group = 3)],
    ['тег', (p) => (p.filters.tags = ['vip'])],
    ['без тегов', (p) => (p.filters.untagged = true)],
    ['сортировка', (p) => (p.filters.descending = !p.filters.descending)],
    ['фильтр «эти профили»', (p) => (p.only = ['p1'])],
  ]

  it.each(cases)('сбрасывается при смене: %s', async (_name, change) => {
    const { profiles, body } = setup()
    body.scrollTop = 500
    expect(body.scrollTop).toBe(500)
    change(profiles)
    await nextTick()
    expect(body.scrollTop).toBe(0)
  })
})
