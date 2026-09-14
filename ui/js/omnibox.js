/**
 * Адресная строка.
 *
 * Два состояния: показ (хост читается, остальное гаснет) и ввод. Переход
 * между ними — подмена содержимого, а не «фокус на input»: иначе URL
 * приходится показывать и редактировать одним элементом, и он либо
 * нечитаемый, либо неудобный.
 */

import { emit, invoke, isNative, listen } from "./bridge.js";
import { anchorOf, el, favicon, hostOf, icon } from "./dom.js";
import { onPopupAction, openMenu, openPopup } from "./popups.js";
import { onPref, pref } from "./prefs.js";
import { activeTab, state } from "./state.js";
import { bookmarkCurrent, hooks, isNewTabUrl, navigate, openSettings, tabAction } from "./actions.js";

const omni = document.getElementById("omni");
const field = document.getElementById("omni-field");
const display = document.getElementById("omni-url");
const site = document.getElementById("omni-site");
const siteLabel = document.getElementById("omni-site-label");
const shield = document.getElementById("omni-shield");
const shieldCount = document.getElementById("omni-shield-count");
const star = document.getElementById("omni-star");
const key = document.getElementById("omni-key");
const zoomChip = document.getElementById("omni-zoom");
const zoomValue = document.getElementById("omni-zoom-value");
const translateButton = document.getElementById("omni-translate");
const suggest = document.getElementById("suggest");

let selected = 0;
let rows = [];
let suggestToken = 0;
/// Подсказки в приложении живут во всплывающем окне поверх страницы: HTML-слой
/// ушёл бы под нативную поверхность. В mock-режиме — прежний выпадающий список.
let suggestShown = false;

export function initOmnibox() {
  display.addEventListener("click", enterEdit);
  omni.addEventListener("click", (event) => {
    if (event.target === omni) enterEdit();
  });

  field.addEventListener("blur", () => {
    // Клик по подсказке успевает отработать до закрытия. Проверяем, что
    // фокус действительно ушёл: возврат клавиатуры интерфейсу (chrome_focus)
    // даёт мимолётный blur, после которого поле снова в фокусе.
    setTimeout(() => {
      if (document.activeElement !== field) exitEdit();
    }, 120);
  });
  field.addEventListener("input", () => renderSuggest(field.value));
  field.addEventListener("keydown", (event) => {
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      move(event.key === "ArrowDown" ? 1 : -1);
      return;
    }
    if (event.key === "Enter") {
      const row = rows[selected];
      const value = row ? row.value : field.value;
      if (value.trim()) navigate(value, { newTab: event.altKey });
      field.blur();
      return;
    }
    if (event.key === "Escape") field.blur();
  });

  suggest.addEventListener("mousedown", (event) => {
    const row = event.target.closest(".suggest__row");
    if (!row) return;
    event.preventDefault();
    navigate(row.dataset.value);
    field.blur();
  });

  onPopupAction("suggest", ({ action, value }) => {
    if (action !== "pick" || typeof value !== "string") return;
    // Щелчок активировал окно подсказок: адресная строка сама не узнает, что
    // ввод закончен, — закрываем её здесь.
    exitEdit();
    field.blur();
    navigate(value);
  });
  listen("popup-closed", () => {
    suggestShown = false;
  });

  // Новая вкладка ставит курсор в адресную строку, как в Chrome.
  // Список подсказок при этом не открываем: он появляется, когда начинают печатать.
  document.addEventListener("browser:focus-omnibox", () => enterEdit({ suggest: false }));

  star.addEventListener("click", bookmarkCurrent);
  shield.addEventListener("click", () => hooks.togglePanel("shield"));
  translateButton.addEventListener("click", () => hooks.togglePanel("translate"));
  key.addEventListener("click", openAccounts);
  site.addEventListener("click", openSiteInfo);
  zoomChip.addEventListener("click", () => {
    const tab = activeTab();
    openMenu(
      "zoom",
      zoomChip,
      [
        { type: "zoom", id: "zoom", label: "Масштаб", value: tab?.zoom ?? 1 },
        { separator: true },
        { id: "zoom_reset", label: "Сбросить до 100%", icon: "reload" },
      ],
      (action) => tabAction(action),
      { width: 260, align: "end" }
    );
  });

  // Закладку могли добавить или убрать где угодно — звезда должна знать.
  listen("bookmarks", () => {
    starUrl = null;
    renderOmnibox();
  });
  onPref(() => renderOmnibox());
}

