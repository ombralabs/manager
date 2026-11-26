import { describe, expect, it } from 'vitest'
import registry from '../../src-tauri/src/flow/ops/mod.rs?raw'
import { OP_ICONS } from './opIcons'

describe('operation icons', () => {
  it('covers every operation registered by the engine', () => {
    const registered = [...registry.matchAll(/\bop\(\s*"([a-z0-9_.]+)"/g)].map((match) => match[1]!)
    expect(registered.length).toBeGreaterThan(50)
    expect(Object.keys(OP_ICONS).sort()).toEqual(registered.sort())
  })
})
