/** Сборка окна браузера: подписки, горячие клавиши, сессия. */

import { invoke, isNative, listen } from "./bridge.js";
import {
  bookmarkCurrent,
  hooks,
  isNewTabUrl,
  openDownloadsPage,
  openMediaExtension,
  openSettings,
  tabAction,
  toggleBookmarksBar,
  zoom,
} from "./actions.js";
import { initBookmarksBar, renderBarVisibility } from "./bookmarks-bar.js";
import { initContextMenu, openContextMenu } from "./context-menu.js";
import { el, hostOf } from "./dom.js";
import { initDownloads } from "./downloads-model.js";
import { closeFind, initFind, isFindOpen, openFind, renderFindResult } from "./find.js";
import { renderInternal } from "./internal/pages.js";
import { initLayout, syncDuring } from "./layout.js";
import { focusOmnibox, initOmnibox, renderOmnibox } from "./omnibox.js";
import { closePalette, initPalette, isPaletteOpen, openPalette } from "./palette.js";
import { initPanels, isLivePanel, isPanelOpen, openPanel, renderPanel, toggle } from "./panels.js";
import { initPopups, onPopupAction, openPopup } from "./popups.js";
import { applyTheme, loadPrefs, onPref, pref, setPref } from "./prefs.js";
import { activeTab, state, subscribe, tabIndex, upsertTab } from "./state.js";
import { activate, close, cycle, initTabs, open, renderTabs, reopenClosed } from "./tabs.js";
import { initToolbar, loadWindowState, renderToolbar, syncWindowState } from "./toolbar.js";
import { initUpdates } from "./updates.js";

const progress = document.getElementById("progress");
const statusDot = document.getElementById("status-dot");
const statusState = document.getElementById("status-state");
const statusTarget = document.getElementById("status-target");
const statusBlocked = document.getElementById("status-blocked");
const statusLatency = document.getElementById("status-latency");
const statusTabs = document.getElementById("status-tabs");
const railBadge = document.getElementById("rail-blocked");

/**
 * Сохранение сессии с задержкой: восстановление само порождает серию
 * изменений, и без задержки каждая вкладка уходила бы в базу отдельно.
 * Объявлено до первой подписки: события приходят ещё во время старта.
 */
let sessionReady = false;
let sessionTimer = 0;
let toastTimer = 0;

await loadPrefs();
applyTheme();
applyStatusbar();
state.adblockOn = pref("adblock_enabled");

hooks.togglePanel = toggle;
hooks.openPanel = openPanel;
hooks.openFind = () => {
  const tab = activeTab();
  if (tab && !tab.internal) openFind();
};
hooks.toast = toast;
hooks.saveSession = saveSessionNow;

initLayout(document.getElementById("stage"));
initPopups();
initTabs();
initFind();
initPanels();
initOmnibox();
initToolbar();
initBookmarksBar();
initDownloads();
initUpdates();
initContextMenu({ translate: translateText });

const web = () => {
  const tab = activeTab();
  return Boolean(tab && !tab.internal);
};

