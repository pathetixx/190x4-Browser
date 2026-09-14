/**
 * Панель инструментов справа от адресной строки: расширения, загрузки,
 * меню «Настройки и прочее».
 */

import { invoke } from "./bridge.js";
import { onDownloads, summary } from "./downloads-model.js";
import { openMenu } from "./popups.js";
import { onPref, pref, setPref } from "./prefs.js";
import { activeTab, state } from "./state.js";
import { hasClosedTabs, open, reopenClosed } from "./tabs.js";
import {
  closeBrowser,
  goHome,
  hooks,
  openDownloadsBubble,
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
  document.getElementById("nav-back").addEventListener("click", () => tabAction("back"));
  document.getElementById("nav-forward").addEventListener("click", () => tabAction("forward"));
  document.getElementById("nav-reload").addEventListener("click", () => tabAction("reload"));
  homeButton.addEventListener("click", goHome);

  downloadsButton.addEventListener("click", () => openDownloadsBubble());
  mediaButton.addEventListener("click", () => openMediaExtension());
  document.getElementById("ext-menu").addEventListener("click", (event) => showExtensionsMenu(event.currentTarget));
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

/* ── Меню «Настройки и прочее» ─────────────────────────────── */

function showMainMenu(button) {
  const tab = activeTab();
  const web = Boolean(tab && !tab.internal);

  const items = [
    { id: "new-tab", label: "Новая вкладка", icon: "tab-add", keys: "Ctrl+T" },
    { id: "reopen", label: "Открыть закрытую вкладку", icon: "history", keys: "Ctrl+Shift+T", disabled: !hasClosedTabs() },
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
        case "new-tab":
          return open("about:newtab");
        case "reopen":
          return reopenClosed();
        case "history":
          return hooks.openPanel("history");
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

/* ── Меню расширений ───────────────────────────────────────── */

function showExtensionsMenu(button) {
  const enabled = pref("ext_media_enabled");
  const items = [
    { type: "header", label: "Расширения" },
    {
      id: "media",
      label: "Загрузчик видео 190x4",
      icon: "video",
      disabled: !enabled,
      trailing: pref("ext_media_pinned") ? "pinned" : "unpinned",
    },
    { separator: true },
    { id: "manage", label: "Управление расширениями", icon: "settings" },
  ];

  openMenu(
    "extensions",
    button,
    items,
    (action) => {
      if (action === "media") openMediaExtension();
      if (action === "media:pin") setPref("ext_media_pinned", !pref("ext_media_pinned"));
      if (action === "manage") openSettings("extensions");
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