export function focusOmnibox() {
  enterEdit();
}

function enterEdit({ suggest: withSuggestions = true } = {}) {
  const tab = activeTab();
  field.value = tab?.internal ? tab.url : isNewTabUrl(tab?.url) ? "" : tab?.url ?? "";
  field.hidden = false;
  display.hidden = true;
  omni.dataset.focused = "true";
  field.focus();
  field.select();
  // Клавиша могла прийти со страницы: без этого текст уйдёт в неё.
  invoke("chrome_focus")
    .then(() => {
      if (!field.hidden) field.focus();
    })
    .catch(() => {});
  if (withSuggestions) renderSuggest(field.value);
}

function exitEdit() {
  field.hidden = true;
  display.hidden = false;
  omni.dataset.focused = "false";
  omni.dataset.suggest = "false";
  suggest.hidden = true;
  rows = [];
  suggestToken += 1;
  if (suggestShown) {
    suggestShown = false;
    invoke("popup_hide").catch(() => {});
  }
}

export function renderOmnibox() {
  const tab = activeTab();
  const url = tab?.url ?? "";

  display.replaceChildren();
  siteLabel.hidden = true;

  if (tab?.internal) {
    setSite("internal", "settings");
    site.replaceChildren(brandMark(), siteLabel);
    siteLabel.hidden = false;
    siteLabel.textContent = "190x4";
    display.append(el("b", null, url));
  } else if (isNewTabUrl(url)) {
    setSite("search", "search-16");
    display.append(el("span", "omni__hint", "Введите запрос или адрес"));
  } else {
    try {
      const parsed = new URL(url);
      const secure = parsed.protocol === "https:";
      setSite(secure ? "secure" : "insecure", secure ? "lock-16" : "info-16");
      if (!secure) {
        siteLabel.hidden = false;
        siteLabel.textContent = "Не защищено";
      }
      display.append(el("b", null, parsed.host), document.createTextNode(parsed.pathname + parsed.search + parsed.hash));
    } catch {
      setSite("insecure", "info-16");
      display.textContent = url;
    }
  }

  const blocked = tab?.blocked ?? 0;
  shield.hidden = blocked === 0 || Boolean(tab?.internal);
  shieldCount.textContent = blocked > 999 ? "999+" : blocked;

  const zoom = tab?.zoom ?? 1;
  zoomChip.hidden = Math.abs(zoom - 1) < 0.001 || Boolean(tab?.internal);
  zoomValue.textContent = `${Math.round(zoom * 100)}%`;

  key.hidden = !(tab && (state.passwordSites.has(tab.id) || state.passwordOffer?.tab === tab.id));
  translateButton.hidden = !pref("translate_button") || Boolean(tab?.internal);
  star.hidden = Boolean(tab?.internal) || isNewTabUrl(url);

  syncStar(tab?.internal || isNewTabUrl(url) ? "" : url);
}

function setSite(kind, iconId) {
  site.dataset.kind = kind;
  if (kind !== "internal") site.replaceChildren(icon(iconId, 16), siteLabel);
  site.title =
    kind === "secure"
      ? "Подключение защищено"
      : kind === "insecure"
        ? "Подключение не защищено"
        : kind === "internal"
          ? "Страница браузера 190x4"
          : "Поиск";
}

function brandMark() {
  const img = el("img");
  img.src = "./assets/brand/mark-72.png";
  img.width = 16;
  img.height = 16;
  img.alt = "";
  return img;
}

/** Состояние звезды — из базы: закладка могла появиться в другом месте. */
let starUrl = null;
async function syncStar(url) {
  if (url === starUrl) return;
  starUrl = url;
  if (!url) {
    star.setAttribute("aria-pressed", "false");
    return;
  }
  const node = await invoke("bookmark_find", { url }).catch(() => null);
  if (starUrl !== url) return;
  star.setAttribute("aria-pressed", String(Boolean(node)));
  star.title = node ? "Изменить закладку (Ctrl+D)" : "Добавить в закладки (Ctrl+D)";
}

