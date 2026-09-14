/**
 * Панель закладок под адресной строкой.
 *
 * Корень панели — папка «Панель закладок» (id 1), справа — «Другие
 * закладки» (id 2). Что не влезло по ширине, уходит в «»». Папки
 * раскрываются меню во всплывающем окне, правый клик — контекстное меню,
 * порядок меняется перетаскиванием.
 */

import { invoke, listen } from "./bridge.js";
import { el, favicon, icon } from "./dom.js";
import { syncDuring } from "./layout.js";
import { onPopupAction, openMenu, openPopup } from "./popups.js";
import { onPref, pref } from "./prefs.js";
import { activeTab, state } from "./state.js";
import {
  addBookmarkFolder,
  bookmarkCurrent,
  editBookmark,
  isNewTabUrl,
  navigate,
  openSettings,
  toggleBookmarksBar,
} from "./actions.js";

const BAR = 1;
const OTHER = 2;

const bar = document.getElementById("bookmarks");
const itemsNode = document.getElementById("bookmarks-items");
const overflowButton = document.getElementById("bookmarks-overflow");
const otherButton = document.getElementById("bookmarks-other");

let nodes = [];
let firstHidden = -1;

export async function initBookmarksBar() {
  listen("bookmarks", reloadBookmarks);
  onPref((key) => {
    if (key === "bookmarks_bar") renderBarVisibility();
  });

  onPopupAction("bookmark-folder", ({ action, url, urls }) => {
    if (action === "open") navigate(url);
    if (action === "open-new") navigate(url, { newTab: true, background: true });
    if (action === "open-all") {
      for (const link of urls ?? []) navigate(link, { newTab: true, background: true });
    }
  });

  overflowButton.addEventListener("click", () =>
    openFolder(BAR, overflowButton, { skip: firstHidden, title: "Панель закладок" })
  );
  otherButton.addEventListener("click", () => openFolder(OTHER, otherButton, { title: "Другие закладки" }));
  otherButton.addEventListener("contextmenu", (event) => {
    event.preventDefault();
    showMenu(null, event);
  });

  bar.addEventListener("contextmenu", (event) => {
    if (event.target.closest(".bookmark")) return;
    event.preventDefault();
    showMenu(null, event);
  });

  new ResizeObserver(() => layoutOverflow()).observe(bar);
  wireDrag();
  await reloadBookmarks();
}

export async function reloadBookmarks() {
  nodes = await invoke("bookmarks_tree").catch(() => []);
  state.bookmarks = nodes;
  render();
  document.dispatchEvent(new CustomEvent("bookmarks-changed"));
}

export function bookmarkNodes() {
  return nodes;
}

function childrenOf(parent) {
  return nodes.filter((node) => node.parent_id === parent).sort((a, b) => a.position - b.position);
}

/** Показывать ли панель: всегда, только на новой вкладке или никогда. */
export function renderBarVisibility() {
  const mode = pref("bookmarks_bar");
  const tab = activeTab();
  const onNewTab = !tab || (!tab.internal && isNewTabUrl(tab.url));
  const shown = mode === "always" || (mode === "newtab" && onNewTab);

  const root = document.documentElement;
  if ((root.dataset.bookmarks === "shown") === shown) return;
  if (shown) root.dataset.bookmarks = "shown";
  else delete root.dataset.bookmarks;
  bar.hidden = !shown;
  // Панель отняла или вернула высоту странице — двигаем нативную поверхность.
  syncDuring(80);
  if (shown) requestAnimationFrame(layoutOverflow);
}

function render() {
  const items = childrenOf(BAR);
  itemsNode.replaceChildren(...items.map(makeItem));

  if (!items.length) {
    const hint = el("div", "bookmarks__empty");
    hint.append(el("span", null, "Чтобы быстро открывать сайты, добавьте их на панель закладок."));
    const importButton = el("button", null, "Импортировать закладки");
    importButton.addEventListener("click", () => openSettings("bookmarks"));
    hint.append(importButton);
    itemsNode.append(hint);
  }

  otherButton.hidden = childrenOf(OTHER).length === 0;
  requestAnimationFrame(layoutOverflow);
}

function makeItem(node) {
  const button = el("button", "bookmark");
  button.type = "button";
  button.dataset.id = node.id;
  button.draggable = true;

  if (node.kind === "folder") {
    button.append(icon("folder-16", 16, "bookmark__icon"));
    button.title = node.title;
  } else {
    button.append(favicon(node.icon, "bookmark__icon"));
    button.title = node.title ? `${node.title}\n${node.url}` : node.url;
  }

  if (node.title) button.append(el("span", "bookmark__title", node.title));
  else button.classList.add("bookmark--bare");

  button.addEventListener("click", (event) => {
    if (node.kind === "folder") {
      openFolder(node.id, button, { title: node.title });
    } else {
      navigate(node.url, { newTab: event.ctrlKey || event.shiftKey, background: event.ctrlKey });
    }
  });
  button.addEventListener("auxclick", (event) => {
    if (event.button === 1 && node.kind === "url") navigate(node.url, { newTab: true, background: true });
  });
  button.addEventListener("contextmenu", (event) => {
    event.preventDefault();
    showMenu(node, event);
  });
  return button;
}