initPalette([
  { group: "Вкладки", title: "Новая вкладка", icon: "tab-add", keys: "Ctrl+T", run: () => open("about:newtab") },
  { group: "Вкладки", title: "Закрыть вкладку", icon: "stop", keys: "Ctrl+W", run: () => activeTab() && close(activeTab().id) },
  { group: "Вкладки", title: "Открыть закрытую вкладку", icon: "history", keys: "Ctrl+Shift+T", run: reopenClosed },
  { group: "Страница", title: "Обновить", icon: "reload", keys: "Ctrl+R", run: () => tabAction("reload"), when: web },
  { group: "Страница", title: "Найти на странице", icon: "find", keys: "Ctrl+F", run: () => hooks.openFind(), when: web },
  { group: "Страница", title: "Добавить в закладки", icon: "star-20", keys: "Ctrl+D", run: bookmarkCurrent, when: web },
  { group: "Страница", title: "Печать", icon: "print", keys: "Ctrl+P", run: () => tabAction("print"), when: web },
  { group: "Страница", title: "Скачать видео со страницы", icon: "video", keys: "Ctrl+Shift+D", run: () => openMediaExtension() },
  { group: "Страница", title: "Инструменты разработчика", icon: "code", keys: "F12", run: () => tabAction("devtools"), when: web },
  { group: "Браузер", title: "Загрузки", icon: "download", keys: "Ctrl+J", run: openDownloadsPage },
  { group: "Браузер", title: "История", icon: "history", keys: "Ctrl+H", run: () => openPanel("history") },
  { group: "Браузер", title: "Диспетчер закладок", icon: "favorites", keys: "Ctrl+Shift+O", run: () => openSettings("bookmarks") },
  { group: "Браузер", title: "Показать или скрыть панель закладок", icon: "favorites", keys: "Ctrl+Shift+B", run: toggleBookmarksBar },
  { group: "Браузер", title: "Пароли", icon: "key", keywords: "password логин", run: () => openSettings("passwords") },
  { group: "Браузер", title: "Блокировка рекламы", icon: "shield", run: () => openPanel("shield") },
  { group: "Браузер", title: "Удалить данные о работе в браузере", icon: "broom", keys: "Ctrl+Shift+Del", run: () => openSettings("privacy") },
  { group: "Браузер", title: "Настройки", icon: "settings", run: () => openSettings() },
  { group: "Браузер", title: "Браузер по умолчанию", icon: "globe", keywords: "default основной", run: () => openSettings("default") },
  {
    group: "Браузер",
    title: "Сменить тему",
    icon: "paint",
    run: () => setPref("theme", document.documentElement.dataset.theme === "shiro" ? "kurogane" : "shiro"),
  },
]);

/* ── Окно ──────────────────────────────────────────────────── */

for (const button of document.querySelectorAll("[data-window]")) {
  button.addEventListener("click", () => invoke("window_command", { action: button.dataset.window }).catch(() => {}));
}
listen("window-state", ({ maximized }) => syncWindowState(maximized));
loadWindowState();

document.getElementById("rail-settings").addEventListener("click", () => openSettings());

// Меню движка в интерфейсе браузера («Назад», «Обновить», «Проверить») —
// чужая деталь. Оставляем его только в полях ввода.
document.addEventListener("contextmenu", (event) => {
  if (event.target.closest("input, textarea, [contenteditable='true']")) return;
  if (event.target.closest(".internal") && getSelection()?.toString()) return;
  event.preventDefault();
});

onPref((key) => {
  if (key === "theme") applyTheme();
  if (key === "statusbar") applyStatusbar();
  if (key === "adblock_enabled") state.adblockOn = pref("adblock_enabled");
});
matchMedia("(prefers-color-scheme: dark)").addEventListener("change", applyTheme);

function applyStatusbar() {
  if (pref("statusbar")) delete document.documentElement.dataset.statusbar;
  else document.documentElement.dataset.statusbar = "hidden";
  syncDuring(80);
}

/* ── События вкладок из Rust ───────────────────────────────── */

