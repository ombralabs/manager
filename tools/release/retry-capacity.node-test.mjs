import assert from 'node:assert/strict'
import test from 'node:test'
import { planRetry } from './retry-capacity.mjs'

function fixture() {
  return {
    repository: 'ombralabs/desktop',
    run: { path: '.github/workflows/desktop-builds.yml', repository: { full_name: 'ombralabs/desktop' }, head_repository: { full_name: 'ombralabs/desktop' }, event: 'push', status: 'completed', conclusion: 'failure', run_attempt: 1, head_sha: 'current' },
    jobs: [{ id: 1, name: 'frontend', conclusion: 'success' }, { id: 2, name: 'darwin-arm64', conclusion: 'cancelled', runner_id: 0, runner_name: '', steps: [], labels: ['macos-15'] }],
    annotations: [{ message: 'The job was not acquired by Runner of type hosted even after multiple attempts' }],
    refSha: 'current', eventAttempt: 1,
  }
}

test('retries only a proven hosted ARM64 allocation failure', () => {
  assert.equal(planRetry(fixture()).jobId, 2)
})

for (const [name, change] of [
  ['manual cancellation without allocation error', f => { f.annotations = [{ message: 'Due to capacity constraints, jobs targeting macOS arm64 runners may experience longer queue times.' }] }],
  ['a job that already started', f => { f.jobs[1].steps = [{ name: 'Build installers', conclusion: 'failure' }] }],
  ['an allocated runner', f => { f.jobs[1].runner_id = 123 }],
  ['a self-hosted runner', f => { f.jobs[1].labels.push('self-hosted') }],
  ['another platform fails', f => { f.jobs.push({ name: 'linux-x64', conclusion: 'failure' }) }],
  ['the run was successful', f => { f.run.conclusion = 'success' }],
  ['the run is already running', f => { f.run.status = 'in_progress' }],
  ['the retry budget is exhausted', f => { f.run.run_attempt = 3; f.eventAttempt = 3 }],
  ['a delayed event from an older attempt', f => { f.run.run_attempt = 2 }],
  ['a newer commit on the branch', f => { f.refSha = 'newer' }],
  ['foreign source repository', f => { f.run.head_repository.full_name = 'someone/desktop' }],
  ['an unrelated workflow', f => { f.run.path = '.github/workflows/other.yml' }],
]) {
  test(`does not retry ${name}`, () => {
    const f = fixture(); change(f)
    assert.equal(planRetry(f).jobId, undefined)
  })
}

test('an explicit manual request can retry an older commit while respecting the budget', () => {
  const f = fixture(); f.manual = true; f.refSha = 'newer'
  assert.equal(planRetry(f).jobId, 2)
  f.run.run_attempt = 3; f.eventAttempt = 3
  assert.equal(planRetry(f).jobId, undefined)
})
