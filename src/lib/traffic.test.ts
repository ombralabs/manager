import { describe, expect, it } from 'vitest'
import { formatBytes, formatDuration, formatWhen, resultText } from './traffic'

describe('форматирование трафика', () => {
  it('байты по 1024, одна цифра после запятой', () => {
    expect(formatBytes(512)).toBe('512 Б')
    expect(formatBytes(1536)).toBe('1,5 КБ')
    expect(formatBytes(5 * 1024 * 1024)).toBe('5,0 МБ')
    expect(formatBytes(3.25 * 1024 ** 3)).toBe('3,3 ГБ')
  })

  it('длительность', () => {
    expect(formatDuration(850)).toBe('850 мс')
    expect(formatDuration(12_300)).toBe('12,3 с')
    expect(formatDuration(240_000)).toBe('4 мин')
  })

  it('границы единиц — по округлённому значению', () => {
    expect(formatDuration(59_950)).toBe('1 мин')
    expect(formatDuration(59_940)).toBe('59,9 с')
    expect(formatBytes(1024 * 1024 - 1)).toBe('1,0 МБ')
    expect(formatBytes(1000)).toMatch(/Б$/)
  })

  it('итог соединения по коду в начале причины', () => {
    expect(resultText('ok')).toBe('успешно')
    expect(resultText('400: неверный заголовок')).toBe('неверный запрос')
    expect(resultText('502: connection refused')).toBe('шлюз не ответил')
    expect(resultText('Connection reset by peer (os error 54)')).toBe('ошибка соединения')
  })

  it('время: сегодня — только время, раньше — с датой', () => {
    const now = new Date(2026, 9, 6, 15, 0).getTime()
    expect(formatWhen(new Date(2026, 9, 6, 14, 5).getTime(), now)).not.toMatch(/2026|06\.10/)
    expect(formatWhen(new Date(2026, 9, 5, 14, 5).getTime(), now)).toMatch(/05\.10/)
  })
})
