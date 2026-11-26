import type { ProfileLabelFilter, ProfileRow } from '../api/types'
import type { GroupFilter } from '../stores/profiles'
import { fold, sameTag } from './tags'
import { matchesProfileLabel } from './profileLabels'

export interface ProfileCriteria {
  group: GroupFilter
  tags: string[]
  label: ProfileLabelFilter
  search: string
}

export function defaultProfileCriteria(): ProfileCriteria {
  return { group: 'all', tags: [], label: 'any', search: '' }
}

/** Все условия соединяются через «И»; поиск повторяет правила списка профилей ядра. */
export function filterProfiles(rows: ProfileRow[], criteria: ProfileCriteria): ProfileRow[] {
  const search = fold(criteria.search)
  return rows.filter((row) => {
    if (row.deleted_at !== null) return false
    if (criteria.group === 'ungrouped' && row.group !== null) return false
    if (typeof criteria.group === 'number' && row.group?.id !== criteria.group) return false
    if (!matchesProfileLabel(row.label, criteria.label)) return false
    if (!criteria.tags.every((tag) => row.tags.some((name) => sameTag(name, tag)))) return false
    return search === ''
      || [row.name, row.note, row.gateway?.ip ?? ''].some((value) => fold(value).includes(search))
      || String(row.serial) === search
  }).sort((a, b) => a.serial - b.serial)
}
