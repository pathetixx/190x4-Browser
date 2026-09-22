/**
 * Строка вкладок: рендер, открытие и закрытие, встроенные страницы,
 * закреплённые и спящие вкладки, контекстное меню, перетаскивание.
 *
 * Вкладка из прошлого сеанса сначала спит: у неё есть место в строке, адрес и
 * заголовок, но нет ни вебвью, ни памяти под страницу. Просыпается она при
 * первом показе — так же ведут себя Chrome и Edge, и двадцать вкладок в
 * сессии больше не поднимают двадцать страниц на старте.
 */

import { invoke } from "./bridge.js";
import { hasLeaveDialog } from "./dialogs.js";
import { el, favicon, hostOf, icon } from "./dom.js";
import { setPageHidden } from "./layout.js";
import { onPopupAction, openMenu, openPopup } from "./popups.js";
import {
  groupBounds,
  insertTabAt,
  markClosed,
  moveTab,
  removeTab,
  replaceTabId,
  setActive,
  setSplit,
  state,
  tabIndex,
  upsertTab,
} from "./state.js";

const strip = document.getElementById("tabstrip");

/** Встроенные страницы браузера. Живут во вкладках, но рисуются chrome-ом. */
export const INTERNAL = {
  settings: { title: "Настройки", icon: "settings" },
  downloads: { title: "Загрузки", icon: "download" },
  history: { title: "История", icon: "history" },
};

/** Цвета групп — как в Chrome: у группы есть имя и цвет, больше ничего. */
export const GROUP_COLORS = [
  ["rose", "Багровый"],
  ["amber", "Янтарный"],
  ["lime", "Лаймовый"],
  ["teal", "Бирюзовый"],
  ["sky", "Небесный"],
  ["violet", "Фиолетовый"],
  ["slate", "Серый"],
];

let groupSeq = 1;
let internalSeq = 1_000_000;
/** Спящие вкладки нумеруются в минус: их номера не встретятся с номерами Rust. */
let sleepSeq = -1;
const closedTabs = [];

export function initTabs() {
  onPopupAction("group", ({ action, id, title, color }) => {
    if (action === "rename") updateGroup(id, { title: String(title ?? "").slice(0, 40) });
    if (action === "color") updateGroup(id, { color });
    if (action === "collapse") toggleGroup(id);
    if (action === "ungroup") ungroupAll(id);
    if (action === "close") closeGroup(id);
  });

  // Ширина вкладок меняется и без перерисовки — окно развернули или сузили.
  new ResizeObserver(() => updateNarrow()).observe(strip);
  // Вкладки не поместились — колесо прокручивает строку, как в Edge.
  strip.addEventListener(
    "wheel",
    (event) => {
      if (strip.scrollWidth <= strip.clientWidth) return;
      event.preventDefault();
      strip.scrollLeft += Math.abs(event.deltaY) > Math.abs(event.deltaX) ? event.deltaY : event.deltaX;
    },
    { passive: false }
  );
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
  const match = /^(?:190x4:\/\/|about:)(settings|downloads|history)(?:\/([\w-]*))?\/?$/i.exec(
    String(url ?? "").trim()
  );
  return match ? { name: match[1].toLowerCase(), section: match[2] ?? "" } : null;
}

/* ── Открыть, переключить, закрыть ─────────────────────────── */

/**
 * Открыть вкладку. `popup` — номер окна, которое открыла страница `opener`:
 * вкладка достаётся этому окну, и страница может с ним разговаривать.
 */
export async function open(url, { background = false, index = null, popup = null, opener = null } = {}) {
  const internal = parseInternal(url);
  if (internal) return openInternal(internal.name, internal.section, { background, index });

  const id = await invoke("tab_open", { url, popup });
  // Следующую новую вкладку — прогретой: эту движок только что отдал.
  if (url === "about:newtab") prewarmSoon(400);
  // События движка могли прийти раньше ответа команды — не затираем их.
  const known = state.tabs.has(id);
  const parent = opener != null ? state.tabs.get(opener) : null;
  // Обычная вкладка не встаёт среди закреплённых.
  const at = Math.max(index ?? (parent ? openerIndex(opener, background) : defaultIndex()), groupBounds(false).from);
  const patch = known ? {} : { loading: true };
  if (parent) {
    patch.opener = opener;
    // Ссылка из вкладки группы открывается в той же группе — как в Chrome.
    if (parent.group) patch.group = { ...parent.group, collapsed: false };
  }
  if (index != null || parent || !known) insertTabAt(id, patch, at);
  settleGroup(id);
  if (!background) {
    await activate(id);
    if (url === "about:newtab") focusOmnibox();
  }
  return id;
}

