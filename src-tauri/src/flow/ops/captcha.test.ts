import { afterEach, beforeAll, describe, expect, it, vi } from 'vitest'
import source from './captcha.js?raw'

// Скрипт страницы 2Captcha из `captcha.rs` — в DOM happy-dom: какой виджет найден и куда попадает токен.
type Page = (kind: string, sitekey: string, mark: string, token: string | null, widget: boolean | null) => unknown
const page = new Function(`return (${source})`)() as Page
const MARK = '_m1'
const find = (kind: string, sitekey = '') => page(kind, sitekey, MARK, null, null)
/** Внедрение после поиска: `widget` — поиск взял виджет из разметки. */
const inject = (kind: string, sitekey: string, token: string, widget: boolean) => page(kind, sitekey, MARK, token, widget)
const value = (selector: string) => (document.querySelector(selector) as HTMLTextAreaElement).value
const globals = window as unknown as Record<string, unknown>

beforeAll(() => {
  // Фреймы капчи в тесте не загружаются: сети нет.
  ;(globals.happyDOM as { settings: { navigation: { disableChildFrameNavigation: boolean } } }).settings.navigation.disableChildFrameNavigation = true
})

afterEach(() => {
  document.body.innerHTML = ''
  for (const name of ['grecaptcha', '___grecaptcha_cfg', 'onLogin', 'onNews', 'onSubmit']) delete globals[name]
})

