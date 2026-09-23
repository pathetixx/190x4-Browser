// Живая проверка правок аудита 23.09 на пробе в отдельном профиле.
// node scripts/probe/audit.mjs [confirm,window-leave,middle,palette,rail,import]
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

// 1. Подтверждение, открытое из пункта меню, не отвечает «нет» само: запоздавшее
//    «меню закрыто» раньше закрывало и его.
if (want("confirm")) {
  await ui(`
    const { openMenu } = await import('./js/popups.js');
    const { confirmAction } = await import('./js/confirm.js');
    window.__confirm = 'pending';
    openMenu('probe', document.getElementById('open-menu'), [{ id: 'go', label: 'Спросить' }], () => {
      confirmAction({ title: 'Проба', text: 'Не закрывайся', confirm: 'Да' }).then((ok) => { window.__confirm = String(ok); });
    });
    return 1;`);
  await sleep(900);
  const popup = await connect(await waitTarget((t) => t.url.includes("popup.html")));
  await popup.evaluate(`(() => { [...document.querySelectorAll('.menu__item')].find(b => b.textContent.includes('Спросить')).click(); return 1; })()`);
  await sleep(1500);
  const shown = await popup.evaluate(`Boolean(document.querySelector('.popup--confirm'))`);
  const answer = await ui(`return window.__confirm;`);
  check(shown && answer === "pending", "подтверждение из меню ждёт ответа", JSON.stringify({ shown, answer }));
  await popup.evaluate(`(() => { [...document.querySelectorAll('button')].find(b => b.textContent.trim() === 'Да').click(); return 1; })()`);
  await sleep(800);
  check((await ui(`return window.__confirm;`)) === "true", "«Да» дошло до окна браузера");
}

// 2. Окно, закрытое крестиком, спрашивает «Покинуть сайт?» у своих страниц.
if (want("window-leave")) {
  const before = (await chromeWindows()).map((t) => t.id);
  await invoke("window_open", { url: `${T}leave.html` });
  await sleep(3500);
  const added = (await chromeWindows()).find((t) => !before.includes(t.id));
  check(Boolean(added), "второе окно открылось");
  const second = await connect(added);
  const page = await connect(await waitTarget((t) => t.url.includes("leave.html")));
  await page.evaluate(`(() => { document.getElementById('t').focus(); document.getElementById('t').value += '!'; return 1; })()`, { gesture: true });
  await sleep(300);
  invoke("window_command", { action: "close" }, second).catch(() => {});
  await sleep(2500);
  let alive = (await chromeWindows()).some((t) => t.id === added.id);
  const leave = alive
    ? await ui(`${S} const d = await import('./js/dialogs.js'); const id = [...state.tabs.keys()][0]; return d.hasLeaveDialog(id);`, second)
    : false;
  check(alive && leave, "закрытие окна спросило «Покинуть сайт?»", JSON.stringify({ alive, leave }));
  const popups = (await targets()).filter((t) => t.url.includes("popup.html"));
  let answered = false;
  for (const target of popups) {
    const p = await connect(target);
    answered ||= await p.evaluate(`(() => { const b = [...document.querySelectorAll('button')].find(b => b.textContent.trim() === 'Остаться'); if (b) b.click(); return Boolean(b); })()`);
  }
  await sleep(1500);
  alive = (await chromeWindows()).some((t) => t.id === added.id);
  check(answered && alive, "«Остаться» — окно осталось", JSON.stringify({ answered, alive }));

  invoke("window_command", { action: "close" }, second).catch(() => {});
  await sleep(2500);
  for (const target of (await targets()).filter((t) => t.url.includes("popup.html"))) {
    const p = await connect(target);
    await p.evaluate(`(() => { const b = [...document.querySelectorAll('button')].find(b => b.textContent.trim() === 'Покинуть'); if (b) b.click(); return 1; })()`);
  }
  await sleep(2500);
  alive = (await chromeWindows()).some((t) => t.id === added.id);
  check(!alive, "«Покинуть» — окно закрылось");
}

// 3. Средняя кнопка по ссылке — вкладка в фоне.
if (want("middle")) {
  const opener = await ui(`${TABS} return tabs.open('${T}opener.html');`);
  await sleep(1800);
  const page = await connect(await waitTarget((t) => t.url.includes("opener.html")));
  const box = await page.evaluate(`JSON.stringify(document.getElementById('lnk').getBoundingClientRect())`);
  const { x, y, width, height } = JSON.parse(box);
  const at = { x: x + width / 2, y: y + height / 2 };
  await page.send("Input.dispatchMouseEvent", { type: "mousePressed", ...at, button: "middle", buttons: 4, clickCount: 1 });
  await page.send("Input.dispatchMouseEvent", { type: "mouseReleased", ...at, button: "middle", buttons: 0, clickCount: 1 });
  await sleep(2500);
  const st = await ui(`${S} return { active: state.activeId, tabs: [...state.tabs.values()].map(t => ({ id: t.id, url: t.url })) };`);
  const opened = st.tabs.find((t) => String(t.url).includes("popup.html?ctrl"));
  check(Boolean(opened) && st.active === opener, "средняя кнопка открыла ссылку фоном", JSON.stringify({ active: st.active, opener, opened }));
  if (opened) await ui(`${TABS} await tabs.close(${opened.id}); return 1;`);
  await ui(`${TABS} await tabs.close(${opener}); return 1;`);
}

// 4. Ctrl+K находит открытую вкладку.
if (want("palette")) {
  const id = await ui(`${TABS} return tabs.open('${T}ticker.html');`);
  await sleep(1500);
  const found = await ui(`
    const p = await import('./js/palette.js');
    p.openPalette();
    const input = document.getElementById('palette-input');
    input.value = 'ticker';
    input.dispatchEvent(new Event('input'));
    const rows = [...document.querySelectorAll('.palette__row .palette__label')].map((n) => n.textContent);
    p.closePalette();
    return rows;`);
  check(found.some((row) => /ticker/i.test(row)), "палитра нашла вкладку", JSON.stringify(found));
  await ui(`${TABS} await tabs.close(${id}); return 1;`);
}

// 5. Боковая панель по умолчанию спрятана, страница — на всю ширину.
if (want("rail")) {
  const st = await ui(`return { rail: getComputedStyle(document.querySelector('.rail')).display, stage: Math.round(document.getElementById('stage').getBoundingClientRect().left), puzzle: Boolean(document.getElementById('ext-menu')) };`);
  check(st.rail === "none" && st.stage === 0 && !st.puzzle, "рельса и пазла нет, страница от левого края", JSON.stringify(st));
}

// 6. Перенос из другого браузера (в профиль пробы; печатаются только числа).
if (want("import")) {
  const sources = await invoke("browsers_found");
  console.log("     найдено:", sources.map((s) => `${s.name} (${s.profiles.length})`).join(", ") || "ничего");
  if (sources.length) {
    const source = sources[0];
    const t0 = Date.now();
    const report = await invoke("browser_import", { browser: source.id, profile: source.profiles[0].dir, bookmarks: true, history: true }).catch((e) => ({ error: String(e) }));
    check(!report.error, `перенос из ${source.name}`, `${JSON.stringify(report)} за ${Date.now() - t0} мс`);
  }
}

process.exit(summary() ? 1 : 0);