/**
 * Прогреть новую вкладку окна: Ctrl+T и «+» показывают уже нарисованную
 * страницу, без пустого кадра и отрисовки с нуля. Не сразу — сначала пусть
 * отрисуется то, что открыли сейчас.
 */
let prewarmTimer = 0;
export function prewarmSoon(delay = 1200) {
  clearTimeout(prewarmTimer);
  prewarmTimer = setTimeout(() => invoke("tab_prewarm").catch(() => {}), delay);
}

/** Новая вкладка (Ctrl+T, «+») встаёт в конец строки — как в Chrome. */
function defaultIndex() {
  return state.tabs.size;
}

/**
 * Место вкладки, которую открыла страница, — как в Chrome: сразу за
 * страницей-родителем, а фоновые — ещё и за теми, что она уже открыла. Иначе
 * ссылки 1, 2, 3, открытые Ctrl+щелчком, ложились бы в строку задом наперёд.
 */
function openerIndex(opener, background) {
  const ids = [...state.tabs.keys()];
  let at = ids.indexOf(opener);
  if (at < 0) return defaultIndex();
  const parent = state.tabs.get(opener);
  // Из закреплённой вкладки — сразу за закреплёнными.
  if (parent.pinned) at = Math.max(at, groupBounds(false).from - 1);
  if (background) {
    // Прежние вкладки этой страницы — только из её же группы: за край группы
    // новая вкладка не уходит.
    const group = parent.group?.id ?? null;
    const sibling = (tab) => tab?.opener === opener && (tab.group?.id ?? null) === group;
    while (at + 1 < ids.length && sibling(state.tabs.get(ids[at + 1]))) at += 1;
  }
  return at + 1;
}

/**
 * Вкладки группы стоят подряд, как в Chrome: вкладка, вставшая между двумя
 * вкладками одной группы, входит в неё, а ушедшая от своей группы — выходит.
 * Без этого группа разрывалась надвое и её ярлык терял часть вкладок.
 */
function settleGroup(id) {
  const tab = state.tabs.get(id);
  if (!tab || tab.pinned) return;
  const ids = [...state.tabs.keys()];
  const at = ids.indexOf(id);
  const prev = state.tabs.get(ids[at - 1]);
  const next = state.tabs.get(ids[at + 1]);
  const around = prev?.group && prev.group.id === next?.group?.id ? prev.group : null;
  if (around) {
    if (tab.group?.id === around.id) return;
    if (tab.internal) {
      // Встроенная страница в группу не входит — встаёт сразу за группой.
      moveTab(id, tabIndex(groupTabs(around.id).at(-1).id));
      return;
    }
    upsertTab(id, { group: { ...around } });
    return;
  }
  if (!tab.group) return;
  const own = tab.group.id;
  if (prev?.group?.id === own || next?.group?.id === own) return;
  if (groupTabs(own).length > 1) upsertTab(id, { group: null });
}

/**
 * Вкладка из сессии: место в строке есть, страницы нет. Просыпается при
 * первом показе.
 */
export function openSleeping({ url, title = "", pinned = false, group = null }) {
  const id = sleepSeq--;
  if (group) groupSeq = Math.max(groupSeq, group.id + 1);
  upsertTab(id, {
    url,
    title: title || hostOf(url) || url,
    pinned,
    group,
    sleeping: true,
    loading: false,
  });
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
  insertTabAt(id, patch, Math.max(index ?? defaultIndex(), groupBounds(false).from));
  settleGroup(id);
  if (!background) activate(id);
  return id;
}

/** Спящая вкладка просыпается: под неё заводится настоящая вкладка движка. */
async function wake(tab) {
  const url = tab.url;
  const realId = await invoke("tab_open", { url }).catch(() => null);
  if (realId == null) return null;
  // Событие «вкладка открылась» могло прийти раньше ответа: свои поля
  // (закрепление, заголовок из сессии) переносим поверх.
  const known = state.tabs.get(realId);
  if (known) removeTab(realId);
  replaceTabId(tab.id, realId, {
    sleeping: false,
    loading: true,
    title: known?.title || tab.title,
    favicon: known?.favicon ?? tab.favicon,
  });
  return realId;
}

