import type { VarDecl } from '../api/types'

/** Значение по умолчанию для поля формы — как приводит `vars::coerce` ядра: булево из `true`/`"true"`, число из числа или строки. */
export function formDefault(decl: VarDecl): string | boolean {
  const d = decl.default
  if (decl.type === 'boolean') return d === true || (typeof d === 'string' && d.trim() === 'true')
  if (d === null || d === undefined) return ''
  if (decl.type === 'number') {
    const n = typeof d === 'number' ? d : typeof d === 'string' && d.trim() !== '' ? Number(d) : NaN
    return Number.isNaN(n) ? '' : String(n)
  }
  return typeof d === 'string' ? d : JSON.stringify(d)
}

/** Число из текста формы: только десятичная запись; `0x10`, `1e3`, `Infinity` — не число (ядро их отвергает при сохранении). */
export function parseStrictNumber(text: string): number | null {
  const t = text.trim()
  return /^-?\d+(\.\d+)?$/.test(t) ? Number(t) : null
}
