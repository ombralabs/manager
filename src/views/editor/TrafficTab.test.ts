import { flushPromises } from '@vue/test-utils'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { ConnRecord } from '../../api/types'
import { mountWithApp } from '../../test/mount'

const api = vi.hoisted(() => ({ trafficSummary: vi.fn(), connectionLog: vi.fn() }))
vi.mock('../../api/core', () => ({ core: api, events: {} }))

import TrafficTab from './TrafficTab.vue'

const record = (n: number, result = 'ok'): ConnRecord => ({
  ts_ms: Date.now() - (10 - n) * 1000,
  proxy_id: 1,
  target: `host${n}.test:443`,
  bytes_up: 100 * n,
  bytes_down: 2048 * n,
  ms: 300 * n,
  result,
})

beforeEach(() => {
  vi.useFakeTimers()
  api.trafficSummary.mockReset().mockResolvedValue({ bytes_up: 1536, bytes_down: 5 * 1024 * 1024, connections: 3, since: Date.now() - 3_600_000, live: true })
  api.connectionLog.mockReset().mockResolvedValue([record(1), record(2, '502: refused'), record(3)])
})
afterEach(() => vi.useRealTimers())

const mountTab = (live: boolean) => mountWithApp(TrafficTab, { props: { profileId: 'p1', live } })

describe('вкладка «Трафик»', () => {
  it('сводка и журнал, новые сверху, итоги словами', async () => {
    const wrapper = mountTab(true)
    await flushPromises()
    expect(wrapper.text()).toContain('1,5 КБ')
    expect(wrapper.text()).toContain('5,0 МБ')
    expect(wrapper.text()).toContain('активное соединение')
    const rows = wrapper.findAll('.body .row')
    expect(rows.map((row) => row.text().includes('host3.test:443'))[0]).toBe(true)
    expect(wrapper.text()).toContain('шлюз не ответил')
    expect(api.connectionLog).toHaveBeenCalledWith('p1', 100)
  })

  it('работающий — опрос раз в 5 с; остановили — опрос прекращается; размонтировали — запросов нет', async () => {
    const wrapper = mountTab(true)
    await flushPromises()
    await vi.advanceTimersByTimeAsync(5000)
    expect(api.trafficSummary).toHaveBeenCalledTimes(2)
    await wrapper.setProps({ live: false })
    await flushPromises()
    // На остановке — одно итоговое чтение, дальше тишина.
    expect(api.trafficSummary).toHaveBeenCalledTimes(3)
    await vi.advanceTimersByTimeAsync(15_000)
    expect(api.trafficSummary).toHaveBeenCalledTimes(3)
    await wrapper.setProps({ live: true })
    await flushPromises()
    // На запуске — сразу чтение, не через 5 с.
    expect(api.trafficSummary).toHaveBeenCalledTimes(4)
    await vi.advanceTimersByTimeAsync(5000)
    expect(api.trafficSummary).toHaveBeenCalledTimes(5)
    wrapper.unmount()
    await vi.advanceTimersByTimeAsync(15_000)
    expect(api.trafficSummary).toHaveBeenCalledTimes(5)
  })

  it('остановленный — без опроса, «Обновить» запрашивает; «Показать 500»; пустой журнал', async () => {
    api.connectionLog.mockResolvedValue([])
    const wrapper = mountTab(false)
    await flushPromises()
    expect(wrapper.text()).toContain('Соединений ещё не было')
    await vi.advanceTimersByTimeAsync(10_000)
    expect(api.trafficSummary).toHaveBeenCalledTimes(1)
    await wrapper.findAll('button').find((b) => b.text() === 'Обновить')!.trigger('click')
    await flushPromises()
    expect(api.trafficSummary).toHaveBeenCalledTimes(2)
    api.connectionLog.mockResolvedValue(Array.from({ length: 100 }, (_, n) => record(n)))
    await wrapper.findAll('button').find((b) => b.text() === 'Обновить')!.trigger('click')
    await flushPromises()
    await wrapper.findAll('button').find((b) => b.text() === 'Показать 500')!.trigger('click')
    await flushPromises()
    expect(api.connectionLog).toHaveBeenLastCalledWith('p1', 500)
  })

  it('«Показать 500» — только когда записей набралось на полный список', async () => {
    const wrapper = mountTab(false)
    await flushPromises()
    expect(wrapper.findAll('button').some((b) => b.text() === 'Показать 500')).toBe(false)
    api.connectionLog.mockResolvedValue(Array.from({ length: 100 }, (_, n) => record(n)))
    await wrapper.findAll('button').find((b) => b.text() === 'Обновить')!.trigger('click')
    await flushPromises()
    expect(wrapper.findAll('button').some((b) => b.text() === 'Показать 500')).toBe(true)
  })
})
