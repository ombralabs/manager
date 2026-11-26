import { describe, expect, it } from 'vitest'
import { archiveFileName, cookieExtension, exportFileName } from './cookieExport'

describe('имя файла экспорта cookies', () => {
  it('расширение по формату', () => {
    expect(cookieExtension('json')).toBe('json')
    expect(cookieExtension('netscape')).toBe('txt')
  })

  it('заменяет разделители путей и управляющие символы', () => {
    expect(exportFileName('a/b:c\\d', 'json')).toBe('a_b_c_d.json')
    expect(exportFileName('строка\nвторая\t', 'netscape')).toBe('строка_вторая_.txt')
    expect(exportFileName('../x', 'json')).toBe('.._x.json')
  })

  it('пустое имя, из одних пробелов, «.» и «..» — «_»', () => {
    expect(exportFileName('', 'json')).toBe('_.json')
    expect(exportFileName('   ', 'json')).toBe('_.json')
    expect(exportFileName('.', 'json')).toBe('_.json')
    expect(exportFileName('..', 'netscape')).toBe('_.txt')
  })

  it('обрезает имя до 100 символов и 200 байт, как ядро', () => {
    expect(exportFileName('я'.repeat(150), 'json')).toBe(`${'я'.repeat(100)}.json`)
    expect(exportFileName('😀'.repeat(101), 'json')).toBe(`${'😀'.repeat(50)}.json`)
  })

  it('имя архива профиля — по тем же правилам, с .tar.gz', () => {
    expect(archiveFileName('Shop: 1/2')).toBe('Shop_ 1_2.tar.gz')
    expect(archiveFileName('  ')).toBe('_.tar.gz')
  })
})
