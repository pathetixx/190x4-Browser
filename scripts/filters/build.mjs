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
//     доверенности (`permission`, тот же бит, что у доверенных списков).
//
// Использование: node scripts/filters/build.mjs <каталог>

import { createHash } from "node:crypto";
import { mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { dirname, join, normalize, resolve } from "node:path";
import { pathToFileURL } from "node:url";

const OUT = resolve(process.argv[2] ?? "filters-out");
const LISTS = ["filters", "quick-fixes", "privacy", "unbreak"];
const LIST_BASE = "https://ublockorigin.github.io/uAssets/filters/";
// Основные списки вшиты и в установщик, но обновлять их только с выпуском
// браузера — значит неделями жить со старыми правилами. Здесь они свежие, и
// браузер берёт скачанную копию вместо вшитой. RU AdList — без EasyList внутри.
const BASE_LISTS = {
  "easylist.txt": "https://easylist.to/easylist/easylist.txt",
  "easyprivacy.txt": "https://easylist.to/easylist/easyprivacy.txt",
  "ruadlist.txt": "https://easylist-downloads.adblockplus.org/advblock+cssfixes.txt",
};
const REPO = "gorhill/uBlock";
const TRUSTED = 1;

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
  rmSync(tmp, { recursive: true, force: true });
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
  return { tag, resources };
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
  const list = await preprocess(await text(url), url);
  if (list.split("\n").length < 50) throw new Error(`список ${name} подозрительно короткий`);
  put(`ubo-${name}.txt`, name === "unbreak" ? `${list}\n${FIXES}` : list);
}
for (const [name, url] of Object.entries(BASE_LISTS)) {
  const list = await preprocess(await text(url), url);
  if (list.split("\n").length < 1000) throw new Error(`список ${name} подозрительно короткий`);
  put(name, list);
}
const { tag, resources } = await scriptlets();
put("resources.json", JSON.stringify(resources));
put(
  "NOTICE.txt",
  `Filter lists and scriptlet resources in this package come from uBlock Origin
(https://github.com/gorhill/uBlock, release ${tag}) and uAssets
(https://github.com/uBlockOrigin/uAssets). They are licensed under the GNU
General Public License v3.0; the source is available at those addresses.
easylist.txt, easyprivacy.txt and ruadlist.txt (RU AdList) come from EasyList
(https://easylist.to/pages/licence.html) and are dual-licensed under the GNU
General Public License v3.0 and Creative Commons Attribution-ShareAlike 3.0.
The lists are preprocessed for 190x4 Browser: conditional directives resolved
and includes inlined. The rules after "Исправления 190x4 Browser" at the end of
ubo-unbreak.txt are 190x4 Browser's own.
`
);
writeFileSync(
  join(OUT, "manifest.json"),
  JSON.stringify({ generated_at: new Date().toISOString(), ubo: tag, files }, null, 2)
);
console.log(`uBlock Origin ${tag}: ${resources.length} scriptlets, ${resources.filter((r) => r.permission).length} trusted`);
for (const [name, file] of Object.entries(files)) console.log(`  ${name} ${file.size}`);
