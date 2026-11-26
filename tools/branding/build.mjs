import { spawnSync } from 'node:child_process'
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { macosIcons } from './macos.mjs'

const root = fileURLToPath(new URL('../..', import.meta.url))
const source = join(root, 'public/branding')
const output = join(root, 'src-tauri/icons')
const rendered = join(root, 'target/branding/color')
const sizes = [16, 24, 32, 48, 64, 128, 256, 512, 1024]

function render(svg, directory, sizes) {
  const result = spawnSync(join(root, 'node_modules/.bin/tauri'), [
    'icon', join(source, svg), '--output', directory, ...sizes.flatMap((size) => ['--png', String(size)]),
  ], { cwd: root, stdio: 'inherit' })
  if (result.error) throw result.error
  if (result.status !== 0) throw new Error(`Icon rendering failed (${result.status})`)
}

render('ombra-c.svg', rendered, sizes)
const png = (size) => readFileSync([16, 32, 64, 256].includes(size)
  ? join(source, `ombra-c-${size}.png`) : join(rendered, `${size}x${size}.png`))
mkdirSync(output, { recursive: true })
for (const [name, size] of [['32x32.png', 32], ['128x128.png', 128], ['128x128@2x.png', 256], ['icon.png', 512]]) {
  writeFileSync(join(output, name), png(size))
}

// ICO stores the supplied transparent PNGs unchanged, including the optical 16px variant.
const icoSizes = sizes.filter((size) => size <= 256)
const header = Buffer.alloc(6)
header.writeUInt16LE(1, 2)
header.writeUInt16LE(icoSizes.length, 4)
let offset = header.length + icoSizes.length * 16
const payloads = icoSizes.map(png)
const entries = icoSizes.map((size, i) => {
  const entry = Buffer.alloc(16)
  entry[0] = entry[1] = size === 256 ? 0 : size
  entry.writeUInt16LE(1, 4)
  entry.writeUInt16LE(32, 6)
  entry.writeUInt32LE(payloads[i].length, 8)
  entry.writeUInt32LE(offset, 12)
  offset += payloads[i].length
  return entry
})
writeFileSync(join(output, 'icon.ico'), Buffer.concat([header, ...entries, ...payloads]))

const macos = macosIcons({
  standard: join(source, 'ombra-c.svg'),
  small: join(source, 'ombra-c-small.svg'),
  output: join(root, 'target/branding/macos-color'),
})
writeFileSync(join(output, 'icon.icns'), macos.icns)
writeFileSync(join(root, 'public/favicon.svg'), readFileSync(join(source, 'ombra-c-small.svg')))
console.log('Updated sidebar source, favicon and desktop icons from the colored Ombra C assets.')
