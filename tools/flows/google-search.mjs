import { writeFile } from 'node:fs/promises'
import { pathToFileURL } from 'node:url'

export function address(value) {
  const text = String(value ?? '').trim()
  if (!text || /\s/.test(text)) throw new Error('url: enter a valid website address')
  const parsed = new URL(/^[a-z][a-z\d+.-]*:/i.test(text) ? text : 'https://' + text)
  if (!['http:', 'https:'].includes(parsed.protocol) || parsed.username || parsed.password) throw new Error('url: use an HTTP(S) address without credentials')
  return {
    host: parsed.hostname.toLowerCase().replace(/\.$/, '').replace(/^www\./, ''),
    port: parsed.port,
    path: parsed.pathname.replace(/\/+$/, '') || '/',
  }
}

export function initialize(keyword, url, page_limit = 20) {
  if (typeof keyword !== 'string' || !keyword.trim()) throw new Error('keyword: enter a Google search query')
  if (!Number.isInteger(page_limit) || page_limit < 1 || page_limit > 20) throw new Error('page_limit: enter a whole number from 1 to 20')
  try { return { keyword: keyword.trim(), target: address(url) } }
  catch (error) { throw new Error('url: ' + error.message.replace(/^url: /, '')) }
}

export function scoreLink(href, target) {
  try {
    const candidate = address(href)
    if (candidate.port !== target.port || (candidate.host !== target.host && !candidate.host.endsWith('.' + target.host))) return 0
    if (target.path !== '/' && candidate.path !== target.path && !candidate.path.startsWith(target.path + '/')) return 0
    return (candidate.host === target.host ? 100 : 50) + (candidate.path === target.path ? 20 : 0)
  } catch { return 0 }
}

export function visibleLink(link) {
  const style = getComputedStyle(link)
  const rect = link.getBoundingClientRect()
  for (let ancestor = link.parentElement; ancestor; ancestor = ancestor.parentElement) {
    const parent = getComputedStyle(ancestor)
    if (parent.display === 'none' || Number(parent.opacity || 1) === 0) return false
  }
  return !link.closest('[hidden], [inert], [aria-hidden="true"], [aria-disabled="true"]') &&
    style.display !== 'none' && !['hidden', 'collapse'].includes(style.visibility) && Number(style.opacity || 1) > 0 && rect.width > 0 && rect.height > 0
}

export function googleState(mode) {
  if (document.documentElement.hasAttribute('data-ombra-navigation')) return { ready: false, state: 'loading' }
  if (location.hostname === 'consent.google.com' || [...document.querySelectorAll('form[action*="consent.google"]')].some(visibleLink)) {
    return { ready: false, state: 'consent' }
  }
  const response = document.querySelector('[name="g-recaptcha-response"]')?.value?.trim()
  const captcha = [...document.querySelectorAll('#captcha-form, iframe[src*="/recaptcha/"]')].some(element => {
    if (!visibleLink(element)) return false
    if (element.tagName !== 'IFRAME') return !response
    const rect = element.getBoundingClientRect()
    if (rect.bottom <= 0 || rect.top >= innerHeight || rect.right <= 0 || rect.left >= innerWidth) return false
    try {
      const url = new URL(element.src, location.href)
      return url.pathname.endsWith('/bframe') || (!response && url.pathname.endsWith('/anchor') && url.searchParams.get('size') !== 'invisible')
    } catch { return false }
  })
  if (location.pathname.startsWith('/sorry') || captcha) return { ready: false, state: 'captcha' }
  if (document.readyState !== 'complete' || !/(^|\.)google\.com$/.test(location.hostname)) return { ready: false, state: 'loading' }
  if (mode === 'home') {
    const box = document.querySelector('textarea[name="q"], input[name="q"]')
    return { ready: Boolean(box && visibleLink(box)), state: box && visibleLink(box) ? 'home' : 'loading' }
  }
  const root = document.querySelector('#search, #rso, main, [role="main"]')
  const results = [...(root?.querySelectorAll('a[href]') ?? [])].some(link => link.querySelector('h3') && visibleLink(link))
  const empty = /did not match any documents|no results found|ничего не найдено|не найдено (?:ни одного )?документа/i.test(document.body?.innerText || document.body?.textContent || '')
  const ready = location.pathname === '/search' && (results || empty)
  return { ready, state: ready ? 'results' : 'loading' }
}

