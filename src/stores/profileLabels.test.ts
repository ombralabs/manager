import { describe, expect, it, vi } from 'vitest'
import { profileRow } from '../test/fixtures'

const api = vi.hoisted(() => ({ listProfileLabels: vi.fn(), setProfileLabel: vi.fn(), saveProfileLabel: vi.fn(), deleteProfileLabel: vi.fn() }))
vi.mock('../api/core', () => ({ core: api, events: {} }))

import { useProfileLabelsStore } from './profileLabels'
import { useProfilesStore } from './profiles'
import { useGroupsStore } from './groups'
import type { ProfileLabel } from '../api/types'

const label: ProfileLabel = { id: 4, name: 'Проверить', color: '#2447c9', preset: null }

describe('ручные статусы профиля', () => {
  it('ошибка сохранения оставляет прежнюю метку и состояние браузера', async () => {
    const profiles = useProfilesStore()
    profiles.rows = [profileRow({ label })]
    useGroupsStore().rows = [profileRow({ label })]
    api.setProfileLabel.mockRejectedValueOnce(new Error('offline'))
    await expect(useProfileLabelsStore().assign('p1', null)).rejects.toThrow('offline')
    expect(profiles.rows[0]!.label).toEqual(label)
    expect(profiles.rows[0]!.status).toEqual({ state: 'stopped' })
    api.setProfileLabel.mockResolvedValueOnce(null)
    await useProfileLabelsStore().assign('p1', null)
    expect(profiles.rows[0]!.label).toBeNull()
    expect(useGroupsStore().rows[0]!.label).toBeNull()
    expect(profiles.rows[0]!.status).toEqual({ state: 'stopped' })
  })

  it('изменение и удаление общей метки обновляет все профили; старый ответ списка не откатывает правку', async () => {
    const labels = useProfileLabelsStore()
    const profiles = useProfilesStore()
    profiles.rows = [profileRow({ label }), profileRow({ id: 'p2', label })]
    useGroupsStore().rows = [profileRow({ label })]
    labels.labels = [label]
    let resolve!: (list: ProfileLabel[]) => void
    api.listProfileLabels.mockReturnValueOnce(new Promise<ProfileLabel[]>((done) => { resolve = done }))
    const loading = labels.refresh()
    const renamed = { ...label, name: 'Готов' }
    api.saveProfileLabel.mockResolvedValueOnce(renamed)
    await labels.save(label.id, renamed.name, renamed.color)
    resolve([label])
    await loading
    expect(labels.labels).toEqual([renamed])
    expect(profiles.rows.every((row) => row.label?.name === 'Готов')).toBe(true)
    expect(useGroupsStore().rows[0]!.label?.name).toBe('Готов')
    api.deleteProfileLabel.mockResolvedValueOnce(null)
    await labels.remove(label.id)
    expect(profiles.rows.every((row) => row.label === null)).toBe(true)
    expect(useGroupsStore().rows[0]!.label).toBeNull()
  })
})
