import type { ExtCategory, ExtensionRecord } from '../api/types'
import { i18n } from '../i18n'
import { GROUP_PALETTE } from './groups'

/** Итог добавления: ядро возвращает существующую запись, если та же версия уже в библиотеке. */
export type AddOutcome = 'added' | 'already' | 'updated'

export function addOutcome(before: ExtensionRecord[], record: ExtensionRecord): AddOutcome {
  const previous = before.find((ext) => ext.id === record.id)
  if (!previous) return 'added'
  return previous.version === record.version ? 'already' : 'updated'
}

/** Состав набора после переключателя: порядок — порядок библиотеки, id не из библиотеки не попадают. */
export function toggledItems(library: ExtensionRecord[], current: string[], id: string, on: boolean): string[] {
  const next = new Set(current)
  if (on) next.add(id)
  else next.delete(id)
  return library.filter((ext) => next.has(ext.id)).map((ext) => ext.id)
}

/** Название набора: набор 0 — «Без набора» по языку интерфейса (его получают профили без своего набора). */
export function setLabel(set: { id: number; name: string }): string {
  return set.id === 0 ? i18n.global.t('extensions.noSet') : set.name
}

const EXTENSION_ID = /^[a-p]{32}$/
const STORE_PREFIXES = ['https://chromewebstore.google.com/detail/', 'https://chrome.google.com/webstore/detail/']

/** ID из ввода — как `store_id` ядра: сам ID или ссылка Chrome Web Store (новая и старая); иначе `null`. */
export function storeId(input: string): string | null {
  const text = input.trim()
  if (EXTENSION_ID.test(text)) return text
  const url = text.split(/[?#]/)[0] ?? ''
  const prefix = STORE_PREFIXES.find((start) => url.startsWith(start))
  if (prefix === undefined) return null
  return url.slice(prefix.length).split('/').find((part) => EXTENSION_ID.test(part)) ?? null
}

/** Цвет плитки: палитра групп по хешу FNV-1a от `id` — у одного расширения всегда один. */
export function tileColor(id: string): string {
  let hash = 2166136261
  for (const char of id) hash = Math.imul(hash ^ char.charCodeAt(0), 16777619) >>> 0
  return GROUP_PALETTE[hash % GROUP_PALETTE.length]!
}

/** Буква плитки: первый символ имени без пробелов по краям (эмодзи — целиком); пустое имя — «?». */
export function tileLetter(name: string): string {
  // Графема, а не кодовая точка: ZWJ-эмодзи, флаги и тон кожи не рвутся.
  return new Intl.Segmenter(undefined, { granularity: 'grapheme' }).segment(name.trim())[Symbol.iterator]().next().value?.segment ?? '?'
}

export type SourceKind = 'store' | 'file' | 'folder'

/** Откуда расширение: магазин; файл — `.zip`/`.crx` (других ядро не принимает) или имени нет; папка — остальное. */
export function sourceKind(ext: Pick<ExtensionRecord, 'source' | 'file_name'>): SourceKind {
  if (ext.source === 'store') return 'store'
  return ext.file_name !== null && !/\.(zip|crx)$/i.test(ext.file_name) ? 'folder' : 'file'
}

/** Подпись источника: «Chrome Web Store», «файл X.zip», «папка X», «файл» без имени. */
export function sourceLabel(ext: Pick<ExtensionRecord, 'source' | 'file_name'>): string {
  const { t } = i18n.global
  const kind = sourceKind(ext)
  if (kind === 'store') return t('extensions.card.store')
  if (kind === 'folder') return t('extensions.card.folder', { name: ext.file_name })
  return ext.file_name === null ? t('extensions.card.fileUnknown') : t('extensions.card.file', { name: ext.file_name })
}

/** Наборы с расширением, кроме набора 0 (его роль — «включено»): названия в порядке списка. */
export function setsWith(categories: ExtCategory[], id: string): string[] {
  return categories.filter((set) => set.id !== 0 && set.extension_ids.includes(id)).map((set) => set.name)
}

/** Последний сегмент пути: имя файла или папки. */
export function baseName(path: string): string {
  return path.split('/').filter(Boolean).pop() ?? path
}
