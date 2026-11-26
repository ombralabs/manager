import { describe, expect, it } from 'vitest'
import { i18n } from '../i18n'
import { searchSettings } from './settingsNavigation'

describe('поиск настроек', () => {
  it.each([
    ['ru', 'движок', 'engine-heading'],
    ['ru', 'шлюз', 'services-proxy'], ['ru', 'language', 'ui-language'],
    ['ru', 'стабильные выпуски', 'engine-automatic'], ['en', 'rollback', 'engine-versions'],
    ['ru', 'занятый', 'profile-proxy-exclusive'], ['ru', 'При обрыве подключения', 'profile-proxy-rules'],
    ['en', 'same IP', 'profile-proxy-exclusive'], ['en', 'used by another profile', 'profile-proxy-exclusive'],
    ['en', 'recovery', 'local-security'],
  ] as const)('%s: %s находит %s', (locale, query, target) => {
    const translate = (key: string) => i18n.global.t(key, {}, { locale })
    expect(searchSettings(query, translate).map((entry) => entry.target)).toContain(target)
  })
  it('пробелы не возвращают все параметры', () => { expect(searchSettings('  ', (key) => key)).toEqual([]) })
})
