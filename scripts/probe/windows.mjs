// Сессии окон: закрытое при других окнах не возвращается, «Закрыть браузер» — все.
import { check, chromeWindows, connect, sleep, summary } from "./cdp.mjs";

const T = "file:///E:/test/probe/t/";
const step = process.argv[2];

async function label(target) {
  const c = await connect(target);
  const value = await c.evaluate("window.__TAURI__.webviewWindow.getCurrentWebviewWindow().label");
  return { c, label: value };
}

if (step === "prepare") {
  const [first] = await chromeWindows();
  const c = await connect(first);
  const invoke = (cmd, args) => c.evaluate(`window.__TAURI__.core.invoke(${JSON.stringify(cmd)}, ${JSON.stringify(args)})`);
  await invoke("window_open", { private: false, url: `${T}popup.html?w2` });
  await sleep(3000);
  await invoke("window_open", { private: false, url: `${T}popup.html?w3` });
  await sleep(4000);
  let wins = await chromeWindows();
  check(wins.length === 3, "открыто три окна", `${wins.length}`);
  // Второе окно закрываем крестиком: при следующем запуске его быть не должно.
  for (const w of wins) {
    const { c: wc, label: l } = await label(w);
    if (l === "chrome-2") await wc.evaluate(`window.__TAURI__.core.invoke('window_command', { action: 'close' }).then(() => true)`).catch(() => {});
    wc.close();
  }
  await sleep(2500);
  wins = await chromeWindows();
  check(wins.length === 2, "окно закрылось", `${wins.length}`);
  // «Закрыть браузер»: оставшиеся два окна вернутся.
  await c.evaluate(`import('./js/actions.js').then(m => { m.closeBrowser(); return true; })`).catch(() => {});
  await sleep(2000);
} else if (step === "verify") {
  await sleep(2000);
  const wins = await chromeWindows();
  check(wins.length === 2, "после перезапуска вернулись два окна (без закрытого)", `${wins.length}`);
  const urls = [];
  for (const w of wins) {
    const c = await connect(w);
    const tabs = await c.evaluate(`import('./js/state.js').then(({ state }) => [...state.tabs.values()].map(t => t.url))`);
    urls.push(...tabs);
    c.close();
  }
  check(urls.some((u) => String(u).includes("w3")), "вкладки третьего окна восстановились", JSON.stringify(urls));
  check(!urls.some((u) => String(u).includes("w2")), "вкладки закрытого окна не вернулись");
  // Новое окно (Ctrl+N) открывается пустым.
  const [first] = await chromeWindows();
  const c = await connect(first);
  await Promise.race([c.evaluate(`window.__TAURI__.core.invoke("window_open", { private: false })`), sleep(8000)]);
  await sleep(4000);
  const now = await chromeWindows();
  let fresh = null;
  for (const w of now) {
    const x = await connect(w);
    const l = await x.evaluate("window.__TAURI__.webviewWindow.getCurrentWebviewWindow().label");
    if (!wins.some((old) => old.id === w.id)) fresh = await x.evaluate(`import('./js/state.js').then(({ state }) => [...state.tabs.values()].map(t => t.url))`);
    x.close();
  }
  check(Array.isArray(fresh) && fresh.every((u) => String(u).includes("newtab") || !u), "новое окно открылось пустым", JSON.stringify(fresh));
}
process.exit(summary() ? 1 : 0);
