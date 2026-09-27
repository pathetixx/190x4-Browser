// Живая проверка правок аудита 27.09 на пробе в отдельном профиле.
// node scripts/probe/lean.mjs [window-close,private,http-error,shield]
import { check, chromeWindows, connect, sleep, summary, targets, waitTarget } from "./cdp.mjs";

const T = "file:///E:/test/probe/t/";
const only = process.argv[2] ?? "all";
const want = (name) => only === "all" || only.split(",").includes(name);

const [win] = await chromeWindows();
const chrome = await connect(win);
const ui = (expr, target = chrome) => target.evaluate(`(async () => { ${expr} })()`);
const S = `const { state } = await import('./js/state.js');`;
const TABS = `const tabs = await import('./js/tabs.js');`;
const invoke = (cmd, args = {}, target = chrome) =>
  ui(`return window.__TAURI__.core.invoke(${JSON.stringify(cmd)}, ${JSON.stringify(args)});`, target);

await ui(`${S} for (let i = 0; i < 50 && !state.tabs.size; i++) await new Promise(r => setTimeout(r, 200)); return state.tabs.size;`);

/** Открыть окно с адресом и вернуть подключение к его интерфейсу. */
async function openWindow(url, { private: incognito = false } = {}) {
  const before = (await chromeWindows()).map((t) => t.id);
  await invoke("window_open", { private: incognito, url });
  await sleep(3500);
  const added = (await chromeWindows()).find((t) => !before.includes(t.id));
  return added ? { target: added, ui: await connect(added) } : null;
}

/** Закрыть окно без вопросов: оно само на вызов уже не ответит. */
async function closeWindow(opened) {
  await Promise.race([invoke("window_command", { action: "close_force" }, opened.ui).catch(() => {}), sleep(1500)]);
  opened.ui.close();
  await sleep(2500);
}

// 1. Закрытое окно закрывает вебвью своих вкладок: страница не живёт невидимой.
if (want("window-close")) {
  const opened = await openWindow(`${T}ticker.html?leak`);
  check(Boolean(opened), "второе окно открылось");
  if (opened) {
    await waitTarget((t) => t.url.includes("ticker.html?leak"));
    await closeWindow(opened);
    const alive = (await targets()).some((t) => t.url.includes("ticker.html?leak"));
    check(!alive, "вкладка закрытого окна закрылась вместе с ним");
  }
}

// 2. Следующее приватное окно начинается с чистой сессии.
if (want("private")) {
  const url = "https://example.com/?x4private";
  const first = await openWindow(url, { private: true });
  check(Boolean(first), "приватное окно открылось");
  if (first) {
    const page = await connect(await waitTarget((t) => t.url.includes("x4private")));
    await page.evaluate(`(() => { document.cookie = "x4probe=1; max-age=3600; path=/"; return document.cookie; })()`);
    page.close();
    await closeWindow(first);
    const second = await openWindow(url, { private: true });
    if (second) {
      const again = await connect(await waitTarget((t) => t.url.includes("x4private")));
      await sleep(1500);
      const cookie = await again.evaluate("document.cookie");
      check(!String(cookie).includes("x4probe"), "куки прежнего приватного окна не дожили до нового", String(cookie));
      again.close();
      await closeWindow(second);
    }
  }
}

// 3. Страница сайта с кодом ошибки остаётся страницей сайта.
if (want("http-error")) {
  const id = await ui(`${TABS} return tabs.open("https://github.com/pathetixx/190x4-page-that-does-not-exist");`);
  const page = await connect(await waitTarget((t) => t.url.includes("190x4-page-that-does-not-exist")));
  await sleep(5000);
  const replaced = await page.evaluate(`Boolean(document.querySelector('.box190x4'))`);
  const title = await page.evaluate("document.title");
  check(!replaced, "404 сайта не заменена страницей «Страница не открылась»", String(title));
  page.close();
  await ui(`${TABS} await tabs.close(${id}, { force: true }); return 1;`);
}

// 4. Щит считает всю пачку блокировок, а не её первую часть.
if (want("shield")) {
  const id = await ui(`${TABS} return tabs.open(${JSON.stringify(`${T}burst.html`)});`);
  await sleep(2500);
  const blocked = await ui(`${S} return state.tabs.get(${id})?.blocked ?? -1;`);
  check(blocked === 12, "счётчик щита — все 12 блокировок пачки", String(blocked));
  await ui(`${TABS} await tabs.close(${id}, { force: true }); return 1;`);
}

chrome.close();
process.exit(summary() ? 1 : 0);