describe('страница капчи 2Captcha', () => {
  it('два виджета reCAPTCHA и hCaptcha: токен — только в выбранный виджет и его обработчик', () => {
    document.body.innerHTML = `
      <form id="login"><div class="g-recaptcha" data-sitekey="K1" data-callback="onLogin"><textarea name="g-recaptcha-response"></textarea></div></form>
      <form id="news"><div class="g-recaptcha" data-sitekey="K2" data-callback="onNews"><textarea name="g-recaptcha-response"></textarea></div></form>
      <div class="h-captcha" data-sitekey="H1"><textarea name="h-captcha-response"></textarea><textarea name="g-recaptcha-response"></textarea></div>`
    globals.onLogin = vi.fn()
    globals.onNews = vi.fn()
    expect(find('recaptcha_v2')).toEqual({ key: 'K1', widget: true, enterprise: false, invisible: false })
    expect(find('recaptcha_v2', 'K2')).toEqual({ key: 'K2', widget: true, enterprise: false, invisible: false })
    expect(inject('recaptcha_v2', 'K2', 'TOKEN', true)).toEqual([1, 1, false])
    expect(value('#news textarea')).toBe('TOKEN')
    expect(value('#login textarea')).toBe('')
    expect(value('.h-captcha [name="g-recaptcha-response"]')).toBe('')
    expect(globals.onNews).toHaveBeenCalledWith('TOKEN')
    expect(globals.onLogin).not.toHaveBeenCalled()
    expect(find('hcaptcha')).toEqual({ key: 'H1', widget: true, enterprise: false, invisible: false })
    expect(inject('hcaptcha', 'H1', 'HT', true)).toEqual([2, 0, false])
    expect(value('.h-captcha [name="h-captcha-response"]')).toBe('HT')
    expect(value('.h-captcha [name="g-recaptcha-response"]')).toBe('HT')
    expect(value('#login textarea')).toBe('')
  })

  it('виджет — тот же узел, что взял поиск: сдвиг соседей не меняет цель, пропавший — токен никуда', () => {
    const widget = (id: string) => `<div id="${id}" class="g-recaptcha" data-sitekey="K"><textarea name="g-recaptcha-response"></textarea></div>`
    document.body.innerHTML = widget('a') + widget('b')
    expect(find('recaptcha_v2', 'K')).toEqual({ key: 'K', widget: true, enterprise: false, invisible: false })
    document.body.insertAdjacentHTML('afterbegin', widget('new'))
    expect(inject('recaptcha_v2', 'K', 'T', true)).toEqual([1, 0, false])
    expect([value('#new textarea'), value('#a textarea'), value('#b textarea')]).toEqual(['', 'T', ''])
    document.body.innerHTML = widget('a') + widget('b')
    find('recaptcha_v2', 'K')
    document.getElementById('a')!.remove()
    expect(inject('recaptcha_v2', 'K', 'T2', true)).toEqual([0, 0, true])
    expect(value('#b textarea')).toBe('')
  })

  it('виджеты, отрисованные скриптом: по фрейму; обработчик — только своего клиента ___grecaptcha_cfg', () => {
    const frame = (key: string) => `<div><div><iframe src="https://www.google.test/recaptcha/api2/anchor?k=${key}"></iframe></div><textarea name="g-recaptcha-response"></textarea></div>`
    document.body.innerHTML = `<div id="w1">${frame('E1')}</div><div id="w2">${frame('E2')}</div>`
    const [cb1, cb2] = [vi.fn(), vi.fn()]
    globals.___grecaptcha_cfg = {
      clients: {
        0: { id: 0, a: { b: { sitekey: 'E1', callback: cb1 } }, el: document.getElementById('w1') },
        1: { id: 1, a: { b: { sitekey: 'E2', callback: cb2 } }, el: document.getElementById('w2') },
      },
    }
    expect(find('recaptcha_v2')).toEqual({ key: 'E1', widget: true, enterprise: false, invisible: false })
    expect(find('recaptcha_v2', 'E2')).toEqual({ key: 'E2', widget: true, enterprise: false, invisible: false })
    expect(inject('recaptcha_v2', 'E2', 'T2', true)).toEqual([1, 1, false])
    expect([value('#w1 textarea'), value('#w2 textarea')]).toEqual(['', 'T2'])
    expect(cb2).toHaveBeenCalledWith('T2')
    expect(cb1).not.toHaveBeenCalled()
  })

  it('у выбранного виджета пропало поле ответа — соседняя капча не трогается', () => {
    const frame = (key: string) => `<div><div><iframe src="https://www.google.test/recaptcha/api2/anchor?k=${key}"></iframe></div><textarea name="g-recaptcha-response"></textarea></div>`
    document.body.innerHTML = `<div id="w1">${frame('K1')}</div><div id="w2">${frame('K2')}</div>`
    const [cb1, cb2] = [vi.fn(), vi.fn()]
    globals.___grecaptcha_cfg = {
      clients: {
        0: { id: 0, a: { b: { sitekey: 'K1', callback: cb1 } }, el: document.getElementById('w1') },
        1: { id: 1, a: { b: { sitekey: 'K2', callback: cb2 } }, el: document.getElementById('w2') },
      },
    }
    expect(find('recaptcha_v2', 'K1')).toEqual({ key: 'K1', widget: true, enterprise: false, invisible: false })
    document.querySelector('#w1 textarea')!.remove()
    expect(inject('recaptcha_v2', 'K1', 'PAID-K1-TOKEN', true)).toEqual([0, 1, false])
    expect(value('#w2 textarea')).toBe('')
    expect(cb2).not.toHaveBeenCalled()
    expect(cb1).toHaveBeenCalledWith('PAID-K1-TOKEN')
    // Размеченные контейнеры: поле одного пропало — в другой не пишется.
    document.body.innerHTML = `
      <div id="a" class="g-recaptcha" data-sitekey="A" data-callback="onLogin"></div>
      <div id="b" class="g-recaptcha" data-sitekey="B" data-callback="onNews"><textarea name="g-recaptcha-response"></textarea></div>`
    globals.onLogin = vi.fn()
    globals.onNews = vi.fn()
    find('recaptcha_v2', 'A')
    expect(inject('recaptcha_v2', 'A', 'TA', true)).toEqual([0, 1, false])
    expect(value('#b textarea')).toBe('')
    expect(globals.onNews).not.toHaveBeenCalled()
  })

  it('поле ответа другой формы без виджета — не поле этой капчи', () => {
    const field = '<textarea name="g-recaptcha-response"></textarea>'
    document.body.innerHTML = `
      <form id="a"><div class="g-recaptcha" data-sitekey="K">${field}</div></form>
      <form id="b">${field}</form>`
    find('recaptcha_v2', 'K')
    expect(inject('recaptcha_v2', 'K', 'T', true)).toEqual([1, 0, false])
    expect([value('#a textarea'), value('#b textarea')]).toEqual(['T', ''])
    // Виджет вне формы, общая обёртка без других виджетов.
    document.body.innerHTML = `<div id="app"><div id="w" class="g-recaptcha" data-sitekey="K">${field}</div><form id="b">${field}</form></div>`
    find('recaptcha_v2', 'K')
    expect(inject('recaptcha_v2', 'K', 'T2', true)).toEqual([1, 0, false])
    expect([value('#w textarea'), value('#b textarea')]).toEqual(['T2', ''])
    // Отрисован скриптом (фрейм), поле своё рядом с фреймом; поле чужой формы не трогается.
    document.body.innerHTML = `<div id="app"><div id="w"><div><iframe src="https://www.google.test/recaptcha/api2/anchor?k=F"></iframe></div>${field}</div><form id="b">${field}</form></div>`
    find('recaptcha_v2', 'F')
    expect(inject('recaptcha_v2', 'F', 'T3', true)).toEqual([1, 0, false])
    expect([value('#w textarea'), value('#b textarea')]).toEqual(['T3', ''])
  })

  it('поле чужой формы не берётся никогда: у виджета нет своего поля или оно пропало за время решения', () => {
    const field = '<textarea name="g-recaptcha-response"></textarea>'
    // (а) общая обёртка: виджет без своего поля и соседняя форма с полем ответа.
    document.body.innerHTML = `<div id="app"><div id="w" class="g-recaptcha" data-sitekey="K" data-callback="onLogin"></div><form id="b">${field}</form></div>`
    globals.onLogin = vi.fn()
    find('recaptcha_v2', 'K')
    expect(inject('recaptcha_v2', 'K', 'T', true)).toEqual([0, 1, false])
    expect(value('#b textarea')).toBe('')
    expect(globals.onLogin).toHaveBeenCalledWith('T')
    // (б) своё поле было при поиске и пропало, пока шло решение.
    document.body.innerHTML = `<div id="app"><div id="w" class="g-recaptcha" data-sitekey="K">${field}</div><form id="b">${field}</form></div>`
    find('recaptcha_v2', 'K')
    document.querySelector('#w textarea')!.remove()
    expect(inject('recaptcha_v2', 'K', 'T2', true)).toEqual([0, 0, false])
    expect(value('#b textarea')).toBe('')
    // Виджет в своей форме без поля, рядом — чужая форма с полем.
    document.body.innerHTML = `<div id="app"><form id="a"><div class="g-recaptcha" data-sitekey="K"></div></form><form id="b">${field}</form></div>`
    find('recaptcha_v2', 'K')
    expect(inject('recaptcha_v2', 'K', 'T3', true)).toEqual([0, 0, false])
    expect(value('#b textarea')).toBe('')
  })

  it('без размеченных виджетов — только единственное поле ответа на странице', () => {
    document.body.innerHTML = '<div id="ts"><input type="hidden" name="cf-turnstile-response"></div>'
    expect(find('turnstile', 'T9')).toEqual({ key: 'T9', widget: false, enterprise: false, invisible: false })
    expect(inject('turnstile', 'T9', 'TT', false)).toEqual([1, 0, false])
    expect(value('#ts input')).toBe('TT')
    document.body.innerHTML = '<input id="x" name="cf-turnstile-response"><input id="y" name="cf-turnstile-response">'
    expect(inject('turnstile', 'T9', 'TT', false)).toEqual([0, 0, false])
    expect([value('#x'), value('#y')]).toEqual(['', ''])
  })

  it('ключ из поля не совпал ни с одним виджетом — чужой виджет не трогаем', () => {
    document.body.innerHTML = '<div class="g-recaptcha" data-sitekey="K1" data-callback="onLogin"><textarea name="g-recaptcha-response"></textarea></div>'
    globals.onLogin = vi.fn()
    expect(find('recaptcha_v2', 'K2')).toEqual({ key: 'K2', widget: false, enterprise: false, invisible: false })
    expect(inject('recaptcha_v2', 'K2', 'T', false)).toEqual([0, 0, false])
    expect(value('textarea')).toBe('')
    expect(globals.onLogin).not.toHaveBeenCalled()
  })

  it('невидимая reCAPTCHA на кнопке: поле значка своего клиента ___grecaptcha_cfg; без клиента — только обработчик', () => {
    const button = '<form><button class="g-recaptcha" data-sitekey="I" data-callback="onSubmit">Войти</button></form>'
    const badge = '<div class="grecaptcha-badge"><textarea name="g-recaptcha-response"></textarea></div>'
    const other = '<div id="other" class="g-recaptcha" data-sitekey="K1"><textarea name="g-recaptcha-response"></textarea></div>'
    document.body.innerHTML = button + badge + other + '<div class="grecaptcha-badge" id="b2"><textarea name="g-recaptcha-response"></textarea></div>'
    const own = vi.fn()
    globals.onSubmit = own
    const badges = document.querySelectorAll('.grecaptcha-badge')
    globals.___grecaptcha_cfg = {
      clients: {
        0: { id: 0, a: { host: document.querySelector('button'), badge: badges[0], c: { sitekey: 'I', callback: own } } },
        1: { id: 1, a: { host: document.getElementById('other') } },
        2: { id: 2, a: { badge: badges[1] } },
      },
    }
    expect(find('recaptcha_v2', 'I')).toEqual({ key: 'I', widget: true, enterprise: false, invisible: false })
    expect(inject('recaptcha_v2', 'I', 'IT', true)).toEqual([1, 1, false])
    expect(value('.grecaptcha-badge textarea')).toBe('IT')
    expect([value('#other textarea'), value('#b2 textarea')]).toEqual(['', ''])
    expect(own).toHaveBeenCalledWith('IT')
    // Без клиента связь со значком не доказана: поле вне своей формы не пишется, токен — обработчику.
    delete globals.___grecaptcha_cfg
    document.body.innerHTML = button + badge + other
    find('recaptcha_v2', 'I')
    expect(inject('recaptcha_v2', 'I', 'IT2', true)).toEqual([0, 1, false])
    expect([value('.grecaptcha-badge textarea'), value('#other textarea')]).toEqual(['', ''])
  })

  it('Enterprise — по фрейму выбранного виджета; без своего фрейма — по странице', () => {
    document.body.innerHTML = `
      <div class="g-recaptcha" data-sitekey="R"><iframe src="https://www.google.test/recaptcha/api2/anchor?k=R"></iframe></div>
      <div class="g-recaptcha" data-sitekey="E"><iframe src="https://www.google.test/recaptcha/enterprise/anchor?k=E"></iframe></div>`
    globals.grecaptcha = { enterprise: {} }
    expect(find('recaptcha_v2')).toEqual({ key: 'R', widget: true, enterprise: false, invisible: false })
    expect(find('recaptcha_v2', 'E')).toEqual({ key: 'E', widget: true, enterprise: true, invisible: false })
    document.body.innerHTML = '<iframe src="https://www.google.test/recaptcha/enterprise/anchor?k=EK"></iframe>'
    delete globals.grecaptcha
    expect(find('recaptcha_v2')).toEqual({ key: 'EK', widget: true, enterprise: true, invisible: false })
    document.body.innerHTML = '<div class="g-recaptcha" data-sitekey="K"></div>'
    expect(find('recaptcha_v3')).toEqual({ key: 'K', widget: true, enterprise: false, invisible: false })
    globals.grecaptcha = { enterprise: {} }
    expect(find('recaptcha_v3')).toEqual({ key: 'K', widget: true, enterprise: true, invisible: false })
    expect(find('hcaptcha')).toBeNull()
  })

  it('невидимая reCAPTCHA v2 — по data-size у виджета или size=invisible во фрейме', () => {
    document.body.innerHTML = `
      <button class="g-recaptcha" data-sitekey="B" data-size="invisible">Войти</button>
      <div class="g-recaptcha" data-sitekey="C"></div>
      <div id="w"><iframe src="https://www.google.test/recaptcha/api2/anchor?k=F&size=invisible"></iframe></div>
      <div class="g-recaptcha" data-sitekey="N"><iframe src="https://www.google.test/recaptcha/api2/anchor?k=N&size=normal"></iframe></div>`
    expect(find('recaptcha_v2', 'B')).toEqual({ key: 'B', widget: true, enterprise: false, invisible: true })
    expect(find('recaptcha_v2', 'C')).toEqual({ key: 'C', widget: true, enterprise: false, invisible: false })
    expect(find('recaptcha_v2', 'F')).toEqual({ key: 'F', widget: true, enterprise: false, invisible: true })
    expect(find('recaptcha_v2', 'N')).toEqual({ key: 'N', widget: true, enterprise: false, invisible: false })
  })

  it('метка виджета — не атрибут DOM и после внедрения снята', () => {
    document.body.innerHTML = '<div id="a" class="g-recaptcha" data-sitekey="K"><textarea name="g-recaptcha-response"></textarea></div>'
    find('recaptcha_v2')
    const a = document.getElementById('a') as unknown as Record<string, unknown>
    expect(document.body.innerHTML).not.toContain(MARK)
    expect(Object.keys(a)).not.toContain(MARK)
    inject('recaptcha_v2', 'K', 'T', true)
    expect(MARK in a).toBe(false)
  })
})