export async function activate(id) {
  let tab = state.tabs.get(id);
  if (!tab) return;

  // Вкладка свёрнутой группы на экран не выйдет — группа разворачивается.
  if (tab.group?.collapsed) updateGroup(tab.group.id, { collapsed: false });

  if (tab.sleeping) {
    const realId = await wake(tab);
    if (realId == null) return;
    id = realId;
    tab = state.tabs.get(id);
  }

  setActive(id);

  if (tab.internal) {
    setPageHidden("internal", true);
    return;
  }
  // Сначала переключаем нативную вкладку, потом показываем контейнер:
  // иначе на кадр мелькнёт предыдущий сайт.
  await invoke("tab_activate", { id }).catch(() => {});
  if (state.activeId === id) setPageHidden("internal", false);
  // Страница упала, пока вкладка была в фоне, — при показе она загружается
  // заново, как в Chrome. Щелчок по упавшей активной вкладке делает то же.
  if (tab.crashed && state.activeId === id) {
    upsertTab(id, { crashed: false });
    invoke("tab_action", { id, action: "reload" }).catch(() => {});
  }
}

/** Вторая вкладка рядом с активной: режим разделения экрана. */
export async function splitWith(id) {
  let tab = state.tabs.get(id);
  if (!tab || tab.internal || id === state.activeId) return;
  if (tab.sleeping) {
    const realId = await wake(tab);
    if (realId == null) return;
    id = realId;
  }
  await invoke("tab_split", { id }).catch(() => {});
  setSplit(id);
  setPageHidden("internal", false);
}

export async function endSplit() {
  if (state.splitId === null) return;
  await invoke("tab_split", { id: null }).catch(() => {});
  setSplit(null);
}

/* ── «Покинуть сайт?» перед закрытием ──────────────────────── */

/** Вкладки, которые ждут ответа страницы: номер → { promise, finish }. */
const leaving = new Map();

/**
 * Спросить страницу, можно ли её закрыть: если она просит «Покинуть сайт?»
 * (несохранённый текст, письмо), браузер покажет это окно. `false` —
 * ответили «Остаться». Страница, которая не отвечает, закрывается через
 * полторы секунды, если окна «Покинуть сайт?» нет.
 */
function confirmClose(id) {
  const known = leaving.get(id);
  if (known) return known.promise;
  let resolve;
  const entry = { promise: new Promise((r) => (resolve = r)), timer: 0 };
  entry.finish = (ok) => {
    clearTimeout(entry.timer);
    leaving.delete(id);
    resolve(ok);
  };
  const check = () => {
    if (!leaving.has(id)) return;
    if (hasLeaveDialog(id)) entry.timer = setTimeout(check, 1000);
    else entry.finish(true);
  };
  leaving.set(id, entry);
  invoke("tab_close_request", { id }).then(
    (asked) => {
      if (!asked) entry.finish(true);
      else entry.timer = setTimeout(check, 1500);
    },
    () => entry.finish(true)
  );
  return entry.promise;
}

/** Ответ страницы на закрытие: `close_confirmed` или `close_cancelled`. */
export function closeAnswered(id, ok) {
  leaving.get(id)?.finish(ok);
}

/** Вкладку сейчас закрывают и ждут ответа её страницы. */
export function isLeaving(id) {
  return leaving.has(id);
}

/**
 * Закрыть вкладку. `toOpener` — вкладка закрылась сама (окно входа через
 * Google закончило работу): фокус возвращается странице, которая её открыла.
 * `force` — не спрашивать страницу (документа нет или она уже согласилась).
 *
 * Вкладка уходит из строки и с экрана сразу, а страница решает в фоне: почти
 * никакая не держит, и ждать её ответа на каждое Ctrl+W незачем. Если она
 * просит «Покинуть сайт?», вкладка возвращается на место с этим окном.
 */
