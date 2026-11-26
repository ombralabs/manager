import type { OpInfo, OpsCatalog } from '../../../api/types'

const op = (id: string, adspower: string, group: string, outputs: string[], inputs = ['in']): OpInfo => ({ id, adspower, group, inputs, outputs, fields: [] })

/** Мини-каталог для тестов редактора. */
export const testCatalog: OpsCatalog = {
  system: [],
  ops: [
    op('start', 'Start', 'control', ['next'], []),
    op('control.if', 'Statement if', 'control', ['true', 'false']),
    op('control.for_times', 'For Loop Times', 'control', ['body', 'next'], ['in', 'loop']),
    op('control.group', 'Group', 'control', ['next']),
    op('web.click', 'Click Element', 'web', ['next']),
    op('key.press', 'Press Key', 'key', ['next']),
    op('wait.time', 'Wait Time', 'wait', ['next']),
  ],
}
