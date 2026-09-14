#!/usr/bin/env node
// 190x4 Browser · релиз одной командой, по схеме Ninety. Две ручные переменные —
// номер версии и заметки — пишутся по одному разу и разъехаться не могут:
//   версия  → bump-version.mjs (tauri.conf.json, Cargo.toml, Cargo.lock);
//   заметки → секция CHANGELOG.md → аннотация тега → latest.json и описание релиза.
//
// Сборка — в CI: push тега запускает build.yml (сборка, подпись, GitLab, GitHub).
// Скрипт завершается сразу после push и за сборкой не следит. Проверка уже
// вышедшего релиза — отдельный режим --verify.
//
// Использование:
//   node scripts/release.mjs X.Y.Z [--dry-run] [--yes]
//   node scripts/release.mjs X.Y.Z --verify
// Перед запуском: секция "## vX.Y.Z — YYYY-MM-DD" в начале CHANGELOG.md.

import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { execFileSync } from "node:child_process";
import { createInterface } from "node:readline/promises";
import { setTimeout as sleep } from "node:timers/promises";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const die = (message) => { console.error(`✗ ${message}`); process.exit(1); };

function exec(cmd, args, { capture = false } = {}) {
  try {
    return execFileSync(cmd, args, {
      cwd: root, encoding: "utf8",
      stdio: capture ? ["ignore", "pipe", "pipe"] : "inherit",
    });
  } catch (error) {
    if (capture) throw error;
    die(`команда упала: ${cmd} ${args.join(" ")}`);
  }
}
const cap = (cmd, args) => exec(cmd, args, { capture: true }).trim();
const run = (cmd, args) => exec(cmd, args);

const REPO = "pathetixx/190x4-Browser";
const PACKAGE = "190x4-browser";
const GITLAB_PROJECT = "86438976";
const GH_LATEST = `https://github.com/${REPO}/releases/latest/download/latest.json`;
const GL_LATEST =
  `https://gitlab.com/api/v4/projects/${GITLAB_PROJECT}/packages/generic/${PACKAGE}/stable/latest.json`;

