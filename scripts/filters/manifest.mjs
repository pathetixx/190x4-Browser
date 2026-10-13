#!/usr/bin/env node
// Манифест канала фильтров — заново, из файлов выкладки, в задаче публикации
// (`filters.yml`, publish). Сборка списков исполняет код uBlock Origin, и её
// manifest.json не подписывается: подписанный байт в байт файл, который
// собрала она, значил бы подпись на что угодно. Здесь манифест пишет свой
// код: проверенные имена, размеры и SHA-256 файлов, время — по часам задачи.
//
// Использование: node scripts/filters/manifest.mjs <каталог выкладки>

import { createHash } from "node:crypto";
import { lstatSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";

const DIR = resolve(process.argv[2] ?? "filters-out");
// Тот же предел, что у браузера (src-tauri/src/filters.rs): больше он не скачает.
const FILE_LIMIT = 32 * 1024 * 1024;
const MANIFEST_LIMIT = 256 * 1024;
const MAX_FILES = 64;
// Без этих файлов выкладка неполная: ресурсы скриптлетов и лицензии.
const REQUIRED = ["resources.json", "NOTICE.txt"];
// Имя — как проверяет браузер: без путей и скрытых файлов.
const NAME = /^[A-Za-z0-9_-][A-Za-z0-9._-]*$/;

const fail = (message) => {
  console.error(`manifest: ${message}`);
  process.exit(1);
};

// Версия uBlock Origin из манифеста сборки — только для сведения, и только
// если похожа на версию.
let ubo;
try {
  const built = JSON.parse(readFileSync(join(DIR, "manifest.json"), "utf8"));
  if (typeof built.ubo === "string" && /^[\w.-]{1,32}$/.test(built.ubo)) ubo = built.ubo;
} catch {
  // Манифеста сборки нет — версия не обязательна.
}

const files = {};
const names = readdirSync(DIR)
  .filter((name) => name !== "manifest.json" && !name.endsWith(".sig"))
  .sort();
if (names.length > MAX_FILES) fail(`слишком много файлов: ${names.length}`);
for (const name of names) {
  if (!NAME.test(name)) fail(`недопустимое имя файла: ${name}`);
  const stat = lstatSync(join(DIR, name));
  if (!stat.isFile()) fail(`не обычный файл: ${name}`);
  if (stat.size === 0 || stat.size > FILE_LIMIT) fail(`недопустимый размер ${name}: ${stat.size}`);
  const content = readFileSync(join(DIR, name));
  files[name] = { size: content.length, sha256: createHash("sha256").update(content).digest("hex") };
}
for (const name of REQUIRED) if (!files[name]) fail(`нет файла ${name}`);
try {
  JSON.parse(readFileSync(join(DIR, "resources.json"), "utf8"));
} catch (error) {
  fail(`resources.json не разбирается: ${error.message}`);
}

const manifest = JSON.stringify({ generated_at: new Date().toISOString(), ...(ubo ? { ubo } : {}), files }, null, 2);
if (Buffer.byteLength(manifest) > MANIFEST_LIMIT) fail("манифест слишком большой");
writeFileSync(join(DIR, "manifest.json"), manifest);
console.log(`manifest.json: ${names.length} файлов${ubo ? `, uBlock Origin ${ubo}` : ""}`);
