import { describe, expect, it } from 'vitest'
import type { ProfileRow } from '../api/types'
import { profileLabel, profileRow } from '../test/fixtures'
import { defaultProfileCriteria, filterProfiles } from './profileFilter'

const group = { id: 5, name: 'Shop', color: '#2447c9' }

describe('фильтр профилей задачи', () => {
  it('соединяет группу, все теги, статус и поиск через И', () => {
    const match = profileRow({ id: 'yes', name: 'Ёжик', group, tags: ['VIP', 'Shop'], label: profileLabel() })
    const rows = [
      match,
      { ...match, id: 'group', group: null },
      { ...match, id: 'tags', tags: ['vip'] },
      { ...match, id: 'status', label: null },
      { ...match, id: 'search', name: 'Другой' },
      { ...match, id: 'trash', deleted_at: 1 },
    ]
    expect(filterProfiles(rows, { group: 5, tags: [' vip ', 'shop'], label: 2, search: ' ЕЖИК ' }).map((r) => r.id))
      .toEqual(['yes'])
  })

  it('ручной статус не зависит от браузера; «Без статуса» отличается от «Любой»', () => {
    const rows = [profileRow({ id: 'stopped', label: profileLabel() }),
      profileRow({ id: 'starting', label: profileLabel(), status: { state: 'starting' } }),
      profileRow({ id: 'none' })]
    expect(filterProfiles(rows, { ...defaultProfileCriteria(), label: 2 }).map((r) => r.id)).toEqual(['stopped', 'starting'])
    expect(filterProfiles(rows, { ...defaultProfileCriteria(), label: 'none' }).map((r) => r.id)).toEqual(['none'])
    expect(filterProfiles(rows, defaultProfileCriteria())).toHaveLength(3)
  })

  it('без условий выбирает все живые профили по номеру, не меняя исходный список', () => {
    const rows = [
      profileRow({ id: 'b', serial: 20 }),
      profileRow({ id: 'trash', serial: 1, deleted_at: 1 }),
      profileRow({ id: 'a', serial: 2 }),
    ]
    expect(filterProfiles(rows, defaultProfileCriteria()).map((r) => r.id)).toEqual(['a', 'b'])
    expect(rows.map((r) => r.id)).toEqual(['b', 'trash', 'a'])
  })

  it('отдельно выбирает профили без группы', () => {
    const rows = [profileRow({ id: 'group', group }), profileRow({ id: 'ungrouped' })]
    expect(filterProfiles(rows, { ...defaultProfileCriteria(), group: 'ungrouped' }).map((r) => r.id))
      .toEqual(['ungrouped'])
  })

  const searchable: ProfileRow = profileRow({
    serial: 67,
    name: 'Ёжик 50%',
    note: 'Нужен прогрев',
    gateway: { kind: 'http', host: 'gateway.example', port: 80, ip: '203.0.113.4', country: null, ok: true },
  })
  it.each([
    [' ЕЖИК ', true],
    ['ПРОГРЕВ', true],
    ['203.0.113', true],
    ['67', true],
    ['6', false],
    ['50%', true],
    ['_', false],
    ['неизвестный', false],
    ['   ', true],
  ])('поиск «%s» совпадает с правилами списка профилей', (search, matches) => {
    expect(filterProfiles([searchable], { ...defaultProfileCriteria(), search }).length).toBe(matches ? 1 : 0)
  })
})
