#!/usr/bin/env node
// 190x4 Browser · единая точка бампа версии. Число дублируется в трёх местах:
//   src-tauri/tauri.conf.json — версия сборки: из неё build.yml берёт номер
//     для latest.json и сверяет с тегом;
//   Cargo.toml                — [workspace.package] version, общая для крейтов;
//   Cargo.lock                — пакеты workspace (иначе cargo перепишет lock сам).
//
// Использование: node scripts/bump-version.mjs X.Y.Z

import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const version = process.argv[2];

if (!version || !/^\d+\.\d+\.\d+$/.test(version)) {
  console.error(`Usage: node scripts/bump-version.mjs X.Y.Z  (got: ${version ?? "<none>"})`);
  process.exit(1);
}

// Падает, если паттерн не найден: молчаливый рассинхрон опаснее явной ошибки.
function edit(rel, re, replacement) {
  const path = join(root, rel);
  const before = readFileSync(path, "utf8");
  const after = before.replace(re, replacement);
  if (!re.test(before)) {
    console.error(`! ${rel}: version pattern not found — aborting`);
    process.exit(1);
  }
  writeFileSync(path, after);
  console.log(`  ${rel}`);
}

edit("src-tauri/tauri.conf.json", /("version":\s*")\d+\.\d+\.\d+(")/, `$1${version}$2`);
edit("Cargo.toml", /(\[workspace\.package\]\r?\nversion\s*=\s*")\d+\.\d+\.\d+(")/, `$1${version}$2`);
// Пакеты workspace называются browser190x4*; сторонние крейты не трогаем.
edit("Cargo.lock", /(name = "browser190x4[\w-]*"\r?\nversion = ")\d+\.\d+\.\d+(")/g, `$1${version}$2`);
