// Меню под кнопкой и меню страницы у точки щелчка — по координатам окон.
import { execFileSync } from "node:child_process";
import { check, chromeWindows, connect, sleep, summary } from "./cdp.mjs";

const WIN = process.env.PROBE_SSH ?? "scripts/probe/win.sh";
const ps = (script, args = "") => execFileSync(WIN, [`powershell -NoProfile -ExecutionPolicy Bypass -File E:\\test\\probe\\${script} ${args}`], { encoding: "latin1" }).trim();
const tag = process.argv[2] ?? "dpi";
const [win] = await chromeWindows();
const chrome = await connect(win);
const ui = (e) => chrome.evaluate(`(async () => { ${e} })()`);
const popupRect = () => ui(`const all = await window.__TAURI__.webviewWindow.getAllWebviewWindows(); const p = all.find(w => w.label.startsWith('popup--chrome')); const pos = await p.innerPosition(); const size = await p.innerSize(); const o = await p.outerPosition(); return { x: pos.x, y: pos.y, w: size.width, h: size.height, ox: o.x, oy: o.y };`);

// Главное меню: правый край попапа у правого края кнопки, верх — под кнопкой.
const button = await ui(`const r = document.getElementById('open-menu').getBoundingClientRect(); const main = window.__TAURI__.window.getCurrentWindow(); const pos = await main.innerPosition(); return { right: r.right, bottom: r.bottom, ox: pos.x, oy: pos.y, dpr: devicePixelRatio };`);
await ui(`document.getElementById('open-menu').click(); return 1;`);
await sleep(700);
let pop = await popupRect();
const expectRight = button.ox + button.right * button.dpr;
const expectTop = button.oy + (button.bottom + 4) * button.dpr;
check(Math.abs(pop.x + pop.w - expectRight) <= 12 && Math.abs(pop.y - expectTop) <= 12, `меню под своей кнопкой (${tag})`, JSON.stringify({ pop, expectRight, expectTop }));
await ui(`await window.__TAURI__.core.invoke('popup_hide'); return 1;`);
await sleep(400);

// Меню страницы — у точки правого щелчка.
const g = await ui(`const r = document.getElementById('stage').getBoundingClientRect(); const pos = await window.__TAURI__.window.getCurrentWindow().innerPosition(); return { ox: pos.x, oy: pos.y, left: r.left, top: r.top, dpr: devicePixelRatio };`);
const px = Math.round(g.ox + (g.left + 300) * g.dpr);
const py = Math.round(g.oy + (g.top + 200) * g.dpr);
console.log("  ", ps("input.ps1", `-X ${px} -Y ${py} -Right 1`).split("\n").pop());
await sleep(900);
pop = await popupRect();
check(Math.abs(pop.x - px) <= 12 && Math.abs(pop.y - py) <= 12, `меню страницы у точки щелчка (${tag})`, JSON.stringify({ pop, px, py }));
await ui(`await window.__TAURI__.core.invoke('popup_hide'); return 1;`);
process.exit(summary() ? 1 : 0);