let failed = 0;
const check = (ok, label) => { console.log(`  ${ok ? "✓" : "✗"} ${label}`); if (!ok) failed++; };
const normalizedNotes = (value) => String(value ?? "")
  .split("\n").map((line) => line.trim()).filter(Boolean)
  .filter((line) => !/^#{1,6}\s+/u.test(line))
  .join("\n");

// OTA-адреса и CDN догоняют публикацию за несколько секунд.
async function retry(fn, tries = 8, delayMs = 5000) {
  for (let i = 0; i < tries; i++) {
    try { if (await fn()) return true; } catch { /* transient */ }
    if (i < tries - 1) await sleep(delayMs);
  }
  return false;
}

async function latest(url, version, urlPart) {
  let metadata = null;
  const ok = await retry(async () => {
    const response = await fetch(url);
    if (!response.ok) return false;
    const json = await response.json();
    const platform = json.platforms?.["windows-x86_64"];
    if (json.version !== version || !platform?.signature) return false;
    if (urlPart && !platform.url?.includes(urlPart)) return false;
    metadata = json;
    return true;
  });
  return ok ? metadata : null;
}

async function verifyRelease(version) {
  const tag = `v${version}`;
  console.log(`\nПроверка релиза ${tag}:`);

  let release;
  try {
    release = JSON.parse(cap("gh", ["release", "view", tag, "-R", REPO,
      "--json", "isDraft,isPrerelease,assets,body"]));
  } catch {
    check(false, "gh release view — релиз не найден");
    return;
  }
  check(!release.isDraft, "опубликован (не draft)");
  check(!release.isPrerelease, "не prerelease (иначе выпадет из Latest и сломает обновление)");
  const names = release.assets.map((asset) => asset.name);
  check(names.some((name) => /-setup\.exe$/.test(name)), "установщик (.exe)");
  check(names.some((name) => /-setup\.exe\.sig$/.test(name)), "подпись установщика (.sig)");
  check(names.includes("latest.json"), "latest.json");

  const gh = await latest(GH_LATEST, version);
  check(Boolean(gh), `GitHub: latest.json = ${version}, подпись есть (релиз стал Latest)`);
  const gl = await latest(GL_LATEST, version, `/packages/generic/${PACKAGE}/${version}/`);
  check(Boolean(gl), `GitLab (основной): latest.json = ${version}, неизменяемый адрес и подпись`);
  check(Boolean(gh && gl) && gh.notes === gl.notes, "GitHub и GitLab отдают одинаковые заметки");
  check(Boolean(gh) && normalizedNotes(release.body) === normalizedNotes(gh.notes),
    "описание релиза совпадает с заметками обновления");
  check(Boolean(gh && gl)
    && gh.platforms["windows-x86_64"].signature === gl.platforms["windows-x86_64"].signature,
    "GitHub и GitLab отдают одну подпись");
}

// --- аргументы ---
const argv = process.argv.slice(2);
const flags = new Set(argv.filter((arg) => arg.startsWith("--")));
const version = argv.find((arg) => !arg.startsWith("--"));
const known = new Set(["--dry-run", "--verify", "--yes"]);
const unknown = [...flags].filter((flag) => !known.has(flag));
if (unknown.length) die(`неизвестный флаг: ${unknown.join(", ")}`);
if (!version || !/^\d+\.\d+\.\d+$/.test(version))
  die(`нужна версия X.Y.Z (получено: ${version ?? "<none>"})`);
const tag = `v${version}`;

if (flags.has("--verify")) {
  await verifyRelease(version);
  console.log(failed ? `\n✗ проверка: ${failed} провал(ов)` : "\n✓ всё встало");
  process.exit(failed ? 1 : 0);
}

// --- предпроверки (только чтение) ---
const branch = cap("git", ["rev-parse", "--abbrev-ref", "HEAD"]);
if (branch !== "main") die(`ветка ${branch}, ожидалась main`);
run("git", ["fetch", "origin", branch]);
if (cap("git", ["rev-parse", "HEAD"]) !== cap("git", ["rev-parse", `origin/${branch}`]))
  die(`локальный ${branch} должен совпадать с origin/${branch}`);

// Коммитятся только файлы релиза: чужая грязь не должна уехать в релизный коммит.
const releaseNotesOnly = /^(?:[ MARC?][ MDARC?]|[MARC?]) CHANGELOG\.md$/;
const unexpected = cap("git", ["status", "--porcelain=v1"]).split("\n").filter(Boolean)
  .filter((line) => !releaseNotesOnly.test(line));
if (unexpected.length) die(`перед релизом разрешено менять только CHANGELOG.md:\n${unexpected.join("\n")}`);

if (cap("git", ["tag", "-l", tag])) die(`тег ${tag} уже существует локально`);
try {
  if (cap("git", ["ls-remote", "--tags", "origin", tag])) die(`тег ${tag} уже существует на origin`);
} catch { /* сетевой сбой ls-remote не фатален */ }

// --- заметки из CHANGELOG.md ---
const changelog = join(root, "CHANGELOG.md");
const lines = readFileSync(changelog, "utf8").split("\n");
const isHeader = (line) => /^## v\d+\.\d+\.\d+\b/.test(line);
const head = lines.findIndex(isHeader);
if (head === -1) die("в CHANGELOG.md нет секций версий");
const top = lines[head].match(/^## v(\d+\.\d+\.\d+)\b/)[1];
if (top !== version) die(`верхняя секция CHANGELOG.md — v${top}, а не ${tag}`);
let end = lines.findIndex((line, i) => i > head && isHeader(line));
if (end === -1) end = lines.length;
const notes = lines.slice(head + 1, end).join("\n").trim();
if (!notes) die(`секция ${tag} в CHANGELOG.md пустая`);

const today = new Date().toISOString().slice(0, 10);
const needDate = !/—\s*\d{4}-\d{2}-\d{2}/.test(lines[head]);

console.log(`\nРелиз ${tag}`);
if (needDate) console.log(`  дата в заголовке CHANGELOG → ${today}`);
console.log("  заметки:");
console.log(notes.split("\n").map((line) => `    ${line}`).join("\n"));
console.log(`\n  дальше: commit · git tag -a ${tag} · push origin main ${tag} · draft релиза`);
if (flags.has("--dry-run")) { console.log("\n(dry-run: ничего не записано)"); process.exit(0); }

if (!flags.has("--yes")) {
  const rl = createInterface({ input: process.stdin, output: process.stdout });
  const answer = (await rl.question(`\nПушим ${tag}? [y/N] `)).trim().toLowerCase();
  rl.close();
  if (answer !== "y" && answer !== "yes") die("отменено — ничего не запушено");
}

// --- выполнение ---
if (needDate) {
  lines[head] = `## ${tag} — ${today}`;
  writeFileSync(changelog, lines.join("\n"));
}
run("node", ["scripts/bump-version.mjs", version]);
run("git", ["add", "src-tauri/tauri.conf.json", "Cargo.toml", "Cargo.lock", "CHANGELOG.md"]);
if (cap("git", ["diff", "--cached", "--name-only"])) run("git", ["commit", "-m", tag]);
else console.log("  версия уже закоммичена — коммит пропущен");

const dir = mkdtempSync(join(tmpdir(), "190x4-browser-release-"));
const notesFile = join(dir, "notes.md");
writeFileSync(notesFile, `${notes}\n`);
// Без verbatim git выкинул бы Markdown-заголовки как комментарии.
run("git", ["tag", "-a", "--cleanup=verbatim", tag, "-F", notesFile]);
run("git", ["push", "origin", branch]);
run("git", ["push", "origin", tag]);
run("gh", ["release", "create", tag, "-R", REPO, "--draft", "--title", `190x4 Browser ${tag}`, "-F", notesFile]);
rmSync(dir, { recursive: true, force: true });
console.log(`\n✓ ${tag} запушен, draft создан. Сборку и публикацию делает CI.`);
