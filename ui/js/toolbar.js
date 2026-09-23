/**
 * Панель инструментов справа от адресной строки: загрузчик видео, загрузки,
 * меню «Настройки и прочее».
 */

import { invoke } from "./bridge.js";
import { displayUrl } from "./dom.js";
import { onDownloads, summary } from "./downloads-model.js";
import { openMenu } from "./popups.js";
import { onPref, pref } from "./prefs.js";
import { activeTab, state } from "./state.js";
import { endSplit, hasClosedTabs, open, reopenClosed, reopenLabel } from "./tabs.js";
import { openUpdateBubble, update } from "./updates.js";
import {
  closeBrowser,
  goHome,
  hooks,
  isNewTabUrl,
  newWindow,
  openDownloadsBubble,
  openHistoryPage,
  openMediaExtension,
  openSettings,
  tabAction,
} from "./actions.js";

const RING = 81.68;

const downloadsButton = document.getElementById("downloads-btn");
const ring = document.getElementById("downloads-ring");
const mediaButton = document.getElementById("ext-media");
const mediaBadge = document.getElementById("ext-media-badge");
const homeButton = document.getElementById("nav-home");

let sawDownloads = false;

export function initToolbar() {
  wireHistoryMenu(document.getElementById("nav-back"), -1);
  wireHistoryMenu(document.getElementById("nav-forward"), 1);
  document.getElementById("nav-back").addEventListener("click", () => tabAction("back"));
  document.getElementById("nav-forward").addEventListener("click", () => tabAction("forward"));
  // Пока страница грузится, «Обновить» становится «Остановить».
  document.getElementById("nav-reload").addEventListener("click", (event) =>
    tabAction(event.currentTarget.dataset.loading === "true" ? "stop" : "reload")
  );
  homeButton.addEventListener("click", goHome);

  downloadsButton.addEventListener("click", () => openDownloadsBubble());
  mediaButton.addEventListener("click", () => openMediaExtension());
  document.getElementById("open-menu").addEventListener("click", (event) => showMainMenu(event.currentTarget));

  onDownloads((event) => {
    renderDownloads(event);
    if (event?.phase === "started" && event.kind === "web" && pref("download_bubble")) {
      openDownloadsBubble();
    }
  });
  onPref(() => renderToolbar());
  renderToolbar();
}

export function renderToolbar() {
  homeButton.hidden = !pref("show_home");
  mediaButton.hidden = !(pref("ext_media_enabled") && pref("ext_media_pinned"));
  renderDownloads(null);

  const tab = activeTab();
  const busy = summary().active > 0;
  mediaBadge.hidden = !(tab?.media || (busy && hasMediaJob()));
}

function hasMediaJob() {
  return state.mediaActive === true;
}

function renderDownloads(event) {
  const { active, progress, count } = summary();
  if (count > 0) sawDownloads = true;
  downloadsButton.hidden = !(pref("downloads_button") === "always" || active > 0 || sawDownloads);

  downloadsButton.dataset.active = String(active > 0);
  const share = active > 0 ? (progress == null ? 0.25 : progress) : 0;
  ring.style.strokeDashoffset = String(RING * (1 - share));
  downloadsButton.title = active > 0 ? `Загрузки: идёт ${active} (Ctrl+J)` : "Загрузки (Ctrl+J)";

  if (event?.phase === "done") {
    downloadsButton.dataset.done = "false";
    void downloadsButton.offsetWidth;
    downloadsButton.dataset.done = "true";
  }
}

/* ── История на «Назад» и «Вперёд» ─────────────────────────── */

/**
 * Правый щелчок или долгое нажатие на «Назад» и «Вперёд» открывает список
 * страниц, как в Chrome: вернуться можно сразу на несколько шагов.
 */
function wireHistoryMenu(button, direction) {
  let timer = 0;
  let held = false;
  button.addEventListener("contextmenu", (event) => {
    event.preventDefault();
    showHistoryMenu(button, direction);
  });
  button.addEventListener("pointerdown", (event) => {
    if (event.button !== 0) return;
    held = false;
    clearTimeout(timer);
    timer = setTimeout(() => {
      held = true;
      showHistoryMenu(button, direction);
    }, 500);
  });
  for (const type of ["pointerup", "pointerleave", "pointercancel"]) {
    button.addEventListener(type, () => clearTimeout(timer));
  }
  // Отпустили после долгого нажатия — это выбор из списка, а не «Назад».
  button.addEventListener(
    "click",
    (event) => {
      if (!held) return;
      held = false;
      event.stopImmediatePropagation();
    },
    true
  );
}

