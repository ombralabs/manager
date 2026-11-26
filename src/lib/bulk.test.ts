import { describe, expect, it } from 'vitest'
import {
  BULK_CREATE_MAX,
  createDraftErrors,
  editDraftEmpty,
  editDraftErrors,
  emptyCreateDraft,
  emptyEditDraft,
  numberWidth,
  previewNames,
  reportLines,
  toBulkCreateForm,
  toBulkEditForm,
} from './bulk'
import { parseIntegerIn } from './settings'

describe('массовое создание', () => {
  it('ширина номера — не меньше трёх знаков', () => {
    expect([numberWidth(1), numberWidth(999), numberWidth(1000)]).toEqual([3, 3, 4])
  })

  it('предпросмотр имён', () => {
    expect(previewNames(' Магазин ', 120)).toEqual({ first: 'Магазин 001', last: 'Магазин 120' })
    expect(previewNames('Магазин', 1000)).toEqual({ first: 'Магазин 0001', last: 'Магазин 1000' })
    expect(previewNames('  ', 10)).toBeNull()
  })

  it('разбирает количество', () => {
    expect(parseIntegerIn(' 120 ', 1, BULK_CREATE_MAX)).toBe(120)
    expect(['0', '1001', '1e2', '', '-5'].map((text) => parseIntegerIn(text, 1, BULK_CREATE_MAX))).toEqual([
      null,
      null,
      null,
      null,
      null,
    ])
  })

  it('ошибки черновика до ядра', () => {
    expect(createDraftErrors(emptyCreateDraft())).toEqual([{ field: 'prefix', code: 'required' }])
    const draft = { ...emptyCreateDraft(), prefix: 'Магазин', count: 'много', proxyMode: 'fixed' as const }
    expect(createDraftErrors(draft)).toEqual([
      { field: 'count', code: 'out_of_range' },
      { field: 'proxy_id', code: 'required' },
    ])
  })

  it('черновик в форму ядра', () => {
    const draft = {
      ...emptyCreateDraft(5),
      count: '120',
      prefix: ' Магазин ',
      tags: ['shop'],
      note: ' заметка ',
      startUrls: 'https://a.example/\n\n https://b.example/ ',
      restoreTabs: false,
      extraArgs: '--disable-sync\n',
      proxyMode: 'fixed' as const,
      proxyId: 3,
    }
    expect(toBulkCreateForm(draft)).toEqual({
      count: 120,
      prefix: 'Магазин',
      group_id: 5,
      tags: ['shop'],
      note: 'заметка',
      start_urls: ['https://a.example/', 'https://b.example/'],
      restore_tabs: false,
      extra_args: ['--disable-sync'],
      proxy: { mode: 'fixed', proxy_id: 3 },
    })
    expect(toBulkCreateForm(emptyCreateDraft()).proxy).toEqual({ mode: 'random' })
    expect(emptyCreateDraft()).toMatchObject({ count: '10', restoreTabs: true, proxyMode: 'random', group: null })
  })
})

describe('массовая правка', () => {
  it('в форму попадают только включённые поля', () => {
    const draft = emptyEditDraft()
    expect(editDraftEmpty(draft)).toBe(true)
    expect(toBulkEditForm(draft)).toEqual({})
    draft.note = ' новая '
    draft.startUrls = 'https://a.example/'
    draft.enabled.note = true
    expect(editDraftEmpty(draft)).toBe(false)
    expect(toBulkEditForm(draft)).toEqual({ note: 'новая' })
  })

  it('все поля включены', () => {
    const draft = emptyEditDraft()
    for (const field of Object.keys(draft.enabled) as (keyof typeof draft.enabled)[]) draft.enabled[field] = true
    Object.assign(draft, {
      addTags: ['vip'],
      removeTags: ['old'],
      note: '',
      proxyAction: 'set',
      proxyId: 4,
      startUrls: 'https://a.example/',
      extraArgs: '--x=1\n--y',
      restoreTabs: false,
    })
    expect(toBulkEditForm(draft)).toEqual({
      add_tags: ['vip'],
      remove_tags: ['old'],
      note: '',
      proxy: { action: 'set', proxy_id: 4 },
      start_urls: ['https://a.example/'],
      extra_args: ['--x=1', '--y'],
      restore_tabs: false,
    })
    draft.proxyAction = 'reset'
    expect(toBulkEditForm(draft).proxy).toEqual({ action: 'reset' })
  })

  it('«Назначить прокси» без прокси — ошибка поля', () => {
    const draft = emptyEditDraft()
    draft.proxyAction = 'set'
    expect(editDraftErrors(draft)).toEqual([])
    draft.enabled.proxy = true
    expect(editDraftErrors(draft)).toEqual([{ field: 'proxy_id', code: 'required' }])
    draft.proxyId = 2
    expect(editDraftErrors(draft)).toEqual([])
  })
})

describe('отчёт массовой операции', () => {
  it('preserves only a masked first-account hint in failed bulk launches', () => {
    const report = { ok: [], failed: [
      { id: 'a', error: { code: 'FreeDeviceBound', message: '', ownerHint: 'iv***v@example.com' } },
      { id: 'b', error: { code: 'FreeDeviceBound', message: '', ownerHint: 'ivan@example.com' } },
    ] }
    expect(reportLines(report, (id) => id)[0]).toMatchObject({ ownerHint: 'iv***v@example.com' })
    expect(reportLines(report, (id) => id)[1]).not.toMatchObject({ ownerHint: 'ivan@example.com' })
  })
  it('строки «профиль — код причины»', () => {
    const report = {
      ok: ['a'],
      failed: [
        { id: 'b', error: { code: 'AlreadyRunning', message: 'лимит' } },
        { id: 'zz', error: { code: 'NotFound', message: 'нет' } },
      ],
    }
    const names: Record<string, string> = { b: 'Магазин 002' }
    expect(reportLines(report, (id) => names[id] ?? id)).toEqual([
      { id: 'b', name: 'Магазин 002', code: 'AlreadyRunning' },
      { id: 'zz', name: 'zz', code: 'NotFound' },
    ])
  })
})
