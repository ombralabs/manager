// Перехват ввода главного окна синхронизатора (спецификация синхронизатора, «Перехват в главном окне»): функция
// `(flags) => …`, ядро вызывает её в изолированном мире `ombra-sync`. DOM общий со страницей, глобальные объекты —
// свои: странице не видно ни имён, ни слушателей. События ядро забирает длинным опросом `__ombraSyncNext()`.
// Только доверенный ввод: синтетику сайта не повторяем.
(flags) => {
  if (globalThis.__ombraSyncNext && globalThis.__ombraSyncReset) return;
  // Без опроса (сессия кончилась, фрейм никто не слушает) очередь дальше не растёт.
  const LIMIT = 1000;
  let queue = [];
  let waiting = null;
  const give = (batch) => {
    const { resolve, timer } = waiting;
    waiting = null;
    clearTimeout(timer);
    resolve(batch);
  };
  const flush = () => {
    if (waiting && queue.length) {
      give(queue);
      queue = [];
    }
  };
  // Промис с накопленным, как только есть хоть одно событие, или пустой через 20 с. Новый опрос закрывает прежний
  // пустым: брошенный ядром (переход, кэш «назад-вперёд») не уносит события.
  globalThis.__ombraSyncNext = () =>
    new Promise((resolve) => {
      if (waiting) give([]);
      waiting = { resolve, timer: setTimeout(() => give([]), 20000) };
      flush();
    });
  const push = (event) => {
    if (queue.length < LIMIT) queue.push(event);
    flush();
  };
  // Нажатие или отпускание мыши, ждущее числа нажатий (`settle`): у pointer-событий `detail` всегда 0.
  let held = null;
  // Called only after the native input-drain fence: discard the whole solving interval.
  globalThis.__ombraSyncReset = () => {
    queue = [];
    held = null;
    moved = -Infinity;
    if (waiting) give([]);
  };
  const settle = () => {
    const event = held;
    held = null;
    if (event) push(event);
  };
  const send = (event) => {
    settle();
    push(event);
  };

  const mods = (e) => (e.altKey ? 1 : 0) | (e.ctrlKey ? 2 : 0) | (e.metaKey ? 4 : 0) | (e.shiftKey ? 8 : 0);
  //@SIG
  // Слушатели — на `window` в фазе захвата: поставлены до скриптов страницы, её `stopPropagation` их не глушит.
  const on = (type, handle) =>
    window.addEventListener(type, (e) => e.isTrusted && handle(e), { capture: true, passive: true });

  // Наведение — только с `flags.mouse` и не чаще раза в 50 мс. Движение с нажатой кнопкой — перетаскивание: идёт всегда
  // и без прореживания, иначе ползунок у ведомых встал бы не там. Слушаем pointermove: страница, погасившая pointerdown
  // (ползунки Radix), гасит и mousemove до отпускания.
  let moved = -Infinity;
  on('pointermove', (e) => {
    if (e.pointerType !== 'mouse') return;
    const { clientX: x, clientY: y, buttons } = e;
    if (!buttons) {
      if (!flags.mouse || e.timeStamp - moved < 50) return;
      moved = e.timeStamp;
    }
    send({ t: 'move', x, y, buttons, mods: mods(e) });
  });
  // Число нажатий даёт mousedown или mouseup той же задачи, и событие уходит сразу с ним: таймер ждал бы дольше перехода,
  // который начал клик по ссылке, и отпускание пропало бы вместе с документом. Таймер — на случай, когда их нет.
  // ponytail: страница их погасила (`preventDefault` у pointerdown) — 1, двойной клик там повторится двумя одиночными;
  // свой счёт нажатий по времени и месту, если это проявится.
  for (const t of ['down', 'up']) {
    on('pointer' + t, (e) => {
      if (e.pointerType !== 'mouse') return;
      settle();
      const { clientX: x, clientY: y, button, buttons, target } = e;
      held = { t, x, y, button, buttons, detail: 1, mods: mods(e), sig: sig(target), select: target instanceof HTMLSelectElement };
      setTimeout(settle, 0);
    });
    on('mouse' + t, (e) => {
      if (held?.t !== t) return;
      held.detail = e.detail;
      settle();
    });
  }
  on('wheel', (e) => {
    const scale = [1, 40, innerHeight][e.deltaMode] ?? 1;
    send({ t: 'wheel', x: e.clientX, y: e.clientY, dx: e.deltaX * scale, dy: e.deltaY * scale, mods: mods(e) });
  });
  for (const kind of ['down', 'up']) {
    on('key' + kind, (e) => {
      const { key, code, keyCode, repeat, location } = e;
      send({ t: 'key', kind, key, code, keyCode, mods: mods(e), repeat, location, sig: sig(document.activeElement) });
    });
  }
  on('change', ({ target }) => {
    if (!(target instanceof HTMLSelectElement)) return;
    const box = target.getBoundingClientRect();
    const label = target.selectedOptions[0]?.text ?? '';
    send({ t: 'select', x: box.left + box.width / 2, y: box.top + box.height / 2, label, sig: sig(target) });
  });
  on('visibilitychange', () => document.visibilityState === 'visible' && send({ t: 'visible' }));
}
