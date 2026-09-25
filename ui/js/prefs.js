/**
 * Настройки браузера.
 *
 * Значения по умолчанию живут здесь, в одном месте: страница настроек рисует
 * их, остальной интерфейс читает. В базе лежит только то, что пользователь
 * поменял. Rust знает умолчания лишь тех ключей, что влияют на движок
 * (поиск, загрузки, пароли, фильтр), и они совпадают с этими.
 */

import { invoke, listen } from "./bridge.js";

export const DEFAULTS = {
  theme: "kurogane",
  bookmarks_bar: "always",
  show_home: false,
  home_page: "newtab",
  home_url: "",
  downloads_button: "auto",
  statusbar: false,
  sidebar: false,
  startup: "restore",
  startup_pages: [],
  search_engine: "duckduckgo",
  search_suggest: true,
  passwords_offer: true,
  passwords_autofill: true,
  adblock_enabled: true,
  adblock_exempt_sites: [],
  popups_allowed_sites: [],
  download_dir: "",
  download_ask: false,
  download_bubble: true,
  translate_lang: "Русский",
  translate_source: "auto",
  ext_translate_enabled: true,
  ext_translate_pinned: true,
  // SponsorBlock: режим каждой категории — skip, ask, show или off. Те же
  // умолчания — в src-tauri/src/sponsorblock.rs (`CATEGORIES`).
  ext_sponsorblock_enabled: true,
  sponsorblock_sponsor: "skip",
  sponsorblock_selfpromo: "show",
  sponsorblock_interaction: "show",
  sponsorblock_intro: "show",
  sponsorblock_outro: "show",
  sponsorblock_preview: "show",
  sponsorblock_filler: "off",
  sponsorblock_music_offtopic: "off",
  // Автопролистывание: общее включение (его же переключает кнопка на панели)
  // и сайты. Те же ключи читает src-tauri/src/autoscroll.rs.
  ext_autoscroll_enabled: true,
  ext_autoscroll_pinned: true,
  autoscroll_youtube: true,
  autoscroll_instagram: true,
  autoscroll_tiktok: true,
  ext_media_enabled: true,
  ext_media_pinned: true,
  updates_auto: true,
  zoom_sites: {},
  smartscreen: true,
  media_autoplay: false,
  // Защищённое видео: Widevine вообще, переход на PlayReady, если Widevine не
  // справился, и сайты, где он уже выключен. Те же ключи читает src-tauri/src/drm.rs.
  drm_widevine: true,
  drm_fallback: true,
  drm_widevine_off_sites: [],
  tabs_sleep: "60",
};

const values = { ...DEFAULTS };
const listeners = new Set();
let listening = false;

export async function loadPrefs() {
  const stored = await invoke("settings_get").catch(() => ({}));
  Object.assign(values, stored ?? {});

  if (!listening) {
    listening = true;
    // Настройку могли поменять в другом окне (попап, страница настроек).
    listen("settings", ({ key, value }) => {
      if (JSON.stringify(values[key]) === JSON.stringify(value)) return;
      values[key] = value;
      notify(key);
    });
  }
  return values;
}

export function pref(key) {
  return key in values ? values[key] : DEFAULTS[key];
}

export async function setPref(key, value) {
  values[key] = value;
  notify(key);
  await invoke("settings_set", { key, value });
}

export function onPref(fn) {
  listeners.add(fn);
  return () => listeners.delete(fn);
}

function notify(key) {
  for (const fn of listeners) fn(key, values[key]);
}

/** Тема: «как в системе» раскрывается в конкретный лак по prefers-color-scheme. */
export function applyTheme() {
  const theme = pref("theme");
  const dark = matchMedia("(prefers-color-scheme: dark)").matches;
  document.documentElement.dataset.theme = theme === "system" ? (dark ? "kurogane" : "shiro") : theme;
}
