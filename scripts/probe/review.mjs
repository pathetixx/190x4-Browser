// Проверки исправлений в настоящем WebView2, только с отдельным тестовым профилем.
import assert from 'node:assert/strict';
import http from 'node:http';
import { chromeWindows, connect, sleep, targets } from './cdp.mjs';

assert(process.env.BROWSER190X4_PROFILE, 'Для пробы нужен отдельный профиль');
const server = http.createServer((req, res) => {
  if (req.url === '/cookie') res.setHeader('Set-Cookie', 'review_cookie=present; Path=/; SameSite=Lax');
  res.setHeader('Content-Type', 'text/html; charset=utf-8');
  res.end(`<!doctype html><title>Проверка браузера</title><input id="edit"><p>Тестовый документ</p>
    ${req.url === '/editor' ? `<script>window.addEventListener('beforeunload', e => { e.preventDefault(); e.returnValue = ''; });</script>` : ''}`);
});
await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
const base = `http://127.0.0.1:${server.address().port}`;
const connections = [];
const bounded = (promise, ms = 10000) => {
  let timer;
  return Promise.race([promise, new Promise((_, reject) => { timer = setTimeout(() => reject(new Error('Истекло время ожидания')), ms); })])
    .finally(() => clearTimeout(timer));
};
async function until(fn, ms = 15000) {
  const end = Date.now() + ms;
  while (Date.now() < end) {
    const result = await fn();
    if (result) return result;
    await sleep(100);
  }
  throw new Error('Условие проверки не выполнено');
}
async function attach(target) {
  const c = await connect(target); connections.push(c);
  const ui = code => bounded(c.evaluate(`(async () => { ${code} })()`));
  const invoke = (command, args = {}) => ui(`return window.__TAURI__.core.invoke(${JSON.stringify(command)}, ${JSON.stringify(args)});`);
  await until(() => ui(`const {state} = await import('./js/state.js'); return state.tabs.size > 0;`));
  await ui(`window.__reviewDialogs = []; const {listen} = await import('./js/bridge.js'); await listen('tab', e => {
    if (e.kind === 'dialog' && e.request?.kind === 'beforeunload') window.__reviewDialogs.push(e);
  }); return true;`);
  return { c, ui, invoke, target };
}
async function open(win, suffix) {
  const id = await win.ui(`const tabs = await import('./js/tabs.js'); return tabs.open(${JSON.stringify(base + suffix)});`);
  await until(() => win.ui(`const {state} = await import('./js/state.js'); const t = state.tabs.get(${id}); return t && !t.loading;`));
  return id;
}
async function lastDialog(win, after = 0) {
  return until(() => win.ui(`return window.__reviewDialogs.length > ${after} ? window.__reviewDialogs.at(-1) : null;`));
}
async function cancel(win, event) {
  await win.invoke('tab_dialog', { id: event.id, tokens: [event.token], answer: { action: 'cancel' } });
  await sleep(300);
}
try {
  const firstTarget = await until(async () => (await chromeWindows())[0]);
  const first = await attach(firstTarget);
  await first.invoke('settings_set', { key: 'newtab_weather', value: false });
  await first.invoke('settings_set', { key: 'newtab_monitor', value: false });

  await open(first, '/plain#190x4-pages.invalid/newtab');
  const address = await first.ui(`return document.getElementById('omni-url').textContent;`);
  assert(address.includes('127.0.0.1') && !address.includes('Введите запрос'), address);
  console.log('OK: сторонняя страница сохраняет видимый адрес');

  const editor = await open(first, '/editor');
  const editorTarget = await until(async () => (await targets()).find(t => t.url === `${base}/editor`));
  const page = await connect(editorTarget); connections.push(page);
  await bounded(page.evaluate(`document.getElementById('edit').focus(); document.getElementById('edit').value = 'несохранённый текст'; true`, { gesture: true }));
  await first.invoke('window_command', { action: 'close' });
  await cancel(first, await lastDialog(first));
  await sleep(9000);
  await first.invoke('window_command', { action: 'close' });
  await cancel(first, await lastDialog(first, 1));
  assert((await chromeWindows()).some(t => t.id === firstTarget.id));
  console.log('OK: после отмены повторное закрытие снова спрашивает страницу');

  await first.invoke('window_open', { private: false, url: `${base}/plain` });
  const secondTarget = await until(async () => (await chromeWindows()).find(t => t.id !== firstTarget.id));
  const second = await attach(secondTarget);
  await second.ui(`window.__reviewQuitDone = false; window.__TAURI__.core.invoke('app_quit').then(() => window.__reviewQuitDone = true); return true;`);
  await cancel(first, await lastDialog(first, 2));
  await until(() => second.ui('return window.__reviewQuitDone;'));
  assert.equal((await chromeWindows()).length, 2);
  assert.equal(await bounded(page.evaluate(`document.getElementById('edit').value`)), 'несохранённый текст');
  console.log('OK: отказ другого окна отменяет общий выход и сохраняет форму');
  await first.ui(`const tabs = await import('./js/tabs.js'); await tabs.close(${editor}, {force: true}); return true;`);

  await open(first, '/cookie');
  const cookieTarget = await until(async () => (await targets()).find(t => t.url === `${base}/cookie`));
  const cookiePage = await connect(cookieTarget); connections.push(cookiePage);
  assert((await bounded(cookiePage.evaluate('document.cookie'))).includes('review_cookie=present'));
  await first.invoke('window_open', { private: true, url: `${base}/plain` });
  const privateTarget = await until(async () => (await chromeWindows()).find(t => t.id !== firstTarget.id && t.id !== secondTarget.id));
  const privateWindow = await attach(privateTarget);
  await first.invoke('window_command', { action: 'close' });
  await second.invoke('window_command', { action: 'close' });
  await until(async () => (await chromeWindows()).length === 1);
  await privateWindow.invoke('browsing_data_clear', { history: false, downloads: false, siteData: true, cache: true });
  await privateWindow.invoke('window_open', { private: false, url: `${base}/plain` });
  const normalTarget = await until(async () => (await chromeWindows()).find(t => t.id !== privateTarget.id));
  const normal = await attach(normalTarget);
  await open(normal, '/check-cookie');
  const checkTarget = await until(async () => (await targets()).find(t => t.url === `${base}/check-cookie`));
  const checkPage = await connect(checkTarget); connections.push(checkPage);
  assert.equal(await bounded(checkPage.evaluate('document.cookie')), '');
  console.log('OK: очистка из приватного окна удаляет cookie обычного профиля');
} finally {
  for (const c of connections) c.close();
  await new Promise(resolve => server.close(resolve));
}
