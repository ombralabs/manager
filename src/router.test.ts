import { describe, expect, it } from 'vitest'
import type { RouteLocationNormalizedLoaded } from 'vue-router'
import { viewKey } from './router'

const at = (path: string, hash = '') => ({ path, hash, fullPath: path + hash }) as RouteLocationNormalizedLoaded

describe('viewKey', () => {
  it('якоря «Настроек» не меняют ключ', () => {
    expect(viewKey(at('/settings', '#a'))).toBe(viewKey(at('/settings', '#b')))
  })
  it('остальные экраны — по полному адресу', () => {
    expect(viewKey(at('/profiles', '#a'))).toBe('/profiles#a')
    expect(viewKey(at('/automation/flows/1'))).toBe('flow-editor')
  })
  it('каталог сохраняет состояние между поиском и карточкой сценария', () => {
    expect(viewKey(at('/automation/library'))).toBe(viewKey(at('/automation/library/cian')))
    expect(viewKey(at('/automation/library', '?q=csv'))).toBe(viewKey(at('/automation/library')))
  })
})