export function inspectSearch(target) {
  const state = googleState('results')
  if (!state.ready) return state
  const root = document.querySelector('#search, #rso, main, [role="main"]')
  const links = [...(root?.querySelectorAll('a[href]') ?? [])].filter(link => link.querySelector('h3') && visibleLink(link))
  const ranked = links.map(link => {
    try {
    let href = link.href
    const redirect = new URL(href, location.href)
    if (redirect.origin === location.origin && redirect.pathname === '/url') href = redirect.searchParams.get('q') || redirect.searchParams.get('url') || ''
    return { href, score: scoreLink(href, target) }
    } catch { return { href: '', score: 0 } }
  }).filter(link => link.score > 0).sort((a, b) => b.score - a.score)
  return { ...state, has_results: links.length > 0, matched_url: ranked[0]?.href ?? '' }
}

export function pollGoogle(mode, target) {
  return mode === 'results' ? inspectSearch(target) : googleState('home')
}

export function requireGoogleReady(google_gate) {
  if (!google_gate.ready) {
    throw new Error(`Google: ${google_gate.state === 'loading' ? 'page did not become ready' : 'CAPTCHA or consent is still pending'} after 180 seconds. Complete the check in the profile, then retry.`)
  }
  return true
}

export function prepareNext(page_index, page_limit = 20) {
  document.querySelectorAll('[data-ombra-next]').forEach(link => link.removeAttribute('data-ombra-next'))
  if (page_index + 1 >= page_limit) return false
  const current = new URL(location.href)
  const link = [...document.querySelectorAll('a#pnnext, a[rel="next"]')].find(link => {
    try {
    const next = new URL(link.href, location.href)
    return visibleLink(link) && next.origin === current.origin && next.pathname === '/search' &&
      next.searchParams.get('q') === current.searchParams.get('q') &&
      Number(next.searchParams.get('start')) > Number(current.searchParams.get('start') || 0)
    } catch { return false }
  })
  if (!link) return false
  link.setAttribute('data-ombra-next', '1')
  link.target = '_self'
  return true
}

export function chooseSafeLink() {
  document.querySelectorAll('[data-ombra-follow]').forEach(link => link.removeAttribute('data-ombra-follow'))
  const current = new URL(location.href)
  const host = current.hostname.replace(/^www\./, '').toLowerCase()
  const seen = new Set()
  const links = [...document.querySelectorAll('a[href]')].filter(link => {
    try {
    if (!visibleLink(link) || link.hasAttribute('download')) return false
    const destination = new URL(link.href, current.href)
    const action = decodeURIComponent(destination.pathname + destination.search).toLowerCase()
    if (destination.protocol !== 'https:' || destination.username || destination.password ||
      destination.hostname.replace(/^www\./, '').toLowerCase() !== host || destination.port !== current.port ||
      /(?:^|[./?&=_-])(?:log[-_]?out|logoff|sign[-_]?out|delete|remove|unsubscribe|checkout|cart|purchase|buy|download)(?:$|[./?&=_-])/.test(action) ||
      /\.(?:pdf|zip|7z|rar|tar|gz|bz2|xz|exe|dmg|pkg|msi|apk|iso|docx?|xlsx?|pptx?|csv|epub)$/i.test(decodeURIComponent(destination.pathname))) return false
    destination.hash = ''
    const here = new URL(current.href)
    here.hash = ''
    if (destination.href === here.href || seen.has(destination.href)) return false
    seen.add(destination.href)
    return true
    } catch { return false }
  })
  const link = links[Math.floor(Math.random() * links.length)]
  if (!link) return false
  link.setAttribute('data-ombra-follow', '1')
  link.target = '_self'
  return true
}