listen("tab", (event) => {
  switch (event.kind) {
    case "opened":
      upsertTab(event.id, { loading: true, url: event.url });
      if (state.activeId === null) activate(event.id);
      // Готовая поверхность вкладки забирает клавиатуру себе — на новой
      // вкладке возвращаем курсор в адресную строку.
      if (state.activeId === event.id && isNewTabUrl(event.url)) {
        document.dispatchEvent(new CustomEvent("browser:focus-omnibox"));
      }
      break;
    case "started":
      upsertTab(event.id, { loading: true, url: event.url, blocked: 0, media: null });
      state.passwordSites.delete(event.id);
      break;
    case "finished":
      // Навигация, ушедшая в загрузку, документ не меняет: в адресной строке —
      // адрес, который остался у вкладки, а не набранный.
      upsertTab(event.id, event.url ? { loading: false, url: event.url } : { loading: false });
      break;
    case "title": {
      upsertTab(event.id, { title: event.title });
      const tab = state.tabs.get(event.id);
      // Визит записываем здесь: только в этот момент известны и адрес, и заголовок.
      if (tab?.url && !isNewTabUrl(tab.url)) {
        invoke("history_record", { url: tab.url, title: event.title }).catch(() => {});
      }
      break;
    }
    case "url":
      upsertTab(event.id, { url: event.url });
      break;
    case "history":
      upsertTab(event.id, { canBack: event.can_back, canForward: event.can_forward });
      break;
    case "favicon":
      upsertTab(event.id, { favicon: event.url || null });
      break;
    case "zoom":
      upsertTab(event.id, { zoom: event.factor });
      break;
    case "popup":
      open(event.url, { index: tabIndex(event.opener) + 1 });
      break;
    case "message":
      handlePageMessage(event);
      break;
    case "shortcut":
      // Клавиша нажата, пока фокус был на странице.
      runShortcut(event.combo);
      break;
    case "audio":
      upsertTab(event.id, { audible: event.audible, muted: event.muted });
      break;
    case "find":
      renderFindResult(event);
      break;
    case "context_menu":
      openContextMenu(event);
      break;
  }
});

/**
 * Сообщение со страницы. Источник недоверенный: разбираем строго, всё
 * незнакомое молча игнорируем. Команды принимаем только от встроенных
 * страниц браузера — адрес документа сообщает движок, подделать его нельзя.
 */
function handlePageMessage({ id, source, payload }) {
  let message;
  try {
    message = JSON.parse(payload);
    if (typeof message === "string") message = JSON.parse(message);
  } catch {
    return;
  }
  if (!message || typeof message !== "object") return;

  const fromPages = typeof source === "string" && source.startsWith("http://190x4-pages.invalid/");
  if (fromPages && message.evt === "navigate" && typeof message.url === "string") {
    invoke("tab_navigate", { id, url: message.url }).catch(() => {});
    return;
  }
  if (message.evt === "media_found" && typeof message.url === "string") {
    upsertTab(id, { media: { url: message.url, title: String(message.title ?? "") } });
  }
}

/* ── Переводчик ────────────────────────────────────────────── */

async function translateText(text) {
  const tr = state.translate;
  Object.assign(tr, { text, result: "", detected: null, error: null, busy: true });

  if (!isPanelOpen("translate")) openPanel("translate");
  else renderPanel();

  try {
    const answer = await invoke("translate_text", { text, targetLang: pref("translate_lang") });
    tr.result = answer.result;
    tr.detected = answer.detected;
  } catch (error) {
    tr.error = String(error?.message ?? error);
  } finally {
    tr.busy = false;
    renderPanel();
  }
}

/* ── Пароли ────────────────────────────────────────────────── */

listen("password-offer", (offer) => {
  state.passwordOffer = offer;
  if (offer.tab !== state.activeId) return;
  renderOmnibox();
  const key = document.getElementById("omni-key");
  key.dataset.attention = "true";
  setTimeout(() => delete key.dataset.attention, 2000);
  openPopup("password", key, { width: 340, align: "end", payload: offer }).catch(() => {});
});

listen("password-site", ({ tab, origin, accounts }) => {
  state.passwordSites.set(tab, { origin, accounts });
  if (tab === state.activeId) renderOmnibox();
});

// «Управление паролями» из списка учёток на странице: Rust просит открыть настройки.
listen("open-settings", ({ section }) => openSettings(section));

/* ── Действия из всплывающих окон ──────────────────────────── */

