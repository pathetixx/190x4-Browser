/**
 * Действия браузера — одно место для всего, что вызывается из меню,
 * палитры, горячих клавиш и кнопок.
 *
 * Модули интерфейса (тулбар, панель закладок, адресная строка) зовут эти
 * функции, а не друг друга: так нет циклических зависимостей, и Ctrl+D из
 * страницы делает ровно то же, что клик по звезде.
 */

import { invoke } from "./bridge.js";
import { openPopup } from "./popups.js";
import { pref, setPref } from "./prefs.js";
import { activeTab, removeTab, state, tabIndex } from "./state.js";
import { open, parseInternal } from "./tabs.js";

/** Точки, которые заполняет main.js: панели и поиск живут в своих модулях. */
export const hooks = {
  togglePanel: (_name) => {},
  openPanel: (_name) => {},
  openFind: () => {},
  toast: (_text) => {},
  saveSession: async () => {},
};

export function isNewTabUrl(url) {
  return !url || url === "about:blank" || url === "about:newtab" || url.includes("190x4-pages.invalid/newtab");
}

/** Ввод из адресной строки, клик по закладке, пункт истории. */
export async function navigate(input, { newTab = false, background = false } = {}) {
  const tab = activeTab();
  if (parseInternal(input) || newTab || !tab) return open(input, { background });

  if (tab.internal) {
    // Во встроенной вкладке набрали адрес — на её месте открывается сайт.
    const index = tabIndex(tab.id);
    const id = await open(input, { index });
    removeTab(tab.id);
    return id;
  }
  await invoke("tab_navigate", { id: tab.id, url: input });
  return tab.id;
}

export function tabAction(name) {
  const tab = activeTab();
  if (tab && !tab.internal) invoke("tab_action", { id: tab.id, action: name }).catch(() => {});
}

export function goHome() {
  const url = pref("home_page") === "url" && pref("home_url") ? pref("home_url") : "about:newtab";
  return navigate(url);
}

export function openSettings(section = "") {
  return open(`190x4://settings${section ? `/${section}` : ""}`);
}

export function openDownloadsPage() {
  return open("190x4://downloads");
}

export function zoom(direction) {
  tabAction(direction > 0 ? "zoom_in" : direction < 0 ? "zoom_out" : "zoom_reset");
}

export function toggleBookmarksBar() {
  return setPref("bookmarks_bar", pref("bookmarks_bar") === "always" ? "never" : "always");
}

export function closeBrowser() {
  invoke("window_command", { action: "close" }).catch(() => {});
}

/** Папки закладок строкой пути — для выпадающего списка «Папка». */
export async function bookmarkFolders() {
  const nodes = await invoke("bookmarks_tree").catch(() => []);
  const byId = new Map(nodes.map((node) => [node.id, node]));
  const path = (node) => {
    const parts = [];
    for (let cursor = node; cursor; cursor = byId.get(cursor.parent_id)) parts.unshift(cursor.title);
    return parts.join(" / ");
  };
  return nodes
    .filter((node) => node.kind === "folder")
    .map((node) => ({ id: node.id, title: path(node), depth: path(node).split(" / ").length - 1 }))
    .sort((a, b) => a.title.localeCompare(b.title, "ru"));
}

/**
 * Ctrl+D и звезда: закладка сохраняется сразу, пузырь позволяет поправить
 * название и папку или отменить — ровно как в Chrome.
 */
export async function bookmarkCurrent() {
  const tab = activeTab();
  if (!tab || tab.internal || isNewTabUrl(tab.url)) return;

  let node = await invoke("bookmark_find", { url: tab.url }).catch(() => null);
  const created = !node;
  if (!node) {
    node = await invoke("bookmark_add", {
      title: tab.title || tab.url,
      url: tab.url,
      icon: tab.favicon ?? null,
    });
  }

  await openPopup("bookmark", document.getElementById("omni-star"), {
    width: 360,
    align: "end",
    payload: { mode: "edit", node, created, folders: await bookmarkFolders() },
  });
}

/** Правка существующей закладки или папки (контекстное меню). */
export async function editBookmark(node, anchor) {
  await openPopup("bookmark", anchor, {
    width: 360,
    payload: { mode: "edit", node, created: false, folders: await bookmarkFolders() },
  });
}

export async function addBookmarkFolder(parent, anchor) {
  await openPopup("bookmark", anchor, {
    width: 360,
    payload: { mode: "folder", parent, folders: await bookmarkFolders() },
  });
}

/** Окно расширения «Загрузчик видео» для текущей страницы. */
export async function openMediaExtension(explicitUrl = null) {
  const tab = activeTab();
  const pinned = document.getElementById("ext-media");
  const anchor = pinned && !pinned.hidden ? pinned : document.getElementById("ext-menu");
  const page = tab && !tab.internal && !isNewTabUrl(tab.url) ? tab.url : "";
  await openPopup("media", anchor, {
    width: 380,
    align: "end",
    payload: {
      url: (typeof explicitUrl === "string" && explicitUrl) || tab?.media?.url || page,
      title: tab?.title ?? "",
      services: state.services,
    },
  });
}

export function openDownloadsBubble() {
  const button = document.getElementById("downloads-btn");
  button.hidden = false;
  return openPopup("downloads", button, { width: 380, align: "end" });
}
