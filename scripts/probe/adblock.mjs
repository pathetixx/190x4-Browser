// Живая проверка блокировщика рекламы на пробе в отдельном профиле.
// node scripts/probe/adblock.mjs [frames,redirect,sites] [адрес,адрес…]
//
// frames и redirect — страница `pages/adblock.html` (положить в E:\test\probe\t\):
// плееры чужих сайтов во фреймах, правила с заглушками.
// sites — Яндекс, Mail.ru, Дзен, ВК, OK (или адреса вторым аргументом) с
// блокировкой и без: сколько закрыто запросов, сколько видно рекламных фреймов
// и подписей «Реклама». Входы на сайты — из профиля пробы.
import { check, chromeWindows, connect, sleep, summary, waitTarget } from "./cdp.mjs";

const T = "file:///E:/test/probe/t/";
const only = process.argv[2] ?? "all";
const want = (name) => only === "all" || only.split(",").includes(name);

const [win] = await chromeWindows();
const chrome = await connect(win);
const ui = (expr) => chrome.evaluate(`(async () => { ${expr} })()`);
const S = `const { state } = await import('./js/state.js');`;
const TABS = `const tabs = await import('./js/tabs.js');`;
const invoke = (cmd, args = {}) =>
  ui(`return window.__TAURI__.core.invoke(${JSON.stringify(cmd)}, ${JSON.stringify(args)});`);

await ui(`${S} for (let i = 0; i < 50 && !state.tabs.size; i++) await new Promise(r => setTimeout(r, 200)); return state.tabs.size;`);

const openTab = (url) => ui(`${TABS} return tabs.open(${JSON.stringify(url)});`);
const closeTab = (id) => ui(`${TABS} await tabs.close(${id}, { force: true }); return 1;`);
const blockedOn = (id) => ui(`${S} return state.tabs.get(${id})?.blocked ?? 0;`);

/** Отработал ли в документе цели исполнитель косметики. */
const MARKED = `Boolean(window[Symbol.for("x4cosmetic")])`;

async function frameTarget(part, timeout = 20000) {
  try {
    return await waitTarget((t) => t.type === "iframe" && t.url.includes(part), timeout);
  } catch {
    return null;
  }
}

// 1. Плеер чужого сайта во фрейме получает скриптлеты и косметику своего сайта,
//    а запросы из чужого фрейма проходят через фильтр.
if (want("frames") || want("redirect")) {
  const id = await openTab(`${T}adblock.html`);
  const page = await connect(await waitTarget((t) => t.url.includes("adblock.html")));
  await sleep(12000);

  if (want("frames")) {
    for (const [name, part] of [
      ["ВК", "vk.com/video_ext"],
      ["YouTube", "youtube.com/embed"],
    ]) {
      const target = await frameTarget(part);
      if (!target) {
        check(false, `фрейм ${name} виден пробе`);
        continue;
      }
      const frame = await connect(target);
      const marked = await frame.evaluate(MARKED).catch(() => false);
      const sheets = await frame.evaluate("document.adoptedStyleSheets.length").catch(() => 0);
      check(marked, `фрейм ${name} получил скрипт косметики`, `листов стилей: ${sheets}`);
      frame.close();
    }

    const blocked = await blockedOn(id);
    check(blocked > 0, "щит вкладки считает блокировки из фреймов", `закрыто: ${blocked}`);
  }

  if (want("redirect")) {
    const result = await page.evaluate("JSON.stringify(window.__redirect)");
    const parsed = JSON.parse(result ?? "{}");
    check(parsed.noopjs === "loaded", "скрипт с правилом redirect=noopjs получил заглушку", result);
    check(parsed.nooptext === "status 200", "XHR с правилом redirect=noop.txt получил заглушку", result);
  }

  page.close();
  await closeTab(id);
}

// 2. Настоящие сайты: блокировка выключена и включена.
const SITES = process.argv[3]?.split(",") ?? [
  "https://ya.ru/",
  "https://mail.ru/",
  "https://dzen.ru/",
  "https://vk.ru/feed",
  "https://vkvideo.ru/",
  "https://ok.ru/video",
];
const AD_FRAME = /an\.yandex|yandex\.ru\/ads|adfox|ad\.mail\.ru|r\.mail\.ru|target\.my\.com|ads\.vk|vk\.com\/ads|doubleclick|googlesyndication|safeframe/;

const MEASURE = `(() => {
  const visible = (el) => { const r = el.getBoundingClientRect(); return r.width >= 50 && r.height >= 30 && getComputedStyle(el).visibility !== "hidden"; };
  const adFrames = [...document.querySelectorAll("iframe")].filter((f) => visible(f) && ${AD_FRAME}.test(f.src || "")).length;
  const labels = [...document.querySelectorAll("body *")].filter((el) =>
    el.childElementCount === 0 && /^\\s*(Реклама|Реклама\\s*18\\+|Яндекс\\s*Директ|Промо)\\s*$/i.test(el.textContent || "") && el.offsetParent !== null
  ).length;
  return { adFrames, labels, marked: ${MARKED}, sheets: document.adoptedStyleSheets.length };
})()`;

async function measure(url) {
  const id = await openTab(url);
  const host = new URL(url).hostname.replace(/^www\./, "");
  let page = null;
  try {
    page = await connect(await waitTarget((t) => t.type === "page" && t.url.includes(host), 20000));
  } catch {
    await closeTab(id);
    return null;
  }
  await sleep(15000);
  // Лента догружает рекламу при прокрутке.
  await page.evaluate("window.scrollBy(0, 2000)").catch(() => {});
  await sleep(4000);
  const metrics = await page.evaluate(MEASURE).catch(() => null);
  const blocked = await blockedOn(id);
  page.close();
  await closeTab(id);
  return metrics && { ...metrics, blocked };
}

if (want("sites")) {
  const rows = [];
  try {
    for (const url of SITES) {
      await invoke("adblock_set_enabled", { on: false });
      const off = await measure(url);
      await invoke("adblock_set_enabled", { on: true });
      const on = await measure(url);
      rows.push({ url, off, on });
      if (!on) {
        check(false, `${url} открылся`);
        continue;
      }
      check(on.marked, `${url}: косметика на странице`, `листов стилей: ${on.sheets}`);
      check(on.blocked > 0, `${url}: запросы закрываются`, `закрыто: ${on.blocked}`);
      check(on.adFrames === 0, `${url}: рекламных фреймов не видно`, `без блокировки: ${off?.adFrames ?? "?"}, с ней: ${on.adFrames}`);
      check(
        on.labels <= (off?.labels ?? on.labels),
        `${url}: подписей «Реклама» не больше, чем без блокировки`,
        `без блокировки: ${off?.labels ?? "?"}, с ней: ${on.labels}`
      );
    }
  } finally {
    await invoke("adblock_set_enabled", { on: true });
  }
  console.log("\nсайт · закрыто / рекламных фреймов / подписей «Реклама» — без блокировки → с ней");
  for (const { url, off, on } of rows) {
    const cell = (m) => (m ? `${m.blocked} / ${m.adFrames} / ${m.labels}` : "—");
    console.log(`  ${url.padEnd(24)} ${cell(off)}  →  ${cell(on)}`);
  }
  console.log("статистика фильтра:", JSON.stringify(await invoke("adblock_stats")));
}

chrome.close();
process.exit(summary() ? 1 : 0);
