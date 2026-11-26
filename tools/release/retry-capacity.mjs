import { appendFileSync, readFileSync } from 'node:fs'
import { pathToFileURL } from 'node:url'

const workflow = '.github/workflows/desktop-builds.yml'
const capacityFailure = 'The job was not acquired by Runner of type hosted even after multiple attempts'

export function planRetry({ repository, run, jobs, annotations, refSha, eventAttempt, manual = false }) {
  const skip = reason => ({ reason })
  if (run.path !== workflow || run.repository?.full_name !== repository || run.head_repository?.full_name !== repository) {
    return skip('Only the desktop workflow in this repository is eligible')
  }
  if (!['push', 'workflow_dispatch'].includes(run.event)) return skip('Unsupported trigger')
  if (run.status !== 'completed' || !['failure', 'cancelled'].includes(run.conclusion)) return skip('Run does not need a retry')
  if (!Number.isInteger(run.run_attempt) || run.run_attempt >= 3) return skip('Three-attempt limit reached')
  if (eventAttempt !== undefined && eventAttempt !== run.run_attempt) return skip('A newer attempt already exists')
  if (!manual && run.head_sha !== refSha) return skip('A newer commit exists on this ref')
  const unsuccessful = jobs.filter(job => job.conclusion !== 'success')
  if (unsuccessful.length !== 1) return skip('Other unfinished or unsuccessful jobs require review')
  const job = unsuccessful[0]
  if (job.name !== 'darwin-arm64' || !['cancelled', 'failure'].includes(job.conclusion)) return skip('Not an ARM64 capacity cancellation')
  if (job.runner_id || job.runner_name || !Array.isArray(job.steps) || job.steps.length) return skip('The job already acquired a runner or started executing')
  if (!job.labels?.some(label => /^macos-/.test(label)) || job.labels.includes('self-hosted')) return skip('Not a GitHub-hosted macOS job')
  if (!annotations.some(annotation => annotation.message?.includes(capacityFailure))) return skip('No explicit runner-allocation failure annotation')
  return { jobId: job.id, reason: `Retry ARM64 allocation, attempt ${run.run_attempt + 1}/3` }
}

async function main() {
  const repository = process.env.GITHUB_REPOSITORY
  const token = process.env.GH_TOKEN
  const event = JSON.parse(readFileSync(process.env.GITHUB_EVENT_PATH, 'utf8'))
  const manual = process.env.GITHUB_EVENT_NAME === 'workflow_dispatch'
  const runId = manual ? event.inputs?.run_id : event.workflow_run?.id
  if (!repository || !token || !/^\d+$/.test(String(runId))) throw new Error('Repository, token and numeric run ID are required')
  const api = 'https://api.github.com'
  const prefix = `/repos/${repository}`
  async function request(path, method = 'GET') {
    const response = await fetch(api + path, {
      method, redirect: 'error', signal: AbortSignal.timeout(30000),
      headers: { Authorization: `Bearer ${token}`, Accept: 'application/vnd.github+json', 'X-GitHub-Api-Version': '2022-11-28' },
    })
    if (!response.ok) throw new Error(`GitHub ${method} ${path}: HTTP ${response.status}`)
    const text = await response.text()
    return text ? JSON.parse(text) : null
  }
  const run = await request(`${prefix}/actions/runs/${runId}`)
  const listed = await request(`${prefix}/actions/runs/${runId}/jobs?filter=latest&per_page=100`)
  if (listed.total_count > 100) throw new Error('Unexpected number of workflow jobs')
  const candidate = listed.jobs.find(job => job.name === 'darwin-arm64' && job.conclusion !== 'success')
  let annotations = []
  if (candidate) {
    const checkPrefix = `${api}${prefix}/check-runs/`
    const checkId = candidate.check_run_url?.slice(checkPrefix.length)
    if (!candidate.check_run_url?.startsWith(checkPrefix) || !/^\d+$/.test(checkId)) throw new Error('Unexpected check-run URL')
    annotations = await request(`${prefix}/check-runs/${checkId}/annotations?per_page=100`)
  }
  const refSha = manual ? run.head_sha : (await request(`${prefix}/commits/${encodeURIComponent(run.head_branch)}`)).sha
  const plan = planRetry({ repository, run, jobs: listed.jobs, annotations, refSha, eventAttempt: manual ? undefined : event.workflow_run.run_attempt, manual })
  console.log(plan.reason)
  if (plan.jobId) {
    // Ignore delayed/duplicate events if another actor has already rerun the job.
    const fresh = await request(`${prefix}/actions/runs/${runId}`)
    if (fresh.status !== 'completed' || fresh.run_attempt !== run.run_attempt) {
      console.log('Run changed while checking; no duplicate retry submitted')
      return
    }
    await request(`${prefix}/actions/jobs/${plan.jobId}/rerun`, 'POST')
    console.log(`Submitted job ${plan.jobId}; the original build retains all tests, packaging and artifact checks`)
  }
  if (process.env.GITHUB_STEP_SUMMARY) {
    appendFileSync(process.env.GITHUB_STEP_SUMMARY, `${plan.reason}. [Desktop run](https://github.com/${repository}/actions/runs/${runId}).\n`)
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) await main()
