import { afterEach, describe, expect, it } from 'vitest'
import type { CoreErrorPayload } from '../api/types'
import { i18n } from '../i18n'
import FatalScreen from './FatalScreen.vue'
import { mountWithApp } from '../test/mount'

const ERROR: CoreErrorPayload = { code: 'Io', message: 'папка данных заблокирована: Permission denied (os error 13)' }

function mountScreen(error: CoreErrorPayload = ERROR) {
  return mountWithApp(FatalScreen, {
    props: { error, dataDir: '/Users/me/Library/Application Support/Ombra' },
  })
}

describe('экран «Не удалось открыть данные Ombra»', () => {
  afterEach(() => {
    i18n.global.locale.value = 'ru'
  })

  it('по-русски — перевод кода и подробность ядра', () => {
    const text = mountScreen().text()
    expect(text).toContain('Ошибка файловой системы')
    expect(text).toContain('папка данных заблокирована')
  })

  it('по-английски — перевод кода без русской подробности; неизвестный код — «Unknown error»', async () => {
    i18n.global.locale.value = 'en'
    const wrapper = mountScreen()
    expect(wrapper.text()).toContain('File system error')
    expect(wrapper.text()).not.toMatch(/[А-Яа-яЁё]/)
    await wrapper.setProps({ error: { code: 'Weird', message: 'странное' } })
    expect(wrapper.text()).toContain('Unknown error')
    expect(wrapper.text()).not.toContain('странное')
  })
})
