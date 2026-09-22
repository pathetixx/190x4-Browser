// Проверки реальным вводом: Escape в полноэкранном режиме, щелчок во вторую
// половину разделённого экрана, Ctrl+щелчок по ссылке.
import { execFileSync } from "node:child_process";
import { check, chromeWindows, connect, sleep, summary, waitTarget } from "./cdp.mjs";

const T = "file:///E:/test/probe/t/";
// Команда на машине с пробой: обёртка над ssh в туннель (см. README рядом).
const WIN = process.env.PROBE_SSH ?? "scripts/probe/win.sh";
const input = (args) =>
  execFileSync(WIN, [`powershell -NoProfile -ExecutionPolicy Bypass -File E:\\test\\probe\\input.ps1 ${args}`], { encoding: "latin1" }).trim();

const [win] = await chromeWindows();
const chrome = await connect(win);
const ui = (expr) => chrome.evaluate(`(async () => { ${expr} })()`);
const S = `const { state } = await import('./js/state.js');`;
const TABS = `const tabs = await import('./js/tabs.js');`;
const openTab = (url) => ui(`${TABS} return tabs.open(${JSON.stringify(url)});`);
const geometry = () => ui(`const r = document.getElementById('stage').getBoundingClientRect(); return { x: window.screenX, y: window.screenY, dpr: devicePixelRatio, left: r.left, top: r.top, width: r.width, height: r.height };`);
const info = () => ui(`${S} return { active: state.activeId, split: state.splitId, swapped: state.swapped, tabs: [...state.tabs.values()].map(t => [t.id, t.url]) };`);

// 1. Escape сворачивает видео.
{
  await openTab(`${T}fs.html?esc`);
  await sleep(1500);
  const page = await connect(await waitTarget((t) => t.url.includes("fs.html?esc")));
  await page.evaluate(`document.documentElement.requestFullscreen().then(() => true)`, { gesture: true });
  await sleep(1200);
  const on = await ui(`return document.documentElement.dataset.fullscreen ?? null;`);
  const g = await geometry();
  console.log("  input:", input(`-Vk 27 -X ${Math.round((g.x + g.left + g.width / 2) * g.dpr)} -Y ${Math.round((g.y + g.top + g.height / 2) * g.dpr)}`));
  await sleep(1500);
  const off = await ui(`return { mode: document.documentElement.dataset.fullscreen ?? null, window: await window.__TAURI__.window.getCurrentWindow().isFullscreen() };`);
  check(on === "true" && off.mode === null && !off.window, "Escape сворачивает видео и окно", JSON.stringify({ on, off }));
  page.close();
}

// 2. Щелчок во вторую половину делает её активной.
{
  const a = await openTab(`${T}opener.html?L`);
  const b = await openTab(`${T}opener.html?R`);
  await sleep(1500);
  await ui(`${TABS} await tabs.activate(${a}); await tabs.splitWith(${b}); return true;`);
  await sleep(800);
  const g = await geometry();
  console.log("  input:", input(`-X ${Math.round((g.x + g.left + g.width * 0.75) * g.dpr)} -Y ${Math.round((g.y + g.top + g.height * 0.6) * g.dpr)}`));
  await sleep(1200);
  let s = await info();
  check(s.active === b && s.split === a && s.swapped, "щелчок по правой половине делает её активной", JSON.stringify([s.active, s.split, s.swapped]));
  const omni = await ui(`return document.getElementById('omni-url').textContent;`);
  check(String(omni).includes("?R"), "адресная строка показывает правую половину", omni);
  console.log("  input:", input(`-X ${Math.round((g.x + g.left + g.width * 0.25) * g.dpr)} -Y ${Math.round((g.y + g.top + g.height * 0.6) * g.dpr)}`));
  await sleep(1200);
  s = await info();
  check(s.active === a && s.split === b && !s.swapped, "щелчок по левой половине возвращает её", JSON.stringify([s.active, s.split, s.swapped]));
  await ui(`${TABS} await tabs.endSplit(); return true;`);
}

// 3. Ctrl+щелчок по ссылке — вкладка в фоне.
{
  const id = await openTab(`${T}opener.html?ctrl`);
  await sleep(1500);
  const page = await connect(await waitTarget((t) => t.url.includes("opener.html?ctrl")));
  const r = await page.evaluate(`(() => { const r = document.getElementById('lnk').getBoundingClientRect(); return { x: r.left + r.width / 2, y: r.top + r.height / 2 }; })()`);
  const g = await geometry();
  console.log("  input:", input(`-Mod 17 -X ${Math.round((g.x + g.left + r.x) * g.dpr)} -Y ${Math.round((g.y + g.top + r.y) * g.dpr)}`));
  await sleep(2000);
  const s = await info();
  check(s.tabs.some(([, url]) => String(url).includes("popup.html?ctrl")), "Ctrl+щелчок открыл вкладку");
  check(s.active === id, "вкладка открылась в фоне", `${s.active} vs ${id}`);
  page.close();
}

chrome.close();
process.exit(summary() ? 1 : 0);
