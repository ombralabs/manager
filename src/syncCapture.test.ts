import captureSource from '../src-tauri/src/engine/sync_capture.js?raw'
import { runInNewContext } from 'node:vm'
import { afterEach, expect, it, vi } from 'vitest'

afterEach(() => vi.useRealTimers())

it('native idle reset drops queued events, held pointer and pending waiter before ordinary capture resumes', async () => {
  vi.useFakeTimers()
  const listeners = new Map<string, (event: Record<string, unknown>) => void>()
  const source = captureSource.replace('//@SIG', 'const sig = () => ({ tag: "DIV" });')
  const capture = runInNewContext(`(${source})({mouse: true}); ({next: __ombraSyncNext, reset: __ombraSyncReset})`, {
    window: { addEventListener: (type: string, listener: (event: Record<string, unknown>) => void) => listeners.set(type, listener) },
    document: {}, HTMLSelectElement: class {}, setTimeout, clearTimeout,
    __ombraSyncNext: () => Promise.resolve([{ t: "legacy" }]),
  }) as { next: () => Promise<{ t: string }[]>; reset: () => void }
  const pointer = { isTrusted: true, pointerType: 'mouse', clientX: 1, clientY: 2, button: 0, buttons: 1, target: {} }
  listeners.get('pointerdown')!(pointer)
  const waiting = capture.next()
  capture.reset()
  expect(await waiting).toEqual([])
  await vi.advanceTimersByTimeAsync(0)
  listeners.get('pointermove')!({ ...pointer, buttons: 0, timeStamp: 1 })
  capture.reset()
  const resumed = capture.next()
  listeners.get('pointerdown')!(pointer)
  listeners.get('mousedown')!({ isTrusted: true, detail: 1 })
  expect((await resumed).map(event => event.t)).toEqual(['down'])
})
