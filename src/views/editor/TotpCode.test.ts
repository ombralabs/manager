import { enableAutoUnmount, flushPromises } from '@vue/test-utils'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { CoreError } from '../../api/errors'
import { mountWithApp } from '../../test/mount'

const api = vi.hoisted(() => ({ totpCode: vi.fn() }))
const clipboard = vi.hoisted(() => ({ writeText: vi.fn(async () => undefined) }))
vi.mock('../../api/core', () => ({ core: api, events: {} }))
vi.mock('@tauri-apps/plugin-clipboard-manager', () => clipboard)

import { useToasts } from '../../stores/toasts'
import TotpCode from './TotpCode.vue'

beforeEach(() => {
  vi.useFakeTimers()
  api.totpCode.mockReset()
  clipboard.writeText.mockClear()
})
enableAutoUnmount(afterEach)
afterEach(() => vi.useRealTimers())

describe('код 2FA', () => {
  it('код группами, отсчёт по секундам, новый запрос на нуле', async () => {
    api.totpCode.mockResolvedValueOnce({ code: '287082', seconds_left: 2 }).mockResolvedValueOnce({ code: '519034', seconds_left: 30 })
    const wrapper = mountWithApp(TotpCode, { props: { profileId: 'p1' } })
    await flushPromises()
    expect(wrapper.text()).toContain('287 082')
    expect(wrapper.text()).toContain('обновится через 2 с')
    await vi.advanceTimersByTimeAsync(1000)
    expect(wrapper.text()).toContain('обновится через 1 с')
    await vi.advanceTimersByTimeAsync(1000)
    await flushPromises()
    expect(api.totpCode).toHaveBeenCalledTimes(2)
    expect(wrapper.text()).toContain('519 034')
  })

  it('после размонтирования запросов нет', async () => {
    api.totpCode.mockResolvedValue({ code: '287082', seconds_left: 1 })
    const wrapper = mountWithApp(TotpCode, { props: { profileId: 'p1' } })
    await flushPromises()
    wrapper.unmount()
    await vi.advanceTimersByTimeAsync(5000)
    expect(api.totpCode).toHaveBeenCalledTimes(1)
  })

  it('размонтировали до первого ответа — интервал не стартует', async () => {
    let resolve!: (value: { code: string; seconds_left: number }) => void
    api.totpCode.mockReturnValue(new Promise((done) => (resolve = done)))
    const wrapper = mountWithApp(TotpCode, { props: { profileId: 'p1' } })
    wrapper.unmount()
    resolve({ code: '287082', seconds_left: 1 })
    await flushPromises()
    await vi.advanceTimersByTimeAsync(5000)
    expect(api.totpCode).toHaveBeenCalledTimes(1)
  })

  it('отсчёт по часам: пропущенные тики не оставляют просроченный код', async () => {
    api.totpCode.mockResolvedValueOnce({ code: '287082', seconds_left: 20 }).mockResolvedValueOnce({ code: '519034', seconds_left: 30 })
    const wrapper = mountWithApp(TotpCode, { props: { profileId: 'p1' } })
    await flushPromises()
    // Часы ушли на 25 с вперёд за один тик (сон, троттлинг скрытого окна).
    vi.setSystemTime(Date.now() + 25_000)
    await vi.advanceTimersByTimeAsync(1000)
    await flushPromises()
    expect(api.totpCode).toHaveBeenCalledTimes(2)
    expect(wrapper.text()).toContain('519 034')
  })

  it('окно снова видно — код перечитывается', async () => {
    api.totpCode.mockResolvedValue({ code: '287082', seconds_left: 20 })
    mountWithApp(TotpCode, { props: { profileId: 'p1' } })
    await flushPromises()
    document.dispatchEvent(new Event('visibilitychange'))
    await flushPromises()
    expect(api.totpCode).toHaveBeenCalledTimes(2)
  })

  it('сбой запроса — повтор на следующем тике; «неверный ключ» — только в новом 30-секундном окне', async () => {
    vi.setSystemTime(new Date('2026-10-06T12:00:05Z'))
    api.totpCode.mockRejectedValueOnce(new Error('boom')).mockResolvedValueOnce({ code: '287082', seconds_left: 20 })
    const first = mountWithApp(TotpCode, { props: { profileId: 'p1' } })
    await flushPromises()
    await vi.advanceTimersByTimeAsync(1000)
    await flushPromises()
    expect(api.totpCode).toHaveBeenCalledTimes(2)
    expect(first.text()).toContain('287 082')
    first.unmount()

    api.totpCode.mockReset().mockRejectedValue(new CoreError('Validation', 'x', [{ field: 'totp', code: 'invalid' }]))
    const second = mountWithApp(TotpCode, { props: { profileId: 'p2' } })
    await flushPromises()
    await vi.advanceTimersByTimeAsync(10_000)
    expect(api.totpCode).toHaveBeenCalledTimes(1)
    await vi.advanceTimersByTimeAsync(20_000)
    await flushPromises()
    expect(api.totpCode).toHaveBeenCalledTimes(2)
    second.unmount()
  })

  it('копирование — в буфер и уведомление; ошибка ключа — текст вместо кода', async () => {
    api.totpCode.mockResolvedValueOnce({ code: '287082', seconds_left: 20 })
    const wrapper = mountWithApp(TotpCode, { props: { profileId: 'p1' } })
    await flushPromises()
    await wrapper.findAll('button').find((b) => b.text() === 'Скопировать')!.trigger('click')
    await flushPromises()
    expect(clipboard.writeText).toHaveBeenCalledWith('287082')
    expect(useToasts().items.map((toast) => toast.title)).toContain('Код скопирован')
    api.totpCode.mockRejectedValueOnce(new CoreError('Validation', 'проверьте поля формы', [{ field: 'totp', code: 'invalid' }]))
    const broken = mountWithApp(TotpCode, { props: { profileId: 'p2' } })
    await flushPromises()
    expect(broken.text()).toContain('Ключ 2FA: допустимы буквы A–Z и цифры 2–7 (Base32)')
  })
})
