// Регрессионные проверки реальных JS-модулей: без сети и профиля пользователя.
// Запуск: node --experimental-vm-modules scripts/test/review.mjs
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';
import { fileURLToPath } from 'node:url';

const repo = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const noop = () => {};
const deferred = () => {
  let resolve;
  const promise = new Promise(r => { resolve = r; });
  return { promise, resolve };
};
class Element {
  constructor(tag = '', cls = '', text = '') {
    this.tag = tag; this.className = cls; this.textContent = text;
    this.children = []; this.events = {}; this.style = { setProperty: noop }; this.dataset = {};
  }
  append(...nodes) { this.children.push(...nodes); }
  replaceChildren(...nodes) { this.children = nodes; this.textContent = ''; }
  addEventListener(name, fn) { this.events[name] = fn; }
  setAttribute() {}
}
const el = (tag, cls, text) => new Element(tag, cls, text);
const allText = node => [node.textContent, ...node.children.flatMap(allText)].filter(Boolean);

async function load(relative, stubs, extra = {}) {
  const timers = [];
  const context = vm.createContext({
    console, URL, performance, Date,
    document: { getElementById: () => new Element(), dispatchEvent: noop },
    setTimeout: fn => { timers.push(fn); return timers.length; },
    clearTimeout: noop, requestAnimationFrame: noop,
    ...extra,
  });
  const cache = new Map();
  function stub(key, exports) {
    if (!cache.has(key)) cache.set(key, new vm.SyntheticModule(Object.keys(exports), function () {
      for (const [name, value] of Object.entries(exports)) this.setExport(name, value);
    }, { context }));
    return cache.get(key);
  }
  async function module(file) {
    if (cache.has(file)) return cache.get(file);
    const mod = new vm.SourceTextModule(fs.readFileSync(file, 'utf8'), { context, identifier: file });
    cache.set(file, mod);
    await mod.link(async (specifier, parent) => {
      const target = path.resolve(path.dirname(parent.identifier), specifier);
      const key = path.relative(repo, target);
      return key in stubs ? stub(key, stubs[key]) : module(target);
    });
    return mod;
  }
  const mod = await module(path.join(repo, relative));
  await mod.evaluate();
  return { api: mod.namespace, timers, cache };
}

const dom = { el, favicon: el, hostOf: () => 'example.test', icon: el,
  iconButton: () => el('button'), clock: String, dayLabel: String, siteIcon: el,
  fileName: String, formatBytes: String, formatDuration: String };

async function tabsFixture() {
  const opens = [], calls = [];
  const invoke = (command, args) => {
    calls.push({ command, args });
    if (command === 'tab_open') { const op = deferred(); opens.push(op); return op.promise; }
    return Promise.resolve();
  };
  const fixture = await load('ui/js/tabs.js', {
    'ui/js/bridge.js': { invoke, listen: noop },
    'ui/js/dialogs.js': { dismissLeaveDialogs: noop, hasLeaveDialog: () => false },
    'ui/js/dom.js': dom,
    'ui/js/layout.js': { setPageHidden: noop },
    'ui/js/popups.js': { onPopupAction: noop, openMenu: noop, openPopup: noop },
    'ui/js/prefs.js': { pref: () => false },
  });
  const state = fixture.cache.get(path.join(repo, 'ui/js/state.js')).namespace.state;
  const sleeping = fixture.api.openSleeping({ url: 'https://example.test/saved' });
  state.tabs.set(20, { id: 20, url: 'https://example.test/other', loading: false });
  state.activeId = 20;
  return { ...fixture, state, sleeping, opens, calls };
}

