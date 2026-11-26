#!/usr/bin/env node
import { spawnSync, execFileSync } from 'node:child_process'
import { readFileSync, mkdirSync, mkdtempSync, copyFileSync, writeFileSync, readdirSync, statSync } from 'node:fs'
import { createHash } from 'node:crypto'
import { resolve, join, basename } from 'node:path'
import { fileURLToPath } from 'node:url'
import { notarizeMacos } from './macos.mjs'

const root = fileURLToPath(new URL('../../', import.meta.url))
process.chdir(root)
const env = { ...process.env }
// Signing uses an installed identity; notarization uses only the keychain profile.
for (const name of ['APPLE_ID', 'APPLE_PASSWORD', 'APPLE_TEAM_ID', 'APPLE_API_ISSUER', 'APPLE_API_KEY', 'APPLE_API_KEY_PATH', 'APPLE_CERTIFICATE', 'APPLE_CERTIFICATE_PASSWORD']) delete env[name]
const platforms = {
  'darwin-arm64': { os: 'darwin', target: 'aarch64-apple-darwin', bundles: 'app,dmg' },
  'darwin-x64': { os: 'darwin', target: 'x86_64-apple-darwin', bundles: 'app,dmg' },
  'windows-x64': { os: 'win32', target: 'x86_64-pc-windows-msvc', bundles: 'nsis' },
  'linux-x64': { os: 'linux', target: 'x86_64-unknown-linux-gnu', bundles: 'deb,rpm,appimage' },
}
const platform = process.argv[2]
const spec = platforms[platform]
const crossWindows = platform === 'windows-x64' && process.platform === 'linux'
if (!spec || (process.platform !== spec.os && !crossWindows)) throw new Error('Choose a platform supported by this build host: ' + Object.keys(platforms).join(', '))
if (execFileSync('git', ['status', '--porcelain'], { encoding: 'utf8', env }).trim()) {
  throw new Error('Commit source changes before building distribution files so the manifest identifies the exact source')
}
const version = JSON.parse(readFileSync('src-tauri/tauri.conf.json', 'utf8')).version
const packageVersion = JSON.parse(readFileSync('package.json', 'utf8')).version
const cargoVersion = readFileSync('src-tauri/Cargo.toml', 'utf8').match(/^version = "([^"]+)"/m)?.[1]
if (version !== packageVersion || version !== cargoVersion) throw new Error('Package, Cargo and Tauri versions must match')
if (process.env.GITHUB_REF_TYPE === 'tag' && process.env.GITHUB_REF_NAME !== `v${version}`) throw new Error('Tag must match the application version')
env.CI = 'true'
function run(command, args, evidence) {
  const result = spawnSync(command, args, { cwd: root, env, stdio: evidence ? 'pipe' : 'inherit', encoding: 'utf8', shell: process.platform === 'win32' && command === 'npm' })
  if (evidence) {
    writeFileSync(evidence, result.stdout || '')
    writeFileSync(evidence + '.stderr', result.stderr || '')
  }
  if (result.error) throw result.error
  if (result.status !== 0) throw new Error(`${command} failed: ${result.status}${evidence ? ` (evidence: ${evidence})` : ''}`)
  return result
}
const identity = spec.os === 'darwin' && env.APPLE_SIGNING_IDENTITY?.trim() !== '-' ? env.APPLE_SIGNING_IDENTITY?.trim() : undefined
run('npm', ['run', 'tauri', '--', 'build', '--target', spec.target, '--bundles', identity ? 'app' : spec.bundles, '--ci', ...(crossWindows ? ['--runner', 'cargo-xwin'] : []), '--', '--locked'])

const out = resolve('artifacts')
const release = resolve('src-tauri/target', spec.target, 'release')
const stem = `ombra-desktop-${version}-${platform}`
let distribution = 'unsigned-preview'
const signedDmg = join(release, 'bundle/dmg', `${stem}.dmg`)
if (identity) {
  const evidence = mkdtempSync(join(release, `notarization-${version}-`))
  console.log(`Signing evidence: ${evidence}`)
  notarizeMacos({ app: join(release, 'bundle/macos/Ombra.app'), dmg: signedDmg, evidence, identity, env, run })
  distribution = 'macos-notarized'
}
mkdirSync(out, { recursive: true })
function files(dir) {
  return readdirSync(dir, { withFileTypes: true }).flatMap(entry => entry.isDirectory() ? files(join(dir, entry.name)) : [join(dir, entry.name)])
}
const installers = identity ? [signedDmg] : files(join(release, 'bundle')).filter(path => /\.(dmg|exe|deb|rpm|AppImage)$/.test(path))
const expected = platform.startsWith('darwin') ? ['.dmg'] : platform.startsWith('windows') ? ['.exe'] : ['.deb', '.rpm', '.AppImage']
const products = []
for (const suffix of expected) {
  const matches = installers.filter(file => file.endsWith(suffix))
  if (matches.length !== 1) throw new Error(`Expected exactly one ${suffix} installer, found ${matches.length}`)
  const dest = join(out, `${stem}${suffix}`)
  copyFileSync(matches[0], dest)
  products.push(dest)
}
const archive = join(out, stem + (platform.startsWith('darwin') ? '.tar.gz' : platform.startsWith('windows') ? '.zip' : '.tar.xz'))
if (platform.startsWith('darwin')) run('tar', ['-czf', archive, '-C', join(release, 'bundle/macos'), 'Ombra.app'])
else if (platform.startsWith('windows')) {
  // Git Bash can put GNU tar first in PATH; use Windows bsdtar for ZIP and drive-letter paths.
  if (crossWindows) run('python3', ['-m', 'zipfile', '-c', archive, join(release, 'ombra.exe')])
  else {
    const windowsTar = join(process.env.SystemRoot || 'C:\\Windows', 'System32', 'tar.exe')
    run(windowsTar, ['-a', '-cf', archive, '-C', release, 'ombra.exe'])
  }
}
else run('tar', ['-cJf', archive, '-C', release, 'ombra'])
products.push(archive)
const manifest = {
  version, platform,
  commit: execFileSync('git', ['rev-parse', 'HEAD'], { encoding: 'utf8', env }).trim(),
  engineCratesRevision: readFileSync('vendor/ombra-engine/REVISION', 'utf8').trim(),
  distribution,
  files: products.map(file => {
    const sha256 = createHash('sha256').update(readFileSync(file)).digest('hex')
    writeFileSync(file + '.sha256', `${sha256}  ${basename(file)}\n`)
    return { name: basename(file), size: statSync(file).size, sha256 }
  }),
}
writeFileSync(join(out, `${stem}.json`), JSON.stringify(manifest, null, 2) + '\n')
console.log(`Build files: ${out}`)