export async function close(id, { toOpener = false, force = false } = {}) {
  const tab = state.tabs.get(id);
  if (!tab || tab.closing) return;

  const ids = visibleIds();
  const index = ids.indexOf(id);
  const wasActive = state.activeId === id;
  // Закрыли активную половину разделённого экрана — вторая занимает окно
  // целиком (так же решает Rust). Иначе фокус уходит на соседа справа, как в
  // любом браузере.
  const partner = wasActive ? state.splitId : null;
  const opener = toOpener && state.tabs.has(tab.opener) ? tab.opener : null;
  const next = partner ?? opener ?? ids[index + 1] ?? ids[index - 1] ?? null;
  const position = tabIndex(id);

  if (!force && !tab.internal && !tab.sleeping) {
    upsertTab(id, { closing: true });
    // Половина разделённого экрана уходит с экрана вместе с разделением.
    if (state.splitId !== null && (state.splitId === id || wasActive)) await endSplit();
    if (wasActive && next != null) await activate(next);
    if (!(await confirmClose(id))) {
      if (state.tabs.has(id)) upsertTab(id, { closing: false });
      return;
    }
    if (!state.tabs.has(id)) return;
  }

  const url = tab.internal ? internalUrl(tab.internal, tab.section) : tab.url;
  if (url && !tab.url?.startsWith("about:")) {
    closedTabs.push({ url, index: position, pinned: tab.pinned });
    if (closedTabs.length > 25) closedTabs.shift();
  }

  markClosed(id);
  removeTab(id);
  if (!tab.internal && !tab.sleeping) await invoke("tab_close", { id }).catch(() => {});

  if (state.tabs.size === 0) {
    // Последнюю вкладку закрыли — окно не пустеет, а открывает новую.
    await open("about:newtab");
    return;
  }
  if (state.activeId === null) {
    const target = next != null && state.tabs.has(next) ? next : visibleIds()[0] ?? [...state.tabs.keys()][0];
    if (target != null) await activate(target);
  }
}

/** Вернуть вкладку, которую закрывают, если её страница спросила «Покинуть сайт?». */
export function returnLeaving(id) {
  if (!state.tabs.get(id)?.closing) return;
  upsertTab(id, { closing: false });
  activate(id);
}

/** Вкладки, которые видны в строке: без спрятанных в свёрнутых группах и закрываемых. */
export function visibleIds() {
  return [...state.tabs.values()].filter((tab) => !tab.group?.collapsed && !tab.closing).map((tab) => tab.id);
}

/** Закрепить или открепить: закреплённые всегда слева и без крестика. */
export function togglePin(id) {
  const tab = state.tabs.get(id);
  if (!tab || tab.internal) return;
  const pinned = !tab.pinned;
  // Закреплённая вкладка выходит из группы — как в Chrome.
  upsertTab(id, pinned ? { pinned, group: null } : { pinned });
  const bounds = groupBounds(pinned);
  moveTab(id, pinned ? bounds.to : bounds.from);
}

/**
 * Ctrl+Shift+T: последняя закрытая вкладка этого окна, а если их нет —
 * последнее окно, закрытое крестиком, со всеми его вкладками.
 */
export async function reopenClosed() {
  const last = closedTabs.pop();
  if (!last) {
    if (state.closedWindows > 0) await invoke("window_reopen_closed").catch(() => {});
    return;
  }
  const id = await open(last.url, { index: last.index });
  if (last.pinned) {
    upsertTab(id, { pinned: true, group: null });
    moveTab(id, last.index);
  }
}

export function hasClosedTabs() {
  return closedTabs.length > 0 || state.closedWindows > 0;
}

/** Подпись пункта «открыть закрытое»: вкладку или целое окно. */
export function reopenLabel() {
  return !closedTabs.length && state.closedWindows > 0 ? "Открыть закрытое окно" : "Открыть закрытую вкладку";
}

/** Ctrl+Tab / Ctrl+Shift+Tab: по видимым вкладкам, свёрнутые группы пропускаются. */
export function cycle(delta) {
  const ids = visibleIds();
  if (ids.length < 2) return;
  const index = ids.indexOf(state.activeId);
  activate(ids[(index + delta + ids.length) % ids.length]);
}

/** Ctrl+Shift+PageUp/PageDown: подвинуть активную вкладку на место соседа. */
export function moveActive(delta) {
  const id = state.activeId;
  if (id === null || !state.tabs.has(id)) return;
  moveTab(id, tabIndex(id) + delta);
  settleGroup(id);
}

/**
 * Закрыть несколько вкладок. Если среди них активная, сначала переключаемся на
 * `keep` — иначе каждое закрытие будило бы и показывало соседнюю вкладку,
 * которую следом тоже закрывают.
 */