const findings = [];
{
  const calls = [];
  const { api } = await load('ui/js/actions.js', {
    'ui/js/bridge.js': { invoke: async command => { calls.push(command); if (command === 'downloads_active') return 0; } },
    'ui/js/confirm.js': { confirmAction: async () => true, downloadsText: String },
    'ui/js/downloads-model.js': { summary: () => ({ active: 0 }) },
    'ui/js/popups.js': { openPopup: noop },
    'ui/js/prefs.js': { pref: noop, setPref: noop },
    'ui/js/tabs.js': { open: noop, parseInternal: noop, splitWith: noop },
  });
  const url = 'https://example.test/#190x4-pages.invalid/newtab';
  assert.equal(api.isNewTabUrl(url), false);
  assert.equal(api.isNewTabUrl('http://190x4-pages.invalid/newtab.html#clock'), true);
  assert.equal(api.isNewTabUrl('https://190x4-pages.invalid/newtab.html'), false);
  assert.equal(api.isNewTabUrl('http://190x4-pages.invalid:8080/newtab.html'), false);
  assert.equal(api.isNewTabUrl('http://190x4-pages.invalid/newtab.html/evil'), false);
  findings.push({ case: 'external_url_keeps_visible_origin', passed: true });
  await api.closeBrowser();
  assert.deepEqual(calls, ['downloads_active', 'app_quit']);
  findings.push({ case: 'close_browser_uses_global_shutdown', commands: calls });
}
{
  const f = await tabsFixture();
  const first = f.api.activate(f.sleeping);
  const second = f.api.activate(f.sleeping);
  const results = Promise.allSettled([first, second]);
  assert.equal(f.opens.length, 1);
  f.opens[0].resolve(101);
  const settled = await results;
  assert.equal(settled.filter(x => x.status === 'rejected').length, 0);
  assert.equal(f.state.activeId, 101);
  assert.equal(f.state.tabs.has(101), true);
  findings.push({ case: 'double_wake', opens: 1, results: settled.map(x => x.status),
    tabs: [...f.state.tabs.keys()], activeId: f.state.activeId });
}
{
  const f = await tabsFixture();
  const waking = f.api.activate(f.sleeping);
  const result = Promise.allSettled([waking]);
  await f.api.close(f.sleeping, { force: true });
  f.opens[0].resolve(103);
  const settled = await result;
  assert.equal(settled[0].status, 'fulfilled');
  assert.equal(f.calls.some(x => x.command === 'tab_close' && x.args?.id === 103), true);
  findings.push({ case: 'close_during_wake', result: settled[0].status,
    closedEngineTab: 103 });
}
{
  const f = await tabsFixture();
  const waking = f.api.activate(f.sleeping);
  await f.api.activate(20);
  f.opens[0].resolve(104); await waking;
  assert.equal(f.state.activeId, 20);
  findings.push({ case: 'wake_preserves_later_user_selection', expected: 20, actual: f.state.activeId });
}
{
  const requests = [];
  const { api, timers } = await load('ui/js/internal/history.js', {
    'ui/js/bridge.js': { invoke: (command, args) => {
      const request = deferred(); requests.push({ ...request, command, args }); return request.promise;
    } },
    'ui/js/dom.js': dom,
    'ui/js/actions.js': { hooks: {}, navigate: noop, openInNewTab: noop },
  });
  const root = el('root'); api.createHistoryPage(root);
  const main = root.children[0].children[0];
  const input = main.children[0].children[1].children[1];
  input.value = 'needle'; input.events.input(); timers.shift()();
  assert.equal(requests.length, 2);
  requests[0].resolve([{ id: 1, url: 'https://example.test/old', title: 'Unrelated old result', visited_at: 1 }]);
  await Promise.resolve(); await Promise.resolve(); await Promise.resolve();
  assert.equal(allText(root).includes('Unrelated old result'), false);
  requests[1].resolve([{ id: 2, url: 'https://example.test/needle', title: 'Needle result', visited_at: 2 }]);
  await Promise.resolve(); await Promise.resolve(); await Promise.resolve();
  assert.equal(allText(root).includes('Needle result'), true);
  findings.push({ case: 'history_query_during_request', requestedQueries: requests.map(r => r.args.query),
    visibleQuery: input.value, staleResultRendered: false });
}
{
  const pending = deferred(); const handlers = new Map();
  const { api } = await load('ui/js/downloads-model.js', {
    'ui/js/bridge.js': { invoke: () => pending.promise, listen: (name, fn) => handlers.set(name, fn) },
    'ui/js/dom.js': dom,
  });
  const loading = api.initDownloads();
  handlers.get('download')({ id: 1, kind: 'web', phase: 'done', url: 'https://example.test/file',
    path: 'file.bin', bytes: 100, total: 100, error: '' });
  assert.equal(api.downloads()[0].state, 'done');
  pending.resolve([{ id: 1, kind: 'web', state: 'running', bytes: 50, total_bytes: 100 }]);
  await loading;
  assert.equal(api.downloads()[0].state, 'done');
  assert.equal(api.summary().active, 0);
  findings.push({ case: 'downloads_snapshot_preserves_completion', state: api.downloads()[0].state,
    active: api.summary().active });
}

// Проверка синтаксиса без выполнения и установки зависимостей.
let syntaxFiles = 0;
for (const dir of ['ui/js', 'pages', 'scripts', 'crates/webview/src/inject']) {
  for (const file of fs.readdirSync(path.join(repo, dir), { recursive: true })) {
    if (!/\.(js|mjs)$/.test(file)) continue;
    new vm.SourceTextModule(fs.readFileSync(path.join(repo, dir, file), 'utf8'));
    syntaxFiles++;
  }
}
new vm.SourceTextModule(fs.readFileSync(path.join(repo, 'crates/adblock/src/cosmetic.js'), 'utf8'));
syntaxFiles++;
console.log(JSON.stringify({ findings, syntaxFiles }, null, 2));