/** Что не влезает по ширине — прячем и отдаём кнопке «»». */
function layoutOverflow() {
  if (bar.hidden) return;
  const children = [...itemsNode.querySelectorAll(".bookmark")];
  for (const child of children) child.hidden = false;
  overflowButton.hidden = true;

  const available = itemsNode.getBoundingClientRect().right;
  firstHidden = children.findIndex((child) => child.getBoundingClientRect().right > available + 0.5);
  if (firstHidden < 0) return;

  overflowButton.hidden = false;
  // Кнопка «»» сама отняла место — пересчитываем границу.
  const limit = itemsNode.getBoundingClientRect().right;
  firstHidden = children.findIndex((child) => child.getBoundingClientRect().right > limit + 0.5);
  children.forEach((child, index) => {
    child.hidden = firstHidden >= 0 && index >= firstHidden;
  });
}

function openFolder(folder, anchor, { skip = 0, title = "" } = {}) {
  return openPopup("bookmark-folder", anchor, {
    width: 320,
    payload: { folder, skip: Math.max(0, skip), title },
  });
}

/* ── Контекстное меню ──────────────────────────────────────── */

function showMenu(node, event) {
  const anchor = { x: event.clientX, y: event.clientY, width: 0, height: 0 };
  const tab = activeTab();
  const canAddPage = Boolean(tab && !tab.internal && !isNewTabUrl(tab.url));
  const items = [];

  if (node?.kind === "url") {
    items.push(
      { id: "open", label: "Открыть", icon: "globe" },
      { id: "open-new", label: "Открыть в новой вкладке", icon: "tab-add" },
      { separator: true }
    );
  }
  if (node) {
    items.push(
      { id: "edit", label: node.kind === "folder" ? "Переименовать…" : "Изменить…", icon: "edit-16" },
      { id: "remove", label: "Удалить", icon: "delete-16" },
      { separator: true }
    );
  }
  items.push(
    { id: "add-page", label: "Добавить текущую страницу", icon: "star-20", disabled: !canAddPage },
    { id: "add-folder", label: "Добавить папку…", icon: "folder" },
    { separator: true },
    { id: "manager", label: "Диспетчер закладок", icon: "favorites", keys: "Ctrl+Shift+O" },
    { id: "toggle-bar", label: "Показывать панель закладок", keys: "Ctrl+Shift+B", checked: pref("bookmarks_bar") !== "never" }
  );

  const target = node ? itemsNode.querySelector(`[data-id="${node.id}"]`) ?? anchor : anchor;
  openMenu(`bookmark:${node?.id ?? "bar"}`, anchor, items, (action) => {
    switch (action) {
      case "open":
        return navigate(node.url);
      case "open-new":
        return navigate(node.url, { newTab: true });
      case "edit":
        return editBookmark(node, target);
      case "remove":
        return invoke("bookmark_remove", { id: node.id });
      case "add-page":
        return bookmarkCurrent();
      case "add-folder":
        return addBookmarkFolder(node?.kind === "folder" ? node.id : node?.parent_id ?? BAR, target);
      case "manager":
        return openSettings("bookmarks");
      case "toggle-bar":
        return toggleBookmarksBar();
    }
  });
}

/* ── Перетаскивание ────────────────────────────────────────── */

function wireDrag() {
  let dragged = null;

  const clearMarks = () => {
    for (const node of bar.querySelectorAll("[data-drop]")) node.removeAttribute("data-drop");
  };

  bar.addEventListener("dragstart", (event) => {
    const item = event.target.closest(".bookmark[data-id]");
    if (!item) return;
    dragged = Number(item.dataset.id);
    item.dataset.dragging = "true";
    event.dataTransfer.effectAllowed = "move";
    event.dataTransfer.setData("text/plain", String(dragged));
  });

  bar.addEventListener("dragover", (event) => {
    if (dragged == null) return;
    const item = event.target.closest(".bookmark[data-id]");
    if (!item || Number(item.dataset.id) === dragged) return;
    event.preventDefault();
    clearMarks();
    const node = nodes.find((n) => n.id === Number(item.dataset.id));
    const rect = item.getBoundingClientRect();
    const x = (event.clientX - rect.left) / rect.width;
    item.dataset.drop = node?.kind === "folder" && x > 0.25 && x < 0.75 ? "into" : x < 0.5 ? "before" : "after";
  });

  bar.addEventListener("drop", (event) => {
    event.preventDefault();
    const item = bar.querySelector("[data-drop]");
    if (dragged != null && item) {
      const target = nodes.find((n) => n.id === Number(item.dataset.id));
      const mode = item.dataset.drop;
      if (target && mode === "into") {
        invoke("bookmark_move", { id: dragged, parent: target.id, index: 1_000_000 }).catch(() => {});
      } else if (target) {
        const siblings = childrenOf(target.parent_id).filter((n) => n.id !== dragged);
        let index = siblings.findIndex((n) => n.id === target.id);
        if (mode === "after") index += 1;
        invoke("bookmark_move", { id: dragged, parent: target.parent_id, index }).catch(() => {});
      }
    }
    finish();
  });

  const finish = () => {
    clearMarks();
    bar.querySelector('[data-dragging="true"]')?.removeAttribute("data-dragging");
    dragged = null;
  };
  bar.addEventListener("dragend", finish);
}
