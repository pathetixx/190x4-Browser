// Проверка окон рекламы «по щелчку» в cosmetic.js в headless Chromium на VPS.
// node scripts/test/popups-script.mjs
import { spawn } from "node:child_process";
import { setTimeout as sleep } from "node:timers/promises";

const PORT = 9399;
const page = new URL("popups-script.html", import.meta.url).href;
const browser = spawn("chromium", ["--headless=new", "--no-sandbox", `--remote-debugging-port=${PORT}`, "--no-first-run", "--allow-file-access-from-files", "about:blank"], { stdio: "ignore" });
let failures = 0;
const check = (ok, label, extra = "") => {
  console.log(`${ok ? "OK  " : "FAIL"} ${label}${extra ? ` — ${extra}` : ""}`);
  if (!ok) failures++;
};
try {
  let target;
  for (let i = 0; i < 50 && !target; i++) {
    await sleep(200);
    target = await fetch(`http://127.0.0.1:${PORT}/json/new?${encodeURIComponent(page)}`, { method: "PUT" }).then((r) => r.json()).catch(() => null);
  }
  const ws = new WebSocket(target.webSocketDebuggerUrl);
  await new Promise((resolve) => (ws.onopen = resolve));
  let seq = 0;
  const pending = new Map();
  ws.onmessage = (msg) => {
    const data = JSON.parse(msg.data);
    if (pending.has(data.id)) pending.get(data.id)(data), pending.delete(data.id);
  };
  const send = (method, params = {}) => new Promise((resolve) => { const id = ++seq; pending.set(id, resolve); ws.send(JSON.stringify({ id, method, params })); });
  const evaluate = async (expression) => (await send("Runtime.evaluate", { expression, returnByValue: true, awaitPromise: true })).result?.result?.value;
  const click = async (x, y) => {
    for (const type of ["mouseMoved", "mousePressed", "mouseReleased"]) await send("Input.dispatchMouseEvent", { type, x, y, button: "left", clickCount: 1 });
    await sleep(100);
  };
  const last = () => evaluate("window.log[window.log.length - 1]");
  await sleep(1500);
  check(await evaluate("Boolean(window[Symbol.for('x4cosmetic')])"), "исполнитель отработал на странице");

  await click(50, 50);
  check((await last()) === "decoy", "щелчок по кнопке плеера: окно на чужой сайт — заглушка");
  await click(300, 250);
  check((await last()) === "decoy", "щелчок по плееру: пустое окно — заглушка");
  check(await evaluate("(() => { const w = window.open('https://ads.example/y'); return window.decoy(w); })()"), "сразу после щелчка по плееру — тоже заглушка");
  await click(100, 340);
  check((await last()) === "window", "щелчок мимо плеера («Поделиться») — настоящее окно");
  check(!(await evaluate("window.decoy(window.open('https://ads.example/z'))")), "без щелчка по плееру — не заглушка");
  await evaluate("document.getElementById('play').click()");
  check((await last()) === "window", "щелчок скрипта по плееру защиту не включает");
  await click(50, 50);
  await sleep(1700);
  check(!(await evaluate("window.decoy(window.open('https://ads.example/late'))")), "через 1,5 с после щелчка по плееру — не заглушка");

  const frame = "document.getElementById('frame').contentWindow";
  check(await evaluate(`Boolean(${frame}[Symbol.for('x4cosmetic')])`), "исполнитель отработал во фрейме");
  check(await evaluate(`window.decoy(${frame}.open('about:blank'))`), "фрейм чужого сайта: пустое окно — заглушка");
  check(await evaluate(`window.decoy(${frame}.open(''))`), "фрейм чужого сайта: окно без адреса — заглушка");
  check(await evaluate(`window.decoy(${frame}.open('https://ads.example/f'))`), "фрейм чужого сайта: окно на чужой сайт — заглушка");
  check(!(await evaluate(`window.decoy(${frame}.open('https://player.example/login'))`)), "фрейм чужого сайта: окно на свой сайт — настоящее");
  check(await evaluate(`(() => { const w = ${frame}.open('about:blank'); w.location.href = 'https://ads.example/'; w.document.write('x'); w.focus(); const was = w.closed; w.close(); return was === false && w.closed === true; })()`), "заглушка переживает location/document.write и закрывается по close()");
  ws.close();
} finally {
  browser.kill();
}
console.log(failures ? `\n${failures} FAIL` : "\nALL OK");
process.exit(failures ? 1 : 0);
