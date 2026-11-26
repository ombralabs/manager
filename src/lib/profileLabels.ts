import type { ProfileLabel, ProfileLabelFilter } from '../api/types'

export const labelFilterId = (filter: ProfileLabelFilter): number | null => filter === 'any' ? null : filter === 'none' ? 0 : filter

export const matchesProfileLabel = (label: ProfileLabel | null, filter: ProfileLabelFilter): boolean =>
  filter === 'any' || (filter === 'none' ? label === null : label?.id === filter)

/** Цвет обводки сохраняется; слишком светлый цвет текста затемняется до контраста 4.5:1 на белом. */
export function labelStyle(label: Pick<ProfileLabel, 'color'> | null) {
  if (!label) return undefined
  const rgb = [1, 3, 5].map((at) => parseInt(label.color.slice(at, at + 2), 16))
  const luminance = () => rgb.reduce((sum, value, i) => {
    const channel = value / 255
    return sum + [0.2126, 0.7152, 0.0722][i]! * (channel <= 0.04045 ? channel / 12.92 : ((channel + 0.055) / 1.055) ** 2.4)
  }, 0)
  while (1.05 / (luminance() + 0.05) < 4.5) for (let i = 0; i < rgb.length; i++) rgb[i] = Math.floor(rgb[i]! * 0.9)
  return { borderColor: label.color, color: `rgb(${rgb.join(', ')})` }
}
