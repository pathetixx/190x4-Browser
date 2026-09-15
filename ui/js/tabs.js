/**
 * Строка вкладок: рендер, открытие и закрытие, встроенные страницы,
 * контекстное меню, перетаскивание.
 */

import { invoke } from "./bridge.js";
import { el, favicon, hostOf, icon } from "./dom.js";
import { setPageHidden } from "./layout.js";
import { openMenu } from "./popups.js";
import { insertTabAt, moveTab, removeTab, setActive, state, tabIndex, upsertTab } from "./state.js";

const strip = document.getElementById("tabstrip");

/** Встроенные страницы браузера. Живут во вкладках, но рисуются chrome-ом. */
export const INTERNAL = {
  settings: { title: "Настройки", icon: "settings" },
  downloads: { title: "Загрузки", icon: "download" },
};

let internalSeq = 1_000_000;
const closedTabs = [];

export function initTabs() {
  // Ширина вкладок меняется и без перерисовки — окно развернули или сузили.
  new ResizeObserver(() => updateNarrow()).observe(strip);
  strip.addEventListener("click", onClick);
  strip.addEventListener("auxclick", (event) => {
    // Средняя кнопка закрывает вкладку — мышечная память из любого браузера.
    if (event.button !== 1) return;
    const tab = event.target.closest(".tab");
    if (tab) close(Number(tab.dataset.id));
  });
  strip.addEventListener("contextmenu", (event) => {
    const tab = event.target.closest(".tab");
    if (!tab) return;
    event.preventDefault();
    showTabMenu(Number(tab.dataset.id), event);
  });
  wireDrag();

  document.getElementById("new-tab").addEventListener("click", () => open("about:newtab"));
}

function onClick(event) {
  const tabNode = event.target.closest(".tab");
  if (!tabNode) return;
  const id = Number(tabNode.dataset.id);

  if (event.target.closest(".tab__close")) {
    close(id);
    return;
  }
  if (event.target.closest(".tab__audio")) {
    const tab = state.tabs.get(id);
    if (tab) invoke("tab_mute", { id, muted: !tab.muted }).catch(() => {});
    return;
  }
  activate(id);
}

/* ── Адреса встроенных страниц ─────────────────────────────── */

export function internalUrl(name, section = "") {
  return `190x4://${name}${section ? `/${section}` : ""}`;
}

/** `190x4://settings/passwords`, `about:downloads` → { name, section }. */
export function parseInternal(url) {
  const match = /^(?:190x4:\/\/|about:)(settings|downloads)(?:\/([\w-]*))?\/?$/i.exec(String(url ?? "").trim());
  return match ? { name: match[1].toLowerCase(), section: match[2] ?? "" } : null;
}

/* ── Открыть, переключить, закрыть ─────────────────────────── */

export async function open(url, { background = false, index = null } = {}) {
  const internal = parseInternal(url);
  if (internal) return openInternal(internal.name, internal.section, { background, index });

  const id = await invoke("tab_open", { url });
  // События движка могли прийти раньше ответа команды — не затираем их.
  const known = state.tabs.has(id);
  if (index != null) insertTabAt(id, known ? {} : { loading: true }, index);
  else if (!known) upsertTab(id, { loading: true });
  if (!background) {
    await activate(id);
    if (url === "about:newtab") focusOmnibox();
  }
  return id;
}

/** Курсор в адресную строку: слушает omnibox.js, так нет циклического импорта. */
export function focusOmnibox() {
  document.dispatchEvent(new CustomEvent("browser:focus-omnibox"));
}

function openInternal(name, section, { background, index }) {
  // Одна страница настроек на окно, как в Chrome: повторный вызов
  // переключает на уже открытую.
  const existing = [...state.tabs.values()].find((tab) => tab.internal === name);
  if (existing) {
    if (section) upsertTab(existing.id, { section, url: internalUrl(name, section) });
    if (!background) activate(existing.id);
    return existing.id;
  }

  const id = internalSeq++;
  const patch = {
    internal: name,
    section,
    title: INTERNAL[name].title,
    url: internalUrl(name, section),
    loading: false,
  };
  if (index != null) insertTabAt(id, patch, index);
  else upsertTab(id, patch);
  if (!background) activate(id);
  return id;
}

export async function activate(id) {
  const tab = state.tabs.get(id);
  if (!tab) return;
  setActive(id);

  if (tab.internal) {
    setPageHidden("internal", true);
    return;
  }
  // Сначала переключаем нативную вкладку, потом показываем контейнер:
  // иначе на кадр мелькнёт предыдущий сайт.
  await invoke("tab_activate", { id }).catch(() => {});
  if (state.activeId === id) setPageHidden("internal", false);
}

