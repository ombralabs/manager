// Подпись цели ввода: общий кусок скрипта перехвата главного и проверок ведомых — окна сравниваются
// одинаково построенными подписями. Подставляется в `sync_capture.js` на место `//@SIG`.
  // Начало текста цели: значение поля или первые текстовые узлы. Без раскладки и обхода всей страницы — обработчик
  // ввода не тормозит окно, и сайт не видит его в Event Timing; у `<body>` и `<html>` текста нет.
  const textOf = (el) => {
    if (el instanceof HTMLInputElement || el instanceof HTMLTextAreaElement || el instanceof HTMLSelectElement) return el.value.slice(0, 80);
    if (el === document.body || el === document.documentElement) return '';
    let text = '';
    const walker = document.createTreeWalker(el, NodeFilter.SHOW_TEXT);
    while (text.length < 80 && walker.nextNode()) text += walker.currentNode.data.slice(0, 80);
    return text;
  };
  // Подпись цели — по ней сверяются окна; текст — для причины паузы.
  const sig = (el) => {
    if (!(el instanceof Element)) return null;
    const text = textOf(el).slice(0, 80).replace(/\s+/g, ' ').trim().slice(0, 40);
    const attr = (name) => el.getAttribute(name) || '';
    return { tag: el.tagName, role: attr('role'), type: typeof el.type === 'string' ? el.type : '', id: el.id || '', name: attr('name'), text };
  };
