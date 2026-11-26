import { afterEach, describe, expect, it } from 'vitest'
import { i18n } from '../i18n'
import { extCategory, extension, riskyExtension } from '../test/fixtures'
import {
  addOutcome,
  baseName,
  setLabel,
  setsWith,
  sourceKind,
  sourceLabel,
  storeId,
  tileColor,
  tileLetter,
  toggledItems,
} from './extensions'
import { GROUP_PALETTE } from './groups'

describe('помощники расширений', () => {
  it('итог добавления по списку до него', () => {
    const before = [extension()]
    expect(addOutcome([], extension())).toBe('added')
    expect(addOutcome(before, extension())).toBe('already')
    expect(addOutcome(before, extension({ version: '1.63.0' }))).toBe('updated')
  })

  it('новый состав набора — в порядке библиотеки, без неизвестных id', () => {
    const library = [extension(), riskyExtension]
    expect(toggledItems(library, [riskyExtension.id], extension().id, true)).toEqual([extension().id, riskyExtension.id])
    expect(toggledItems(library, [riskyExtension.id, 'gone'], riskyExtension.id, false)).toEqual([])
  })
})

describe('название набора', () => {
  afterEach(() => {
    i18n.global.locale.value = 'ru'
  })

  it('набор 0 — «Без набора» по языку, даже переименованный; остальные — как есть', () => {
    expect(setLabel({ id: 0, name: 'Общий' })).toBe('Без набора')
    expect(setLabel({ id: 0, name: 'Мой' })).toBe('Без набора')
    i18n.global.locale.value = 'en'
    expect(setLabel({ id: 0, name: 'Общий' })).toBe('No set')
    expect(setLabel({ id: 1, name: 'Общий' })).toBe('Общий')
  })
})

describe('ссылка Chrome Web Store', () => {
  const id = 'cjpalhdlnbpafiamejdnhcphjbkeiagm'

  it('как store_id ядра: ID, новая и старая ссылки с ?/#, пробелы по краям; остальное — null', () => {
    expect(storeId(`  ${id} `)).toBe(id)
    expect(storeId(`https://chromewebstore.google.com/detail/ublock-origin/${id}?hl=ru#reviews`)).toBe(id)
    expect(storeId(`https://chrome.google.com/webstore/detail/ublock-origin/${id}`)).toBe(id)
    expect(storeId(`https://chromewebstore.google.com/detail/${id}`)).toBe(id)
    for (const bad of [
      '',
      'ublock',
      id.toUpperCase(),
      `${id}a`,
      `http://chromewebstore.google.com/detail/x/${id}`,
      `https://example.com/detail/${id}`,
      'https://chromewebstore.google.com/detail/ublock',
    ]) {
      expect(storeId(bad), bad).toBeNull()
    }
  })
})

describe('плитка и источник', () => {
  it('цвет плитки — из палитры групп и один для одного id; разные id — разные цвета; буква — первый символ имени', () => {
    const ids = [extension().id, riskyExtension.id, 'nngceckbapebfimnlniiiahkandclblb', 'nkbihfbeogaeaoehlefnkodbefgpgknn']
    for (const id of ids) {
      expect(GROUP_PALETTE).toContain(tileColor(id))
      expect(tileColor(id)).toBe(tileColor(id))
    }
    expect(new Set(ids.map(tileColor)).size).toBeGreaterThan(1)
    expect(tileLetter('uBlock Origin')).toBe('u')
    expect(tileLetter('  🦊 MetaMask')).toBe('🦊')
    expect(tileLetter('   ')).toBe('?')
    expect(tileLetter('👨‍👩‍👧 Семья')).toBe('👨‍👩‍👧')
    expect(tileLetter('🇷🇺 Россия')).toBe('🇷🇺')
  })

  it('источник: магазин, файл .zip/.crx и без имени, папка — с подписью; наборы — без набора 0; имя из пути', () => {
    expect([sourceKind(extension()), sourceLabel(extension())]).toEqual(['store', 'Chrome Web Store'])
    expect(sourceLabel(extension({ source: 'file', file_name: 'team.ZIP' }))).toBe('файл team.ZIP')
    expect([sourceKind(extension({ source: 'file', file_name: null })), sourceLabel(extension({ source: 'file', file_name: null }))]).toEqual(['file', 'файл'])
    expect([sourceKind(extension({ source: 'file', file_name: 'my-ext' })), sourceLabel(extension({ source: 'file', file_name: 'my-ext' }))]).toEqual(['folder', 'папка my-ext'])
    const categories = [
      extCategory({ extension_ids: [riskyExtension.id] }),
      extCategory({ id: 2, name: 'Крипто', extension_ids: [riskyExtension.id] }),
      extCategory({ id: 3, name: 'Реклама', extension_ids: [] }),
    ]
    expect(setsWith(categories, riskyExtension.id)).toEqual(['Крипто'])
    expect(setsWith(categories, extension().id)).toEqual([])
    expect(baseName('/Users/demo/Downloads/team-autologin.zip')).toBe('team-autologin.zip')
    expect(baseName('/Users/demo/my-ext/')).toBe('my-ext')
  })
})