onPopupAction("downloads", ({ action }) => {
  if (action === "open-page") openDownloadsPage();
});
onPopupAction("bookmark", ({ action }) => {
  if (action === "manage") openSettings("bookmarks");
});
onPopupAction("password", () => {
  state.passwordOffer = null;
  renderOmnibox();
});
onPopupAction("accounts", ({ action }) => {
  if (action === "manage") openSettings("passwords");
});
onPopupAction("site", ({ action }) => {
  if (action === "privacy") openSettings("privacy");
  if (action === "passwords") openSettings("passwords");
  // Блокировку на сайте переключили — страница перезагружается уже с новым правилом.
  if (action === "reload") tabAction("reload");
});
onPopupAction("media", ({ action }) => {
  if (action === "settings") openSettings("extensions");
});

listen("media", (event) => {
  state.mediaActive = event.phase === "progress";
  renderToolbar();
  if (event.phase === "done") toast(`Скачано: ${String(event.path ?? "").split(/[\\/]/).pop()}`);
  if (event.phase === "failed") toast(`Видео не скачалось: ${event.error ?? ""}`);
});

/* ── Горячие клавиши ───────────────────────────────────────── */

/**
 * Клавиша приходит двумя путями: из интерфейса (фокус в нём) и из Rust,
 * когда фокус был на странице — нативная вкладка забирает клавиатуру себе.
 */
function runShortcut(combo) {
  const tab = activeTab();
  switch (combo) {
    case "ctrl+k":
      openPalette();
      return true;
    case "ctrl+t":
      open("about:newtab");
      return true;
    case "ctrl+w":
      if (tab) close(tab.id);
      return true;
    case "ctrl+shift+t":
      reopenClosed();
      return true;
    case "ctrl+tab":
      cycle(1);
      return true;
    case "ctrl+shift+tab":
      cycle(-1);
      return true;
    case "ctrl+l":
    case "alt+d":
      focusOmnibox();
      return true;
    case "ctrl+f":
      hooks.openFind();
      return true;
    case "ctrl+d":
      bookmarkCurrent();
      return true;
    case "ctrl+j":
      openDownloadsPage();
      return true;
    case "ctrl+h":
      toggle("history");
      return true;
    case "ctrl+p":
      tabAction("print");
      return true;
    case "ctrl+=":
      zoom(1);
      return true;
    case "ctrl+-":
      zoom(-1);
      return true;
    case "ctrl+0":
      zoom(0);
      return true;
    case "ctrl+shift+b":
      toggleBookmarksBar();
      return true;
    case "ctrl+shift+o":
      openSettings("bookmarks");
      return true;
    case "ctrl+shift+delete":
      openSettings("privacy");
      return true;
    case "ctrl+shift+d":
      openMediaExtension();
      return true;
    case "ctrl+r":
    case "ctrl+shift+r":
    case "f5":
      tabAction("reload");
      return true;
    case "alt+left":
      tabAction("back");
      return true;
    case "alt+right":
      tabAction("forward");
      return true;
    case "f12":
      tabAction("devtools");
      return true;
    case "alt+f":
      document.getElementById("open-menu").click();
      return true;
    default:
      break;
  }

  const byNumber = /^ctrl\+([1-9])$/.exec(combo);
  if (byNumber) {
    const ids = [...state.tabs.keys()];
    const index = byNumber[1] === "9" ? ids.length - 1 : Number(byNumber[1]) - 1;
    if (ids[index] != null) activate(ids[index]);
    return true;
  }
  return false;
}