export async function waitReady() {
  const deadline = Date.now() + 15000
  let completeSince = 0
  while (Date.now() < deadline) {
    if (document.readyState === 'complete') {
      completeSince ||= Date.now()
      if (Date.now() - completeSince >= 500) return true
    } else completeSince = 0
    await new Promise(resolve => setTimeout(resolve, 100))
  }
  throw new Error('Page did not finish loading within 15 seconds')
}

export function prepareNavigation() {
  const root = document.documentElement
  const previous = location.href
  root.setAttribute('data-ombra-navigation', 'pending')
  const clear = () => { root.removeAttribute('data-ombra-navigation'); clearInterval(timer) }
  const timer = setInterval(() => { if (location.href !== previous) clear() }, 100)
  // A cached document may return with this marker after Go Back.
  window.addEventListener('pageshow', clear, { once: true })
  setTimeout(() => clearInterval(timer), 30000)
}

export async function navigationFinished(navigation_ready) {
  if (!navigation_ready) throw new Error('The click did not navigate within 30 seconds')
  return waitReady()
}

export async function settleScroll(position) {
  const deadline = Date.now() + 12000
  let previous = -1, stable = 0
  while (Date.now() < deadline) {
    const height = Math.max(document.documentElement.scrollHeight, document.body?.scrollHeight ?? 0)
    const goal = position === 'bottom' ? Math.max(0, height - innerHeight) : 0
    window.scrollTo({ top: goal, behavior: 'instant' })
    if (height === previous && Math.abs(scrollY - goal) <= 2) stable++
    else stable = 0
    if (stable >= 3) return true
    previous = height
    await new Promise(resolve => setTimeout(resolve, 250))
  }
  throw new Error('Scrolling did not settle within 12 seconds')
}

