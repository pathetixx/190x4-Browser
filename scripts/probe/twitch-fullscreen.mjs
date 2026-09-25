// Twitch во весь экран и смена качества: окно, интерфейс и вкладка после каждого шага.
//   node scripts/probe/twitch-fullscreen.mjs [канал]
import { chromeWindows, connect, sleep, waitTarget } from "./cdp.mjs";

const channel = process.argv[2] ?? "ohnepixel";
const [win] = await chromeWindows();
const chrome = await connect(win);
const ui = (expr) => chrome.evaluate(`(async () => { ${expr} })()`);
await ui(`const tabs = await import('./js/tabs.js'); return tabs.open(${JSON.stringify(`https://www.twitch.tv/${channel}`)});`);
const page = await connect(await waitTarget((t) => t.type === "page" && t.url.includes(`twitch.tv/${channel}`)));
await sleep(12000);

const measure = async (label) => {
  const outside = await ui(`const w = window.__TAURI__.window.getCurrentWindow();
    const { state } = await import('./js/state.js');
    const [p, s, is, fs, max] = await Promise.all([w.outerPosition(), w.outerSize(), w.innerSize(), w.isFullscreen(), w.isMaximized()]);
    const stage = document.getElementById('stage').getBoundingClientRect();
    return { outer: [p.x, p.y, s.width, s.height], inner: [is.width, is.height], fs, max, ui: [innerWidth, innerHeight],
      stage: [stage.x, stage.y, stage.width, stage.height].map(Math.round), page: state.fullscreen.page, mode: document.documentElement.dataset.fullscreen ?? null };`);
  const inside = await page.evaluate(`JSON.stringify({ view: [innerWidth, innerHeight], screen: [screen.width, screen.height], dpr: devicePixelRatio,
    fullscreen: document.fullscreenElement ? document.fullscreenElement.className.slice(0, 40) : null,
    video: (() => { const v = document.querySelector("video"); return v ? [v.videoWidth, v.videoHeight] : null; })() })`);
  console.log(label.padEnd(22), JSON.stringify(outside), inside);
};

const openQuality = () =>
  page.evaluate(`(async () => {
    const wait = () => new Promise((r) => setTimeout(r, 400));
    const options = () => document.querySelectorAll('[data-a-target="player-settings-submenu-quality-option"]');
    if (options().length) return true;
    if (!document.querySelector('[data-a-target="player-settings-menu-item-quality"]')) {
      document.querySelector('[data-a-target="player-settings-button"]').click();
      await wait();
    }
    document.querySelector('[data-a-target="player-settings-menu-item-quality"]').click();
    await wait();
    return options().length > 0;
  })()`, { gesture: true });

const click = (selector) =>
  page.evaluate(`(() => { const n = document.querySelector(${JSON.stringify(selector)}); if (!n) return false; n.click(); return true; })()`, { gesture: true });

await ui(`return window.__TAURI__.core.invoke("window_command", { action: "maximize" });`);
await sleep(1500);
await measure("до (развёрнуто)");
console.log("fullscreen button:", await click('[data-a-target="player-fullscreen-button"]'));
await sleep(2500);
await measure("во весь экран");
for (const wanted of ["160p", "480p", "1080p60"]) {
  await openQuality();
  const picked = await page.evaluate(`(() => {
    const option = [...document.querySelectorAll('[data-a-target="player-settings-submenu-quality-option"]')].find((n) => n.textContent.includes(${JSON.stringify(wanted)}));
    if (!option) return null;
    option.querySelector("input").click();
    return option.textContent.trim();
  })()`, { gesture: true });
  await sleep(5000);
  await measure(`качество ${picked}`);
}
await page.evaluate(`document.exitFullscreen().then(() => true, () => false)`);
await sleep(2000);
await measure("после выхода");
chrome.close();
page.close();
