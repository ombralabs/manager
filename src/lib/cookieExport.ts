import type { CookieFormat } from '../api/types'

const NAME_CHARS = 100
/** APFS принимает имена до 255 байт; с расширением 200 байт имени укладываются. */
const NAME_BYTES = 200
const utf8 = new TextEncoder()

export function cookieExtension(format: CookieFormat): 'json' | 'txt' {
  return format === 'json' ? 'json' : 'txt'
}

/**
 * Безопасное имя файла без расширения для одного профиля — по правилу ядра (`export_file_name` без id, задача 2):
 * `/`, `:`, `\` и управляющие символы → `_`, имя не длиннее 100 символов и 200 байт UTF-8;
 * пустое, из одних пробелов, `.` и `..` → `_`.
 */
export function safeFileBase(name: string): string {
  const chars = [...name].map((char) => (/[/:\\\p{Cc}]/u.test(char) ? '_' : char)).slice(0, NAME_CHARS)
  while (utf8.encode(chars.join('')).length > NAME_BYTES) chars.pop()
  const base = chars.join('')
  return base.trim() === '' || base === '.' || base === '..' ? '_' : base
}

/** Имя файла экспорта cookies: безопасное имя + расширение формата. */
export function exportFileName(name: string, format: CookieFormat): string {
  return `${safeFileBase(name)}.${cookieExtension(format)}`
}

/** Имя архива профиля по умолчанию. */
export function archiveFileName(name: string): string {
  return `${safeFileBase(name)}.tar.gz`
}
