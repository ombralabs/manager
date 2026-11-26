// Страница капчи для 2Captcha (`captcha.rs`), главный мир страницы. Без `token` — поиск: ключ сайта (`sitekey`, если
// задан полем, иначе первого виджета вида), взят ли виджет из разметки, Enterprise ли он и невидимый ли (reCAPTCHA v2
// с `data-size="invisible"` у виджета или `size=invisible` в адресе его фрейма). Взятый виджет помечается
// свойством `mark` в памяти (не атрибутом DOM): за время решения соседи могут сдвинуться, а сам он — пропасть.
// С `token` — внедрение только в помеченный виджет: его поля ответа и его обработчики; пропал — никуда. Ответ внедрения —
// `[полей, обработчиков, виджет пропал]`. Исключения обработчиков сайта глушатся: токен уже в полях. Поля не создаются:
// страница, которая их не завела (обычно v3), ждёт токен своим путём — он в `answer_to`.
(kind, sitekey, mark, token, widget) => {
  const family = kind.startsWith('recaptcha') ? 'recaptcha' : kind;
  const own = { recaptcha: '.g-recaptcha', hcaptcha: '.h-captcha', turnstile: '.cf-turnstile' }[family];
  const other = ['.g-recaptcha', '.h-captcha', '.cf-turnstile'].filter((c) => c !== own).join(', ');
  const [frame, param] = { recaptcha: [/\/recaptcha\/(api2|enterprise)\//, 'k'], hcaptcha: [/hcaptcha/, 'sitekey'] }[family] || [];
  // Ключ виджета: `data-sitekey` или параметр адреса фрейма (`k=` у reCAPTCHA, `sitekey=` у hCaptcha, в запросе или после `#`).
  const keyOf = (e) => {
    if (e.tagName !== 'IFRAME') return e.getAttribute('data-sitekey') || '';
    let url;
    try { url = new URL(e.src); } catch { return ''; }
    return new URLSearchParams(url.search).get(param) || new URLSearchParams(url.hash.slice(1)).get(param) || '';
  };
  // Виджеты вида по порядку, без повторов: элемент своего класса с ключом, фрейм своего вида, любой `[data-sitekey]` не
  // чужого вида.
  const widgets = [...new Set([
    ...document.querySelectorAll(`${own}[data-sitekey]`),
    ...(frame ? [...document.querySelectorAll('iframe[src]')].filter((f) => frame.test(f.src)) : []),
    ...[...document.querySelectorAll('[data-sitekey]')].filter((e) => !e.matches(other)),
  ])].filter((e) => keyOf(e));
  if (token === null) {
    const chosen = sitekey ? widgets.find((e) => keyOf(e) === sitekey) : widgets[0];
    const key = sitekey || (chosen ? keyOf(chosen) : '');
    if (!key) return null;
    // Enterprise — по фрейму самого виджета; своего фрейма нет (не отрисован, невидимый) — по странице.
    const enterpriseFrame = (f) => /\/recaptcha\/enterprise\//.test(f.src);
    const ownFrame = chosen && frame && (chosen.tagName === 'IFRAME' ? chosen : [...chosen.querySelectorAll('iframe[src]')].find((f) => frame.test(f.src)));
    const enterprise = family === 'recaptcha' && (ownFrame
      ? enterpriseFrame(ownFrame)
      : !!window.grecaptcha?.enterprise || (!chosen && [...document.querySelectorAll('iframe[src]')].some(enterpriseFrame)));
    const invisible = family === 'recaptcha' && !!chosen && (chosen.getAttribute('data-size') === 'invisible'
      || (!!ownFrame && /[?&]size=invisible(&|#|$)/.test(ownFrame.src)));
    if (chosen) Object.defineProperty(chosen, mark, { value: true, configurable: true });
    return { key, widget: !!chosen, enterprise, invisible };
  }
  const chosen = widgets.find((e) => e[mark] && keyOf(e) === sitekey) || null;
  for (const e of widgets) delete e[mark];
  if (widget && !chosen) return [0, 0, true];
  const primary = { recaptcha: 'g-recaptcha-response', hcaptcha: 'h-captcha-response', turnstile: 'cf-turnstile-response' }[family];
  // hCaptcha заводит в своём контейнере и поле reCAPTCHA — для совместимости.
  const names = family === 'hcaptcha' ? [primary, 'g-recaptcha-response'] : [primary];
  // Поля в элементе (и он сам), кроме полей внутри капчи чужого вида: reCAPTCHA не пишет в `.h-captcha`.
  const fieldsIn = (e, list) => {
    const selector = list.map((name) => `[name="${name}"]`).join(', ');
    return [...(e.matches(selector) ? [e] : []), ...e.querySelectorAll(selector)].filter((f) => !f.closest(other));
  };
  // Один виджет: фрейм в размеченном контейнере и сам контейнер — одно и то же.
  const same = (a, b) => a === b || a.contains(b) || b.contains(a);
  const others = (w) => widgets.filter((o) => !same(o, w));
  // Своя область виджета: от него вверх, пока предок не держит других виджетов и чужих форм; граница — форма или первый
  // предок с полем ответа (включительно); `body` и `html` — никогда. Поля вне области — не его.
  const areaOf = (w) => {
    let area = w;
    const form = w.closest('form');
    for (let e = w; e && e !== document.body && e !== document.documentElement; e = e.parentElement) {
      if (others(w).some((o) => e.contains(o)) || [...e.querySelectorAll('form')].some((f) => f !== form)) break;
      area = e;
      if (e.tagName === 'FORM' || fieldsIn(e, [primary]).length) break;
    }
    return area;
  };
  // Поиск не взял виджет: ключ из поля не совпал ни с одним размеченным — они чужие, не трогаем. Размеченных нет вовсе
  // (виджет отрисован скриптом) — годится только единственное поле ответа вида на странице.
  let start = chosen;
  if (!widget) {
    const lone = widgets.length ? [] : fieldsIn(document.documentElement, [primary]);
    start = lone.length === 1 ? lone[0] : null;
  }
  // Контейнер виджета (его `data-callback`) — только если в нём нет других виджетов.
  const alone = (node) => !chosen || !others(chosen).some((o) => node.contains(o));
  let box = start?.closest(`${own}, [data-sitekey]`) || null;
  if (box && !alone(box)) box = null;
  // reCAPTCHA: клиенты `___grecaptcha_cfg.clients`, что держат элемент этого виджета (или предка без других виджетов), —
  // их элементы и обработчики.
  const anchor = box || start;
  const holds = (node) => node === anchor || anchor.contains(node) || (node.contains(anchor) && alone(node));
  const clients = [];
  if (family === 'recaptcha' && anchor) try {
    for (const client of Object.values(window.___grecaptcha_cfg?.clients || {})) {
      const found = { nodes: [], callbacks: [] };
      const seen = new Set();
      const walk = (o, depth) => {
        if (!o || typeof o !== 'object' || depth > 4 || seen.has(o) || o === window) return;
        seen.add(o);
        for (const [k, v] of Object.entries(o)) {
          if (v instanceof Node) found.nodes.push(v);
          else if (k === 'callback') found.callbacks.push(v);
          else walk(v, depth + 1);
        }
      };
      walk(client, 0);
      if (found.nodes.some(holds)) clients.push(found);
    }
  } catch {}
  // Поля виджета — только в его области и его форме (оба вне форм — тоже «его»): поле соседней капчи или чужой формы не
  // берётся, даже если своё пропало. Два поля в области — непонятно, какое своё: не пишем никуда.
  const home = chosen?.closest('form') || null;
  const ownForm = (f) => f.closest('form') === home;
  const area = chosen && areaOf(chosen);
  const inArea = chosen ? fieldsIn(area, [primary]).filter(ownForm) : [];
  let fields = !start ? [] : chosen ? (inArea.length === 1 ? fieldsIn(area, names).filter(ownForm) : []) : [start];
  if (!fields.length && chosen) {
    // Своего поля нет (невидимая reCAPTCHA на кнопке): поле — в значке, который держит тот же клиент
    // `___grecaptcha_cfg` (отдельный элемент, не предок виджета), не в области другого виджета и не в чужой форме
    // (значок вне форм годится).
    const free = (f) => !others(chosen).some((o) => areaOf(o).contains(f)) && [home, null].includes(f.closest('form'));
    const held = clients.flatMap((c) => c.nodes)
      .filter((n) => n instanceof Element && !n.contains(chosen) && fieldsIn(n, [primary]).filter(free).length === 1);
    fields = [...new Set(held.flatMap((n) => fieldsIn(n, names).filter(free)))];
  }
  for (const field of fields) field.value = token;
  const called = new Set();
  const call = (callback) => {
    const fn = typeof callback === 'function' ? callback : String(callback).split('.').reduce((o, k) => o?.[k], window);
    if (typeof fn === 'function' && !called.has(fn)) {
      called.add(fn);
      try { fn(token); } catch {}
    }
  };
  if (box?.hasAttribute('data-callback')) call(box.getAttribute('data-callback'));
  for (const client of clients) client.callbacks.forEach(call);
  return [fields.length, called.size, false];
}
