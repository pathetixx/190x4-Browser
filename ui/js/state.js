/**
 * Модель вкладок. Один источник правды для строки вкладок, адресной строки,
 * панели закладок и статуса — иначе они разъезжаются на первой же гонке
 * событий.
 *
 * Порядок вкладок — порядок ключей Map: так строка вкладок рисуется без
 * отдельного массива, который пришлось бы держать в согласии. Закреплённые
 * вкладки живут в начале этого же порядка.
 */

const listeners = new Set();

export const state = {
  tabs: new Map(),
  activeId: null,
  /// Вторая вкладка разделённого экрана. Обычно она справа от активной;
  /// `swapped` — активной стала правая половина (по ней щёлкнули), и вкладки
  /// остались на своих местах. Так же считает Rust (`TabHost`).
  splitId: null,
  swapped: false,
  /// Вкладка во весь экран: видео, развёрнутое страницей, или F11.
  fullscreen: { page: null, window: false },
  /// Окна, которые сайт открыл сам по себе и которые браузер не пустил:
  /// вкладка → [{ url, site }].
  blockedPopups: new Map(),
  blockedTotal: 0,
  latencyMicros: 0,
  adblockOn: true,
  maximized: false,
  /// Это окно: обычное или приватное, и под каким номером его сессия.
  window: { label: "chrome", private: false, session: 0, windows: 1 },
  /// Доступность сервисов 190x4: без ключей интерфейс не должен обещать того,
  /// чего не будет.
  services: { translate: false, media: false },
  translate: { text: "", result: "", detected: null, busy: false, error: null },
  /// Сайты с сохранёнными паролями: вкладка → { origin, accounts }.
  passwordSites: new Map(),
  /// Масштаб, который помнит сайт: хост → множитель.
  zoomSites: {},
};

export function subscribe(fn) {
  listeners.add(fn);
  return () => listeners.delete(fn);
}

export function emit() {
  for (const fn of listeners) fn(state);
}

function blankTab(id) {
  return {
    id,
    url: "",
    title: "Новая вкладка",
    favicon: null,
    loading: false,
    audible: false,
    muted: false,
    canBack: false,
    canForward: false,
    blocked: 0,
    media: null,
    zoom: 1,
    /// Закреплённая вкладка: только значок, без крестика, всегда слева.
    pinned: false,
    /// Вкладка из прошлого сеанса, которую ещё не открывали: место в строке
    /// есть, а страницы и памяти под неё — нет, как в Chrome.
    sleeping: false,
    /// Встроенная страница: "settings" | "downloads" | "history" | null.
    internal: null,
    section: "",
  };
}

/**
 * Закрытые вкладки. События движка, отправленные до закрытия, приходят и
 * после него — и `upsertTab` вернул бы вкладку-призрак в строку. Номера
 * вкладок не повторяются, так что список только растёт.
 */
const closedIds = new Set();

export function markClosed(id) {
  closedIds.add(id);
}

export function isClosed(id) {
  return closedIds.has(id);
}

export function upsertTab(id, patch) {
  const current = state.tabs.get(id) ?? blankTab(id);
  const next = { ...current, ...patch };
  // Без сравнения каждая секунда опроса статистики порождала бы полный
  // цикл перерисовки на неизменившихся данных.
  const changed = !state.tabs.has(id) || Object.keys(patch).some((key) => next[key] !== current[key]);
  state.tabs.set(id, next);
  if (changed) emit();
}

/** Вставить (или переставить) вкладку на позицию `index`. */
export function insertTabAt(id, patch, index) {
  const tab = { ...(state.tabs.get(id) ?? blankTab(id)), ...patch };
  const entries = [...state.tabs.entries()].filter(([key]) => key !== id);
  entries.splice(Math.max(0, Math.min(index, entries.length)), 0, [id, tab]);
  state.tabs = new Map(entries);
  emit();
}

export function moveTab(id, index) {
  if (!state.tabs.has(id)) return;
  // Закреплённые и обычные вкладки не перемешиваются: перетаскивание внутри
  // своей группы, как в Chrome.
  const tab = state.tabs.get(id);
  const bounds = groupBounds(tab.pinned);
  insertTabAt(id, {}, Math.max(bounds.from, Math.min(index, bounds.to)));
}

/** Границы группы (закреплённые или обычные) в текущем порядке. */
export function groupBounds(pinned) {
  const tabs = [...state.tabs.values()];
  const pinnedCount = tabs.filter((tab) => tab.pinned).length;
  return pinned ? { from: 0, to: Math.max(0, pinnedCount - 1) } : { from: pinnedCount, to: tabs.length };
}

/** Спящая вкладка проснулась: её номер сменился на номер настоящей вкладки. */
export function replaceTabId(oldId, newId, patch = {}) {
  const index = tabIndex(oldId);
  if (index < 0) return;
  const tab = { ...state.tabs.get(oldId), ...patch, id: newId };
  const entries = [...state.tabs.entries()].filter(([key]) => key !== oldId);
  entries.splice(index, 0, [newId, tab]);
  state.tabs = new Map(entries);
  if (state.activeId === oldId) state.activeId = newId;
  if (state.splitId === oldId) state.splitId = newId;
  emit();
}

export function tabIndex(id) {
  return [...state.tabs.keys()].indexOf(id);
}

export function removeTab(id) {
  state.tabs.delete(id);
  state.passwordSites.delete(id);
  state.blockedPopups.delete(id);
  if (state.activeId === id) state.activeId = null;
  if (state.splitId === id) {
    state.splitId = null;
    state.swapped = false;
  }
  emit();
}

export function activeTab() {
  return state.activeId === null ? null : state.tabs.get(state.activeId) ?? null;
}

export function splitTab() {
  return state.splitId === null ? null : state.tabs.get(state.splitId) ?? null;
}

/**
 * Сделать вкладку активной. Вторая половина разделённого экрана становится
 * активной на своём месте, вкладка не из пары — на месте активной половины.
 */
export function setActive(id) {
  if (state.splitId !== null && state.splitId === id) {
    state.splitId = state.activeId;
    state.swapped = state.splitId !== null && !state.swapped;
  }
  state.activeId = id;
  emit();
}

export function setSplit(id) {
  state.splitId = id;
  state.swapped = false;
  emit();
}

/** Какая вкладка в правой половине разделённого экрана. */
export function rightPaneId() {
  if (state.splitId === null) return null;
  return state.swapped ? state.activeId : state.splitId;
}
