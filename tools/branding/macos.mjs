import { spawnSync } from 'node:child_process'
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'
import { fileURLToPath } from 'node:url'

const root = fileURLToPath(new URL('../..', import.meta.url))
// The supplied mark is 992px wide on a 1024px canvas. Match the macOS 824px artwork frame.
const scale = 824 / 992
const inset = 32 * (1 - scale)
const sizes = [16, 32, 64, 128, 256, 512, 1024]

function framed(svg) {
  return svg.replace(/(<svg\b[^>]*>)([\s\S]*)(<\/svg>\s*)$/, (_, start, body, end) =>
    `${start}\n  <g transform="translate(${inset} ${inset}) scale(${scale})">${body}</g>\n${end}`)
}

export function macosIcons({ standard, small, output }) {
  mkdirSync(output, { recursive: true })
  const rendered = {}
  for (const [variant, source, dimensions] of [['standard', standard, sizes], ['small', small, [16, 32]]]) {
    const svg = join(output, `${variant}.svg`)
    writeFileSync(svg, framed(readFileSync(source, 'utf8')))
    const directory = join(output, variant)
    const result = spawnSync(join(root, 'node_modules/.bin/tauri'), [
      'icon', svg, '--output', directory, ...dimensions.flatMap((size) => ['--png', String(size)]),
    ], { cwd: root, stdio: 'inherit' })
    if (result.error) throw result.error
    if (result.status !== 0) throw new Error(`macOS icon rendering failed (${result.status})`)
    rendered[variant] = directory
  }
  const png = (size, optical = false) => readFileSync(join(rendered[optical ? 'small' : 'standard'], `${size}x${size}.png`))
  const types = { icp4: 16, icp5: 32, icp6: 64, ic07: 128, ic08: 256, ic09: 512, ic10: 1024, ic11: 32, ic12: 64, ic13: 256, ic14: 512 }
  const chunks = Object.entries(types).map(([type, size]) => {
    const data = png(size, type === 'icp4' || type === 'ic11')
    const chunk = Buffer.alloc(8)
    chunk.write(type)
    chunk.writeUInt32BE(data.length + chunk.length, 4)
    return Buffer.concat([chunk, data])
  })
  const header = Buffer.alloc(8)
  header.write('icns')
  header.writeUInt32BE(8 + chunks.reduce((sum, chunk) => sum + chunk.length, 0), 4)
  return { icns: Buffer.concat([header, ...chunks]), png }
}
