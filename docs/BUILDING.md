# Сборки Ombra Desktop (Ombra Manager)

Репозиторий: https://github.com/ombralabs/manager. Версии в `package.json`, `src-tauri/Cargo.toml` и `src-tauri/tauri.conf.json` должны совпадать; тег обязан соответствовать этой версии.

| Платформа | Rust target | Результат |
| --- | --- | --- |
| macOS Apple Silicon | `aarch64-apple-darwin` | `.dmg`, `.tar.gz` с `Ombra.app` |
| macOS Intel | `x86_64-apple-darwin` | `.dmg`, `.tar.gz` с `Ombra.app` |
| Windows x64 | `x86_64-pc-windows-msvc` | установщик NSIS `.exe`, `.zip` |
| Linux x64 | `x86_64-unknown-linux-gnu` | `.deb`, `.rpm`, `.AppImage`, `.tar.xz` |

Каждый файл имеет соседнюю контрольную сумму `.sha256`. JSON-манифест содержит версию, платформу, коммит приложения, ревизию общих библиотек движка, размеры и SHA-256. Результаты сохраняются в `artifacts/`. Без Developer ID identity конфигурация выпускает **неподписанные предварительные сборки** (`distribution: "unsigned-preview"`). Подписанная macOS-сборка описана ниже.

## Локальная сборка

