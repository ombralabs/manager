import { getCurrentWebview } from '@tauri-apps/api/webview'
import { open, save } from '@tauri-apps/plugin-dialog'
import { cookieExtension } from '../lib/cookieExport'
import type { CookieFormat } from './types'

/** Окно сохранения одного файла экспорта; `null` — пользователь отменил. */
export async function pickSaveFile(defaultName: string, format: CookieFormat): Promise<string | null> {
  const extension = cookieExtension(format)
  return save({ defaultPath: defaultName, filters: [{ name: extension.toUpperCase(), extensions: [extension] }] })
}

/** Окно выбора папки для экспорта нескольких профилей; `null` — пользователь отменил. */
export async function pickFolder(): Promise<string | null> {
  const picked = await open({ directory: true, multiple: false })
  return typeof picked === 'string' ? picked : null
}

/** Окно выбора пакета расширения (`.crx` или `.zip`); `null` — пользователь отменил. */
export async function pickExtensionFile(): Promise<string | null> {
  const picked = await open({ multiple: false, directory: false, filters: [{ name: 'Chrome', extensions: ['crx', 'zip'] }] })
  return typeof picked === 'string' ? picked : null
}

/** Окно выбора файла cookies для импорта (JSON AdsPower или cookies.txt); `null` — пользователь отменил. */
export async function pickCookieFile(): Promise<string | null> {
  const picked = await open({ multiple: false, directory: false, filters: [{ name: 'Cookies', extensions: ['json', 'txt'] }] })
  return typeof picked === 'string' ? picked : null
}

/** Архив движка для текущей платформы; `null` — пользователь отменил. */
export async function pickEngineArchive(): Promise<string | null> {
  const picked = await open({ multiple: false, directory: false, filters: [{ name: 'Ombra engine', extensions: ['gz', 'tgz', 'xz', 'zip'] }] })
  return typeof picked === 'string' ? picked : null
}

/** Окно выбора таблицы профилей (`.csv`); `null` — пользователь отменил. */
export async function pickCsvFile(): Promise<string | null> {
  const picked = await open({ multiple: false, directory: false, filters: [{ name: 'CSV', extensions: ['csv'] }] })
  return typeof picked === 'string' ? picked : null
}

/** Окно выбора архива профиля (`.tar.gz`); `null` — пользователь отменил. */
export async function pickArchiveFile(): Promise<string | null> {
  const picked = await open({ multiple: false, directory: false, filters: [{ name: 'tar.gz', extensions: ['gz', 'tgz'] }] })
  return typeof picked === 'string' ? picked : null
}

/** Окно сохранения таблицы профилей; `null` — пользователь отменил. */
export async function pickSaveCsv(defaultName: string): Promise<string | null> {
  return save({ defaultPath: defaultName, filters: [{ name: 'CSV', extensions: ['csv'] }] })
}

/** Окно сохранения архива профиля; `null` — пользователь отменил. */
export async function pickSaveArchive(defaultName: string): Promise<string | null> {
  return save({ defaultPath: defaultName, filters: [{ name: 'tar.gz', extensions: ['gz'] }] })
}

export type FileDrop = { type: 'enter' | 'over' | 'leave' } | { type: 'drop'; paths: string[] }

/** Файлы, перетащенные на окно: Tauri отдаёт пути (HTML-событие `drop` их не знает). Результат — отписка. */
export function onFileDrop(handler: (event: FileDrop) => void): Promise<() => void> {
  return getCurrentWebview().onDragDropEvent(({ payload }) => {
    handler(payload.type === 'drop' ? { type: 'drop', paths: payload.paths } : { type: payload.type })
  })
}

export async function pickFlowFile(): Promise<string | null> {
  const picked = await open({ multiple: false, directory: false, filters: [{ name: 'JSON', extensions: ['json'] }] })
  return typeof picked === 'string' ? picked : null
}

export async function pickGoogleKeyFile(): Promise<string | null> {
  const picked = await open({ multiple: false, directory: false, filters: [{ name: 'JSON', extensions: ['json'] }] })
  return typeof picked === 'string' ? picked : null
}

export async function pickSaveFlow(defaultName: string): Promise<string | null> {
  return save({ defaultPath: defaultName, filters: [{ name: 'JSON', extensions: ['json'] }] })
}

export async function pickTableFile(): Promise<string | null> {
  const picked = await open({ multiple: false, directory: false, filters: [{ name: 'CSV, XLSX, JSON', extensions: ['csv', 'xlsx', 'json'] }] })
  return typeof picked === 'string' ? picked : null
}