/** Ключ в адресной строке: учётки этого сайта. */
function openAccounts() {
  const tab = activeTab();
  const offer = state.passwordOffer;
  if (offer && offer.tab === tab?.id) {
    openPopup("password", key, { width: 340, align: "end", payload: offer });
    return;
  }
  const saved = tab ? state.passwordSites.get(tab.id) : null;
  if (!saved) return;
  openPopup("accounts", key, { width: 320, align: "end", payload: { tab: tab.id, ...saved } });
}

/** Пузырь о сайте: соединение, блокировки, быстрые ссылки в настройки. */
function openSiteInfo() {
  const tab = activeTab();
  if (!tab || tab.internal || isNewTabUrl(tab.url)) {
    if (tab?.internal) openSettings();
    return;
  }
  openPopup("site", site, {
    width: 340,
    payload: {
      url: tab.url,
      host: hostOf(tab.url),
      secure: tab.url.startsWith("https:"),
      blocked: tab.blocked ?? 0,
      adblock: state.adblockOn,
      passwords: state.passwordSites.get(tab.id)?.accounts?.length ?? 0,
    },
  });
}

/**
 * Подсказки: переход по адресу, закладки, история, поиск. Порядок
 * фиксированный — пользователь не должен угадывать, что окажется первым.
 */
async function renderSuggest(query) {
  const value = query.trim();
  const token = ++suggestToken;

  const history = value
    ? await invoke("history_search", { query: value, limit: 5 }).catch(() => [])
    : await invoke("history_recent", { limit: 6 }).catch(() => []);
  if (token !== suggestToken) return;

  rows = [];
  if (value) {
    const looksLikeUrl = value.includes("://") || (/\./.test(value) && !/\s/.test(value));
    if (looksLikeUrl) rows.push({ text: value, hint: "перейти", value, iconId: "globe-16" });

    const needle = value.toLowerCase();
    const seen = new Set();
    for (const node of state.bookmarks ?? []) {
      if (node.kind !== "url" || seen.size >= 3) continue;
      if (!node.title.toLowerCase().includes(needle) && !node.url.toLowerCase().includes(needle)) continue;
      seen.add(node.url);
      rows.push({ text: node.title || node.url, hint: "закладка", value: node.url, iconId: "star-16", image: node.icon });
    }
    for (const entry of history) {
      if (seen.has(entry.url)) continue;
      rows.push({ text: entry.title || entry.url, hint: hostOf(entry.url), value: entry.url, iconId: "history-16" });
    }
    rows.push({ text: value, hint: "поиск", value, iconId: "search-16" });
  } else {
    for (const entry of history) {
      rows.push({ text: entry.title || entry.url, hint: hostOf(entry.url), value: entry.url, iconId: "history-16" });
    }
  }

  selected = 0;
  if (field.hidden) return;

  if (isNative) {
    if (rows.length) {
      suggestShown = true;
      invoke("popup_open", {
        kind: "suggest",
        anchor: anchorOf(omni),
        width: omni.getBoundingClientRect().width,
        align: "start",
        payload: { rows, selected },
      }).catch(() => {});
    } else if (suggestShown) {
      suggestShown = false;
      invoke("popup_hide").catch(() => {});
    }
    return;
  }

  suggest.hidden = rows.length === 0;
  omni.dataset.suggest = String(rows.length > 0);
  suggest.replaceChildren(
    ...rows.map((row, index) => {
      const node = el("button", "suggest__row");
      node.type = "button";
      node.dataset.selected = String(index === selected);
      node.dataset.value = row.value;
      const iconNode = row.image ? favicon(row.image, "favicon suggest__icon") : icon(row.iconId, 16, "suggest__icon");
      node.append(iconNode, el("span", "suggest__text", row.text), el("span", "suggest__hint", row.hint));
      return node;
    })
  );
}

function move(delta) {
  if (!rows.length) return;
  selected = (selected + delta + rows.length) % rows.length;
  if (isNative) {
    emit("suggest-select", { selected });
    return;
  }
  [...suggest.children].forEach((node, index) => {
    node.dataset.selected = String(index === selected);
  });
}
