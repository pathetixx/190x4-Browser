# Релиз 190x4 Browser

Счётчик один — semver в `src-tauri/tauri.conf.json`. Тег — `vX.Y.Z`.

1. Добавить в начало `CHANGELOG.md` секцию `## vX.Y.Z — ГГГГ-ММ-ДД` с пунктами.
2. `node scripts/release.mjs X.Y.Z` — бамп версии, коммит, аннотированный тег с
   заметками, push `main` и тега, draft релиза.
3. Push тега запускает `.github/workflows/build.yml`: проверки, подписанная
   сборка установщика, `latest.json`, выкладка в GitLab, публикация релиза на
   GitHub, `release.mjs --verify`.

## Откуда браузер берёт обновления

`plugins.updater.endpoints` в `tauri.conf.json`, по порядку:

1. GitLab Generic Packages проекта `pathetix/190x4-browser` —
   `…/packages/generic/190x4-browser/stable/latest.json`. Установщик и
   `latest.json` каждой версии лежат там же под номером версии и не
   перезаписываются; `stable` переключается последним.
2. GitHub Releases — `releases/latest/download/latest.json`. Релиз не должен
   быть prerelease: иначе он не станет Latest.

Откат GitLab на уже выложенную версию: `GITLAB_TOKEN=… GITLAB_PROJECT_ID=86438976
python scripts/gitlab_mirror.py --rollback X.Y.Z`.

## Подпись

Установщик подписывается ключом minisign из секрета `TAURI_SIGNING_PRIVATE_KEY`
(без пароля). Публичный ключ — `plugins.updater.pubkey`. Потеря приватного ключа
означает, что установленные браузеры больше не смогут обновиться.

Секреты репозитория: `TAURI_SIGNING_PRIVATE_KEY`, `GITLAB_TOKEN`.
