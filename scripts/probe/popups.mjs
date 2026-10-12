// Сравнение с Brave на сайтах-плеерах: открывается ли страница, сколько окон
// и переходов рождают щелчки по плееру (в том числе мгновенно закрытых).
// node scripts/probe/popups.mjs [ours|brave|both] адрес,адрес…
//
// Наша проба — порт 19333 (BROWSER190X4_DEBUG_PORT), Brave — 19334
// (`brave-launch.ps1`, чистый профиль, Shields по умолчанию). Щелчки — события
// CDP внутри страницы, мышь пользователя не трогают.
import { chromeWindows, connect, sleep } from "./cdp.mjs";

const which = process.argv[2] ?? "both";
const SITES = (process.argv[3] ?? "").split(",").filter(Boolean);
const CLICKS = 4;

async function list(base) {
  return (await fetch(`${base}/json`)).json();
}

/** Опрашивает список целей, пока идут щелчки: ловит и окна, живущие доли секунды. */
function watchTargets(base, before) {
  const seen = new Map();
  let on = true;
  (async () => {
    while (on) {
      try {
        for (const t of await list(base)) {
          if (t.type === "page" && !before.has(t.id) && !seen.has(t.id)) seen.set(t.id, t.url);
        }
      } catch {}
      await sleep(50);
    }
  })();
  return () => {
    on = false;
    return [...seen.values()];
  };
}

const PLAYER = `(() => {
  const all = [...document.querySelectorAll("video, iframe, canvas, .jw-wrapper, .plyr, #player, [id*=player]")];
  let best = null, area = 0;
  for (const el of all) {
    const r = el.getBoundingClientRect();
    if (r.width * r.height > area && r.width > 100 && r.height > 80) { best = r; area = r.width * r.height; }
  }
  const r = best ?? { left: 0, top: 0, width: innerWidth, height: innerHeight };
  return { x: Math.round(r.left + r.width / 2), y: Math.round(r.top + r.height / 2), found: Boolean(best) };
})()`;

async function click(page, x, y) {
  for (const type of ["mouseMoved", "mousePressed", "mouseReleased"]) {
    await page.send("Input.dispatchMouseEvent", { type, x, y, button: "left", clickCount: 1 });
  }
}

/** Видимость документа живого окна: заглушка рекламы у нас — `hidden`. */
async function visibility(target) {
  try {
    const page = await connect(target);
    const state = await page.evaluate("document.visibilityState");
    page.close();
    return state;
  } catch {
    return "?";
  }
}

async function run(name, base, open, closeTab, tabCount) {
  console.log(`\n=== ${name}`);
  for (const url of SITES) {
    const tab = await open(url);
    await sleep(12000);
    const page = await connect(tab);
    const info = await page
      .evaluate(`({ href: location.href, title: document.title, text: (document.body?.innerText ?? "").slice(0, 120).replace(/\\s+/g, " ") })`)
      .catch((e) => ({ error: String(e) }));
    console.log(`${url}\n  страница: ${JSON.stringify(info)}`);
    const pos = await page.evaluate(PLAYER).catch(() => ({ x: 400, y: 300, found: false }));
    const before = new Set((await list(base)).map((t) => t.id));
    const tabsBefore = tabCount ? await tabCount() : null;
    const stop = watchTargets(base, before);
    const hrefs = [];
    for (let i = 0; i < CLICKS; i++) {
      await click(page, pos.x, pos.y);
      await sleep(2500);
      hrefs.push(await page.evaluate("location.href").catch(() => "?"));
    }
    const born = stop();
    const aliveTargets = (await list(base)).filter((t) => t.type === "page" && !before.has(t.id));
    const alive = aliveTargets.map((t) => t.url);
    const states = await Promise.all(aliveTargets.map(visibility));
    const shown = states.filter((s) => s === "visible").length;
    console.log(`  щелчки по ${pos.found ? "плееру" : "центру"} (${pos.x},${pos.y}): окон родилось ${born.length}, живы ${alive.length} (видимых ${shown}, скрытых ${states.length - shown})`);
    if (tabCount) console.log(`  вкладок в строке: было ${tabsBefore}, стало ${await tabCount()}`);
    for (const u of born) console.log(`    + ${u.slice(0, 140)}`);
    const moved = hrefs.filter((h) => h !== info.href);
    if (moved.length) console.log(`  вкладка ушла: ${[...new Set(moved)].join(" | ").slice(0, 200)}`);
    page.close();
    if (process.env.KEEP) continue;
    await closeTab(tab).catch(() => {});
    for (const t of (await list(base)).filter((t) => t.type === "page" && !before.has(t.id))) {
      await closeTab(t).catch(() => {});
    }
  }
}

if (which === "ours" || which === "both") {
  const base = "http://127.0.0.1:19333";
  const [win] = await chromeWindows();
  const chrome = await connect(win);
  const ui = (expr) => chrome.evaluate(`(async () => { ${expr} })()`);
  console.log("фильтр:", JSON.stringify(await ui(`return window.__TAURI__.core.invoke("adblock_stats");`)));
  await run(
    "190x4",
    base,
    async (url) => {
      const before = new Set((await list(base)).map((t) => t.id));
      await ui(`const tabs = await import('./js/tabs.js'); return tabs.open(${JSON.stringify(url)});`);
      for (let i = 0; i < 50; i++) {
        const t = (await list(base)).find((t) => t.type === "page" && !before.has(t.id) && !t.url.startsWith("http://tauri.localhost"));
        if (t) return t;
        await sleep(200);
      }
      throw new Error("вкладка не появилась");
    },
    async (t) => {
      await ui(`const { state } = await import('./js/state.js'); const tabs = await import('./js/tabs.js');
        for (const [id, tab] of state.tabs) if (tab.url && ${JSON.stringify(t.url)}.startsWith(tab.url.slice(0, 30))) await tabs.close(id, { force: true });
        return 1;`);
    },
    () => ui(`const { state } = await import('./js/state.js'); return state.tabs.size;`),
  );
  console.log("фильтр:", JSON.stringify(await ui(`return window.__TAURI__.core.invoke("adblock_stats");`)));
  chrome.close();
}

if (which === "brave" || which === "both") {
  const base = "http://127.0.0.1:19334";
  await run(
    "Brave",
    base,
    async (url) => (await fetch(`${base}/json/new?${encodeURIComponent(url)}`, { method: "PUT" })).json(),
    async (t) => fetch(`${base}/json/close/${t.id}`),
  );
}
process.exit(0);
