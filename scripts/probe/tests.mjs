// Живая проверка правок аудита на сборке в отдельном профиле.
import { check, chromeWindows, connect, sleep, summary, targets, waitTarget } from "./cdp.mjs";

const T = "file:///E:/test/probe/t/";
const only = process.argv[2] ?? "all";
const want = (name) => only === "all" || only.split(",").includes(name);

const [win] = await chromeWindows();
const chrome = await connect(win);
const ui = (expr) => chrome.evaluate(`(async () => { ${expr} })()`);
const S = `const { state } = await import('./js/state.js');`;
const TABS = `const tabs = await import('./js/tabs.js');`;
const invoke = (cmd, args = {}) => ui(`return window.__TAURI__.core.invoke(${JSON.stringify(cmd)}, ${JSON.stringify(args)});`);

async function openTab(url, opts = {}) {
  return ui(`${TABS} return tabs.open(${JSON.stringify(url)}, ${JSON.stringify(opts)});`);
}
async function tabInfo() {
  return ui(`${S} return { active: state.activeId, split: state.splitId, swapped: state.swapped, tabs: [...state.tabs.values()].map(t => ({ id: t.id, url: t.url, title: t.title, opener: t.opener ?? null, loading: t.loading })) };`);
}
async function tabTarget(part) {
  const t = await waitTarget((t) => t.type === "page" && t.url.includes(part));
  return connect(t);
}

await ui(`${S} for (let i = 0; i < 50 && !state.tabs.size; i++) await new Promise(r => setTimeout(r, 200)); return state.tabs.size;`);

if (want("popup")) {
  const openerId = await openTab(`${T}opener.html`);
  await sleep(1500);
  const page = await tabTarget("opener.html");
  const opened = await page.evaluate(`(() => { window.w = window.open('popup.html?a'); return window.w !== null; })()`, { gesture: true });
  check(opened, "window.open со щелчком возвращает окно");
  await sleep(2500);
  const title = await page.evaluate("document.title");
  check(String(title).includes("hi:a"), "окно-вкладка пишет в window.opener", title);
  let info = await tabInfo();
  const pop = info.tabs.find((t) => String(t.url).includes("popup.html?a"));
  check(Boolean(pop), "окно открылось вкладкой");
  check(pop?.opener === openerId, "у вкладки записан открывший", `${pop?.opener} vs ${openerId}`);
  check(String(pop?.title).includes("opener=true"), "у страницы окна есть window.opener", pop?.title);

  // Окно само закрывается window.close(): вкладка уходит, фокус — открывшему.
  await ui(`${TABS} await tabs.activate(${openerId}); return true;`);
  await page.evaluate(`(() => { window.open('popup.html?close'); return true; })()`, { gesture: true });
  await sleep(3000);
  info = await tabInfo();
  check(!info.tabs.some((t) => String(t.url).includes("popup.html?close")), "window.close() закрывает вкладку окна");
  check(info.active === openerId, "после window.close() активна вкладка, открывшая окно", `${info.active}`);
  check(String(await page.evaluate("document.title")).includes("hi:close"), "закрывшееся окно успело ответить открывшему");

  // Пустое окно и адрес следом — частый приём оплаты и входа.
  await page.evaluate(`(() => { const w = window.open(''); w.location = 'popup.html?blank'; return true; })()`, { gesture: true });
  await sleep(2500);
  check(String(await page.evaluate("document.title")).includes("hi:blank"), "window.open('') + location работает");

  // Окно без щелчка — блокируется.
  await ui(`${TABS} await tabs.activate(${openerId}); return true;`);
  const blocked = await page.evaluate(`(() => window.open('popup.html?blocked') === null)()`);
  await sleep(800);
  check(blocked, "window.open без щелчка получает null");
  info = await tabInfo();
  check(!info.tabs.some((t) => String(t.url).includes("blocked")), "заблокированное окно не открылось вкладкой");
  const chip = await ui(`${S} return { count: state.blockedPopups.get(${openerId})?.length ?? 0, hidden: document.getElementById('omni-popups').hidden };`);
  check(chip.count === 1 && !chip.hidden, "значок заблокированного окна в адресной строке", JSON.stringify(chip));

  // Ссылка на файл в новой вкладке: вкладка закрывается, загрузка остаётся.
  const before = (await tabInfo()).tabs.length;
  await page.evaluate(`(() => { document.getElementById('dl').click(); return true; })()`, { gesture: true });
  await sleep(4000);
  info = await tabInfo();
  check(!info.tabs.some((t) => String(t.url).includes("blob.bin")), "вкладка ссылки на файл закрылась", `tabs ${before}→${info.tabs.length}`);
  const downloads = await invoke("downloads_list", { limit: 10 });
  check(downloads.some((d) => String(d.path).includes("blob") && d.state === "done"), "файл скачался", JSON.stringify(downloads.slice(0, 2).map((d) => [d.path, d.state])));
  page.close();
}

if (want("history")) {
  await openTab(`${T}ticker.html`);
  await sleep(5500);
  const visits = await invoke("history_page", { query: "ticker", limit: 20 });
  check(visits.length === 1, "смена заголовка не плодит посещения", `visits=${visits.length}`);
  check(visits[0]?.title === "tick 8", "в истории последний заголовок", visits[0]?.title);
}

