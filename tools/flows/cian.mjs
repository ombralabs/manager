import { writeFile } from 'node:fs/promises'
import { pathToFileURL } from 'node:url'

// Эти функции вкладываются в JSON целиком: импортированный процесс не зависит от файлов этого каталога.
export function initialize(search_url, page_limit, offer_limit, pause_ms, output_name) {
  const url = new URL(search_url)
  if (url.protocol !== 'https:' || !/(^|\.)cian\.ru$/i.test(url.hostname) || url.pathname !== '/cat.php' || url.username || url.password) {
    throw new Error('search_url: нужна HTTPS-ссылка на выдачу Циан /cat.php с фильтрами')
  }
  for (const [name, value, max] of [['page_limit', page_limit, 100], ['offer_limit', offer_limit, 10000], ['pause_ms', pause_ms, 60000]]) {
    if (!Number.isInteger(value) || value < (name === 'pause_ms' ? 0 : 1) || value > max) throw new Error(`${name}: неверное значение`)
  }
  if (!/^[\p{L}\p{N}_ -]{1,80}$/u.test(output_name)) throw new Error('output_name: имя файла без пути и расширения')
  return { next_url: url.href, page_count: 0, seen: [], rows: [] }
}

export async function collectSearch(crawl, offer_limit) {
  const blocked = () => /captcha|доступ ограничен|подтвердите.*человек|проверка безопасности/i.test(document.title + ' ' + location.pathname)
  let cards = []
  const until = Date.now() + 20000
  while (Date.now() < until) {
    if (blocked()) throw new Error('Циан запросил проверку доступа. Откройте профиль, пройдите проверку и повторите запуск.')
    cards = [...document.querySelectorAll('[data-name="CardComponent"]')]
    if (!cards.length) cards = [...document.querySelectorAll('[data-testid="offer-card"]')]
    if (cards.length) break
    if (document.querySelector('[data-name="EmptySearchResults"], [data-name="EmptyOffers"]')) break
    await new Promise(resolve => setTimeout(resolve, 250))
  }
  if (!cards.length) throw new Error('В выдаче нет карточек: проверьте фильтры, доступ к Циан и структуру страницы.')
  const seen = new Set(crawl.seen)
  const queue = []
  const remaining = offer_limit - crawl.rows.length
  for (const card of cards) {
    for (const link of card.querySelectorAll('a[href]')) {
      const url = new URL(link.getAttribute('href'), location.href)
      if (url.protocol !== 'https:' || !/(^|\.)cian\.ru$/i.test(url.hostname) || !/^\/(sale|rent)\/[a-z_]+\/\d+\/?$/i.test(url.pathname)) continue
      url.search = ''; url.hash = ''
      const canonical = url.href.replace(/\/?$/, '/')
      const key = url.pathname.replace(/\/?$/, '/')
      if (seen.has(key) || queue.length >= remaining) continue
      seen.add(key)
      queue.push(canonical)
    }
  }
  // Следуем только реально существующей ссылке пагинации, сохраняя фильтры поиска.
  const current = Number(new URL(location.href).searchParams.get('p') || 1)
  const pagination = document.querySelector('[data-name="Pagination"], [data-name="PaginationSection"]')
  const next = [...(pagination?.querySelectorAll('a[href]') ?? [])].map(link => new URL(link.getAttribute('href'), location.href))
    .find(url => /(^|\.)cian\.ru$/i.test(url.hostname) && url.protocol === 'https:' && url.pathname === '/cat.php' && Number(url.searchParams.get('p')) === current + 1)
  return { ...crawl, page_count: crawl.page_count + 1, seen: [...seen], queue, next_url: next?.href ?? null }
}