export async function close(id) {
  const tab = state.tabs.get(id);
  if (!tab) return;

  const ids = [...state.tabs.keys()];
  const index = ids.indexOf(id);
  const wasActive = state.activeId === id;

  const url = tab.internal ? internalUrl(tab.internal, tab.section) : tab.url;
  if (url) {
    closedTabs.push({ url, index });
    if (closedTabs.length > 25) closedTabs.shift();
  }

  removeTab(id);
  if (!tab.internal) await invoke("tab_close", { id }).catch(() => {});

  if (state.tabs.size === 0) {
    // Последнюю вкладку закрыли — окно не пустеет, а открывает новую.
    await open("about:newtab");
    return;
  }
  if (wasActive) {
    // Фокус уходит на соседа справа, как в любом браузере.
    const next = ids[index + 1] ?? ids[index - 1];
    if (next != null) await activate(next);
  }
}

/** Ctrl+Shift+T. */
export async function reopenClosed() {
  const last = closedTabs.pop();
  if (last) await open(last.url, { index: last.index });
}

export function hasClosedTabs() {
  return closedTabs.length > 0;
}

/** Ctrl+Tab / Ctrl+Shift+Tab. */
export function cycle(delta) {
  const ids = [...state.tabs.keys()];
  if (ids.length < 2) return;
  const index = ids.indexOf(state.activeId);
  activate(ids[(index + delta + ids.length) % ids.length]);
}

async function closeMany(ids) {
  for (const id of ids) await close(id);
}

/* ── Контекстное меню вкладки ──────────────────────────────── */

function showTabMenu(id, event) {
  const tab = state.tabs.get(id);
  if (!tab) return;
  const ids = [...state.tabs.keys()];
  const index = ids.indexOf(id);

  const items = [
    { id: "new-right", label: "Новая вкладка справа", icon: "tab-add" },
    { separator: true },
    { id: "reload", label: "Обновить", icon: "reload", keys: "Ctrl+R", disabled: Boolean(tab.internal) },
    { id: "duplicate", label: "Дублировать", icon: "copy-16" },
    {
      id: "mute",
      label: tab.muted ? "Включить звук" : "Выключить звук",
      icon: tab.muted ? "speaker-16" : "mute-16",
      disabled: Boolean(tab.internal),
    },
    { separator: true },
    { id: "close", label: "Закрыть", icon: "dismiss-16", keys: "Ctrl+W" },
    { id: "close-others", label: "Закрыть другие вкладки", disabled: ids.length < 2 },
    { id: "close-right", label: "Закрыть вкладки справа", disabled: index === ids.length - 1 },
    { separator: true },
    { id: "reopen", label: "Открыть закрытую вкладку", keys: "Ctrl+Shift+T", disabled: !hasClosedTabs() },
  ];

  const anchor = { x: event.clientX, y: event.clientY, width: 0, height: 0 };
  openMenu("tab", anchor, items, (action) => {
    switch (action) {
      case "new-right":
        open("about:newtab", { index: tabIndex(id) + 1 });
        break;
      case "reload":
        invoke("tab_action", { id, action: "reload" }).catch(() => {});
        break;
      case "duplicate":
        open(tab.internal ? internalUrl(tab.internal) : tab.url, { index: tabIndex(id) + 1 });
        break;
      case "mute":
        invoke("tab_mute", { id, muted: !tab.muted }).catch(() => {});
        break;
      case "close":
        close(id);
        break;
      case "close-others":
        closeMany([...state.tabs.keys()].filter((other) => other !== id));
        break;
      case "close-right": {
        const all = [...state.tabs.keys()];
        closeMany(all.slice(all.indexOf(id) + 1));
        break;
      }
      case "reopen":
        reopenClosed();
        break;
    }
  });
}

/* ── Перетаскивание вкладок ────────────────────────────────── */

function wireDrag() {
  let dragged = null;

  strip.addEventListener("dragstart", (event) => {
    const tab = event.target.closest(".tab");
    if (!tab) return;
    dragged = Number(tab.dataset.id);
    tab.dataset.dragging = "true";
    event.dataTransfer.effectAllowed = "move";
    event.dataTransfer.setData("text/plain", String(dragged));
  });

  strip.addEventListener("dragover", (event) => {
    if (dragged == null) return;
    event.preventDefault();
    const target = event.target.closest(".tab");
    if (!target || Number(target.dataset.id) === dragged) return;
    const rect = target.getBoundingClientRect();
    const after = event.clientX > rect.left + rect.width / 2;
    const index = tabIndex(Number(target.dataset.id));
    const from = tabIndex(dragged);
    let to = after ? index + 1 : index;
    if (from < to) to -= 1;
    if (to !== from) moveTab(dragged, to);
  });

  const finish = () => {
    strip.querySelector('[data-dragging="true"]')?.removeAttribute("data-dragging");
    dragged = null;
  };
  strip.addEventListener("drop", (event) => {
    event.preventDefault();
    finish();
  });
  strip.addEventListener("dragend", finish);
}

/* ── Рендер ────────────────────────────────────────────────── */

