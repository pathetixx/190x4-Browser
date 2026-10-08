// Проверка «Скрыть элемент» в headless Chromium: щелчок выбирает блок, «Шире»
// поднимается к родителю, «Скрыть» прячет его и шлёт браузеру правило.
//   chromium --headless=new --remote-debugging-port=9451 about:blank &
//   node scripts/test/picker.mjs 9451
import { readFileSync } from "node:fs";

const port = Number(process.argv[2] ?? 9451);
const root = new URL("../..", import.meta.url).pathname;
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
let list;
for (let i = 0; i < 60 && !list; i++) {
  try {
    list = await (await fetch(`http://127.0.0.1:${port}/json`)).json();
  } catch {
    await sleep(250);
  }
}
const ws = new WebSocket(list.find((t) => t.type === "page").webSocketDebuggerUrl);
await new Promise((r) => (ws.onopen = r));
let seq = 0;
const pending = new Map();
ws.onmessage = (m) => {
  const d = JSON.parse(m.data);
  if (d.id && pending.has(d.id)) pending.get(d.id)(d), pending.delete(d.id);
};
const send = (method, params = {}) =>
  new Promise((r) => {
    const id = ++seq;
    pending.set(id, r);
    ws.send(JSON.stringify({ id, method, params }));
  });
const evaluate = async (expression) => (await send("Runtime.evaluate", { expression, returnByValue: true })).result.result.value;
const click = async (x, y) => {
  for (const type of ["mousePressed", "mouseReleased"]) await send("Input.dispatchMouseEvent", { type, x, y, button: "left", clickCount: 1 });
  await sleep(150);
};

await send("Emulation.setDeviceMetricsOverride", { width: 900, height: 600, deviceScaleFactor: 1, mobile: false });
await send("Page.navigate", { url: `file://${root}scripts/test/picker-page.html` });
await sleep(800);
const ad = await evaluate(`JSON.stringify(document.querySelector(".promo-label").getBoundingClientRect())`);
const label = JSON.parse(ad);
const script = readFileSync(`${root}crates/webview/src/inject/picker.js`, "utf8").replaceAll(
  "__X4_PICKER__",
  JSON.stringify({ token: 7, x: label.x + 2, y: label.y + 2 })
);
await evaluate(script);
await sleep(200);

const checks = {};
checks.opened = await evaluate(`Boolean(document.querySelector("x4-picker"))`);
// Щелчок по ссылке страницы во время выбора не доходит до сайта.
const link = JSON.parse(await evaluate(`JSON.stringify(document.querySelector('a[href="#news"]').getBoundingClientRect())`));
await click(link.x + 3, link.y + 3);
checks.page_click_swallowed = (await evaluate("window.__pageClicks")) === 0 && (await evaluate("location.hash")) === "";
// Снова выбрать подпись рекламы и подняться на блок кнопкой «Шире».
await click(label.x + 2, label.y + 2);
// Кнопки — по координатам панели: 420×~150 у правого нижнего угла 900×600.
const panelButtons = { wider: [16 + 40 + 470, 600 - 16 - 22], hide: [900 - 16 - 16 - 40, 600 - 16 - 22] };
await click(...panelButtons.wider);
await click(...panelButtons.hide);
await sleep(200);
const sent = JSON.parse(await evaluate("JSON.stringify(window.__sent)"));
const rule = sent.find((m) => m.evt === "picker_rule");
checks.rule_sent = Boolean(rule) && rule.token === 7;
checks.selector_is_block = rule?.selector === "#ad";
checks.hidden_now = (await evaluate(`getComputedStyle(document.getElementById("ad")).display`)) === "none";
checks.closed = (await evaluate(`Boolean(document.querySelector("x4-picker"))`)) === false;
checks.news_visible = (await evaluate(`getComputedStyle(document.querySelector(".card")).display`)) !== "none";

const ok = Object.values(checks).every(Boolean);
console.log(JSON.stringify({ ok, checks, sent }));
ws.close();
process.exit(ok ? 0 : 1);
