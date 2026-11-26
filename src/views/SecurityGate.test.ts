import { enableAutoUnmount, flushPromises } from '@vue/test-utils'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { createMemoryHistory, createRouter } from 'vue-router'
import { CoreError } from '../api/errors'
import { mountWithApp } from '../test/mount'
const api = vi.hoisted(() => ({ securityStatus: vi.fn(), securityUnlock: vi.fn(), listAnnouncements: vi.fn(async () => []) }))
vi.mock('../api/core', () => ({ core: api }))
import { useSecurityStore } from '../stores/security'
import SecurityGate from './SecurityGate.vue'
enableAutoUnmount(afterEach)
beforeEach(() => { vi.clearAllMocks(); useSecurityStore().status = { configured: true, unlocked: false, migrating: false, recovery_remaining: 8, retry_after: 0 }; api.securityStatus.mockImplementation(async () => useSecurityStore().status) })
function mountGate() {
  const router = createRouter({ history: createMemoryHistory(), routes: ['/', '/settings'].map((path) => ({ path, component: { template: '<div />' } })) })
  return mountWithApp(SecurityGate, { global: { plugins: [router] }, attachTo: document.body })
}
describe('разблокировка сеанса', () => {
  it('ошибка 2FA не разблокирует данные; повторная успешная проверка очищает секреты', async () => {
    api.securityUnlock.mockRejectedValueOnce(new CoreError('SecurityCredentials', 'incorrect')).mockImplementationOnce(async () => { useSecurityStore().status!.unlocked = true })
    const w = mountGate()
    await w.get('#unlock-password').setValue('test user password')
    await w.get('#unlock-code').setValue('000000')
    await w.get('form').trigger('submit'); await flushPromises()
    expect(useSecurityStore().unlocked).toBe(false)
    expect(w.get('[role="alert"]').text()).toContain('Неверный пароль или код')
    await w.get('#unlock-code').setValue('123456')
    await w.get('form').trigger('submit'); await flushPromises()
    expect(api.securityUnlock).toHaveBeenLastCalledWith('test user password', '123456', false)
    expect(useSecurityStore().unlocked).toBe(true)
    expect(w.get<HTMLInputElement>('#unlock-password').element.value).toBe('')
    expect(w.get<HTMLInputElement>('#unlock-code').element.value).toBe('')
  })
  it('код восстановления передаётся как отдельный способ подтверждения', async () => {
    api.securityUnlock.mockResolvedValue(null)
    const w = mountGate()
    await w.get('button.link').trigger('click')
    expect(w.text()).toContain('Код восстановления')
    await w.get('#unlock-password').setValue('test user password')
    await w.get('#unlock-code').setValue('ABCDE-FGHIJ-KLMNO')
    await w.get('form').trigger('submit'); await flushPromises()
    expect(api.securityUnlock).toHaveBeenCalledExactlyOnceWith('test user password', 'ABCDE-FGHIJ-KLMNO', true)
  })
})