async function closeMany(ids, keep = null) {
  if (keep != null && ids.includes(state.activeId) && state.tabs.has(keep)) await activate(keep);
  // Разом: каждая страница решает сама, ждать их по очереди незачем.
  await Promise.all(ids.map((id) => close(id)));
}

/* ── Контекстное меню вкладки ──────────────────────────────── */

function showTabMenu(id, event) {
  const tab = state.tabs.get(id);
  if (!tab) return;
  const ids = [...state.tabs.keys()];
  const index = ids.indexOf(id);
  const web = !tab.internal;
  const splitting = state.splitId !== null;
  // «Закрыть другие», «слева» и «справа» закреплённые вкладки не трогают —
  // как в Chrome: закрепляют именно то, что должно пережить уборку.
  const closable = (list) => list.filter((other) => other !== id && !state.tabs.get(other)?.pinned);
  const left = closable(ids.slice(0, index));
  const right = closable(ids.slice(index + 1));

  const items = [
    { id: "new-right", label: "Новая вкладка справа", icon: "tab-add" },
    { separator: true },
    { id: "reload", label: "Обновить", icon: "reload", keys: "Ctrl+R", disabled: !web },
    { id: "duplicate", label: "Дублировать", icon: "copy-16" },
    { id: "pin", label: tab.pinned ? "Открепить" : "Закрепить", icon: "pin-16", disabled: !web },
    {
      id: "mute",
      label: tab.muted ? "Включить звук" : "Выключить звук",
      icon: tab.muted ? "speaker-16" : "mute-16",
      disabled: !web,
    },
    { separator: true },
    {
      id: "split",
      label: splitting && state.splitId === id ? "Выйти из разделения экрана" : "Открыть в режиме разделения экрана",
      icon: "split-16",
      disabled: !web || (id === state.activeId && !splitting),
    },
    { id: "to-window", label: "Переместить в новое окно", icon: "window-16", disabled: !web },
    { separator: true },
    ...groupItems(tab),
    { separator: true },
    { id: "close", label: "Закрыть", icon: "dismiss-16", keys: "Ctrl+W" },
    { id: "close-left", label: "Закрыть вкладки слева", disabled: !left.length },
    { id: "close-right", label: "Закрыть вкладки справа", disabled: !right.length },
    { id: "close-others", label: "Закрыть другие вкладки", disabled: !left.length && !right.length },
    { separator: true },
    { id: "reopen", label: reopenLabel(), keys: "Ctrl+Shift+T", disabled: !hasClosedTabs() },
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
      case "pin":
        togglePin(id);
        break;
      case "mute":
        invoke("tab_mute", { id, muted: !tab.muted }).catch(() => {});
        break;
      case "split":
        if (state.splitId === id) endSplit();
        else splitWith(id);
        break;
      case "to-window":
        moveToNewWindow(id);
        break;
      case "group-new": {
        const group = groupTab(id);
        if (group) editGroup(group.id);
        break;
      }
      case "group-out":
        ungroupTab(id);
        break;
      default:
        if (action?.startsWith("group:")) {
          const target = groups().find((group) => group.id === Number(action.slice(6)));
          if (target) groupTab(id, { ...target, collapsed: false });
        }
        break;
      case "close":
        close(id);
        break;
      case "close-others":
        closeMany([...left, ...right], id);
        break;
      case "close-left":
        closeMany(left, id);
        break;
      case "close-right":
        closeMany(right, id);
        break;
      case "reopen":
        reopenClosed();
        break;
    }
  });
}

/** Пункты меню про группы: новая, существующие и «убрать из группы». */
function groupItems(tab) {
  if (tab.internal) return [];
  const items = [{ id: "group-new", label: "Добавить вкладку в новую группу", icon: "tab-group-16" }];
  for (const group of groups()) {
    if (group.id === tab.group?.id) continue;
    items.push({
      id: `group:${group.id}`,
      label: `Добавить в группу «${group.title || "Без имени"}»`,
      icon: "tab-group-16",
    });
  }
  if (tab.group) items.push({ id: "group-out", label: "Убрать из группы", icon: "dismiss-16" });
  return items;
}

/** Пузырь группы: имя, цвет и действия над всей группой. */
export function editGroup(groupId, anchor = null) {
  const members = groupTabs(groupId);
  if (!members.length) return;
  const group = members[0].group;
  const target = anchor ?? strip.querySelector(`[data-group-pill="${groupId}"]`) ?? strip;
  openPopup("group", target, {
    width: 300,
    payload: { group, tabs: members.length, colors: GROUP_COLORS },
  }).catch(() => {});
}

