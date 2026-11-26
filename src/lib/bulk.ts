import type { BulkCreateForm, BulkEditForm, BulkReport, FieldError } from '../api/types'
import { maskedOwnerHint } from '../api/errors'
import { splitLines } from './profileForm'
import { parseIntegerIn } from './settings'

export const BULK_CREATE_MAX = 1000

/** Ширина номера: не меньше трёх знаков и не меньше, чем знаков в количестве (правило ядра 2А). */
export function numberWidth(count: number): number {
  return Math.max(3, String(count).length)
}

/** Первое и последнее имя: «Магазин 001 … Магазин 120». `count` уже проверен. */
export function previewNames(prefix: string, count: number): { first: string; last: string } | null {
  const name = prefix.trim()
  if (name === '') return null
  const width = numberWidth(count)
  const label = (n: number) => `${name} ${String(n).padStart(width, '0')}`
  return { first: label(1), last: label(count) }
}

/** Диалог «Создать несколько…»: строки полей ввода, как их держит форма. */
export interface BulkCreateDraft {
  count: string
  prefix: string
  group: number | null
  tags: string[]
  note: string
  startUrls: string
  restoreTabs: boolean
  extraArgs: string
  proxyMode: 'random' | 'fixed'
  proxyId: number | null
}

export function emptyCreateDraft(group: number | null = null): BulkCreateDraft {
  return {
    count: '10',
    prefix: '',
    group,
    tags: [],
    note: '',
    startUrls: '',
    restoreTabs: true,
    extraArgs: '',
    proxyMode: 'random',
    proxyId: null,
  }
}

/** Проверки, которым не нужно ядро; остальное (вкладки, аргументы, теги, длина префикса) проверит ядро. */
export function createDraftErrors(draft: BulkCreateDraft): FieldError[] {
  const errors: FieldError[] = []
  if (parseIntegerIn(draft.count, 1, BULK_CREATE_MAX) === null) errors.push({ field: 'count', code: 'out_of_range' })
  if (draft.prefix.trim() === '') errors.push({ field: 'prefix', code: 'required' })
  if (draft.proxyMode === 'fixed' && draft.proxyId === null) errors.push({ field: 'proxy_id', code: 'required' })
  return errors
}

export function toBulkCreateForm(draft: BulkCreateDraft): BulkCreateForm {
  return {
    count: parseIntegerIn(draft.count, 1, BULK_CREATE_MAX) ?? 0,
    prefix: draft.prefix.trim(),
    group_id: draft.group,
    tags: draft.tags,
    note: draft.note.trim(),
    start_urls: splitLines(draft.startUrls),
    restore_tabs: draft.restoreTabs,
    extra_args: splitLines(draft.extraArgs),
    proxy:
      draft.proxyMode === 'fixed' && draft.proxyId !== null ? { mode: 'fixed', proxy_id: draft.proxyId } : { mode: 'random' },
  }
}

export const EDIT_FIELDS = ['addTags', 'removeTags', 'note', 'proxy', 'startUrls', 'extraArgs', 'restoreTabs'] as const
export type EditField = (typeof EDIT_FIELDS)[number]

/** Диалог «Изменить…»: каждое поле включается своим флажком, невключённые не меняются. */
export interface BulkEditDraft {
  enabled: Record<EditField, boolean>
  addTags: string[]
  removeTags: string[]
  note: string
  proxyAction: 'reset' | 'set'
  proxyId: number | null
  startUrls: string
  extraArgs: string
  restoreTabs: boolean
}

export function emptyEditDraft(): BulkEditDraft {
  return {
    enabled: {
      addTags: false,
      removeTags: false,
      note: false,
      proxy: false,
      startUrls: false,
      extraArgs: false,
      restoreTabs: false,
    },
    addTags: [],
    removeTags: [],
    note: '',
    proxyAction: 'reset',
    proxyId: null,
    startUrls: '',
    extraArgs: '',
    restoreTabs: true,
  }
}

export function editDraftEmpty(draft: BulkEditDraft): boolean {
  return EDIT_FIELDS.every((field) => !draft.enabled[field])
}

export function editDraftErrors(draft: BulkEditDraft): FieldError[] {
  if (draft.enabled.proxy && draft.proxyAction === 'set' && draft.proxyId === null) {
    return [{ field: 'proxy_id', code: 'required' }]
  }
  return []
}

/** Форма ядра: только включённые поля. */
export function toBulkEditForm(draft: BulkEditDraft): BulkEditForm {
  const form: BulkEditForm = {}
  if (draft.enabled.addTags) form.add_tags = draft.addTags
  if (draft.enabled.removeTags) form.remove_tags = draft.removeTags
  if (draft.enabled.note) form.note = draft.note.trim()
  if (draft.enabled.proxy) {
    form.proxy =
      draft.proxyAction === 'set' && draft.proxyId !== null ? { action: 'set', proxy_id: draft.proxyId } : { action: 'reset' }
  }
  if (draft.enabled.startUrls) form.start_urls = splitLines(draft.startUrls)
  if (draft.enabled.extraArgs) form.extra_args = splitLines(draft.extraArgs)
  if (draft.enabled.restoreTabs) form.restore_tabs = draft.restoreTabs
  return form
}

/** Строка отчёта: профиль и код причины (перевод — `errors.<код>`). */
export interface ReportLine {
  id: string
  name: string
  code: string
  ownerHint?: string
}

export function reportLines(report: BulkReport, nameOf: (id: string) => string): ReportLine[] {
  return report.failed.map((failure) => {
    const ownerHint = failure.error.code === 'FreeDeviceBound' ? maskedOwnerHint(failure.error.ownerHint) : undefined
    return { id: failure.id, name: nameOf(failure.id), code: failure.error.code, ...(ownerHint ? { ownerHint } : {}) }
  })
}
