// Поиск элементов процесса в изолированном мире документа: CSS, XPath или видимый текст (спецификация
// автоматизации, «Контекст страницы»). Ответ — массив элементов в порядке документа.
(by, selector) => {
  if (by === 'css') return Array.from(document.querySelectorAll(selector));
  if (by === 'xpath') {
    const found = document.evaluate(selector, document, null, XPathResult.ORDERED_NODE_SNAPSHOT_TYPE, null);
    const out = [];
    for (let i = 0; i < found.snapshotLength; i++) {
      const node = found.snapshotItem(i);
      if (node.nodeType === Node.ELEMENT_NODE) out.push(node);
    }
    return out;
  }
  // Текст: видимый текст элемента равен искомому, иначе — содержит его; из вложенных подходящих — самый глубокий.
  // Подходящими считаются только элементы с рамкой: скрытый потомок (его innerText — текст разметки) не отнимает
  // совпадение у родителя, а обёртка без рамки (`display: contents`) не прячет от него подходящего потомка.
  const norm = (text) => (text || '').replace(/\s+/g, ' ').trim();
  const want = norm(selector);
  const shown = Array.from(document.querySelectorAll('body, body *')).filter((el) => el.getClientRects().length > 0);
  // Подходящие идут в порядке документа (сначала элемент, потом его поддерево): если внутри элемента есть
  // подходящий потомок, то он и есть следующий подходящий — достаточно проверить его одного.
  const pick = (test) => {
    const hits = shown.filter((el) => test(norm(el.innerText)));
    return hits.filter((el, i) => !(i + 1 < hits.length && el.contains(hits[i + 1])));
  };
  const exact = pick((text) => text === want);
  return exact.length ? exact : pick((text) => text.includes(want));
}