window.addEventListener("keydown", (event) => {
  if (event.key === "Escape" && isPaletteOpen()) return closePalette();
  if (event.key === "Escape" && isFindOpen()) return closeFind();
  if (event.defaultPrevented) return;

  const parts = [];
  if (event.ctrlKey || event.metaKey) parts.push("ctrl");
  if (event.altKey) parts.push("alt");
  if (event.shiftKey) parts.push("shift");

  // Буквы и цифры — по физической клавише: на русской раскладке event.key
  // для D — «в», и Ctrl+D не сработал бы.
  let key = event.key.toLowerCase();
  if (/^Key[A-Z]$/.test(event.code)) key = event.code.slice(3).toLowerCase();
  else if (/^(Digit|Numpad)\d$/.test(event.code)) key = event.code.slice(-1);
  else if (event.code === "Equal" || event.code === "NumpadAdd") key = "=";
  else if (event.code === "Minus" || event.code === "NumpadSubtract") key = "-";
  // Ввод без скан-кода (экранная клавиатура, удалённый доступ, автоматизация):
  // code пуст, а key — буква текущей раскладки. Код виртуальной клавиши от
  // раскладки не зависит.
  else if (!event.code && event.keyCode >= 65 && event.keyCode <= 90) key = String.fromCharCode(event.keyCode).toLowerCase();
  else if (!event.code && event.keyCode >= 48 && event.keyCode <= 57) key = String.fromCharCode(event.keyCode);
  key = { arrowleft: "left", arrowright: "right" }[key] ?? key;
  if (["control", "shift", "alt", "meta"].includes(key)) return;
  if (!parts.some((part) => part === "ctrl" || part === "alt") && !/^f\d+$/.test(key)) return;

  if (runShortcut([...parts, key].join("+"))) event.preventDefault();
});

/* ── Представления ─────────────────────────────────────────── */

subscribe(() => {
  renderTabs();
  renderOmnibox();
  renderStatus();
  renderNav();
  renderToolbar();
  renderBarVisibility();
  renderInternal();
  scheduleSessionSave();
  if (isLivePanel()) renderPanel();
});

function renderNav() {
  const tab = activeTab();
  const isWeb = Boolean(tab && !tab.internal);
  document.getElementById("nav-back").disabled = !(isWeb && tab.canBack);
  document.getElementById("nav-forward").disabled = !(isWeb && tab.canForward);
  document.getElementById("nav-reload").disabled = !isWeb;

  const loading = Boolean(isWeb && tab.loading);
  progress.dataset.active = String(loading);
  progress.style.width = loading ? "70%" : "100%";
  if (!loading) setTimeout(() => (progress.style.width = "0"), 220);
}

function renderStatus() {
  const tab = activeTab();
  if (!toastTimer) {
    statusDot.dataset.state = tab?.loading ? "busy" : "idle";
    statusState.textContent = tab?.loading ? "загрузка" : "готов";
  }
  statusTarget.textContent = tab?.internal ? tab.url : tab?.url && !isNewTabUrl(tab.url) ? hostOf(tab.url) : "";
  statusBlocked.textContent = state.blockedTotal;
  statusTabs.textContent = state.tabs.size;

  statusLatency.textContent = state.latencyMicros ? `${state.latencyMicros.toFixed(1)} мкс` : "—";
  statusLatency.dataset.grade = state.latencyMicros > 100 ? "warn" : "good";

  railBadge.hidden = state.blockedTotal === 0;
  railBadge.textContent = state.blockedTotal > 999 ? "999+" : state.blockedTotal;
}

/**
 * Короткое сообщение. На встроенной странице и без строки состояния —
 * всплывающая плашка внизу, иначе — в строке состояния.
 */
function toast(text) {
  clearTimeout(toastTimer);
  const tab = activeTab();
  if (tab?.internal || !pref("statusbar")) {
    let node = document.getElementById("toast");
    if (!node) {
      node = el("div", "toast");
      node.id = "toast";
      node.setAttribute("role", "status");
      document.getElementById("stage").append(node);
    }
    node.textContent = text;
    node.hidden = false;
    toastTimer = setTimeout(() => {
      node.hidden = true;
      toastTimer = 0;
    }, 4000);
    return;
  }
  statusState.textContent = text;
  toastTimer = setTimeout(() => {
    toastTimer = 0;
    renderStatus();
  }, 4000);
}

/* ── Счётчики фильтра ──────────────────────────────────────── */

async function pollStats() {
  try {
    const snapshot = await invoke("adblock_stats");
    state.blockedTotal = snapshot.blocked;
    state.latencyMicros = snapshot.avg_micros;
    const tab = activeTab();
    if (tab && !tab.internal) upsertTab(tab.id, { blocked: snapshot.blocked });
    else renderStatus();
  } catch {
    /* фильтр ещё не поднялся */
  }
}

