#!/usr/bin/env node
// Расширенные фильтры 190x4 Browser: списки uAssets и ресурсы скриптлетов
// uBlock Origin в формате adblock-rust. Собирается workflow `filters.yml` и
// выкладывается рядом с обновлениями браузера; в репозитории результат не
// хранится — это данные под GPL-3.0 (см. NOTICE.txt в выдаче).
//
// Что делает скрипт:
//   - списки: раскрывает `!#if`/`!#else`/`!#endif` под наш движок (Chromium,
//     без HTML-фильтрации) и подставляет `!#include` — adblock-rust этих
//     директив не понимает и применил бы обе ветки условий;
//   - ресурсы: берёт модули скриптлетов последнего релиза uBlock Origin,
//     импортирует их и выгружает функции с зависимостями и признаком
//     доверенности (`permission`, тот же бит, что у доверенных списков);
//   - заглушки правил `$redirect=` (`noop.js`, пустой VAST, `google-ima.js`,
//     беззвучный mp3) — файлы `web_accessible_resources` того же релиза с
//     именами и псевдонимами из `redirect-resources.js`. Без них правило с
//     заглушкой просто закрывало бы запрос, и плеер видел бы блокировщик.
//
// Использование: node scripts/filters/build.mjs <каталог>

import { createHash } from "node:crypto";
import { mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { dirname, join, normalize, resolve } from "node:path";
import { pathToFileURL } from "node:url";

const OUT = resolve(process.argv[2] ?? "filters-out");
const LISTS = ["filters", "quick-fixes", "privacy", "unbreak"];
const LIST_BASE = "https://ublockorigin.github.io/uAssets/filters/";
// Набор списков — как каталог Brave (brave/adblock-resources,
// filter_lists/list_catalog.json) для русского языка: те же источники, тот же
// состав в каждом списке. Свои правила под отдельные сайты не пишутся — это
// дело авторов списков.
//
// Основные списки вшиты и в установщик, но обновлять их только с выпуском
// браузера — значит неделями жить со старыми правилами. Здесь они свежие, и
// браузер берёт скачанную копию вместо вшитой. Список из нескольких
// источников склеивается в один файл.
const UASSETS = "https://raw.githubusercontent.com/uBlockOrigin/uAssets/master/filters/";
const BRAVE = "https://raw.githubusercontent.com/brave/adblock-lists/master/";
const BASE_LISTS = {
  "easylist.txt": ["https://easylist.to/easylist/easylist.txt"],
  "easyprivacy.txt": ["https://easylist.to/easylist/easyprivacy.txt"],
  // RU AdList у Brave: без EasyList и без «cssfixes» (его правила-приманки
  // Дзена будят защиту от блокировщиков), зато с JS Fixes — скриптлетами,
  // которые эту защиту и снимают.
  "ruadlist.txt": [
    "https://easylist-downloads.adblockplus.org/advblock.txt",
    "https://raw.githubusercontent.com/easylist/ruadlist/master/js-fixes-experimental.txt",
  ],
  // Списки самого Brave из его «Default» (без iOS и Android) и отдельно
  // «First Party» — у Brave он без защиты своего содержимого сайта.
  "brave.txt": [
    `${BRAVE}brave-unbreak.txt`,
    `${BRAVE}brave-lists/brave-specific.txt`,
    `${BRAVE}brave-lists/brave-social.txt`,
    `${BRAVE}brave-lists/brave-unbreak.txt`,
    `${BRAVE}brave-lists/brave-sugarcoat.txt`,
  ],
  "brave-firstparty.txt": [
    `${BRAVE}brave-lists/brave-firstparty.txt`,
    `${BRAVE}brave-lists/brave-firstparty-regional.txt`,
  ],
  // «Cookie notice blocker» и «Mobile app promo blocker» — у Brave включены по умолчанию.
  "cookies.txt": [
    "https://secure.fanboy.co.nz/fanboy-cookiemonster_ubo.txt",
    `${UASSETS}annoyances-cookies.txt`,
    `${BRAVE}brave-lists/brave-cookie-specific.txt`,
  ],
  "mobile-promo.txt": ["https://secure.fanboy.co.nz/fanboy-mobile-notifications.txt"],
  "urlhaus.txt": ["https://malware-filter.gitlab.io/malware-filter/urlhaus-filter-agh-online.txt"],
  // AdGuard Russian в синтаксисе uBlock Origin — его выпускает сам AdGuard. У
  // Brave его нет; в браузере он по выбору, выключен.
  "adguard-russian.txt": ["https://filters.adtidy.org/extension/ublock/filters/1.txt"],
};
// Из «Default» Brave, чего нет в filters.txt uBlock Origin (годовые файлы и
// ubo-link-shorteners он подключает сам).
const UBO_EXTRA = ["badware", "resource-abuse"];
// Пороги «список подозрительно короткий»: обрыв выкладки не должен молча
// превратиться в пустой фильтр.
const MIN_LINES = {
  "mobile-promo.txt": 20,
  "brave.txt": 200,
  "brave-firstparty.txt": 200,
  "cookies.txt": 200,
  "urlhaus.txt": 50,
};
const REPO = "gorhill/uBlock";
const TRUSTED = 1;
// Тип заглушки по расширению файла — те, что знает adblock-rust.
const MIME = {
  css: "text/css",
  gif: "image/gif",
  html: "text/html",
  js: "application/javascript",
  json: "application/json",
  mp3: "audio/mp3",
  mp4: "video/mp4",
  png: "image/png",
  txt: "text/plain",
  xml: "text/xml",
};

// Окружение браузера для `!#if`. Неизвестные токены — ложь.
const ENV = {
  env_chromium: true,
  env_edge: true,
  cap_user_stylesheet: true,
  ext_ublock: true,
  true: true,
};

const headers = process.env.GITHUB_TOKEN ? { Authorization: `Bearer ${process.env.GITHUB_TOKEN}` } : {};
async function get(url, options = {}) {
  for (let attempt = 1; ; attempt++) {
    const response = await fetch(url, options);
    if (response.ok) return response;
    if (attempt === 3 || response.status < 500) throw new Error(`${url}: HTTP ${response.status}`);
    await new Promise((done) => setTimeout(done, attempt * 2000));
  }
}
const text = async (url) => (await get(url)).text();
const api = async (path) => (await get(`https://api.github.com/${path}`, { headers })).json();

function evaluate(expression) {
  const tokens = expression.match(/!|&&|\|\||\(|\)|\w+/g) ?? [];
  let i = 0;
  const primary = () => {
    const token = tokens[i++];
    if (token === "!") return !primary();
    if (token === "(") {
      const value = or();
      i++;
      return value;
    }
    return Boolean(ENV[token]);
  };
  const and = () => {
    let value = primary();
    while (tokens[i] === "&&") {
      i++;
      value = primary() && value;
    }
    return value;
  };
  const or = () => {
    let value = and();
    while (tokens[i] === "||") {
      i++;
      value = and() || value;
    }
    return value;
  };
  return or();
}

async function preprocess(source, base, depth = 0) {
  const out = [];
  const stack = [];
  for (const line of source.split(/\r?\n/)) {
    if (line.startsWith("!#if ")) {
      stack.push(evaluate(line.slice(5)));
      continue;
    }
    if (line.startsWith("!#else")) {
      if (stack.length) stack[stack.length - 1] = !stack[stack.length - 1];
      continue;
    }
    if (line.startsWith("!#endif")) {
      stack.pop();
      continue;
    }
    if (!stack.every(Boolean)) continue;
    if (line.startsWith("!#include ")) {
      if (depth >= 3) throw new Error(`слишком глубокий !#include в ${base}`);
      const url = new URL(line.slice(10).trim(), base).href;
      out.push(await preprocess(await text(url), url, depth + 1));
      continue;
    }
    out.push(line);
  }
  if (stack.length) throw new Error(`незакрытый !#if в ${base}`);
  return out.join("\n");
}

async function scriptlets() {
  const { tag_name: tag } = await api(`repos/${REPO}/releases/latest`);
  const tmp = join(OUT, ".ubo");
  rmSync(tmp, { recursive: true, force: true });
  const listing = await api(`repos/${REPO}/contents/src/js/resources?ref=${tag}`);
  const queue = listing.filter((file) => file.name.endsWith(".js")).map((file) => file.path);
  const seen = new Set();
  while (queue.length) {
    const path = queue.shift();
    if (seen.has(path)) continue;
    seen.add(path);
    const source = await text(`https://raw.githubusercontent.com/${REPO}/${tag}/${path}`);
    const file = join(tmp, path);
    mkdirSync(dirname(file), { recursive: true });
    writeFileSync(file, source);
    for (const match of source.matchAll(/^\s*import\s[^'"]*['"](\.{1,2}\/[^'"]+)['"]/gm)) {
      queue.push(normalize(join(dirname(path), match[1])));
    }
  }
  const module = await import(pathToFileURL(join(tmp, "src/js/resources/scriptlets.js")).href);
  const resources = module.builtinScriptlets.map((scriptlet) => ({
    name: scriptlet.name,
    aliases: scriptlet.aliases ?? [],
    // Встраивать как скриптлет adblock-rust позволяет только application/javascript;
    // вспомогательные функции uBlock (`*.fn`) годятся лишь как зависимости.
    kind: { mime: scriptlet.name.endsWith(".fn") ? "fn/javascript" : "application/javascript" },
    content: Buffer.from(scriptlet.fn.toString()).toString("base64"),
    dependencies: (scriptlet.dependencies ?? []).map((dep) => (typeof dep === "function" ? dep.details?.name : dep)),
    permission: scriptlet.requiresTrust ? TRUSTED : 0,
  }));
  const names = new Set(resources.flatMap((resource) => [resource.name, ...resource.aliases]));
  const missing = resources.flatMap((resource) => resource.dependencies.filter((dep) => !names.has(dep)));
  if (missing.length) throw new Error(`не найдены зависимости скриптлетов: ${[...new Set(missing)].join(", ")}`);
  for (const required of ["trusted-replace-xhr-response.js", "trusted-replace-fetch-response.js", "json-prune.js"]) {
    if (!names.has(required)) throw new Error(`нет скриптлета ${required}`);
  }
  resources.push(...(await redirects(tag, tmp, names)));
  resources.push(...(await braveScriptlets(new Set(resources.flatMap((r) => [r.name, ...r.aliases])))));
  rmSync(tmp, { recursive: true, force: true });
  return { tag, resources };
}

// Скриптлеты самого Brave (brave/adblock-resources): на них ссылаются его
// списки (`brave-fix`, `de-amp`, `vaft-ublock-origin`…), и без них эти правила
// не работают. Описание — в их metadata.json, в формате adblock-rust.
async function braveScriptlets(taken) {
  const base = "https://raw.githubusercontent.com/brave/adblock-resources/master/";
  const metadata = JSON.parse(await text(`${base}metadata.json`));
  const out = [];
  for (const entry of metadata) {
    if (taken.has(entry.name) || entry.kind?.mime !== "application/javascript") continue;
    const source = await text(`${base}resources/${entry.resourcePath}`);
    out.push({
      name: entry.name,
      aliases: (entry.aliases ?? []).filter((alias) => !taken.has(alias)),
      kind: { mime: "application/javascript" },
      content: Buffer.from(source).toString("base64"),
      dependencies: entry.dependencies ?? [],
      permission: entry.permission ?? 0,
    });
  }
  if (!out.some((resource) => resource.name === "brave-fix.js")) throw new Error("нет скриптлетов Brave");
  return out;
}

// Заглушки `$redirect=`. Записи с параметрами (`click2load.html`) — страницы
// самого uBlock, а не заглушки; их нет.
async function redirects(tag, tmp, taken) {
  const path = "src/js/redirect-resources.js";
  const file = join(tmp, path);
  mkdirSync(dirname(file), { recursive: true });
  writeFileSync(file, await text(`https://raw.githubusercontent.com/${REPO}/${tag}/${path}`));
  const { default: map } = await import(pathToFileURL(file).href);
  const out = [];
  for (const [name, details] of map) {
    if (details.params || taken.has(name)) continue;
    const extension = name.includes(".") ? name.slice(name.lastIndexOf(".") + 1) : "";
    const mime = name === "empty" ? "text/plain" : MIME[extension];
    if (!mime) continue;
    const response = await get(`https://raw.githubusercontent.com/${REPO}/${tag}/src/web_accessible_resources/${name}`);
    const aliases = [details.alias ?? []].flat().filter((alias) => !taken.has(alias));
    out.push({
      name,
      aliases,
      kind: { mime },
      content: Buffer.from(await response.arrayBuffer()).toString("base64"),
      dependencies: [],
      permission: details.requiresTrust ? TRUSTED : 0,
    });
  }
  const all = new Set(out.flatMap((resource) => [resource.name, ...resource.aliases]));
  for (const required of ["noopjs", "google-ima.js", "noop-vast4.xml", "noopmp3-0.1s", "nooptext", "1x1.gif"]) {
    if (!all.has(required)) throw new Error(`нет заглушки ${required}`);
  }
  return out;
}

rmSync(OUT, { recursive: true, force: true });
mkdirSync(OUT, { recursive: true });
const files = {};
const put = (name, content) => {
  writeFileSync(join(OUT, name), content);
  files[name] = { size: Buffer.byteLength(content), sha256: createHash("sha256").update(content).digest("hex") };
};

// Свои исправления поломок — в конец списка исправлений: браузер уже на него
// подписан, и правка доходит обновлением фильтров.
const FIXES = readFileSync(new URL("./190x4-fixes.txt", import.meta.url), "utf8");
for (const name of LISTS) {
  const url = `${LIST_BASE}${name}.txt`;
  let list = await preprocess(await text(url), url);
  if (list.split("\n").length < 50) throw new Error(`список ${name} подозрительно короткий`);
  if (name === "filters") {
    for (const extra of UBO_EXTRA) {
      const extraUrl = `${LIST_BASE}${extra}.txt`;
      list += `\n${await preprocess(await text(extraUrl), extraUrl)}`;
    }
  }
  put(`ubo-${name}.txt`, name === "unbreak" ? `${list}\n${FIXES}` : list);
}
for (const [name, urls] of Object.entries(BASE_LISTS)) {
  const parts = [];
  for (const url of urls) parts.push(await preprocess(await text(url), url));
  const list = parts.join("\n");
  if (list.split("\n").length < (MIN_LINES[name] ?? 1000)) throw new Error(`список ${name} подозрительно короткий`);
  put(name, list);
}
const { tag, resources } = await scriptlets();
put("resources.json", JSON.stringify(resources));
put(
  "NOTICE.txt",
  `Filter lists, scriptlet and redirect resources in this package come from uBlock Origin
(https://github.com/gorhill/uBlock, release ${tag}) and uAssets
(https://github.com/uBlockOrigin/uAssets). They are licensed under the GNU
General Public License v3.0; the source is available at those addresses.
easylist.txt, easyprivacy.txt and ruadlist.txt (RU AdList) come from EasyList
(https://easylist.to/pages/licence.html) and are dual-licensed under the GNU
General Public License v3.0 and Creative Commons Attribution-ShareAlike 3.0.
ruadlist.txt also includes RU AdList JS Fixes (https://github.com/easylist/ruadlist).
adguard-russian.txt is AdGuard Russian filter in its uBlock Origin syntax
(https://github.com/AdguardTeam/AdguardFilters), licensed under the GNU General
Public License v3.0.
brave.txt and parts of cookies.txt come from Brave (https://github.com/brave/adblock-lists),
licensed under the Mozilla Public License 2.0. cookies.txt and mobile-promo.txt
include Fanboy's lists (https://secure.fanboy.co.nz), Creative Commons
Attribution 3.0. urlhaus.txt is malware-filter's URLhaus list
(https://gitlab.com/malware-filter), CC0 and MIT.
The lists are preprocessed for 190x4 Browser: conditional directives resolved
and includes inlined. The rules after "Исправления 190x4 Browser" at the end of
ubo-unbreak.txt are 190x4 Browser's own.
`
);
writeFileSync(
  join(OUT, "manifest.json"),
  JSON.stringify({ generated_at: new Date().toISOString(), ubo: tag, files }, null, 2)
);
console.log(`uBlock Origin ${tag}: ${resources.length} resources, ${resources.filter((r) => r.permission).length} trusted`);
for (const [name, file] of Object.entries(files)) console.log(`  ${name} ${file.size}`);