/**
 * Инкрементальный рендер.
 *
 * Строка вкладок перерисовывается по каждому событию — а их поток плотный.
 * Пересоздавать DOM целиком нельзя: элемент заново проигрывает анимацию
 * появления, теряется hover и позиция крестика под курсором.
 */
const nodes = new Map();

export function renderTabs() {
  const tabs = [...state.tabs.values()];

  for (const [id, node] of nodes) {
    if (!state.tabs.has(id)) {
      node.remove();
      nodes.delete(id);
    }
  }

  tabs.forEach((tab, index) => {
    let node = nodes.get(tab.id);
    if (!node) {
      node = el("div", "tab");
      node.dataset.id = tab.id;
      node.draggable = true;
      node.setAttribute("role", "tab");
      nodes.set(tab.id, node);
    }
    updateTab(node, tab);
    if (strip.children[index] !== node) strip.insertBefore(node, strip.children[index] ?? null);
  });

  // Ширина полосы — от числа вкладок, а не от их содержимого: иначе узкие
  // вкладки без подписей сжимали полосу под себя и не расширялись, когда место
  // появлялось (окно развернули, вкладки закрыли).
  strip.parentElement.style.setProperty("--tab-count", String(tabs.length));
  requestAnimationFrame(updateNarrow);
}

/** Узкий режим — по фактической ширине плитки: при открытой панели места меньше при том же счёте. */
function updateNarrow() {
  for (const node of strip.children) {
    const width = node.getBoundingClientRect().width;
    // Подпись прячем, только когда от неё остались бы две-три буквы.
    const narrow = width < 64 ? "true" : "false";
    // Тесная вкладка: крестик неактивной не держит место под себя — как в
    // Chrome, иначе подписи не остаётся уже на 70 px.
    const compact = width < 110 ? "true" : "false";
    if (node.dataset.narrow !== narrow) node.dataset.narrow = narrow;
    if (node.dataset.compact !== compact) node.dataset.compact = compact;
  }
}

/** Оборот колеса загрузки — `tab-spin` в tabs.css. */
const SPIN_MS = 1570;

function updateTab(node, tab) {
  const active = state.activeId === tab.id;
  setAttr(node, "data-active", String(active));
  setAttr(node, "aria-selected", String(active));
  setAttr(node, "data-muted", String(tab.muted));

  const title = tab.title || hostOf(tab.url) || "Новая вкладка";
  if (node.title !== title) node.title = title;

  const iconKind = tab.internal
    ? `internal:${tab.internal}`
    : tab.loading
      ? "spinner"
      : tab.favicon
        ? `favicon:${tab.favicon}`
        : "globe";
  if (node.dataset.icon !== iconKind) {
    node.dataset.icon = iconKind;
    node.querySelector("[data-slot='icon']")?.remove();
    let iconNode;
    if (tab.internal) {
      iconNode = icon(`${INTERNAL[tab.internal].icon}`, 16, "tab__glyph tab__glyph--brand");
    } else if (tab.loading) {
      iconNode = el("span", "tab__spinner");
      // Фаза — от общих часов: колесо не начинает оборот заново каждый раз,
      // когда вкладка снова грузится (редирект, перезагрузка), и не дёргается.
      iconNode.style.animationDelay = `${-(performance.now() % SPIN_MS)}ms`;
    } else if (tab.favicon) {
      iconNode = favicon(tab.favicon, "tab__favicon");
    } else {
      iconNode = icon("globe-16", 16, "tab__glyph");
    }
    iconNode.dataset.slot = "icon";
    node.prepend(iconNode);
  }

  let titleNode = node.querySelector(".tab__title");
  if (!titleNode) {
    titleNode = el("span", "tab__title");
    node.append(titleNode);
  }
  if (titleNode.textContent !== title) titleNode.textContent = title;

  const audioState = tab.muted ? "muted" : tab.audible ? "audible" : "";
  let audioNode = node.querySelector(".tab__audio");
  if (audioState && audioNode?.dataset.state !== audioState) {
    audioNode?.remove();
    audioNode = el("span", "tab__audio");
    audioNode.dataset.state = audioState;
    audioNode.title = tab.muted ? "Включить звук" : "Выключить звук";
    audioNode.append(icon(tab.muted ? "mute-16" : "speaker-16", 16));
    titleNode.after(audioNode);
  } else if (!audioState && audioNode) {
    audioNode.remove();
  }

  let closeNode = node.querySelector(".tab__close");
  if (!closeNode) {
    closeNode = el("span", "tab__close");
    closeNode.title = "Закрыть вкладку (Ctrl+W)";
    closeNode.append(icon("dismiss-12", 12));
  }
  // Крестик всегда последний, даже если между делом появился динамик.
  if (node.lastElementChild !== closeNode) node.append(closeNode);
}

function setAttr(node, name, value) {
  if (node.getAttribute(name) !== value) node.setAttribute(name, value);
}
