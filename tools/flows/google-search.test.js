import { readFileSync } from 'node:fs'
import { execFileSync } from 'node:child_process'
import { Script } from 'node:vm'
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest'
import { initialize, scoreLink, inspectSearch, googleState, requireGoogleReady, prepareNext, chooseSafeLink, prepareNavigation, navigationFinished, waitReady, settleScroll, makeFlow } from './google-search.mjs'

const visible = () => {
  for (const a of document.querySelectorAll('a')) {
    a.getBoundingClientRect = () => ({ width: 80, height: 24, top: 0, left: 0, right: 80, bottom: 24 })
  }
}
const results = html => {
  document.body.innerHTML = `<main id="search">${html}</main>`
  visible()
}
beforeAll(() => { window.happyDOM.settings.navigation.disableChildFrameNavigation = true })
beforeEach(() => {
  document.body.innerHTML = ''
  location.href = 'https://www.google.com/search?q=sample'
  vi.restoreAllMocks()
  vi.spyOn(document, 'readyState', 'get').mockReturnValue('complete')
  document.documentElement.removeAttribute('data-ombra-navigation')
})
afterEach(() => { vi.useRealTimers() })

describe('Google search workflow', () => {
  it('requires the search globals and validates target and configured depth before browsing', () => {
    expect(initialize(' sample ', 'EXAMPLE.com/docs/').keyword).toBe('sample')
    for (const keyword of ['', '  ']) expect(() => initialize(keyword, 'example.com')).toThrow(/keyword/)
    for (const url of ['', 'javascript:alert(1)', 'https://u:p@example.com', 'not a url']) {
      expect(() => initialize('sample', url)).toThrow(/url/)
    }
    for (const depth of [0, -1, 1.5, 21, NaN, Infinity, '20']) {
      expect(() => initialize('sample', 'example.com', depth)).toThrow(/page_limit/)
    }
    for (const depth of [1, 7, 20]) expect(() => initialize('sample', 'example.com', depth)).not.toThrow()
  })

  it('ranks fuzzy host/path matches without accepting unrelated domains or path prefixes', () => {
    const target = initialize('sample', 'https://www.example.com/docs/?track=1#top').target
    expect(scoreLink('http://example.com/docs?x=2', target)).toBeGreaterThan(scoreLink('https://help.example.com/docs/chapter', target))
    expect(scoreLink('https://example.com/docs/chapter', target)).toBeGreaterThan(0)
    for (const bad of ['https://example.com.evil.test/docs', 'https://notexample.com/docs', 'https://example.com/docstring', 'https://other.test/?url=example.com/docs']) {
      expect(scoreLink(bad, target)).toBe(0)
    }
  })

  it('examines all result headings, unwraps Google redirects and chooses the best match', () => {
    results('<a href="https://other.test"><h3>Other</h3></a><a href="https://example.com/docs/more"><h3>Deeper</h3></a>' +
      '<a href="/url?q=https%3A%2F%2Fexample.com%2Fdocs%3Fx%3D1&sa=U"><h3>Exact</h3></a>')
    const found = inspectSearch(initialize('sample', 'example.com/docs').target)
    expect(found.has_results).toBe(true)
    expect(found.matched_url).toBe('https://example.com/docs?x=1')
  })

  it('distinguishes no results from consent, CAPTCHA and unreadable markup', () => {
    document.body.innerHTML = '<div id="topstuff">Your search did not match any documents.</div>'
    expect(inspectSearch(initialize('sample', 'example.com').target)).toMatchObject({ ready: true, has_results: false, matched_url: '' })
    document.body.innerHTML = '<form action="https://consent.google.com/save"></form>'
    visible()
    const form = document.querySelector('form')
    form.getBoundingClientRect = () => ({ width: 400, height: 300 })
    expect(inspectSearch(initialize('sample', 'example.com').target)).toMatchObject({ ready: false, state: 'consent' })
    document.body.innerHTML = '<form id="captcha-form"></form>'
    location.href = 'https://www.google.com/sorry/index'
    expect(inspectSearch(initialize('sample', 'example.com').target)).toMatchObject({ ready: false, state: 'captcha' })
    document.body.innerHTML = '<div>Unexpected page</div>'
    location.href = 'https://www.google.com/search?q=sample'
    expect(inspectSearch(initialize('sample', 'example.com').target)).toMatchObject({ ready: false, state: 'loading' })
  })

  it('resumes after a CAPTCHA redirect and ignores hidden reCAPTCHA frames on valid results', () => {
    const target = initialize('sample', 'example.com').target
    location.href = 'https://www.google.com/sorry/index'
    document.body.innerHTML = '<form id="captcha-form"><iframe src="https://www.google.com/recaptcha/api2/anchor"></iframe></form>'
    expect(inspectSearch(target)).toMatchObject({ ready: false, state: 'captcha' })
    location.href = 'https://www.google.com/search?q=sample'
    results('<a href="https://example.com"><h3>Matched</h3></a><iframe style="display:none" src="https://www.google.com/recaptcha/api2/anchor"></iframe>')
    expect(inspectSearch(target)).toMatchObject({ ready: true, has_results: true, matched_url: 'https://example.com/' })
    expect(requireGoogleReady(inspectSearch(target))).toBe(true)
  })

  it('waits for real consent/challenges and reports a timeout only after the graph wait', () => {
    location.href = 'https://consent.google.com/m'
    expect(googleState('home')).toEqual({ ready: false, state: 'consent' })
    expect(() => requireGoogleReady({ ready: false, state: 'captcha' })).toThrow(/180 seconds/)
    expect(() => requireGoogleReady({ ready: false, state: 'loading' })).toThrow(/180 seconds/)
    location.href = 'https://www.google.com/'
    document.body.innerHTML = '<input name="q">'
    document.querySelector('input').getBoundingClientRect = () => ({ width: 200, height: 44 })
    expect(googleState('home')).toEqual({ ready: true, state: 'home' })
  })

  it('distinguishes an active challenge from invisible, solved and service frames', () => {
    const target = initialize('sample', 'example.com').target
    results('<a href="https://example.com"><h3>Matched</h3></a><iframe src="https://www.google.com/recaptcha/api2/anchor?size=invisible"></iframe>')
    const frame = document.querySelector('iframe')
    frame.getBoundingClientRect = () => ({ width: 300, height: 80, top: 10, left: 10, bottom: 90, right: 310 })
    expect(inspectSearch(target).ready).toBe(true)
    frame.src = 'https://www.google.com/recaptcha/api2/aframe'
    expect(inspectSearch(target).ready).toBe(true)
    frame.src = 'https://www.google.com/recaptcha/api2/anchor?size=normal'
    expect(inspectSearch(target)).toEqual({ ready: false, state: 'captcha' })
    document.body.insertAdjacentHTML('beforeend', '<textarea name="g-recaptcha-response">solved-token</textarea>')
    expect(inspectSearch(target).ready).toBe(true)
    frame.src = 'https://www.google.com/recaptcha/api2/bframe'
    expect(inspectSearch(target)).toEqual({ ready: false, state: 'captcha' })
  })

  it('ignores a challenge frame hidden by an ancestor and waits during a pending navigation', () => {
    const target = initialize('sample', 'example.com').target
    results('<a href="https://example.com"><h3>Matched</h3></a><div style="opacity:0"><iframe src="https://www.google.com/recaptcha/api2/bframe"></iframe></div>')
    document.querySelector('iframe').getBoundingClientRect = () => ({ width: 300, height: 80, top: 10, left: 10, bottom: 90, right: 310 })
    expect(inspectSearch(target)).toMatchObject({ ready: true, matched_url: 'https://example.com/' })
    document.documentElement.setAttribute('data-ombra-navigation', 'pending')
    expect(inspectSearch(target)).toEqual({ ready: false, state: 'loading' })
  })

  it('only follows forward Google pagination for the same query and configured depth', () => {
    for (const [href, expected] of [
      ['/search?q=sample&start=10', true], ['/search?q=sample&start=0', false],
      ['/search?q=other&start=10', false], ['https://evil.test/search?q=sample&start=10', false],
    ]) {
      document.body.innerHTML = `<a id="pnnext" href="${href}">Next</a>`
      visible()
      expect(prepareNext(0, 20)).toBe(expected)
    }
    document.body.innerHTML = '<a id="pnnext" href="/search?q=sample&start=50">Next</a>'
    visible()
    expect(prepareNext(4, 20)).toBe(true)
    expect(prepareNext(4, 5)).toBe(false)
    expect(document.querySelector('[data-ombra-next]')).toBeNull()
    expect(prepareNext(18, 20)).toBe(true)
    expect(prepareNext(19, 20)).toBe(false)
    expect(prepareNext(0, 1)).toBe(false)
    expect(document.querySelector('[data-ombra-next]')).toBeNull()
  })

  it('randomly chooses a visible same-site HTTPS link and makes it stay in the current tab', () => {
    location.href = 'https://example.com/landing'
    document.body.innerHTML = `
      <a href="https://example.com/one" target="_blank">One</a><a href="/two">Two</a>
      <a href="http://example.com/insecure">HTTP</a><a href="https://external.test">External</a>
      <a href="#section">Anchor</a><a href="/logout">Logout</a><a href="/delete?id=1">Delete</a>
      <a href="/bad%zz">Malformed</a><a href="http://[">Malformed URL</a>
      <a href="/file" download>Download</a><a href="/hidden" hidden>Hidden</a><a href="/disabled" aria-disabled="true">Disabled</a>`
    visible()
    vi.spyOn(Math, 'random').mockReturnValue(0)
    expect(chooseSafeLink()).toBe(true)
    expect(document.querySelector('[data-ombra-follow]')?.getAttribute('href')).toBe('https://example.com/one')
    expect(document.querySelector('[data-ombra-follow]')?.target).toBe('_self')
    vi.spyOn(Math, 'random').mockReturnValue(0.99)
    expect(chooseSafeLink()).toBe(true)
    expect(document.querySelector('[data-ombra-follow]')?.getAttribute('href')).toBe('/two')
    document.body.innerHTML = '<a href="mailto:user@example.com">Mail</a>'
    expect(chooseSafeLink()).toBe(false)
  })

  it('waits for actual navigation and stable document completion; times out explicitly', async () => {
    vi.useFakeTimers()
    prepareNavigation()
    expect(document.documentElement.hasAttribute('data-ombra-navigation')).toBe(true)
    await vi.advanceTimersByTimeAsync(200)
    expect(document.documentElement.hasAttribute('data-ombra-navigation')).toBe(true)
    history.replaceState({}, '', '/search?q=sample&start=10')
    await vi.advanceTimersByTimeAsync(100)
    expect(document.documentElement.hasAttribute('data-ombra-navigation')).toBe(false)
    await expect(navigationFinished(false)).rejects.toThrow(/did not navigate/)
    const state = vi.spyOn(document, 'readyState', 'get').mockReturnValue('complete')
    const ready = waitReady()
    await vi.advanceTimersByTimeAsync(600)
    await expect(ready).resolves.toBe(true)
    state.mockReturnValue('loading')
    const failure = expect(waitReady()).rejects.toThrow(/finish loading/)
    await vi.advanceTimersByTimeAsync(15100)
    await failure
  })

  it.each(['/logout.php', '/delete.aspx?id=1', '/sign_out.php', '/files/client.zip', '/docs/manual.pdf', '/exports/report.xlsx'])(
    'does not randomly click an action filename or downloadable file: %s', (href) => {
      location.href = 'https://example.com/landing'
      document.body.innerHTML = `<a href="${href}">Link</a>`
      visible()
      expect(chooseSafeLink()).toBe(false)
      expect(document.querySelector('[data-ombra-follow]')).toBeNull()
    },
  )

  it('waits for scroll completion and follows content added at the bottom', async () => {
    vi.useFakeTimers()
    let height = 2000
    let position = 0
    vi.spyOn(document.documentElement, 'scrollHeight', 'get').mockImplementation(() => height)
    vi.spyOn(window, 'scrollY', 'get').mockImplementation(() => position)
    vi.spyOn(window, 'scrollTo').mockImplementation(({ top }) => { position = top })
    const bottom = settleScroll('bottom')
    await vi.advanceTimersByTimeAsync(250)
    height = 3000
    await vi.advanceTimersByTimeAsync(1500)
    await expect(bottom).resolves.toBe(true)
    expect(scrollY).toBe(3000 - innerHeight)
    const top = settleScroll('top')
    await vi.advanceTimersByTimeAsync(1000)
    await expect(top).resolves.toBe(true)
    expect(scrollY).toBe(0)
  })

  it('ships RU/EN graphs with configurable 20-page default and native browsing operations', () => {
    for (const language of ['ru', 'en']) {
      const flow = makeFlow(language)
      const file = `tools/flows/examples/google-search${language === 'en' ? '.en' : ''}.json`
      const generated = execFileSync(process.execPath, ['--input-type=module', '-e', `import {makeFlow} from './tools/flows/google-search.mjs';process.stdout.write(JSON.stringify(makeFlow('${language}')))`], { encoding: 'utf8' })
      expect(JSON.parse(readFileSync(file, 'utf8'))).toEqual(JSON.parse(generated))
      expect(flow.variables.map(v => v.name)).toEqual(['keyword', 'url', 'page_limit'])
      expect(flow.variables.find(v => v.name === 'page_limit')).toMatchObject({ type: 'number', default: 20 })
      expect(flow.nodes.find(n => n.id === 'pages').config.times).toBe('${page_limit}')
      const walk = nodes => nodes.flatMap(n => [n, ...walk(n.nodes ?? [])])
      const nodes = walk(flow.nodes)
      for (const op of ['web.new_tab', 'web.close_tab', 'web.input', 'key.press', 'web.click', 'web.scroll', 'web.back']) {
        expect(nodes.some(n => n.op === op), op).toBe(true)
      }
      for (const n of nodes.filter(n => n.op === 'web.js')) expect(() => new Script(n.config.code)).not.toThrow()
      expect(flow.settings.after_tabs).toBe('keep')
      expect(flow.settings.after_browser).toBe('keep')
    }
  })
})