/** Вкладку — в отдельное окно: адрес переезжает, здесь она закрывается. */
export async function moveToNewWindow(id) {
  const tab = state.tabs.get(id);
  if (!tab || tab.internal || !tab.url) return;
  await invoke("window_open", { private: false, url: tab.url }).catch(() => {});
  await close(id);
}

/* ── Группы вкладок ────────────────────────────────────────── */

/** Все вкладки группы по порядку. */
export function groupTabs(id) {
  return [...state.tabs.values()].filter((tab) => tab.group?.id === id);
}

/** Группы окна: по одной записи на группу, в порядке появления в строке. */
export function groups() {
  const seen = new Map();
  for (const tab of state.tabs.values()) {
    if (tab.group && !seen.has(tab.group.id)) seen.set(tab.group.id, tab.group);
  }
  return [...seen.values()];
}

/** Новая группа из одной вкладки: имя пользователь задаст в пузыре. */
export function groupTab(id, group = null) {
  const tab = state.tabs.get(id);
  if (!tab || tab.internal) return null;
  const next =
    group ??
    {
      id: groupSeq++,
      title: "",
      color: GROUP_COLORS[(groupSeq - 2) % GROUP_COLORS.length][0],
      collapsed: false,
    };
  upsertTab(id, { group: next, pinned: false });
  // Вкладки группы стоят рядом: место новой — сразу за последней из группы.
  const members = groupTabs(next.id).filter((other) => other.id !== id);
  if (members.length) {
    moveTab(id, tabIndex(members[members.length - 1].id) + 1);
  }
  return next;
}

export function ungroupTab(id) {
  if (state.tabs.get(id)?.group) upsertTab(id, { group: null });
}

/** Поменять свойства группы у всех её вкладок разом. */
export function updateGroup(groupId, patch) {
  for (const tab of groupTabs(groupId)) {
    upsertTab(tab.id, { group: { ...tab.group, ...patch } });
  }
}

export function ungroupAll(groupId) {
  for (const tab of groupTabs(groupId)) upsertTab(tab.id, { group: null });
}

export async function closeGroup(groupId) {
  const members = groupTabs(groupId).map((tab) => tab.id);
  const outside = visibleIds().filter((id) => !members.includes(id));
  // Фокус — на ближайшую вкладку справа от группы, иначе слева.
  const last = tabIndex(members[members.length - 1]);
  const keep = outside.find((id) => tabIndex(id) > last) ?? outside[outside.length - 1] ?? null;
  await closeMany(members, keep);
}