async function showHistoryMenu(button, direction) {
  const tab = activeTab();
  if (!tab || tab.internal) return;
  const history = await invoke("tab_history", { id: tab.id }).catch(() => null);
  const entries = Array.isArray(history?.entries) ? history.entries : [];
  const current = Number(history?.currentIndex ?? -1);
  if (current < 0 || state.activeId !== tab.id) return;
  const list = (direction < 0 ? entries.slice(0, current).reverse() : entries.slice(current + 1)).slice(0, 15);
  if (!list.length) return;
  // Значки — из кэша профиля: список не должен ходить на каждый сайт.
  const icons = await Promise.all(list.map((entry) => invoke("site_icon", { url: entry.url }).catch(() => null)));
  const items = list.map((entry, index) => ({
    id: `entry:${entry.id}`,
    label: isNewTabUrl(entry.url) ? "Новая вкладка" : entry.title || displayUrl(entry.url),
    image: icons[index] ?? undefined,
    icon: icons[index] ? undefined : "globe-16",
  }));
  items.push({ separator: true }, { id: "history", label: "Показать всю историю", icon: "history", keys: "Ctrl+H" });
  openMenu(
    "nav-history",
    button,
    items,
    (action) => {
      if (action === "history") return openHistoryPage();
      if (action?.startsWith("entry:")) {
        invoke("tab_history_go", { id: tab.id, entry: Number(action.slice(6)) }).catch(() => {});
      }
    },
    { width: 320 }
  );
}

/* ── Меню «Настройки и прочее» ─────────────────────────────── */

function showMainMenu(button) {
  const tab = activeTab();
  const web = Boolean(tab && !tab.internal);

  const items = [
    ...(update.info
      ? [{ id: "update", label: `Обновить 190x4 до версии ${update.info.version}`, icon: "reload" }, { separator: true }]
      : []),
    { id: "new-tab", label: "Новая вкладка", icon: "tab-add", keys: "Ctrl+T" },
    { id: "new-window", label: "Новое окно", icon: "window-16", keys: "Ctrl+N" },
    { id: "new-private", label: "Новое приватное окно", icon: "private-16", keys: "Ctrl+Shift+N" },
    { id: "reopen", label: reopenLabel(), icon: "history", keys: "Ctrl+Shift+T", disabled: !hasClosedTabs() },
    ...(state.splitId !== null
      ? [{ id: "end-split", label: "Выйти из разделения экрана", icon: "split-16" }]
      : []),
    { separator: true },
    { id: "history", label: "История", icon: "history", keys: "Ctrl+H" },
    { id: "downloads", label: "Загрузки", icon: "download", keys: "Ctrl+J" },
    { id: "bookmarks", label: "Закладки", icon: "favorites", keys: "Ctrl+Shift+O" },
    { id: "passwords", label: "Пароли", icon: "key" },
    { id: "extensions", label: "Расширения", icon: "puzzle" },
    { separator: true },
    { type: "zoom", id: "zoom", label: "Масштаб", value: tab?.zoom ?? 1, disabled: !web },
    { separator: true },
    { id: "print", label: "Печать…", icon: "print", keys: "Ctrl+P", disabled: !web },
    { id: "save-as", label: "Сохранить страницу как…", icon: "document", keys: "Ctrl+S", disabled: !web },
    { id: "find", label: "Найти на странице", icon: "find", keys: "Ctrl+F", disabled: !web },
    { id: "devtools", label: "Инструменты разработчика", icon: "code", keys: "F12", disabled: !web },
    { separator: true },
    { id: "settings", label: "Настройки", icon: "settings" },
    { id: "about", label: "О браузере 190x4", icon: "info" },
    { separator: true },
    { id: "exit", label: "Закрыть браузер", icon: "exit" },
  ];

  openMenu(
    "main",
    button,
    items,
    (action) => {
      switch (action) {
        case "update":
          return openUpdateBubble();
        case "new-tab":
          return open("about:newtab");
        case "new-window":
          return newWindow();
        case "new-private":
          return newWindow({ private: true });
        case "end-split":
          return endSplit();
        case "reopen":
          return reopenClosed();
        case "history":
          return openHistoryPage();
        case "downloads":
          return open("190x4://downloads");
        case "bookmarks":
          return openSettings("bookmarks");
        case "passwords":
          return openSettings("passwords");
        case "extensions":
          return openSettings("extensions");
        case "zoom_in":
        case "zoom_out":
        case "zoom_reset":
          return tabAction(action);
        case "print":
          return tabAction("print");
        case "save-as":
          return tabAction("save_as");
        case "find":
          return hooks.openFind();
        case "devtools":
          return tabAction("devtools");
        case "settings":
          return openSettings();
        case "about":
          return openSettings("about");
        case "exit":
          return closeBrowser();
      }
    },
    { width: 300, align: "end" }
  );
}

export function syncWindowState(maximized) {
  state.maximized = maximized;
  const button = document.getElementById("win-max");
  button.title = maximized ? "Свернуть в окно" : "Развернуть";
  button.setAttribute("aria-label", button.title);
  button.querySelector("use").setAttribute("href", `./assets/icons.svg#i-${maximized ? "cap-restore" : "cap-max"}`);
}

export async function loadWindowState() {
  const { maximized } = await invoke("window_state").catch(() => ({ maximized: false }));
  syncWindowState(maximized);
}
