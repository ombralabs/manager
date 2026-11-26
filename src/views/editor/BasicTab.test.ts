import { describe, expect, it, vi } from 'vitest'
import { mountWithApp } from '../../test/mount'

vi.mock('../../api/core', () => ({ core: { totpCode: vi.fn(async () => ({ code: '287082', seconds_left: 20 })) }, events: {} }))

import { emptyModel } from '../../lib/profileForm'
import BasicTab from './BasicTab.vue'
import { fingerprint as fp } from '../../test/fixtures'

function mountTab(patch: Partial<ReturnType<typeof emptyModel>> = {}, profileId: string | null = 'p1') {
  const model = { ...emptyModel(structuredClone(fp)), name: 'p', ...patch }
  return mountWithApp(BasicTab, { props: { modelValue: model, errors: [], profileId } })
}

describe('«Основное»: 2FA и автозаполнение', () => {
  it('код показан у сохранённого ключа; удалённый или новый ключ — без кода', () => {
    expect(mountTab({ totp: { value: '', saved: true, clear: false } }).findComponent({ name: 'TotpCode' }).exists()).toBe(true)
    expect(mountTab({ totp: { value: '', saved: true, clear: true } }).findComponent({ name: 'TotpCode' }).exists()).toBe(false)
    const typed = mountTab({ totp: { value: 'JBSW', saved: true, clear: false } })
    expect(typed.findComponent({ name: 'TotpCode' }).exists()).toBe(false)
    expect(typed.text()).toContain('Сохраните профиль, чтобы увидеть код')
    expect(mountTab({ totp: { value: '', saved: true, clear: false } }, null).findComponent({ name: 'TotpCode' }).exists()).toBe(false)
  })

  it('сохранённый профиль без ключа или с удаляемым ключом — без подписи «Код 2FA»', () => {
    expect(mountTab({ totp: { value: '', saved: false, clear: false } }).text()).not.toContain('Код 2FA')
    expect(mountTab({ totp: { value: '', saved: true, clear: true } }).text()).not.toContain('Код 2FA')
    expect(mountTab({ totp: { value: '', saved: true, clear: false } }).text()).toContain('Код 2FA')
  })

  it('подсказка автозаполнения зависит от домена', () => {
    expect(mountTab({ platformDomain: 'shop.example' }).text()).toContain('Логин и пароль подставляются в форму входа')
    expect(mountTab({ platformDomain: '' }).text()).toContain('Укажите домен платформы')
  })
})