/** Свернуть или развернуть группу. Свёрнутая группа прячет свои вкладки. */
export function toggleGroup(groupId) {
  const members = groupTabs(groupId);
  if (!members.length) return;
  const collapsed = !members[0].group.collapsed;
  // Активную вкладку прятать нельзя: перед сворачиванием уходим на соседнюю.
  if (collapsed && members.some((tab) => tab.id === state.activeId)) {
    const outside = [...state.tabs.values()].find((tab) => tab.group?.id !== groupId);
    if (!outside) return;
    activate(outside.id);
  }
  updateGroup(groupId, { collapsed });
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
    // Вкладку бросили: между вкладками группы она входит в группу, а унесённая
    // от своей — выходит из неё.
    if (dragged != null) settleGroup(dragged);
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
const pills = new Map();

export function renderTabs() {
  const tabs = [...state.tabs.values()];

  for (const [id, node] of nodes) {
    if (!state.tabs.has(id)) {
      node.remove();
      nodes.delete(id);
    }
  }
  const alive = new Set(tabs.map((tab) => tab.group?.id).filter((id) => id != null));
  for (const [id, pill] of pills) {
    if (!alive.has(id)) {
      pill.remove();
      pills.delete(id);
    }
  }

  // Порядок в строке: перед первой вкладкой группы стоит её ярлык. Ярлык у
  // группы один, даже если её вкладки на миг разошлись (вкладку тащат
  // через группу): один и тот же узел дважды сбил бы порядок строки.
  const order = [];
  const placed = new Set();
  let lastGroup = null;
  for (const tab of tabs) {
    const group = tab.group ?? null;
    if (group && group.id !== lastGroup && !placed.has(group.id)) {
      placed.add(group.id);
      order.push(pillFor(group));
    }
    lastGroup = group?.id ?? null;
    order.push(tabNode(tab));
  }
  order.forEach((node, index) => {
    if (strip.children[index] !== node) strip.insertBefore(node, strip.children[index] ?? null);
  });

  // Ширина полосы — от числа видимых вкладок, а не от их содержимого: иначе
  // узкие вкладки без подписей сжимали полосу под себя и не расширялись, когда
  // место появлялось (окно развернули, вкладки закрыли).
  const visible = tabs.filter((tab) => !tab.group?.collapsed && !tab.closing).length;
  strip.parentElement.style.setProperty("--tab-count", String(visible + pills.size));
  requestAnimationFrame(() => {
    updateNarrow();
    revealActive();
  });
}

/**
 * Вкладок больше, чем помещается: строка прокручивается, а активная вкладка
 * всегда на виду. Раньше лишние вкладки просто обрезались краем окна, и до
 * них нельзя было дотянуться мышью.
 */
let revealed = null;
function revealActive() {
  const id = state.activeId;
  if (id === revealed) return;
  revealed = id;
  const node = nodes.get(id);
  if (!node || node.hidden || strip.scrollWidth <= strip.clientWidth) return;
  const box = strip.getBoundingClientRect();
  const rect = node.getBoundingClientRect();
  // Запас на ушки активной вкладки, выходящие за её края.
  const pad = 8;
  if (rect.left < box.left + pad) strip.scrollLeft -= box.left + pad - rect.left;
  else if (rect.right > box.right - pad) strip.scrollLeft += rect.right - (box.right - pad);
}

function tabNode(tab) {
  let node = nodes.get(tab.id);
  if (!node) {
    node = el("div", "tab");
    node.dataset.id = tab.id;
    node.draggable = true;
    node.setAttribute("role", "tab");
    nodes.set(tab.id, node);
  }
  updateTab(node, tab);
  // Свёрнутая группа прячет свои вкладки: на экране остаётся только ярлык.
  // Закрываемая вкладка ждёт ответа страницы уже не в строке.
  node.hidden = Boolean(tab.group?.collapsed || tab.closing);
  return node;
}

/** Ярлык группы: щелчок сворачивает, правый щелчок открывает пузырь. */
function pillFor(group) {
  let pill = pills.get(group.id);
  if (!pill) {
    pill = el("button", "tabgroup");
    pill.type = "button";
    pill.dataset.groupPill = group.id;
    pill.addEventListener("click", () => toggleGroup(group.id));
    pill.addEventListener("contextmenu", (event) => {
      event.preventDefault();
      editGroup(group.id, pill);
    });
    pills.set(group.id, pill);
  }
  pill.dataset.color = group.color;
  pill.dataset.collapsed = String(Boolean(group.collapsed));
  const label = group.title || `${groupTabs(group.id).length}`;
  if (pill.textContent !== label) pill.textContent = label;
  pill.title = group.title
    ? `Группа «${group.title}» — щелчок сворачивает, правый щелчок открывает настройки`
    : "Группа вкладок — правый щелчок открывает настройки";
  return pill;
}

/** Узкий режим — по фактической ширине плитки: при открытой панели места меньше при том же счёте. */
function updateNarrow() {
  for (const node of strip.children) {
    if (!node.classList.contains("tab") || node.hidden) continue;
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
  const split = state.splitId === tab.id;
  setAttr(node, "data-active", String(active || split));
  setAttr(node, "aria-selected", String(active));
  setAttr(node, "data-muted", String(tab.muted));
  setAttr(node, "data-pinned", String(tab.pinned));
  setAttr(node, "data-group", tab.group ? tab.group.color : "");
  setAttr(node, "data-sleeping", String(tab.sleeping));
  setAttr(node, "data-split", String(split));

  const title = tab.title || hostOf(tab.url) || "Новая вкладка";
  const hint = tab.crashed ? `${title}\nСтраница перестала работать — щёлкните, чтобы загрузить заново` : title;
  if (node.title !== hint) node.title = hint;

  const iconKind = tab.internal
    ? `internal:${tab.internal}`
    : tab.crashed
      ? "crashed"
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
    } else if (tab.crashed) {
      // Страница упала: щелчок по вкладке загрузит её заново.
      iconNode = icon("warning-16", 16, "tab__glyph tab__glyph--crashed");
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
