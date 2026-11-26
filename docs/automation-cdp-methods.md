# Методы CDP автоматизации — для проверки детекторами

Список методов, которые вызывает `src-tauri/src/engine/page.rs` (автоматизация Ombra). Страница не должна отличать процесс от ручной работы по следам этих вызовов. `Runtime.enable` не вызывается никогда.

| Домен | Методы | Когда |
| --- | --- | --- |
| Target | `getTargets`, `getTargetInfo` (адрес вкладки, URL), `createTarget`, `closeTarget`, `attachToTarget` (`flatten`), `detachFromTarget`, `setAutoAttach` (на вкладке и межсайтовых фреймах) | Вкладки, сессии, межсайтовые фреймы |
| Page | `enable` (на вкладке и межсайтовых фреймах), `navigate` (без ожидания загрузки при тайм-ауте 0), `reload`, `getNavigationHistory`, `navigateToHistoryEntry`, `createIsolatedWorld`, `getLayoutMetrics`, `captureScreenshot` | Переходы, мир процесса, снимки |
| Runtime | `evaluate` (главный мир — только `web.js`; иначе изолированный), `callFunctionOn` (изолированный мир), `getProperties` (элементы найденного списка), `releaseObject` (временные объекты после операции) | Поиск и чтение элементов, JS процесса |
| DOM | `describeNode`, `resolveNode`, `getContentQuads`, `getBoxModel`, `scrollIntoViewIfNeeded`, `focus`, `setFileInputFiles` (Input File) | Элементы по `backendNodeId` без `DOM.enable` |
| Input | `dispatchMouseEvent`, `dispatchKeyEvent` (с `commands` для ⌘ на macOS), `insertText` | Ввод; очеловечивание — в движке |
| Network | `enable` (`maxPostDataSize`) и `disable` — на время слушателей, Request to finish и узла перед ним; `getRequestPostData`, `getResponseBody`, `getCookies`, `deleteCookies` (без `enable`) | Прослушивание запросов, cookies страницы |

Живые тесты автоматизации запускают браузер без окна (`settings.headless`); окно — с переменной окружения `OMBRA_PROBE_HEADFUL=1`.

Изолированный мир называется `ombra`; странице он не виден.
