import { readFileSync } from 'node:fs'
import { execFileSync } from 'node:child_process'
import { Script } from 'node:vm'
import { beforeEach, describe, expect, it } from 'vitest'
import { appendOffer, collectSearch, extractOffer, initialize, shouldContinue, summarize } from './cian.mjs'

const search = 'https://www.cian.ru/cat.php?deal_type=sale&offer_type=flat&region=1'
const url = 'https://www.cian.ru/sale/flat/123/'
const state = () => initialize(search, 2, 3, 2000, 'cian')
const offer = () => ({
  cianId: 123, category: 'flatSale', title: 'Квартира', description: 'Описание',
  bargainTerms: { price: 200000, currency: 'usd', prices: { rur: 15000000 } },
  geo: { address: [{ shortName: 'Москва' }, { fullName: 'Тестовая улица' }, { name: '1' }] },
  photos: [{ fullUrl: 'https://images.cdn-cian.ru/1.jpg' }, { fullUrl: 'https://images.cdn-cian.ru/1.jpg' }],
  phones: [{ countryCode: '7', number: '9001234567' }, { countryCode: '7', number: '900***4567' }],
  creationDate: '2026-10-20T13:41:39.727', publicationDate: 1792492899,
})
const pageState = (authenticated, value = offer()) => {
  window._cianConfig = { 'frontend-offer-card': [{ key: 'defaultState', value: { offerData: {
    offer: value, agent: { name: 'Продавец' }, user: { isAuthenticated: authenticated },
  } } }] }
}

beforeEach(() => {
  document.body.innerHTML = ''
  document.title = 'Циан'
  location.href = search
  delete window._cianConfig
})

describe('пример обхода Циан', () => {
  it('готовый JSON актуален, а скрипты допустимы как выражения Execute JavaScript', () => {
    const source = execFileSync(process.execPath, ['--input-type=module', '-e', "import {makeFlow} from './tools/flows/cian.mjs';process.stdout.write(JSON.stringify(makeFlow()))"], { encoding: 'utf8' })
    const generated = JSON.parse(readFileSync('tools/flows/examples/cian.json', 'utf8'))
    expect(generated).toEqual(JSON.parse(source))
    const checkScripts = nodes => {
      for (const node of nodes) {
        if (node.op === 'web.js') expect(() => new Script(node.config.code)).not.toThrow()
        if (node.nodes) checkScripts(node.nodes)
      }
    }
    checkScripts(generated.nodes)
  })

  it('ограничивает адрес, лимиты и имя выходного файла до открытия сайта', () => {
    expect(state().next_url).toBe(search)
    for (const bad of ['https://cian.ru.evil.test/cat.php', 'http://www.cian.ru/cat.php', 'https://www.cian.ru/sale/flat/1/', 'https://u:p@www.cian.ru/cat.php']) {
      expect(() => initialize(bad, 1, 3, 0, 'cian')).toThrow(/search_url/)
    }
    expect(() => initialize(search, 0, 3, 0, 'cian')).toThrow(/page_limit/)
    expect(() => initialize(search, 1, 3.5, 0, 'cian')).toThrow(/offer_limit/)
    expect(() => initialize(search, 1, 3, -1, 'cian')).toThrow(/pause_ms/)
    expect(() => initialize(search, 1, 3, 0, '../cian')).toThrow(/output_name/)
  })

  it('собирает карточки с лимитом, убирает отслеживание, рекламу и дубли между доменами', async () => {
    document.body.innerHTML = `<div data-name="CardComponent">
      <a href="${url}?context=one">1</a><a href="${url}?context=two">1</a>
      <a href="https://moskva.cian.ru/sale/flat/123/">1</a><a href="https://evil.test/sale/flat/9/">x</a>
      <a href="/sale/flat/124/">2</a><a href="/sale/flat/125/">3</a></div>
      <a href="/sale/flat/999/">Рекомендация вне выдачи</a>
      <div data-name="Pagination"><a href="/cat.php?deal_type=sale&region=1&p=2">2</a></div>`
    const crawl = await collectSearch(state(), 2)
    expect(crawl.queue).toEqual([url, 'https://www.cian.ru/sale/flat/124/'])
    expect(crawl.next_url).toBe('https://www.cian.ru/cat.php?deal_type=sale&region=1&p=2')
    expect(crawl.page_count).toBe(1)
    expect((await collectSearch({ ...crawl, rows: [{}, {}] }, 2)).queue).toEqual([])
  })

  it('останавливает обход в конце выдачи и при запросе проверки доступа', async () => {
    document.body.innerHTML = `<div data-testid="offer-card"><a href="${url}">1</a></div>`
    expect(shouldContinue(await collectSearch(state(), 3), 3)).toBe(false)
    expect(shouldContinue({ ...state(), rows: [{}, {}, {}] }, 3)).toBe(false)
    document.title = 'Проверка безопасности — CAPTCHA'
    await expect(collectSearch(state(), 3)).rejects.toThrow(/проверку доступа/)
    await expect(extractOffer(url, false)).rejects.toThrow(/проверку доступа/)
  })

  it('извлекает данные только текущей карточки и честно отмечает гостевую сессию', async () => {
    location.href = url
    pageState(false)
    const row = await extractOffer(url, false)
    expect(row).toMatchObject({ url, title: 'Квартира', price_rub: 15000000, address: 'Москва, Тестовая улица, 1',
      created_at: '2026-10-20T13:41:39.727', author: 'Продавец', phones: '+79001234567', auth_status: 'guest', photos: 'https://images.cdn-cian.ru/1.jpg' })
    await expect(extractOffer(url, true)).rejects.toThrow(/Вход в Циан не подтверждён/)
    pageState(true)
    expect((await extractOffer(url, true)).auth_status).toBe('authenticated')
    await expect(extractOffer('https://www.cian.ru/sale/flat/999/', false)).rejects.toThrow(/другую карточку/)
  })

  it('использует публичную разметку как запасной источник, оставляя неизвестные дату и контакты пустыми', async () => {
    document.body.innerHTML = '<button data-name="LoginButton">Войти</button>'
    const script = document.createElement('script')
    script.type = 'application/ld+json'
    script.textContent = JSON.stringify({ '@type': 'Product', sku: 123, name: 'Квартира', description: 'Описание', image: ['https://images.cdn-cian.ru/2.jpg'], offers: { price: 12345, priceCurrency: 'RUB' } })
    document.body.append(script)
    const row = await extractOffer(url, false)
    expect(row).toMatchObject({ title: 'Квартира', price_rub: 12345, phones: '', created_at: '', author: '', auth_status: 'guest' })
    const crawl = appendOffer({ ...state(), page_count: 1 }, row)
    expect(summarize(crawl, 'cian', 'profile', 7)).toMatchObject({ pages: 1, count: 1, authenticated: false, auth_statuses: ['guest'], file_name: 'cian-profile-7.xlsx', rows: [row] })
    expect(() => summarize(state(), 'cian', 'profile', '')).toThrow(/не дал объявлений/)
  })
})
