import { describe, expect, it } from 'vitest'
import { flowTemplates, templateCategories } from './flowTemplates'

function executable(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(executable)
  if (value && typeof value === 'object') return Object.fromEntries(Object.entries(value)
    .filter(([key]) => !['description', 'remark', 'group'].includes(key))
    .map(([key, child]) => [key, executable(key === 'config' && 'op' in value && value.op === 'control.group'
      ? { ...child, name: '' } : child)]))
  return value
}

describe('bundled templates', () => {
  it('contains distinct templates with the same executable content in both languages', () => {
    expect(flowTemplates.map(template => template.id)).toEqual(['google-search'])
    for (const template of flowTemplates) {
      expect(templateCategories).toContain(template.category)
      expect(template.summary.ru.length).toBeGreaterThan(20)
      expect(template.summary.en).not.toMatch(/[А-Яа-яЁё]/)
      expect(template.instructions.ru.length).toBeGreaterThan(2)
      expect(template.instructions.en).toHaveLength(template.instructions.ru.length)
      expect(template.instructions.en.join(' ')).not.toMatch(/[А-Яа-яЁё]/)
      const ru = JSON.parse(template.sources.ru)
      const en = JSON.parse(template.sources.en)
      expect(ru.format).toBe('ombra-flow/1')
      expect(en.name).not.toMatch(/[А-Яа-яЁё]/)
      expect(en.remark).not.toMatch(/[А-Яа-яЁё]/)
      expect(template.sources.en).not.toMatch(/"name":\s*"[^"]*[А-Яа-яЁё]/)
      expect(executable({ ...en, name: '' })).toEqual(executable({ ...ru, name: '' }))
      for (const parameter of en.variables) expect(parameter.description).not.toMatch(/[А-Яа-яЁё]/)
    }
  })
})
