import { spawnSync } from 'node:child_process'
import { fileURLToPath } from 'node:url'
import { resolve } from 'node:path'
const targets = { 'darwin-arm64': 'aarch64-apple-darwin', 'darwin-x64': 'x86_64-apple-darwin', 'linux-x64': 'x86_64-unknown-linux-gnu', 'windows-x64': 'x86_64-pc-windows-msvc' }
const target = targets[process.argv[2]]
if (!target) throw new Error('Unknown platform')
const root = fileURLToPath(new URL('../../', import.meta.url))
const binary = resolve(root, 'src-tauri/target', target, 'release',
  process.platform === 'darwin' ? 'bundle/macos/Ombra.app/Contents/MacOS/ombra' : process.platform === 'win32' ? 'ombra.exe' : 'ombra')
const result = spawnSync(binary, ['flow', '--help'], { encoding: 'utf8', timeout: 20000 })
if (result.error || result.status !== 0 || !(result.stdout + result.stderr).includes('flow')) {
  throw new Error(`CLI startup failed: ${result.error || result.stderr || result.status}`)
}
console.log('CLI startup passed')
