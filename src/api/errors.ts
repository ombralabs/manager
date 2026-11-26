import type { FieldError } from './types'

/** The server hides all but three local-part characters; never display a raw address. */
export function maskedOwnerHint(value: unknown): string | undefined {
  const pattern = /^[\p{L}\p{N}._+-]{0,2}\*{3}[\p{L}\p{N}._+-]?@[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?(?:\.[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?)+$/iu
  return typeof value === 'string' && value.trim() === value && new TextEncoder().encode(value).length <= 254 && pattern.test(value)
    ? value : undefined
}

/** Ошибка команды ядра: `code` — стабильная строка для перевода, `message` — по-русски, для подробностей. */
export class CoreError extends Error {
  readonly code: string
  readonly fields: FieldError[]
  readonly ownerHint?: string

  constructor(code: string, message: string, fields: FieldError[] = [], ownerHint?: string) {
    super(message)
    this.name = 'CoreError'
    this.code = code
    this.fields = fields
    this.ownerHint = code === 'FreeDeviceBound' ? maskedOwnerHint(ownerHint) : undefined
  }
}

/** Любая ошибка вызова — `CoreError`; строки `[object Object]` в тексте не бывает. */
export function toCoreError(error: unknown): CoreError {
  if (error instanceof CoreError) return error
  if (error instanceof Error) return new CoreError('Internal', error.message)
  if (typeof error === 'string') return new CoreError('Unknown', error)
  if (typeof error === 'object' && error !== null) {
    const payload = error as { code?: unknown; message?: unknown; fields?: unknown; ownerHint?: unknown }
    const message = typeof payload.message === 'string' ? payload.message : ''
    if (typeof payload.code !== 'string') return new CoreError('Unknown', message)
    return new CoreError(payload.code, message, Array.isArray(payload.fields) ? (payload.fields as FieldError[]) : [], maskedOwnerHint(payload.ownerHint))
  }
  return new CoreError('Unknown', '')
}
