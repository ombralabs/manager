/** Порт Local API: ядро не принимает системные порты (`API_PORT_MIN`). */
export const API_PORT_MIN = 1024
export const PORT_MAX = 65535
/** Целое из поля ввода в границах `min..=max`, иначе `null`. Пробелы по краям допустимы. */
export function parseIntegerIn(text: string, min: number, max: number): number | null {
  const trimmed = text.trim()
  if (!/^\d+$/.test(trimmed)) return null
  const value = Number(trimmed)
  return value >= min && value <= max ? value : null
}

export function parsePort(text: string): number | null {
  return parseIntegerIn(text, API_PORT_MIN, PORT_MAX)
}

/** Local API слушает только 127.0.0.1. */
export function apiAddress(port: number): string {
  return `http://127.0.0.1:${port}`
}