setInterval(pollStats, 1000);

/* ── Старт и сессия ────────────────────────────────────────── */

invoke("services_state")
  .then((services) => {
    state.services = services;
  })
  .catch(() => {});

// Ссылка из другой программы, пока браузер открыт: адреса ждут в очереди Rust.
// До конца восстановления сессии их заберёт restoreSession.
listen("launch", () => {
  if (sessionReady) openLaunched();
});

try {
  await restoreSession();
} catch (error) {
  toast(String(error?.message ?? error));
}

/**
 * Что открыть при запуске — по настройке «При запуске». Если браузер запустили
 * ссылкой или файлом, пустая новая вкладка не нужна: откроется сама ссылка.
 */
async function restoreSession() {
  const mode = pref("startup");
  const launched = await invoke("launch_take").catch(() => []);
  const blank = !launched.length;

  if (mode === "pages") {
    const pages = (pref("startup_pages") ?? []).filter(Boolean);
    if (!pages.length) {
      if (blank) await open("about:newtab");
    } else {
      const ids = [];
      for (const url of pages) ids.push(await open(url, { background: true }));
      if (blank) await activate(ids[0]);
    }
  } else if (mode === "restore") {
    const saved = await invoke("session_restore").catch(() => []);
    if (!saved.length) {
      if (blank) await open("about:newtab");
    } else {
      let activeIndex = saved.findIndex((tab) => tab.active);
      if (activeIndex < 0) activeIndex = 0;
      const ids = [];
      for (const tab of saved) ids.push(await open(tab.url, { background: true }));
      if (blank && ids[activeIndex] != null) await activate(ids[activeIndex]);
    }
  } else if (blank) {
    await open("about:newtab");
  }
  for (const url of launched) await open(url);
  sessionReady = true;
  // Повторный запуск мог прийти, пока открывалась сессия.
  await openLaunched();
}

/** Ссылки и файлы, которыми браузер открыли снаружи, — каждая в своей вкладке. */
async function openLaunched() {
  const urls = await invoke("launch_take").catch(() => []);
  for (const url of urls) await open(url);
}

function sessionTabs() {
  return [...state.tabs.values()]
    .filter((tab) => tab.internal || (tab.url && !isNewTabUrl(tab.url) && !tab.url.startsWith("about:")))
    .map((tab) => ({ url: tab.url, title: tab.title ?? "", active: tab.id === state.activeId }));
}

function scheduleSessionSave() {
  if (!sessionReady) return;
  clearTimeout(sessionTimer);
  sessionTimer = setTimeout(() => invoke("session_save", { tabs: sessionTabs() }).catch(() => {}), 800);
}

/** Перед обновлением браузер закроется — сессию пишем сразу, без отложенного таймера. */
async function saveSessionNow() {
  if (!sessionReady) return;
  clearTimeout(sessionTimer);
  await invoke("session_save", { tabs: sessionTabs() }).catch(() => {});
}

/* ── Режим без Rust: макет для ревью вёрстки ───────────────── */

if (!isNative) {
  const mock = await import("./mock.js");
  mock.paintStage();

  // ?demo=palette|find|settings|downloads|shield|bookmarks|history|translate
  // ?section=passwords — раздел настроек; ?motion=off — без анимаций.
  const params = new URLSearchParams(location.search);
  if (params.get("motion") === "off") document.documentElement.dataset.motion = "off";
  const demo = params.get("demo");
  setTimeout(() => {
    if (demo === "palette") openPalette();
    else if (demo === "find") openFind();
    else if (demo === "settings") openSettings(params.get("section") ?? "");
    else if (demo === "downloads") openDownloadsPage();
    else if (demo) openPanel(demo);
  }, 200);
}

document.fonts?.ready.then(() => syncDuring(120));