export function makeFlow(language = 'ru') {
  const en = language === 'en'
  const text = (ru, english) => en ? english : ru
  const node = (id, op, ru, english, config = {}, pos = [300, 0]) => ({ id, op, description: text(ru, english), config, pos })
  const edge = (from, to, port = 'next', to_port = 'in') => ({ from, to, port, to_port })
  const script = (fn, inject = [], save_to, helpers = []) => ({
    code: `(function(){${helpers.map(helper => helper.toString()).join('\n')}\nreturn (${fn.toString()})(${inject.join(', ')});})()`,
    inject, ...(save_to ? { save_to } : {}),
  })
  const scroll = (id, position) => ({
    ...node(id, 'control.group', position === 'bottom' ? 'Прокрутить до конца' : 'Вернуться к началу', position === 'bottom' ? 'Scroll to bottom' : 'Scroll to top', { name: position === 'bottom' ? text('Вниз', 'Bottom') : text('Вверх', 'Top') }),
    nodes: [
      node(id + '_scroll', 'web.scroll', 'Прокрутка', 'Scroll', { position, kind: 'smooth', step_px: 600, step_ms: 100 }),
      node(id + '_settle', 'web.js', 'Дождаться окончания прокрутки и догрузки', 'Wait for scrolling and lazy content', { code: `(${settleScroll.toString()})(${JSON.stringify(position)})`, inject: [] }, [300, 130]),
    ], edges: [edge(id + '_scroll', id + '_settle')],
  })
  const navigation = id => [
    node(id + '_wait', 'wait.element', 'Дождаться новой страницы', 'Wait for the new page', { target: { by: 'css', selector: 'html:not([data-ombra-navigation]) body' }, timeout_ms: 30000, save_to: 'navigation_ready' }),
    // Google can redirect when CAPTCHA finishes; its retry gate owns readiness and timeout.
    { ...node(id + '_ready', 'web.js', 'Дождаться полной загрузки', 'Wait for complete load', script(navigationFinished, ['navigation_ready'], undefined, [waitReady])),
      ...(['submit', 'next'].includes(id) ? { on_error: 'skip' } : {}) },
  ]
  const googleGate = (id, mode, save_to) => ({
    ...node(id, 'control.group', 'Ожидать готовности Google, CAPTCHA или согласия', 'Wait for Google readiness, CAPTCHA or consent', { name: text('Google: ожидание до 3 минут', 'Google: wait up to 3 minutes') }),
    nodes: [
      node(id + '_mode', 'proc.vars', 'Режим ожидания Google', 'Google wait mode', { vars: [{ name: 'google_mode', type: 'string', value: mode }] }),
      node(id + '_loop', 'control.for_times', 'Ждать до 180 секунд', 'Wait up to 180 seconds', { times: 180, index_to: 'google_wait_index' }),
      node(id + '_reset', 'proc.vars', 'Сбросить прошлый ответ перед опросом', 'Reset the previous response before polling', { vars: [{ name: 'google_gate', type: 'object', value: { ready: false, state: 'loading' } }] }),
      { ...node(id + '_sample', 'web.js', 'Проверить Google; при CAPTCHA ждать решения', 'Check Google; wait for CAPTCHA completion', script(pollGoogle, ['google_mode', 'target'], 'google_gate', [address, scoreLink, visibleLink, googleState, inspectSearch])), on_error: 'skip' },
      node(id + '_ready', 'proc.field', 'Готов ли Google?', 'Is Google ready?', { data: '${google_gate}', key: 'ready', save_to: 'google_ready' }),
      node(id + '_if', 'control.if', 'Продолжить после проверки?', 'Continue after the check?', { variable: 'google_ready', condition: 'eq', value: true }),
      node(id + '_break', 'control.break', 'Google готов: продолжить процесс', 'Google is ready: resume the workflow'),
      node(id + '_pause', 'wait.time', 'Ожидание CAPTCHA, согласия или загрузки Google', 'Waiting for CAPTCHA, consent or Google loading', { ms: 1000 }),
      node(id + '_verify', 'web.js', 'Проверить срок ожидания Google', 'Check the Google wait deadline', script(requireGoogleReady, ['google_gate'])),
      node(id + '_save', 'proc.vars', 'Сохранить готовый ответ Google', 'Save the ready Google response', { vars: [{ name: save_to, type: 'object', value: '${google_gate}' }] }),
    ].map((n, index) => ({ ...n, pos: [300, index * 130] })),
    edges: [edge(id + '_mode', id + '_loop'), edge(id + '_loop', id + '_reset', 'body'),
      edge(id + '_reset', id + '_sample'), edge(id + '_sample', id + '_ready'), edge(id + '_ready', id + '_if'),
      edge(id + '_if', id + '_break', 'true'), edge(id + '_if', id + '_pause', 'false'), edge(id + '_pause', id + '_loop', 'next', 'loop'),
      edge(id + '_loop', id + '_verify'), edge(id + '_verify', id + '_save')],
  })
  const nodes = [
    node('start', 'start', 'Начало', 'Start'),
    node('init', 'web.js', 'Проверить keyword, url и глубину поиска', 'Validate keyword, url and search depth', script(initialize, ['keyword', 'url', 'page_limit'], 'configuration', [address])),
    node('target', 'proc.field', 'Адрес для сопоставления', 'Target to match', { data: '${configuration}', key: 'target', save_to: 'target' }),
    node('keyword', 'proc.field', 'Поисковая фраза', 'Search query', { data: '${configuration}', key: 'keyword', save_to: 'keyword' }),
    node('new_tab', 'web.new_tab', 'Создать вкладку', 'Create a new tab'),
    node('google', 'web.goto', 'Открыть Google', 'Open Google', { url: 'https://www.google.com/', timeout_ms: 45000 }),
    googleGate('home_ready', 'home', 'google_home'),
    node('search_box', 'wait.element', 'Найти поле поиска', 'Find the search box', { target: { by: 'css', selector: 'textarea[name="q"], input[name="q"]' }, visible: true, timeout_ms: 15000 }),
    node('input', 'web.input', 'Ввести keyword', 'Type keyword', { target: { by: 'css', selector: 'textarea[name="q"], input[name="q"]' }, content: '${keyword}', clear: true, interval_ms: 50 }),
    node('submit_prepare', 'web.js', 'Подготовить ожидание выдачи', 'Prepare search navigation wait', script(prepareNavigation)),
    node('enter', 'key.press', 'Нажать Enter', 'Press Enter', { key: 'Enter' }),
    ...navigation('submit'),
    node('pages', 'control.for_times', 'Проверить страницы до лимита page_limit', 'Check pages up to page_limit', { times: '${page_limit}', index_to: 'page_index' }),
    googleGate('inspect', 'results', 'search_result'),
    node('matched_url', 'proc.field', 'Совпадающий адрес', 'Matching address', { data: '${search_result}', key: 'matched_url', save_to: 'matched_url' }),
    node('has_results', 'proc.field', 'Есть ли результаты', 'Are there any results', { data: '${search_result}', key: 'has_results', save_to: 'has_results' }),
    node('any_results', 'control.if', 'Есть результаты?', 'Any results?', { variable: 'has_results', condition: 'eq', value: true }),
    node('close_empty', 'web.close_tab', 'Нет результатов: закрыть созданную вкладку', 'No results: close the workflow tab'),
    node('match', 'control.if', 'Найдено совпадение?', 'Found a match?', { variable: 'matched_url', condition: 'ne', value: '' }),
    scroll('search_bottom', 'bottom'),
    node('prepare_next', 'web.js', 'Найти следующую страницу и проверить лимит', 'Find the next page and check the limit', script(prepareNext, ['page_index', 'page_limit'], 'more', [visibleLink])),
    node('more', 'control.if', 'Есть следующая страница?', 'Is there a next page?', { variable: 'more', condition: 'eq', value: true }),
    node('next_prepare', 'web.js', 'Подготовить ожидание следующей страницы', 'Prepare next-page wait', script(prepareNavigation)),
    node('next_click', 'web.click', 'Нажать «Следующая»', 'Click Next', { target: { by: 'css', selector: 'a[data-ombra-next="1"]' } }),
    ...navigation('next'),
    node('stop_search', 'control.break', 'Закончить поиск', 'Finish searching'),
    node('found', 'control.if', 'Открыть найденный адрес?', 'Open the matching address?', { variable: 'matched_url', condition: 'ne', value: '' }),
    node('visit', 'web.goto', 'Открыть совпадающую ссылку', 'Open the matching link', { url: '${matched_url}', timeout_ms: 45000 }),
    node('visit_ready', 'web.js', 'Дождаться загрузки сайта', 'Wait for the site to load', script(waitReady)),
    scroll('visit_bottom', 'bottom'), scroll('visit_top', 'top'),
    node('pick_link', 'web.js', 'Выбрать случайную HTTPS-ссылку этого сайта', 'Choose a random same-site HTTPS link', script(chooseSafeLink, [], 'has_safe_link', [visibleLink])),
    node('safe_link', 'control.if', 'Есть подходящая ссылка?', 'Is there an eligible link?', { variable: 'has_safe_link', condition: 'eq', value: true }),
    node('follow_prepare', 'web.js', 'Подготовить ожидание перехода', 'Prepare link navigation wait', script(prepareNavigation)),
    node('follow_click', 'web.click', 'Нажать выбранную ссылку', 'Click the selected link', { target: { by: 'css', selector: 'a[data-ombra-follow="1"]' } }),
    ...navigation('follow'),
    scroll('follow_bottom', 'bottom'), scroll('follow_top', 'top'),
    node('back', 'web.back', 'Вернуться на целевую страницу', 'Return to the target page'),
    node('back_ready', 'web.js', 'Дождаться завершения возврата', 'Wait for the return to finish', script(waitReady)),
    node('export', 'proc.vars', 'Завершить и показать найденный адрес', 'Finish and show the matched address', { mode: 'export', names: ['matched_url'] }),
  ].map((n, index) => ({ ...n, pos: [300, index * 130] }))
  const chain = (...ids) => ids.slice(1).map((id, i) => edge(ids[i], id))
  return {
    format: 'ombra-flow/1', name: text('Google — поиск и переход на сайт', 'Google — search and visit'), group: text('Поиск и переходы', 'Search and navigation'),
    remark: text('Поиск по keyword, сопоставление url и обход выдачи на глубину page_limit (1–20 страниц, по умолчанию 20). URL сравниваются по домену и пути без протокола, www, параметров и фрагмента. После совпадения: прокрутка вниз/вверх, случайная видимая HTTPS-ссылка того же сайта, повторная прокрутка и возврат. Если результатов нет, новая вкладка закрывается. Если нет совпадения или подходящих HTTPS-ссылок, процесс завершается. При CAPTCHA или согласии Google процесс ждёт до трёх минут и продолжает после проверки. Её может завершить обработчик капч профиля или пользователь. Браузер и прочие вкладки остаются открыты.', 'Search for keyword, match url and check page_limit result pages (1–20, default 20). Matching compares domain and path, ignoring protocol, www, query and fragment. After a match: scroll down/up, click a random visible same-site HTTPS link, scroll again and go back. Empty results close the new tab. No match or eligible HTTPS links ends the workflow. Google CAPTCHA and consent pause the workflow for up to three minutes, then it resumes after the profile handler or user completes the check. The browser and other tabs remain open.'),
    variables: [
      { name: 'keyword', type: 'string', default: '', description: text('Ключевая фраза для поиска Google', 'Keyword for Google search') },
      { name: 'url', type: 'string', default: '', description: text('Целевой домен или URL для нечёткого сопоставления', 'Target domain or URL for fuzzy matching') },
      { name: 'page_limit', type: 'number', default: 20, description: text('Глубина поиска: от 1 до 20 страниц выдачи, по умолчанию 20', 'Search depth: 1 to 20 result pages, default 20') },
    ],
    settings: { after_tabs: 'keep', after_browser: 'keep', timeout_s: 1800 }, nodes,
    edges: [
      ...chain('start', 'init', 'target', 'keyword', 'new_tab', 'google', 'home_ready', 'search_box', 'input', 'submit_prepare', 'enter', 'submit_wait', 'submit_ready', 'pages'),
      edge('pages', 'inspect', 'body'), ...chain('inspect', 'matched_url', 'has_results', 'any_results'),
      edge('any_results', 'close_empty', 'false'), edge('close_empty', 'stop_search'), edge('any_results', 'match', 'true'),
      edge('match', 'stop_search', 'true'), edge('match', 'search_bottom', 'false'), ...chain('search_bottom', 'prepare_next', 'more'),
      edge('more', 'stop_search', 'false'), edge('more', 'next_prepare', 'true'), ...chain('next_prepare', 'next_click', 'next_wait', 'next_ready'),
      edge('next_ready', 'pages', 'next', 'loop'), edge('pages', 'found'), edge('found', 'export', 'false'), edge('found', 'visit', 'true'),
      ...chain('visit', 'visit_ready', 'visit_bottom', 'visit_top', 'pick_link', 'safe_link'),
      edge('safe_link', 'export', 'false'), edge('safe_link', 'follow_prepare', 'true'),
      ...chain('follow_prepare', 'follow_click', 'follow_wait', 'follow_ready', 'follow_bottom', 'follow_top', 'back', 'back_ready', 'export'),
    ],
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  for (const language of ['ru', 'en']) {
    await writeFile(new URL(`./examples/google-search${language === 'en' ? '.en' : ''}.json`, import.meta.url), JSON.stringify(makeFlow(language), null, 2) + '\n')
  }
}
