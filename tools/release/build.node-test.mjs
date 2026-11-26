import assert from 'node:assert/strict'
import { spawnSync } from 'node:child_process'
import { cpSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, symlinkSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'

const credentialKeys = ['APPLE_ID', 'APPLE_PASSWORD', 'APPLE_TEAM_ID', 'APPLE_API_ISSUER', 'APPLE_API_KEY', 'APPLE_API_KEY_PATH', 'APPLE_CERTIFICATE', 'APPLE_CERTIFICATE_PASSWORD']
const fakeTools = String.raw`
const fs = require('node:fs'), path = require('node:path')
const command = path.basename(process.argv[1]), args = process.argv.slice(2), last = args.at(-1)
fs.appendFileSync('commands.jsonl', JSON.stringify({ command, args, credentials: Object.keys(process.env).filter(key => /^APPLE_(ID|PASSWORD|TEAM_ID|API_.*|CERTIFICATE.*)$/.test(key)) }) + '\n')
function put(file, data) { fs.mkdirSync(path.dirname(file), { recursive: true }); fs.writeFileSync(file, data) }
function fail(point) { if (process.env.TEST_FAIL === point) process.exit(9) }
if (command === 'git') {
  if (args[0] === 'rev-parse') console.log('a'.repeat(40))
} else if (command === 'npm') {
  fail('tauri-build')
  const target = args[args.indexOf('--target') + 1], bundles = args[args.indexOf('--bundles') + 1]
  const release = path.resolve('src-tauri/target', target, 'release')
  if (target.includes('apple')) {
    put(path.join(release, 'bundle/macos/Ombra.app/Contents/MacOS/ombra'), 'app')
    // A stale installer must never be reused by signed mode.
    put(path.join(release, 'bundle/dmg/stale.dmg'), 'unstapled')
    if (process.env.APPLE_SIGNING_IDENTITY && process.env.APPLE_SIGNING_IDENTITY !== '-' && bundles !== 'app') process.exit(8)
  } else if (target.includes('windows')) {
    put(path.join(release, 'bundle/nsis/Ombra.exe'), 'installer')
    put(path.join(release, 'ombra.exe'), 'exe')
  } else {
    for (const ext of ['deb', 'rpm', 'AppImage']) put(path.join(release, 'bundle', 'Ombra.' + ext), ext)
    put(path.join(release, 'ombra'), 'elf')
  }
} else if (command === 'codesign') {
  if (args.includes('--sign')) fail('dmg-sign')
  if (args.includes('--verify')) fail(last.endsWith('.app') ? 'app-signature' : 'dmg-signature')
  if (args.includes('--display')) {
    let details = 'Authority=Developer ID Application: Test (TESTTEAM)\nTeamIdentifier=TESTTEAM\nCodeDirectory flags=0x10000(runtime)\nTimestamp=Nov 11, 2026\n'
    if (process.env.TEST_SIGNATURE === 'ad-hoc') details = 'Signature=adhoc\n'
    if (process.env.TEST_SIGNATURE === 'no-runtime') details = details.replace('(runtime)', '')
    if (process.env.TEST_SIGNATURE === 'no-timestamp') details = details.replace('Timestamp=Nov 11, 2026\n', '')
    process.stderr.write(details)
  }
} else if (command === 'ditto') {
  const source = args.at(-2)
  if (args.includes('-c')) put(last, 'notary zip')
  else fs.cpSync(source, last, { recursive: true })
} else if (command === 'xcrun') {
  if (args[0] === 'notarytool') {
    const kind = args[2].endsWith('.dmg') ? 'dmg' : 'app'
    fail(kind + '-submit')
    if (process.env.TEST_FAIL === kind + '-malformed') { console.log('not JSON'); process.exit(0) }
    console.log(JSON.stringify({ id: kind + '-submission-id', status: process.env.TEST_FAIL === kind + '-rejected' ? 'Invalid' : 'Accepted', message: 'Processing complete' }))
  } else if (args[0] === 'stapler') {
    const kind = last.endsWith('.app') ? 'app' : 'dmg'
    fail(kind + '-' + args[1])
    if (args[1] === 'staple' && kind === 'app') put(path.join(last, 'ticket'), 'stapled')
  }
} else if (command === 'hdiutil') {
  const stage = args[args.indexOf('-srcfolder') + 1]
  if (fs.readlinkSync(path.join(stage, 'Applications')) !== '/Applications') process.exit(10)
  put(last, fs.readFileSync(path.join(stage, 'Ombra.app/ticket')))
} else if (command === 'spctl') {
  fail(last.endsWith('.app') ? 'app-gatekeeper' : 'dmg-gatekeeper')
} else if (command === 'tar') {
  const archive = args[1], source = args[args.indexOf('-C') + 1]
  put(archive, last === 'Ombra.app' && fs.existsSync(path.join(source, last, 'ticket')) ? fs.readFileSync(path.join(source, last, 'ticket')) : 'archive')
} else if (command === 'python3') put(args[3], 'archive')
else throw new Error('Unexpected tool: ' + command)
`

function build(t, { platform = 'darwin-arm64', identity, fail, signature, credentials = false, profile, keychain } = {}) {
  const root = mkdtempSync(join(tmpdir(), 'ombra-build-test-'))
  t.after(() => rmSync(root, { recursive: true, force: true }))
  cpSync(new URL('./', import.meta.url), join(root, 'tools/release'), { recursive: true })
  mkdirSync(join(root, 'src-tauri'), { recursive: true })
  mkdirSync(join(root, 'vendor/ombra-engine'), { recursive: true })
  mkdirSync(join(root, 'bin'))
  mkdirSync(join(root, 'artifacts'))
  writeFileSync(join(root, 'package.json'), JSON.stringify({ version: '1.1.29' }))
  writeFileSync(join(root, 'src-tauri/tauri.conf.json'), JSON.stringify({ version: '1.1.29' }))
  writeFileSync(join(root, 'src-tauri/Cargo.toml'), 'version = "1.1.29"\n')
  writeFileSync(join(root, 'vendor/ombra-engine/REVISION'), 'engine-revision\n')
  writeFileSync(join(root, 'artifacts/previous-release.json'), 'old public artifact\n')
  const tool = join(root, 'bin/tool.cjs')
  writeFileSync(tool, '#!' + process.execPath + '\n' + fakeTools, { mode: 0o755 })
  for (const name of ['git', 'npm', 'codesign', 'ditto', 'xcrun', 'hdiutil', 'spctl', 'tar', 'python3']) symlinkSync(tool, join(root, 'bin', name))
  const env = { ...process.env, PATH: join(root, 'bin') + ':' + process.env.PATH }
  for (const key of [...credentialKeys, 'APPLE_SIGNING_IDENTITY', 'OMBRA_NOTARY_PROFILE', 'OMBRA_NOTARY_KEYCHAIN', 'GITHUB_REF_TYPE', 'GITHUB_REF_NAME']) delete env[key]
  if (credentials) for (const key of credentialKeys) env[key] = 'fixture-secret'
  if (identity !== undefined) env.APPLE_SIGNING_IDENTITY = identity
  if (profile) env.OMBRA_NOTARY_PROFILE = profile
  if (keychain) env.OMBRA_NOTARY_KEYCHAIN = keychain
  if (fail) env.TEST_FAIL = fail
  if (signature) env.TEST_SIGNATURE = signature
  const host = platform.startsWith('darwin') ? 'darwin' : 'linux'
  const result = spawnSync(process.execPath, ['--import', `data:text/javascript,Object.defineProperty(process,'platform',{value:'${host}'})`, join(root, 'tools/release/build.mjs'), platform], { env, encoding: 'utf8' })
  const commands = readFileSync(join(root, 'commands.jsonl'), 'utf8').trim().split('\n').map(JSON.parse)
  const stem = `ombra-desktop-1.1.29-${platform}`
  return { root, result, commands, stem }
}

for (const [platform, identity] of [['darwin-arm64', undefined], ['darwin-x64', '-'], ['linux-x64', 'Developer ID Application: Test'], ['windows-x64', 'Developer ID Application: Test']]) {
  test(`${platform} with ${identity ?? 'no identity'} keeps the unsigned preview policy`, t => {
    const { root, result, commands, stem } = build(t, { platform, identity })
    assert.equal(result.status, 0, result.stderr)
    assert.equal(JSON.parse(readFileSync(join(root, 'artifacts', stem + '.json'))).distribution, 'unsigned-preview')
    assert.equal(commands.some(c => c.command === 'xcrun'), false)
  })
}

for (const platform of ['darwin-arm64', 'darwin-x64']) {
  test(`${platform} publishes a stapled app and DMG only after successful checks`, t => {
    const { root, result, commands, stem } = build(t, { platform, identity: 'Developer ID Application: Test', credentials: true, profile: 'release-notary', keychain: '/test/login.keychain-db' })
    assert.equal(result.status, 0, result.stderr)
    const manifest = JSON.parse(readFileSync(join(root, 'artifacts', stem + '.json')))
    assert.equal(manifest.distribution, 'macos-notarized')
    assert.equal(manifest.commit, 'a'.repeat(40))
    assert.deepEqual(manifest.files.map(file => file.name), [stem + '.dmg', stem + '.tar.gz'])
    assert.equal(readFileSync(join(root, 'artifacts', stem + '.dmg'), 'utf8'), 'stapled')
    assert.equal(readFileSync(join(root, 'artifacts', stem + '.tar.gz'), 'utf8'), 'stapled')
    assert.deepEqual(commands.flatMap(c => c.credentials), [])
    const submissions = commands.filter(c => c.args[0] === 'notarytool')
    assert.equal(submissions.length, 2)
    for (const { args } of submissions) {
      assert.equal(args[args.indexOf('--keychain-profile') + 1], 'release-notary')
      assert.equal(args[args.indexOf('--keychain') + 1], '/test/login.keychain-db')
      assert.ok(args.includes('--wait'))
      assert.equal(args[args.indexOf('--output-format') + 1], 'json')
    }
    assert.equal(commands.filter(c => c.command === 'spctl').length, 2)
    const evidence = readdirSync(join(root, 'src-tauri/target'), { recursive: true }).filter(file => file.endsWith('notary.json'))
    assert.equal(evidence.length, 2)
    for (const file of evidence) assert.equal(JSON.parse(readFileSync(join(root, 'src-tauri/target', file))).status, 'Accepted')
    assert.equal(readdirSync(join(root, 'artifacts')).length, 6)
  })
}

for (const fail of ['tauri-build', 'app-signature', 'app-submit', 'app-rejected', 'app-malformed', 'app-staple', 'app-validate', 'app-gatekeeper', 'dmg-sign', 'dmg-submit', 'dmg-rejected', 'dmg-malformed', 'dmg-staple', 'dmg-signature', 'dmg-validate', 'dmg-gatekeeper']) {
  test(`${fail} stops publication and preserves existing public artifacts`, t => {
    const { root, result, commands } = build(t, { identity: 'Developer ID Application: Test', fail })
    assert.notEqual(result.status, 0)
    assert.deepEqual(readdirSync(join(root, 'artifacts')), ['previous-release.json'])
    assert.equal(readFileSync(join(root, 'artifacts/previous-release.json'), 'utf8'), 'old public artifact\n')
    assert.equal(commands.some(c => c.command === 'tar'), false)
    const submission = commands.find(c => c.args[0] === 'notarytool')
    if (submission) assert.equal(submission.args[submission.args.indexOf('--keychain-profile') + 1], 'ombra-notary')
    // Reaching this injected external failure proves the test did not fail earlier by accident.
    assert.ok(result.stderr.includes(fail.includes('rejected') ? 'Invalid' : fail.includes('malformed') ? 'JSON' : 'failed: 9'), result.stderr)
    if (fail.includes('rejected')) {
      const evidence = readdirSync(join(root, 'src-tauri/target'), { recursive: true }).find(file => file.endsWith(`${fail.split('-')[0]}-notary.json`))
      const submission = JSON.parse(readFileSync(join(root, 'src-tauri/target', evidence)))
      assert.equal(submission.status, 'Invalid')
      assert.equal(submission.id, `${fail.split('-')[0]}-submission-id`)
    }
  })
}

for (const signature of ['ad-hoc', 'no-runtime', 'no-timestamp']) {
  test(`rejects ${signature} signatures before submission`, t => {
    const { root, result, commands } = build(t, { identity: 'Developer ID Application: Test', signature })
    assert.notEqual(result.status, 0)
    assert.deepEqual(readdirSync(join(root, 'artifacts')), ['previous-release.json'])
    assert.equal(commands.some(c => c.args[0] === 'notarytool'), false)
    assert.match(result.stderr, /Developer ID|runtime|timestamp/i)
  })
}