if (want("ghost")) {
  const id = await ui(`${TABS} const id = await tabs.open(${JSON.stringify(`${T}ticker.html?ghost`)}); await tabs.close(id); return id;`);
  await sleep(2500);
  const info = await tabInfo();
  check(!info.tabs.some((t) => t.id === id), "вкладка, закрытая сразу, не вернулась");
  const alive = await invoke("tab_activate", { id }).then(() => true, () => false);
  check(!alive, "движок не держит страницу закрытой вкладки");
  const list = await targets();
  check(!list.some((t) => t.url.includes("ticker.html?ghost")), "в движке нет страницы-призрака");

  const id2 = await openTab(`${T}ticker.html?loading`);
  await sleep(250);
  await ui(`${TABS} await tabs.close(${id2}); return true;`);
  await sleep(2500);
  check(!(await tabInfo()).tabs.some((t) => t.id === id2), "вкладка, закрытая во время загрузки, не вернулась");
}

if (want("fullscreen")) {
  const id = await openTab(`${T}fs.html`);
  await sleep(1500);
  const page = await tabTarget("fs.html");
  await page.evaluate(`document.documentElement.requestFullscreen().then(() => true)`, { gesture: true });
  await sleep(1200);
  let fs = await ui(`return { mode: document.documentElement.dataset.fullscreen ?? null, window: await window.__TAURI__.window.getCurrentWindow().isFullscreen(), stage: document.getElementById('stage').getBoundingClientRect().top, tab: ${id} };`);
  check(fs.mode === "true" && fs.window, "видео во весь экран: окно и интерфейс", JSON.stringify(fs));
  check(fs.stage === 0, "страница занимает окно от верхнего края", `top=${fs.stage}`);
  await page.evaluate(`document.exitFullscreen().then(() => true)`);
  await sleep(1200);
  fs = await ui(`return { mode: document.documentElement.dataset.fullscreen ?? null, window: await window.__TAURI__.window.getCurrentWindow().isFullscreen() };`);
  check(fs.mode === null && !fs.window, "выход из полноэкранного режима возвращает окно", JSON.stringify(fs));
  page.close();
}

if (want("omnibox")) {
  await openTab(`${T}opener.html?omni`);
  await sleep(1200);
  await ui(`const f = document.getElementById('omni-field'); document.getElementById('omni-url').click(); f.value = 'node.js'; f.dispatchEvent(new Event('input')); await new Promise(r => setTimeout(r, 50)); f.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter' })); return true;`);
  await sleep(2000);
  const info = await tabInfo();
  const active = info.tabs.find((t) => t.id === info.active);
  check(String(active?.url).startsWith("https://duckduckgo.com/?q=node.js"), "«node.js» ищется, а не открывается", active?.url);

  await openTab(`${T}%D1%82%D0%B5%D1%81%D1%82.html`);
  await sleep(1500);
  const shown = await ui(`return document.getElementById('omni-url').textContent;`);
  check(String(shown).includes("тест.html"), "кириллица в адресе показывается буквами", shown);
}

if (want("split")) {
  const a = await openTab(`${T}opener.html?left`);
  const b = await openTab(`${T}opener.html?right`);
  await sleep(1500);
  await ui(`${TABS} await tabs.activate(${a}); await tabs.splitWith(${b}); return true;`);
  let info = await tabInfo();
  check(info.active === a && info.split === b && !info.swapped, "разделённый экран: A слева, B справа", JSON.stringify([info.active, info.split, info.swapped]));
  await ui(`${TABS} await tabs.activate(${b}); return true;`);
  info = await tabInfo();
  check(info.active === b && info.split === a && info.swapped, "щелчок по второй половине делает её активной, экран не распадается", JSON.stringify([info.active, info.split, info.swapped]));
  // Проверка раскладки движка: активная вкладка по-прежнему справа.
  const left = await tabTarget("opener.html?left");
  const right = await tabTarget("opener.html?right");
  const lw = await left.evaluate("window.innerWidth");
  const rw = await right.evaluate("window.innerWidth");
  const stage = await ui(`return document.getElementById('stage').getBoundingClientRect().width;`);
  check(lw < stage * 0.6 && rw < stage * 0.6, "обе половины видны по половине ширины", `${lw}/${rw} of ${stage}`);
  await ui(`${TABS} await tabs.endSplit(); return true;`);
  info = await tabInfo();
  check(info.split === null && info.active === b, "выход из разделения оставляет активную вкладку", JSON.stringify([info.active, info.split]));
  left.close();
  right.close();
}

if (want("stop")) {
  const id = await openTab("http://10.255.255.1/");
  await sleep(1500);
  let loading = await ui(`${S} return { loading: state.tabs.get(${id})?.loading, button: document.getElementById('nav-reload').dataset.loading, title: document.getElementById('nav-reload').title };`);
  check(loading.loading && loading.button === "true", "пока грузится, кнопка — «Остановить»", JSON.stringify(loading));
  await invoke("tab_action", { id, action: "stop" });
  await sleep(1200);
  loading = await ui(`${S} return { loading: state.tabs.get(${id})?.loading, button: document.getElementById('nav-reload').dataset.loading };`);
  check(!loading.loading && loading.button === "false", "«Остановить» прерывает загрузку", JSON.stringify(loading));
}

chrome.close();
process.exit(summary() ? 1 : 0);
