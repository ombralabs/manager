import { mkdirSync, mkdtempSync, rmSync, symlinkSync } from 'node:fs'
import { dirname, join } from 'node:path'

export function notarizeMacos({ app, dmg, evidence, identity, env, run }) {
  mkdirSync(evidence, { recursive: true })
  mkdirSync(dirname(dmg), { recursive: true })
  const stage = mkdtempSync(join(evidence, 'staging-'))
  const notaryArgs = ['--keychain-profile', env.OMBRA_NOTARY_PROFILE || 'ombra-notary']
  if (env.OMBRA_NOTARY_KEYCHAIN) notaryArgs.push('--keychain', env.OMBRA_NOTARY_KEYCHAIN)
  const checked = (command, args, name) => run(command, args, join(evidence, name))
  function verifySignature(path, name, runtime = false) {
    checked('codesign', ['--verify', '--deep', '--strict', path], `${name}-verify.txt`)
    const result = checked('codesign', ['--display', '--verbose=4', path], `${name}-signature.txt`)
    const details = result.stdout + result.stderr
    if (!/^Authority=Developer ID Application:/m.test(details)) throw new Error(`${name} requires a Developer ID Application signature`)
    if (!/^Timestamp=(?!none\s*$)\S.+$/m.test(details)) throw new Error(`${name} requires a secure timestamp`)
    if (runtime && !/^CodeDirectory .*flags=.*\bruntime\b/m.test(details)) throw new Error(`${name} requires hardened runtime`)
  }
  function notarize(archive, path, name) {
    const result = checked('xcrun', ['notarytool', 'submit', archive, ...notaryArgs, '--wait', '--output-format', 'json'], `${name}-notary.json`)
    const submission = JSON.parse(result.stdout)
    if (submission.status !== 'Accepted' || !submission.id) throw new Error(`${name} notarization was not accepted: ${submission.status}`)
    checked('xcrun', ['stapler', 'staple', path], `${name}-staple.txt`)
    checked('xcrun', ['stapler', 'validate', path], `${name}-stapler-validate.txt`)
    checked('codesign', ['--verify', '--deep', '--strict', path], `${name}-stapled-verify.txt`)
    checked('spctl', ['--assess', '--verbose=4', '--type', name === 'app' ? 'execute' : 'open', ...(name === 'dmg' ? ['--context', 'context:primary-signature'] : []), path], `${name}-gatekeeper.txt`)
  }
  try {
    verifySignature(app, 'app', true)
    const zip = join(stage, 'app.zip')
    run('ditto', ['-c', '-k', '--keepParent', app, zip])
    notarize(zip, app, 'app')
    const contents = join(stage, 'dmg')
    mkdirSync(contents)
    // ditto preserves the stapled ticket and bundle metadata in the DMG copy.
    run('ditto', [app, join(contents, 'Ombra.app')])
    symlinkSync('/Applications', join(contents, 'Applications'))
    run('hdiutil', ['create', '-volname', 'Ombra', '-srcfolder', contents, '-format', 'ULFO', '-ov', dmg])
    checked('codesign', ['--force', '--options', 'runtime', '--timestamp', '--sign', identity, dmg], 'dmg-sign.txt')
    verifySignature(dmg, 'dmg')
    notarize(dmg, dmg, 'dmg')
  } finally {
    rmSync(stage, { recursive: true, force: true })
  }
}
