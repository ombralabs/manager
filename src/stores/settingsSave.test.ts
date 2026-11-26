import { describe, expect, it } from 'vitest'
import { useSettingsSaveStore } from './settingsSave'

describe('обратная связь сохранения настроек', () => {
  it('ждёт все запросы; успех другого поля не скрывает ошибку, повтор снимает её', async () => {
    const saves = useSettingsSaveStore()
    expect(saves.status('integrations')).toBe('automatic')
    let finish!: () => void
    const pending = saves.run('integrations.api.port', () => new Promise<void>((resolve) => { finish = resolve }))
    expect(saves.status('integrations')).toBe('saving')
    await expect(saves.run('integrations.proxy', async () => { throw new Error('offline') })).rejects.toThrow('offline')
    expect(saves.status('integrations')).toBe('saving')
    finish()
    await pending
    expect(saves.status('integrations')).toBe('error')
    expect(saves.status('general')).toBe('automatic')
    await saves.run('integrations.proxy', async () => {})
    expect(saves.status('integrations')).toBe('saved')
  })
  it('ответ на прежний запрос не объявляет новый ввод сохранённым', async () => {
    const saves = useSettingsSaveStore()
    saves.edited('profiles.limit')
    let finish!: () => void
    const pending = saves.run('profiles.limit', () => new Promise<void>((resolve) => { finish = resolve }))
    saves.edited('profiles.limit')
    finish()
    await pending
    expect(saves.status('profiles')).toBe('unsaved')
    await saves.run('profiles.limit', async () => {})
    expect(saves.status('profiles')).toBe('saved')
  })
  it('ошибка локальной проверки тоже не даёт показать «Сохранено»', async () => {
    const saves = useSettingsSaveStore()
    await saves.run('profiles.limit', async () => {})
    saves.invalid('profiles.limit')
    expect(saves.status('profiles')).toBe('error')
    saves.reset('profiles.limit')
    expect(saves.status('profiles')).toBe('saved')
  })
})
