# 190x4 Browser

Браузер для Windows в эстетике 190x4. Chrome браузера — веб-UI, вкладки —
нативные WebView2 поверх одного окна, фильтр — adblock-rust на горячем пути.

**Статус: работает.** Браузер собирается и запускается: вкладки, панель
закладок с папками, импорт и экспорт закладок (HTML), менеджер паролей с
автозаполнением и импортом/экспортом CSV, загрузки с паузой и отменой,
настройки, блокировка рекламы с исключениями по сайтам, история,
восстановление сессии, открытие ссылок и файлов из других программ (браузер по умолчанию), поиск по странице, командная палитра, своё меню
страницы, новая вкладка с плитками сайтов, погодой и монитором ресурсов
браузера, перевод выделенного и расширение «Загрузчик видео» на сервисах 190x4. Сборка и тесты идут на Windows-машине разработчика.

Замеры, на которых держатся решения, — в
[docs/measurements/2026-09-12.md](docs/measurements/2026-09-12.md).

## Структура

```
crates/adblock        фильтр: ArcSwap + adblock-rust, собирается везде
crates/webview        вкладки: сырой COM (webview2-com), только Windows
crates/store          история, закладки, сессия, загрузки, настройки, пароли (SQLite)
crates/services       клиент к сервисам 190x4: переводчик и загрузчик
crates/spike          замеры: память, задержка фильтра, DRM
src-tauri             приложение: команды, состояние, сборка бандла
ui                    chrome браузера (vanilla, без сборщиков); popup.html — меню и пузыри
pages                 встроенные страницы (новая вкладка) через virtual host
scripts/icons         сборка спрайта иконок и иконки приложения
docs                  ARCHITECTURE.md — решения, SPIKE.md — как мерить
```

## Интерфейс без сборки

Chrome браузера поднимается в любом браузере на моках — так его и правят:

```bash
python3 -m http.server 8777 --directory ui
```

Дальше `http://127.0.0.1:8777/index.html`. Параметры для ревью вёрстки:
`?demo=palette|find|settings|downloads|shield|bookmarks|history|translate`,
`&section=passwords` — раздел настроек, `?motion=off` — без анимаций (нужно для
детерминированных скриншотов). Всплывающее окно отдельно:
`popup.html?kind=menu|downloads|bookmark|bookmark-folder|media|password|accounts|site|extensions`.

## Сервисы

Переводчик и загрузчик ходят на сервер 190x4. Ключи лежат в
`%LOCALAPPDATA%\pw.x190x4.browser\services.json` и в репозиторий не попадают:

```json
{ "base_url": "https://190x4.pw", "translate_key": "…", "media_key": "…" }
```

Без него браузер работает, просто эти две функции честно говорят, что не
настроены.

## Сборка

```bash
pwsh scripts/fetch-lists.ps1     # фильтр-списки не хранятся в репозитории
npx @tauri-apps/cli@2.11.4 build # только Windows; установщик NSIS в target/release/bundle/nsis
```

Установщик для релиза подписывается ключом обновлений (`TAURI_SIGNING_PRIVATE_KEY`).
Без ключа локальная сборка — с `-c '{"bundle":{"createUpdaterArtifacts":false}}'`.
Релизы собирает `.github/workflows/build.yml` по тегу; порядок — `RELEASING.md`.

## Замеры

См. [docs/SPIKE.md](docs/SPIKE.md) — память на 20 вкладках, задержка фильтра,
проверка DRM.
