import { describe, expect, it, vi } from 'vitest'
import { i18n } from '../i18n'
import { notify } from './notify'
import type { SupervisorEvent } from '../api/types'
import { createSupervisorNotices } from './supervisorNotices'
import { toCoreError } from '../api/errors'

function setup(names: Record<string, string> = {}, fetched: Record<string, string> = {}) {
  let clock = 0
  const warning = vi.spyOn(notify, 'warning').mockImplementation(() => {})
  const fetchName = vi.fn(async (id: string) => fetched[id] ?? null)
  const notices = createSupervisorNotices({
    nameOf: (id) => names[id] ?? null,
    fetchName,
    now: () => clock,
  })
  const tick = (ms: number) => {
    clock += ms
  }
  return { notices, warning, fetchName, tick }
}

const crashed = (id: string, code: number | null): SupervisorEvent => ({
  kind: 'status',
  profile_id: id,
  status: { state: 'crashed', code, signal: null, at: 0 },
})
const unreachable = (id: string): SupervisorEvent => ({ kind: 'gateway_unreachable', profile_id: id })

describe('уведомления о событиях супервизора', () => {
  it('offers one update for legacy license waits across profiles and never treats them as ready', async () => {
    const { notices } = setup()
    const error = vi.spyOn(notify, 'error').mockImplementation(toCoreError)
    for (const license of ['no_slot', 'offline', 'expired', 'suspended', 'device_bound', 'unknown']) {
      await notices.handle({ kind: 'status', profile_id: license, status: { state: 'waiting', license, info: null } })
    }
    expect(error).toHaveBeenCalledExactlyOnceWith(expect.objectContaining({ code: 'EngineUpdateRequired' }))
  })

  it('имя скрытого фильтром профиля берётся из ядра, а без имени — id', async () => {
    const { notices, warning, fetchName } = setup({ a1: 'Магазин' }, { b2: 'Почта 02' })
    await notices.handle(crashed('a1', 137))
    await notices.handle(crashed('b2', null))
    await notices.handle(crashed('c3', null))
    expect(fetchName.mock.calls).toEqual([['b2'], ['c3']])
    expect(warning.mock.calls).toEqual([
      ['Браузер профиля «Магазин» завершился с ошибкой', 'Код завершения: 137'],
      ['Браузер профиля «Почта 02» завершился с ошибкой', undefined],
      ['Браузер профиля «c3» завершился с ошибкой', undefined],
    ])
  })

  it('ошибка запроса имени не теряет уведомление', async () => {
    const { notices, warning, fetchName } = setup()
    fetchName.mockRejectedValueOnce(new Error('нет связи с ядром'))
    await notices.handle(crashed('d4', 1))
    expect(warning.mock.calls).toEqual([['Браузер профиля «d4» завершился с ошибкой', 'Код завершения: 1']])
  })

  it('«шлюз не отвечает» по одному профилю — не чаще раза в минуту', async () => {
    const { notices, warning, fetchName, tick } = setup({ a1: 'Магазин', b2: 'Почта' })
    await notices.handle(unreachable('a1'))
    tick(30_000)
    await notices.handle(unreachable('a1'))
    await notices.handle(unreachable('b2'))
    tick(31_000)
    await notices.handle(unreachable('a1'))
    expect(warning.mock.calls.map(([title]) => title)).toEqual([
      'Профиль «Магазин» приостановлен: прокси не отвечает',
      'Профиль «Почта» приостановлен: прокси не отвечает',
      'Профиль «Магазин» приостановлен: прокси не отвечает',
    ])
    expect(warning.mock.calls[0]?.[1]).toBe(i18n.global.t('toasts.gatewayUnreachableText'))
    expect(fetchName).not.toHaveBeenCalled()
  })

  it('замена шлюза берёт имя из события, другие статусы молчат', async () => {
    const { notices, warning, fetchName } = setup()
    await notices.handle({ kind: 'gateway_replaced', profile_id: 'a1', name: 'Реклама 01' })
    await notices.handle({ kind: 'status', profile_id: 'a1', status: { state: 'stopped' } })
    await notices.handle({ kind: 'status', profile_id: 'a1', status: { state: 'starting' } })
    expect(warning.mock.calls).toEqual([['Прокси профиля «Реклама 01» заменён', i18n.global.t('toasts.gatewayReplacedText')]])
    expect(fetchName).not.toHaveBeenCalled()
  })
})
