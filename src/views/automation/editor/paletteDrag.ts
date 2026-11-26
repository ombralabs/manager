import type { OpInfo } from '../../../api/types'

/** Операция из палитры и положение указателя в координатах окна. */
export interface PaletteDrag {
  op: OpInfo
  x: number
  y: number
}