Нужны Node.js 22+, npm, Rust из `rust-toolchain.toml` и [системные зависимости Tauri](https://v2.tauri.app/start/prerequisites/). Сборка выполняется на соответствующей ОС; обе архитектуры macOS можно собрать на Mac с установленными Rust targets. Windows требует MSVC Build Tools. Linux собирается на Ubuntu 22.04 для совместимости с glibc 2.35 и новее, WebKitGTK 4.1.

```sh
npm ci
npm run check:web
cargo test --locked --manifest-path src-tauri/Cargo.toml --lib
rustup target add aarch64-apple-darwin
APPLE_SIGNING_IDENTITY=- npm run build:desktop -- darwin-arm64
node tools/release/smoke.mjs darwin-arm64
```

Другие значения аргумента: `darwin-x64`, `windows-x64`, `linux-x64`. Результаты — в `artifacts/`. Зависимости для Ubuntu 22.04:

```sh
sudo apt-get update
sudo apt-get install -y --no-install-recommends libwebkit2gtk-4.1-dev build-essential libssl-dev libdbus-1-dev libxdo-dev libayatana-appindicator3-dev librsvg2-dev patchelf libfuse2 rpm xdg-utils xclip wl-clipboard
```

Команда RPA чтения буфера обмена использует `wl-paste` на Wayland или `xclip` на X11. Текущая версия хранит ключ шифрования локально, без Keychain, Windows Credential Manager и Secret Service. Условия хранения и переноса прежних ключей описаны в [local-keys.md](local-keys.md).

Для предварительных macOS-сборок `APPLE_SIGNING_IDENTITY=-` задаёт ad-hoc подпись всего приложения. Она позволяет проверить целостность пакета, но не заменяет Developer ID и нотариализацию. Проверка имён часовых поясов использует встроенную базу IANA на всех ОС и не требует `/usr/share/zoneinfo`.

## Подписанный выпуск macOS

Нужны установленная в Keychain identity типа **Developer ID Application** с приватным ключом, доступные `codesign`, `hdiutil`, `xcrun notarytool`, `xcrun stapler`, `spctl` и заранее сохранённый профиль notarytool. Исходники должны быть закоммичены. Identity можно задать именем сертификата или его SHA-1 из `security find-identity -v -p codesigning` ([настройка подписи Tauri](https://v2.tauri.app/distribute/sign/macos/)).

```sh
APPLE_SIGNING_IDENTITY='<Developer ID identity>' \
OMBRA_NOTARY_PROFILE=ombra-notary \
OMBRA_NOTARY_KEYCHAIN="$HOME/Library/Keychains/login.keychain-db" \
npm run build:desktop -- darwin-arm64
```

Повторить с `darwin-x64` для Intel. `OMBRA_NOTARY_PROFILE` по умолчанию равен `ombra-notary`; `OMBRA_NOTARY_KEYCHAIN` можно не задавать, если профиль доступен в обычном поиске Keychain. Пароль Apple ID, экспорт сертификата и ключа для этой команды не нужны: сборщик удаляет credential-переменные Tauri из окружения дочерних процессов и использует только существующий профиль.

При настоящей `APPLE_SIGNING_IDENTITY` Tauri собирает только `.app` и подписывает вложенный код с hardened runtime. Сборщик проверяет Developer ID, timestamp и runtime, отправляет ZIP приложения через `notarytool --keychain-profile`, требует `Accepted`, прикрепляет и проверяет ticket. Затем создаёт DMG из этого же приложения с ссылкой на `/Applications`, подписывает и нотариализует DMG. Проверки `codesign --verify --deep --strict`, `stapler validate` и `spctl` обязательны для приложения и DMG. Только после их успеха в `artifacts/` записываются DMG, архив того же stapled `.app`, контрольные суммы и манифест с `distribution: "macos-notarized"`.

JSON-ответы notarytool с submission ID и вывод проверок сохраняются отдельно: `src-tauri/target/<rust-target>/release/notarization-<версия>-<случайный суффикс>/`. Путь выводится перед нотариализацией. Этот каталог не публикуется. При ошибке подписи, нотариализации или проверки старые файлы в `artifacts/` не изменяются.

## Движок и платформенные ограничения

При запуске Desktop после открытия хранилища проверяет публичные стабильные выпуски браузера в GitHub Releases репозитория [`ombralabs/ombra`](https://github.com/ombralabs/ombra/releases). Более новая версия скачивается, проверяется и распаковывается в `engines/versions/<версия>/`; прежние сборки, включая локальный слот `engines/Ombra.app` / `engines/Chromium.app` (`engines/Ombra` / `engines/Chromium` в Linux/Windows), сохраняются. В настройках браузера доступны проверка вручную, прогресс и отмена скачивания, импорт архива и выбор любой установленной версии. Выбор старой версии или ручной импорт выключает автообновление; его можно включить обратно. Уже работающие профили продолжают использовать свой бинарник, следующие запускаются с выбранным. Бинарники браузера в установщики Desktop не включаются.

По умолчанию Desktop читает выпуски из `https://api.github.com/repos/ombralabs/ombra/releases` и скачивает файлы с `https://github.com/ombralabs/ombra/releases/download` (с проверкой допустимых хостов редиректа GitHub). Каждый выпуск содержит файл `release.json` — подписанный конверт (`signed` + Ed25519 `sig` в Base64). Публичный ключ зафиксирован в `src-tauri/engine-signing.pub`. Внутри `signed` — `version`, `chromium`, `minManager`, опциональный `commit` и `platforms` с записями `file`, `sha256`, `size`. Номер браузера имеет вид `<четыре числа Chromium>-<номер выпуска>`; сравниваются числа, включая номер выпуска. Черновики, pre-release и теги SDK пропускаются.

Архивы именуются `ombra-engine-<версия>-<платформа>.<расширение>`: `darwin-arm64`/`darwin-x64` — `tar.gz`, `windows-x64` — `zip`, `linux-x64` — `tar.xz`. В каждом архиве нужны метаданные `ombra-build.json` с полями `version`, `chromium`, `platform` (на macOS — внутри `Ombra.app/Contents/Resources/`). Версия и платформа должны совпадать с подписанным выпуском. До распаковки проверяются Ed25519-подпись `release.json`, минимальная версия Desktop, размер и SHA-256 архива; распаковка проверяет границы путей, ссылок и лимиты размера. Ошибка или отмена оставляет активный браузер на месте; временная загрузка удаляется.

Адреса можно переопределить при компиляции через `OMBRA_ENGINE_RELEASES_URL` и `OMBRA_ENGINE_DOWNLOAD_URL`; требуются HTTPS-адреса. Состояние выбора и автообновления хранится в `engines/selection.json`; одноимённые версии не перезаписываются другим архивом.

Библиотека `ombra-humanize` включена в `vendor/ombra-engine/crates/ombra-humanize` под лицензией `GPL-3.0-only`; исходная ревизия записана в `vendor/ombra-engine/REVISION`. Для сборки не нужны соседние репозитории или GitHub token.

Полное зашифрованное хранилище профилей использует APFS и доступно только в macOS. В Windows/Linux настройка этой функции недоступна; попытка открыть существующее защищённое хранилище завершается ошибкой, данные не открываются без защиты.

Rust-тесты Unix-сценариев запускаются на macOS Apple Silicon и Linux; Windows проходит компиляцию приложения и проверку запуска CLI. Тесты, требующие реального браузера или монтирования APFS, выполняются отдельно.
