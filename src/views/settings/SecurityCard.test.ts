import { enableAutoUnmount, flushPromises } from '@vue/test-utils'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { createMemoryHistory, createRouter } from 'vue-router'
import { CoreError } from '../../api/errors'
import type { SecurityStatus } from '../../api/types'
import { mountWithApp } from '../../test/mount'

const api = vi.hoisted(() => ({ securityStatus: vi.fn(), securityBeginSetup: vi.fn(), securityVerifySetup: vi.fn(), securityCancelSetup: vi.fn(), securityFinishSetup: vi.fn(), securityChangePassword: vi.fn(), securityReplaceAuthenticator: vi.fn() }))
vi.mock('../../api/core', () => ({ core: api }))
vi.mock('qrcode', () => ({ default: { toDataURL: vi.fn(async () => 'data:image/png;base64,qr') } }))
vi.mock('@tauri-apps/plugin-clipboard-manager', () => ({ writeText: vi.fn(async () => {}) }))
import { useSecurityStore } from '../../stores/security'
import SecurityCard from './SecurityCard.vue'

enableAutoUnmount(afterEach)
const initial: SecurityStatus = { configured: false, unlocked: false, migrating: false, recovery_remaining: 0, retry_after: 0 }
const details = { secret: 'JBSWY3DPEHPK3PXP', uri: 'otpauth://totp/Ombra?secret=JBSWY3DPEHPK3PXP', recovery_codes: ['ABCDE-FGHIJ-KLMNO', 'PQRST-UVWXY-Z2345'] }
beforeEach(() => {
  vi.clearAllMocks()
  api.securityStatus.mockResolvedValue({ ...initial })
  api.securityBeginSetup.mockResolvedValue(details)
  api.securityVerifySetup.mockResolvedValue(null)
  api.securityCancelSetup.mockResolvedValue(null)
  api.securityFinishSetup.mockImplementation(async () => { api.securityStatus.mockResolvedValue({ ...initial, configured: true, migrating: true, recovery_remaining: 8 }) })
  useSecurityStore().status = { ...initial }
})
function mountCard() {
  const router = createRouter({ history: createMemoryHistory(), routes: [{ path: '/settings', component: { template: '<div />' } }] })
  return mountWithApp(SecurityCard, { global: { plugins: [router] }, attachTo: document.body })
}
const button = (w: ReturnType<typeof mountCard>, text: string) => w.findAll('button').find((b) => b.text() === text)!
async function start(w: ReturnType<typeof mountCard>) {
  await button(w, 'Настроить защиту профилей').trigger('click')
  await w.get('#security-password').setValue('a sufficiently long password')
  await w.get('#security-confirm').setValue('a sufficiently long password')
  await w.get('form').trigger('submit')
  await flushPromises()
}
describe('настройка защиты пользователя', () => {
  it('объясняет отсутствие защиты на платформе до ввода пароля', () => {
    useSecurityStore().status = { ...initial, supported: false }
    const w = mountCard()
    expect(w.get('[role="status"]').text()).toContain('только в macOS')
    expect(w.findAll('button')).toHaveLength(0)
    expect(api.securityBeginSetup).not.toHaveBeenCalled()
  })
  it('не активируется без совпавшего пароля, проверенного кода и сохранённого восстановления', async () => {
    const w = mountCard()
    await button(w, 'Настроить защиту профилей').trigger('click')
    await w.get('#security-password').setValue('short')
    await w.get('#security-confirm').setValue('different')
    expect(button(w, 'Далее').attributes('disabled')).toBeDefined()
    await w.get('form').trigger('submit')
    expect(api.securityBeginSetup).not.toHaveBeenCalled()
    await w.get('#security-password').setValue('a sufficiently long password')
    await w.get('#security-confirm').setValue('a sufficiently long password')
    await w.get('form').trigger('submit'); await flushPromises()
    expect(w.get('img').attributes('alt')).toContain('QR-код')
    expect(w.text()).toContain(details.secret)
    expect(w.find('#security-password').exists()).toBe(false)
    api.securityVerifySetup.mockRejectedValueOnce(new CoreError('SecurityCredentials', 'incorrect'))
    await w.get('#security-verify-code').setValue('000000')
    await w.get('form').trigger('submit'); await flushPromises()
    expect(w.get('[role="alert"]').text()).toContain('Неверный пароль или код')
    expect(api.securityFinishSetup).not.toHaveBeenCalled()
    await w.get('#security-verify-code').setValue('123456')
    await w.get('form').trigger('submit'); await flushPromises()
    expect(w.findAll('.recovery-codes code')).toHaveLength(2)
    expect(button(w, 'Включить защиту и перезапустить').attributes('disabled')).toBeDefined()
    await w.get('input[type="checkbox"]').setValue(true)
    await button(w, 'Включить защиту и перезапустить').trigger('click'); await flushPromises()
    expect(api.securityFinishSetup).toHaveBeenCalledExactlyOnceWith(true)
    expect(w.text()).toContain('Настройка не завершена')
    expect(w.find('.recovery-codes').exists()).toBe(false)
    expect(w.find('.setup-secret').exists()).toBe(false)
  })
  it('отмена удаляет временную настройку и секрет из интерфейса', async () => {
    const w = mountCard()
    await start(w)
    await button(w, 'Отмена').trigger('click'); await flushPromises()
    expect(api.securityCancelSetup).toHaveBeenCalled()
    expect(w.text()).not.toContain(details.secret)
    expect(w.find('.recovery-codes').exists()).toBe(false)
    expect(api.securityFinishSetup).not.toHaveBeenCalled()
  })
})