export async function extractOffer(offer_url, require_login) {
  let data, offer, product
  const until = Date.now() + 20000
  while (Date.now() < until) {
    if (/captcha|доступ ограничен|подтвердите.*человек|проверка безопасности/i.test(document.title + ' ' + location.pathname)) {
      throw new Error('Циан запросил проверку доступа. Пройдите её в профиле и повторите запуск.')
    }
    const entries = window._cianConfig?.['frontend-offer-card'] ?? []
    data = [...entries].reverse().find(entry => entry.key === 'defaultState')?.value?.offerData
    offer = data?.offer
    for (const script of document.querySelectorAll('script[type="application/ld+json"]')) {
      try {
        const parsed = JSON.parse(script.textContent)
        const items = Array.isArray(parsed) ? parsed : (parsed['@graph'] ?? [parsed])
        product = items.find(item => [].concat(item['@type'] ?? []).includes('Product')) ?? product
      } catch { /* У страницы может быть несколько независимых блоков разметки. */ }
    }
    if (offer || product) break
    await new Promise(resolve => setTimeout(resolve, 250))
  }
  if (!offer && !product) throw new Error('Карточка объявления не загрузилась или больше недоступна.')
  const expected = new URL(offer_url).pathname.match(/\/(\d+)\/?$/)?.[1]
  const actual = offer?.cianId ?? offer?.id ?? product?.sku
  if (actual && String(actual) !== expected) throw new Error('Циан открыл другую карточку: результат не сохранён.')
  const auth_status = typeof data?.user?.isAuthenticated === 'boolean'
    ? (data.user.isAuthenticated ? 'authenticated' : 'guest')
    : (document.querySelector('[data-name="LoginButton"]') ? 'guest' : 'unknown')
  if (require_login && auth_status !== 'authenticated') throw new Error('Вход в Циан не подтверждён. Войдите вручную в этом профиле и повторите запуск.')
  const text = selector => document.querySelector(selector)?.textContent.trim() ?? ''
  const quotedPrice = [].concat(product?.offers ?? [])[0]
  const terms = offer?.bargainTerms
  const price = terms?.prices?.rur ?? (['rur', 'rub'].includes(terms?.currency?.toLowerCase()) ? terms.price : null)
    ?? (quotedPrice?.priceCurrency === 'RUB' ? quotedPrice.price : null)
  const photos = (offer?.photos ?? [].concat(product?.image ?? [])).map(photo => typeof photo === 'string' ? photo : photo.fullUrl)
    .filter(url => typeof url === 'string' && /^https?:\/\//.test(url))
  // Только номера, уже отданные странице для этого объявления; скрытые контакты остаются пустыми.
  const phones = (offer?.phones ?? []).map(phone => {
    const number = String(phone.number ?? '')
    if (!/^[+\d\s()-]+$/.test(number)) return ''
    const full = (number.startsWith('+') ? number : String(phone.countryCode ?? '') + number).replace(/\D/g, '')
    return full.length >= 10 && full.length <= 15 ? '+' + full : ''
  }).filter(Boolean)
  return {
    url: offer_url,
    category: offer?.category ?? new URL(offer_url).pathname.split('/').slice(1, 3).join('/'),
    title: offer?.title || text('[data-name="OfferTitleNew"], [data-name="OfferTitle"], h1') || product?.name || '',
    price_rub: price === null || price === undefined || price === '' || !Number.isFinite(Number(price)) ? '' : Number(price),
    address: offer?.geo?.address?.map(item => item.shortName || item.fullName || item.name).filter(Boolean).join(', ')
      || document.querySelector('meta[property="og:description"]')?.content || '',
    created_at: offer?.creationDate ?? '',
    published_at: offer?.publicationDate ? new Date(offer.publicationDate * 1000).toISOString() : product?.datePublished ?? '',
    description: offer?.description ?? product?.description ?? text('[data-name="Description"]'),
    photos: [...new Set(photos)].join('\n'),
    author: data?.agent?.name || data?.company?.name || '',
    phones: [...new Set(phones)].join('; '),
    auth_status,
  }
}

export function appendOffer(crawl, offer) {
  return { ...crawl, rows: [...crawl.rows, offer] }
}

export function shouldContinue(crawl, offer_limit) {
  return Boolean(crawl.next_url) && crawl.rows.length < offer_limit
}

export function summarize(crawl, output_name, acc_id, task_id) {
  if (!crawl.rows.length) throw new Error('Обход не дал объявлений: проверьте выдачу и фильтры.')
  return {
    pages: crawl.page_count, count: crawl.rows.length,
    authenticated: crawl.rows.every(row => row.auth_status === 'authenticated'),
    auth_statuses: [...new Set(crawl.rows.map(row => row.auth_status))],
    file_name: `${output_name}-${acc_id}${task_id ? '-' + task_id : ''}.xlsx`,
    rows: crawl.rows,
  }
}

export const columns = ['url', 'category', 'title', 'price_rub', 'address', 'created_at', 'published_at', 'description', 'photos', 'author', 'phones', 'auth_status']

export function makeFlow() {
  const node = (id, op, description, config = {}, pos = [300, 0]) => ({ id, op, description, config, pos })
  const edge = (from, to, port = 'next', to_port = 'in') => ({ from, to, port, to_port })
  const script = (fn, inject, save_to) => ({ code: `(${fn.toString()})(${inject.join(', ')});`, inject, save_to })
  const fields = columns.map((name, i) => node(`field_${name}`, 'proc.field', name, { data: '${offer}', key: name, save_to: name }, [300, i * 130]))
  const save = node('save_xlsx', 'data.save_excel', 'Записать строку Excel', { file_name: '${output_name}-${acc_id}', columns }, [300, columns.length * 130])
  const saveGroup = {
    ...node('save_row', 'control.group', 'Записать объявление в таблицу', { name: 'Сохранение строки' }, [300, 1170]),
    nodes: [...fields, save], edges: [...fields.slice(1).map((n, i) => edge(fields[i].id, n.id)), edge(fields.at(-1).id, save.id)],
  }
  const pageNodes = [
    node('page_url', 'proc.field', 'Адрес страницы выдачи', { data: '${crawl}', key: 'next_url', save_to: 'current_search_url' }),
    node('search_open', 'web.goto', 'Открыть выдачу Циан', { url: '${current_search_url}', timeout_ms: 45000 }, [300, 130]),
    node('search_pause', 'wait.time', 'Дать странице загрузиться', { ms: '${pause_ms}' }, [300, 260]),
    node('search_links', 'web.js', 'Собрать уникальные ссылки и найти следующую страницу', script(collectSearch, ['crawl', 'offer_limit'], 'crawl'), [300, 390]),
    node('queue', 'proc.field', 'Ссылки этой страницы', { data: '${crawl}', key: 'queue', save_to: 'offer_urls' }, [300, 520]),
    node('offers_loop', 'control.for_data', 'Обойти карточки', { data: '${offer_urls}', item_to: 'offer_url', index_to: 'offer_index' }, [300, 650]),
    node('offer_open', 'web.goto', 'Открыть объявление', { url: '${offer_url}', timeout_ms: 45000 }, [300, 780]),
    node('offer_pause', 'wait.time', 'Пауза между карточками', { ms: '${pause_ms}' }, [300, 910]),
    node('offer_read', 'web.js', 'Поля объявления и статус входа в аккаунт', script(extractOffer, ['offer_url', 'require_login'], 'offer'), [300, 1040]),
    saveGroup,
    node('remember', 'web.js', 'Добавить объявление в результат', script(appendOffer, ['crawl', 'offer'], 'crawl'), [300, 1300]),
    node('continue', 'web.js', 'Есть следующая страница и не достигнут лимит?', script(shouldContinue, ['crawl', 'offer_limit'], 'crawl_more'), [650, 780]),
  ]
  return {
    format: 'ombra-flow/1', name: 'Циан — обход выдачи и карточек', group: 'Реальные сценарии',
    remark: 'По образцу https://a-parser.com/resources/452/. Укажите search_url с фильтрами. По умолчанию одна страница и три объявления. Результат — XLSX и переменные summary/offers. Для проверки входа сначала войдите в Циан вручную в выбранном профиле, затем включите require_login. Обычный запуск с require_login=false допускает гостевую сессию. Контакты заполняются только если сайт уже отдал их странице. Проверка доступа или недоступная карточка останавливает процесс; записанные строки Excel сохраняются.',
    variables: [
      { name: 'search_url', type: 'string', default: 'https://www.cian.ru/cat.php?deal_type=sale&engine_version=2&offer_type=flat&region=-1', description: 'Ссылка на выдачу Циан со своими фильтрами' },
      { name: 'page_limit', type: 'number', default: 1, description: 'Максимум страниц выдачи' },
      { name: 'offer_limit', type: 'number', default: 3, description: 'Максимум объявлений за весь запуск' },
      { name: 'pause_ms', type: 'number', default: 2000, description: 'Пауза после открытия страницы, миллисекунды' },
      { name: 'output_name', type: 'string', default: 'cian', description: 'Имя таблицы без пути и расширения' },
      { name: 'require_login', type: 'boolean', default: false, description: 'Остановить процесс, если вход в аккаунт Циан не подтверждён' },
    ],
    settings: { after_tabs: 'keep', after_browser: 'quit', timeout_s: 1800 },
    nodes: [
      node('start', 'start', 'Параметры обхода'),
      node('init', 'web.js', 'Проверить параметры', script(initialize, ['search_url', 'page_limit', 'offer_limit', 'pause_ms', 'output_name'], 'crawl'), [300, 130]),
      node('pages_loop', 'control.for_times', 'Обойти страницы выдачи', { times: '${page_limit}', index_to: 'page_index' }, [300, 260]),
      { ...node('page', 'control.group', 'Выдача → карточки → Excel', { name: 'Обход страницы' }, [300, 390]), nodes: pageNodes,
        edges: [...pageNodes.slice(1, 6).map((n, i) => edge(pageNodes[i].id, n.id)), edge('offers_loop', 'offer_open', 'body'),
          ...pageNodes.slice(7, 11).map((n, i) => edge(pageNodes[i + 6].id, n.id)), edge('remember', 'offers_loop', 'next', 'loop'), edge('offers_loop', 'continue')],
      },
      node('more', 'control.if', 'Продолжить обход?', { variable: 'crawl_more', condition: 'eq', value: true }, [300, 520]),
      node('next_pause', 'wait.time', 'Пауза перед следующей страницей', { ms: '${pause_ms}' }, [460, 650]),
      node('finish', 'control.break', 'Выдача закончилась или достигнут лимит', {}, [140, 650]),
      node('summary', 'web.js', 'Итог обхода и проверки сессии', script(summarize, ['crawl', 'output_name', 'acc_id', 'task_id'], 'summary'), [760, 390]),
      node('result_rows', 'proc.field', 'Все собранные объявления', { data: '${summary}', key: 'rows', save_to: 'offers' }, [760, 520]),
      node('export', 'proc.vars', 'Показать результат запуска', { mode: 'export', names: ['summary', 'offers'] }, [760, 650]),
    ],
    edges: [edge('start', 'init'), edge('init', 'pages_loop'), edge('pages_loop', 'page', 'body'), edge('page', 'more'),
      edge('more', 'next_pause', 'true'), edge('next_pause', 'pages_loop', 'next', 'loop'), edge('more', 'finish', 'false'),
      edge('pages_loop', 'summary'), edge('summary', 'result_rows'), edge('result_rows', 'export')],
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  await writeFile(new URL('./examples/cian.json', import.meta.url), JSON.stringify(makeFlow(), null, 2) + '\n')
  console.log('tools/flows/examples/cian.json')
}
