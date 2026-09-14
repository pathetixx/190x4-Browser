/**
 * Модель вкладок. Один источник правды для строки вкладок, адресной строки,
 * панели закладок и статуса — иначе они разъезжаются на первой же гонке
 * событий.
 *
 * Порядок вкладок — порядок ключей Map: так строка вкладок рисуется без
 * отдельного массива, который пришлось бы держать в согласии.
 */

const listeners = new Set();

export const state = {
  tabs: new Map(),
  activeId: null,
  blockedTotal: 0,
  latencyMicros: 0,
  adblockOn: true,
  maximized: false,
  /// Доступность сервисов 190x4: без ключей интерфейс не должен обещать того,
  /// чего не будет.
  services: { translate: false, media: false },
  translate: { text: "", result: "", detected: null, busy: false, error: null },
  /// Сайты с сохранёнными паролями: вкладка → { origin, accounts }.
  passwordSites: new Map(),
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
    /// Встроенная страница: "settings" | "downloads" | null.
    internal: null,
    section: "",
  };
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
  insertTabAt(id, {}, index);
}

export function tabIndex(id) {
  return [...state.tabs.keys()].indexOf(id);
}

export function removeTab(id) {
  state.tabs.delete(id);
  state.passwordSites.delete(id);
  if (state.activeId === id) state.activeId = null;
  emit();
}

export function activeTab() {
  return state.activeId === null ? null : state.tabs.get(state.activeId) ?? null;
}

export function setActive(id) {
  state.activeId = id;
  emit();
}
