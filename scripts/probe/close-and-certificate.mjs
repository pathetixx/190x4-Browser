import { check, chromeWindows, connect, sleep, summary, targets, waitTarget } from "./cdp.mjs";
const [w] = await chromeWindows();
const c = await connect(w);
const ui = (e) => c.evaluate(`(async () => { ${e} })()`);
const S = `const { state } = await import('./js/state.js');`;

// 1. Неверный сертификат: своя страница, «Всё равно перейти», «Не защищено».
{
  const id = await ui(`const t = await import('./js/tabs.js'); return t.open('https://wrong.host.badssl.com/');`);
  await sleep(4500);
  const target = (await targets()).find((t) => t.url.includes("wrong.host.badssl"));
  const page = await connect(target);
  const before = await page.evaluate(`JSON.stringify({ title: document.title, proceed: Boolean(document.getElementById('proceed190x4')) })`);
  check(before.includes('"proceed":true'), "своя страница ошибки сертификата с «Всё равно перейти»", before);
  await page.evaluate(`(() => { document.querySelector('details').open = true; document.getElementById('proceed190x4').click(); return 1; })()`, { gesture: true });
  await sleep(4000);
  const after = await (await connect((await targets()).find((t) => t.url.includes("wrong.host.badssl")))).evaluate(`document.title`);
  check(after === "wrong.host.badssl.com", "сайт открылся после «Всё равно перейти»", after);
  const omni = await ui(`${S} return { label: document.getElementById('omni-site-label').textContent, hidden: document.getElementById('omni-site-label').hidden, kind: document.getElementById('omni-site').dataset.kind, hosts: [...state.insecureHosts] };`);
  check(omni.kind === "insecure" && !omni.hidden, "адресная строка: «Не защищено»", JSON.stringify(omni));
  await ui(`const t = await import('./js/tabs.js'); await t.close(${id}); return 1;`);
}

// 2. «Покинуть сайт?» при закрытии вкладки.
{
  const id = await ui(`const t = await import('./js/tabs.js'); return t.open('file:///E:/test/probe/t/leave.html');`);
  await sleep(1500);
  const page = await connect(await waitTarget((t) => t.url.includes("leave.html")));
  await page.evaluate(`(() => { document.getElementById('t').focus(); document.getElementById('t').value += '!'; return 1; })()`, { gesture: true });
  await sleep(300);
  ui(`const t = await import('./js/tabs.js'); await t.close(${id}); return 1;`).catch(() => {});
  await sleep(1500);
  let st = await ui(`${S} const d = await import('./js/dialogs.js'); return { open: state.tabs.has(${id}), leave: d.hasLeaveDialog(${id}) };`);
  check(st.open && st.leave, "закрытие спросило «Покинуть сайт?», вкладка ждёт", JSON.stringify(st));
  const popup = await connect((await targets()).find((t) => t.url.includes("tauri.localhost/popup.html")));
  await popup.evaluate(`(() => { const b = [...document.querySelectorAll('button')].find(b => b.textContent.trim() === 'Остаться'); b.click(); return 1; })()`);
  await sleep(1200);
  st = await ui(`${S} return { open: state.tabs.has(${id}), active: state.activeId };`);
  check(st.open, "«Остаться» — вкладка осталась", JSON.stringify(st));
  ui(`const t = await import('./js/tabs.js'); await t.close(${id}); return 1;`).catch(() => {});
  await sleep(1500);
  await popup.evaluate(`(() => { const b = [...document.querySelectorAll('button')].find(b => b.textContent.trim() === 'Покинуть'); b.click(); return 1; })()`);
  await sleep(1200);
  st = await ui(`${S} return { open: state.tabs.has(${id}) };`);
  const alive = (await targets()).some((t) => t.url.includes("leave.html"));
  check(!st.open && !alive, "«Покинуть» — вкладка закрылась", JSON.stringify({ ...st, alive }));
}

// 3. Обычная страница закрывается без задержек.
{
  const id = await ui(`const t = await import('./js/tabs.js'); return t.open('file:///E:/test/probe/t/popup.html?plain');`);
  await sleep(1200);
  const t0 = Date.now();
  await ui(`const t = await import('./js/tabs.js'); await t.close(${id}); return 1;`);
  const took = Date.now() - t0;
  const open = await ui(`${S} return state.tabs.has(${id});`);
  check(!open && took < 400, "обычная вкладка закрывается сразу", `${took} мс`);
}
process.exit(summary() ? 1 : 0);
