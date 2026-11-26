import { describe, expect, it } from 'vitest'
import en from './en.json'
import ru from './ru.json'
import { ruPlural } from './plural'

function leaves(value: unknown, prefix = ''): [string, unknown][] {
  if (typeof value !== 'object' || value === null) return [[prefix, value]]
  return Object.entries(value).flatMap(([key, child]) => leaves(child, prefix ? `${prefix}.${key}` : key))
}

/** Имена подстановок `{name}` и номера `{0}` во всех формах строки; литералы вроде `{'@'}` — не подстановки. */
function placeholders(text: string): string[] {
  const names = [...text.matchAll(/\{\s*([A-Za-z_]\w*|\d+)\s*\}/g)].map((match) => match[1])
  return [...new Set(names)].sort()
}

describe('словари', () => {
  it('содержат одни и те же ключи по-русски и по-английски', () => {
    const keys = (dictionary: unknown) => leaves(dictionary).map(([key]) => key).sort()
    expect(keys(en)).toEqual(keys(ru))
  })

  it('не содержат пустых строк', () => {
    for (const [key, value] of [...leaves(ru), ...leaves(en)]) {
      expect(typeof value === 'string' && value.trim() !== '', key).toBe(true)
    }
  })

  it('выбирают русскую форму множественного числа', () => {
    expect([0, 1, 2, 5, 11, 21, 22, 25, 111].map((n) => ruPlural(n, 4))).toEqual([0, 1, 2, 3, 3, 1, 2, 3, 3])
  })

  it('подстановки находятся, литералы пропускаются', () => {
    expect(placeholders("Строка {line}: {reason}, снова {line}; {'@'} и { n } | {0}")).toEqual([
      '0',
      'line',
      'n',
      'reason',
    ])
  })

  it('у каждого ключа одинаковые подстановки {…} по-русски и по-английски', () => {
    const english = new Map(leaves(en))
    const differ = leaves(ru)
      .filter(([key, value]) => {
        const other = english.get(key)
        return (
          typeof value === 'string' &&
          typeof other === 'string' &&
          placeholders(value).join() !== placeholders(other).join()
        )
      })
      .map(([key]) => key)
    expect(differ).toEqual([])
  })
})
