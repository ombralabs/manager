import { describe, expect, it } from 'vitest'
import type { ProfileRow, ProfileStatus, RunningInfo } from '../api/types'
import { profileRow } from '../test/fixtures'
import { GROUP_PALETTE, countStatuses, extSetCounts, nextGroupColor, panelProfiles, summaryParts } from './groups'

const info: RunningInfo = {
  pid: 1,
  cdp_ws: 'ws://127.0.0.1:1/devtools/browser/x',
  debug_port: 1,
  relay_port: 2,
  proxy_id: 1,
  exit_ip: null,
  country: null,
  timezone: 'UTC',
  started_at: 1,
}

function status(state: ProfileStatus['state']): ProfileStatus {
  switch (state) {
    case 'running':
      return { state: 'running', ...info }
    case 'paused':
      return { state: 'paused', ...info }
    case 'crashed':
      return { state: 'crashed', code: 1, signal: null, at: 1 }
    case 'starting':
      return { state: 'starting' }
    case 'waiting':
      return { state: 'waiting', license: 'no_slot', info: null }
    case 'stopped':
      return { state: 'stopped' }
  }
}

const row = (serial: number, state: ProfileStatus['state'], group: number | null = null): ProfileRow =>
  profileRow({
    id: `p${serial}`,
    serial,
    name: `Профиль ${serial}`,
    group: group === null ? null : { id: group, name: `Группа ${group}`, color: '#2447c9' },
    seed: serial,
    status: status(state),
    created_at: serial,
  })

describe('цвет новой группы', () => {
  it('первый свободный цвет палитры без учёта регистра', () => {
    expect(nextGroupColor([])).toBe('#0A6B66')
    expect(nextGroupColor([{ color: '#0a6b66' }])).toBe('#B02A6B')
    expect(nextGroupColor([{ color: '#123456' }, { color: '#B02A6B' }, { color: '#0a6b66' }])).toBe('#6B46C1')
  })

  it('все цвета заняты — первый', () => {
    expect(nextGroupColor(GROUP_PALETTE.map((color) => ({ color: color.toLowerCase() })))).toBe('#0A6B66')
  })
})

describe('сводка группы', () => {
  it('считает статусы и строит сводку без остановленных', () => {
    const rows = [row(1, 'running'), row(2, 'running'), row(3, 'running'), row(4, 'crashed'), row(5, 'stopped')]
    const counts = countStatuses(rows)
    expect(counts).toEqual({ stopped: 1, starting: 0, waiting: 0, running: 3, paused: 0, crashed: 1, total: 5 })
    expect(summaryParts(counts)).toEqual([
      { state: 'running', n: 3 },
      { state: 'crashed', n: 1 },
    ])
  })

  it('все остановлены и пустая группа — пустая сводка', () => {
    expect(summaryParts(countStatuses([row(1, 'stopped')]))).toEqual([])
    const empty = countStatuses([])
    expect(empty.total).toBe(0)
    expect(summaryParts(empty)).toEqual([])
  })

  it('порядок частей: работают, запускаются, на паузе, упали', () => {
    const rows = [row(1, 'crashed'), row(2, 'paused'), row(3, 'starting'), row(4, 'running')]
    expect(summaryParts(countStatuses(rows)).map((part) => part.state)).toEqual(['running', 'starting', 'paused', 'crashed'])
  })
})

describe('профили в боковой панели группы', () => {
  it('сначала упавшие, на паузе, запускающиеся, работающие; внутри статуса — по номеру', () => {
    const rows = [
      row(1, 'running'),
      row(2, 'stopped'),
      row(3, 'crashed'),
      row(4, 'starting'),
      row(5, 'paused'),
      row(6, 'running'),
      row(7, 'stopped'),
    ]
    const { shown, rest } = panelProfiles(rows)
    expect(shown.map((r) => r.serial)).toEqual([3, 5, 4, 1, 6])
    expect(rest.map((r) => r.serial)).toEqual([2, 7])
    expect(rows.map((r) => r.serial)).toEqual([1, 2, 3, 4, 5, 6, 7])
  })
})

describe('«Ожидает запуска» в панели группы', () => {
  it('идёт между «Запускается» и «Работает»', () => {
    const rows = [row(1, 'running'), row(2, 'waiting'), row(3, 'starting')]
    expect(panelProfiles(rows).shown.map((r) => r.serial)).toEqual([3, 2, 1])
  })
})

describe('extSetCounts', () => {
  it('наборы профилей: сначала у большинства, при равенстве — по id', () => {
    const rows = [3, 0, 3, 5, 0, 3].map((id) => profileRow({ ext_category_id: id }))
    expect(extSetCounts(rows)).toEqual([
      { id: 3, n: 3 },
      { id: 0, n: 2 },
      { id: 5, n: 1 },
    ])
    expect(extSetCounts([])).toEqual([])
  })
})
