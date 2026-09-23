/**
 * Адресная строка.
 *
 * Два состояния: показ (хост читается, остальное гаснет) и ввод. Переход
 * между ними — подмена содержимого, а не «фокус на input»: иначе URL
 * приходится показывать и редактировать одним элементом, и он либо
 * нечитаемый, либо неудобный.
 */

import { emit, invoke, isNative, listen } from "./bridge.js";
import { anchorOf, displayHost, displayPath, displayUrl, el, favicon, hostOf, icon } from "./dom.js";
import { onPopupAction, onPopupClosed, openMenu, openPopup } from "./popups.js";
import { onPref, pref, setPref } from "./prefs.js";
import { activeTab, emit as emitState, state } from "./state.js";
import { open } from "./tabs.js";
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
const popupsChip = document.getElementById("omni-popups");
const popupsCount = document.getElementById("omni-popups-count");
const suggest = document.getElementById("suggest");

let selected = 0;
let rows = [];
/// Строку выбрали стрелкой. Пока нет — Enter открывает набранный текст как
/// есть: подсказки приходят с опозданием и могли остаться от прошлой буквы.
let picked = false;
let suggestToken = 0;
/// Подсказки в приложении живут во всплывающем окне поверх страницы: HTML-слой
/// ушёл бы под нативную поверхность. В mock-режиме — прежний выпадающий список.
let suggestShown = false;
/// Номер показа подсказок (его выдаёт Rust): закрытие чужого попапа не
/// значит, что закрылись подсказки.
let suggestSeq = 0;
/// Адрес, дописанный прямо в строке: набранное `typed`, всё поле `text`
/// (дописанный хвост выделен) и `url`, куда он ведёт.
let inline = null;
/// Последний ввод — печать вперёд. Дописывать можно только тогда: стирание
/// снимает дописанное, а вставку Chrome тоже не дописывает.
let completable = false;

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
  field.addEventListener("input", (event) => {
    completable = event.inputType === "insertText" && !event.isComposing;
    inline = null;
    renderSuggest(field.value);
  });
  field.addEventListener("keydown", (event) => {
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      move(event.key === "ArrowDown" ? 1 : -1);
      return;
    }
    if (event.key === "Enter") {
      const row = picked ? rows[selected] : null;
      // Дописанный адрес приняли (Enter или сначала End) — туда, куда он ведёт.
      const accepted = !row && inline && field.value === inline.text ? inline.url : null;
      const value = row ? row.value : (accepted ?? field.value);
      if (value.trim()) navigate(value, { newTab: event.altKey });
      field.blur();
      return;
    }
    if (event.key === "Escape") field.blur();
    // Shift+Delete убирает выбранную строку истории из подсказок и из истории —
    // как в Chrome: опечатка или случайный сайт больше не всплывает.
    if (event.key === "Delete" && event.shiftKey && picked && rows[selected]?.iconId === "history-16") {
      event.preventDefault();
      const url = rows[selected].value;
      invoke("history_forget", { url })
        .then(() => renderSuggest(field.value))
        .catch(() => {});
    }
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
  onPopupClosed((seq) => {
    if (!seq || seq !== suggestSeq) return;
    suggestShown = false;
    suggestSeq = 0;
  });

  // Новая вкладка ставит курсор в адресную строку, как в Chrome.
  // Список подсказок при этом не открываем: он появляется, когда начинают печатать.
  document.addEventListener("browser:focus-omnibox", () => enterEdit({ suggest: false }));

  star.addEventListener("click", bookmarkCurrent);
  shield.addEventListener("click", () => hooks.togglePanel("shield"));
  translateButton.addEventListener("click", () => hooks.togglePanel("translate"));
  key.addEventListener("click", openAccounts);
  site.addEventListener("click", openSiteInfo);
  popupsChip.addEventListener("click", openBlockedPopups);
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
  completable = false;
  inline = null;
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
  completable = false;
  inline = null;
  field.hidden = true;
  display.hidden = false;
  omni.dataset.focused = "false";
  omni.dataset.suggest = "false";
  suggest.hidden = true;
  rows = [];
  suggestToken += 1;
  hideSuggestions();
}

function hideSuggestions() {
  if (!suggestShown) return;
  suggestShown = false;
  invoke("popup_hide", { seq: suggestSeq || null }).catch(() => {});
  suggestSeq = 0;
}

/** Что уже нарисовано: адресная строка перестраивается, только когда это меняется. */
let rendered = "";

export function renderOmnibox() {
  const tab = activeTab();
  const url = tab?.url ?? "";

  // Интерфейс перерисовывается на каждое событие любой вкладки (счётчик
  // блокировок, звук, заголовок фоновой вкладки) — пересобирать строку ради
  // них незачем.
  const signature = JSON.stringify([
    tab?.id,
    tab?.internal,
    url,
    tab?.blocked ?? 0,
    tab?.zoom ?? 1,
    tab ? state.blockedPopups.get(tab.id)?.length ?? 0 : 0,
    tab ? state.passwordSites.has(tab.id) : false,
    state.passwordOffer?.tab ?? null,
    state.insecureHosts.size,
    pref("translate_button"),
  ]);
  syncStar(tab?.internal || isNewTabUrl(url) ? "" : url);
  if (signature === rendered) return;
  rendered = signature;

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
      // https с неверным сертификатом, который открыли «всё равно», — тоже
      // не защищено: замок там был бы ложью.
      const bypassed = state.insecureHosts.has(parsed.host.toLowerCase());
      const secure = parsed.protocol === "https:" && !bypassed;
      setSite(secure ? "secure" : "insecure", secure ? "lock-16" : "info-16");
      if (!secure) {
        siteLabel.hidden = false;
        siteLabel.textContent = "Не защищено";
      }
      // Кириллица в адресе — буквами, а не `%D0%9C…` и `xn--…`.
      const host = displayHost(parsed.hostname) + (parsed.port ? `:${parsed.port}` : "");
      display.append(el("b", null, host), document.createTextNode(displayPath(parsed.pathname + parsed.search + parsed.hash)));
    } catch {
      setSite("insecure", "info-16");
      display.textContent = displayUrl(url);
    }
  }
  display.title = tab && !tab.internal && !isNewTabUrl(url) ? displayUrl(url) : "";

  const blocked = tab?.blocked ?? 0;
  shield.hidden = blocked === 0 || Boolean(tab?.internal);
  shieldCount.textContent = blocked > 999 ? "999+" : blocked;

  const popups = tab ? state.blockedPopups.get(tab.id)?.length ?? 0 : 0;
  popupsChip.hidden = popups === 0;
  popupsCount.textContent = popups > 9 ? "9+" : String(popups);

  const zoom = tab?.zoom ?? 1;
  zoomChip.hidden = Math.abs(zoom - 1) < 0.001 || Boolean(tab?.internal);
  zoomValue.textContent = `${Math.round(zoom * 100)}%`;

  key.hidden = !(tab && (state.passwordSites.has(tab.id) || state.passwordOffer?.tab === tab.id));
  translateButton.hidden = !pref("translate_button") || Boolean(tab?.internal);
  star.hidden = Boolean(tab?.internal) || isNewTabUrl(url);
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
  img.src = "./assets/brand/mark-32.png";
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

/** Сайт как ключ настроек: хост без `www.`. */
export function siteKey(url) {
  return hostOf(url).replace(/^www\./, "").toLowerCase();
}

/**
 * Окна, которые сайт хотел открыть сам по себе. Chrome их тоже не пускает и
 * держит значок в адресной строке: открыть окно всё-таки или разрешить сайту
 * открывать окна всегда.
 */
function openBlockedPopups() {
  const tab = activeTab();
  const blocked = tab ? state.blockedPopups.get(tab.id) ?? [] : [];
  if (!tab || !blocked.length) return;
  const site = siteKey(tab.url);
  const recent = blocked.slice(-5).reverse();
  const items = [
    { type: "header", label: blocked.length > 1 ? "Сайт хотел открыть новые окна" : "Сайт хотел открыть новое окно" },
    ...recent.map((entry, index) => ({
      id: `open:${index}`,
      label: entry.url === "about:blank" ? "Пустое окно" : displayUrl(entry.url),
      icon: "open",
    })),
    { separator: true },
    { id: "allow", label: `Всегда разрешать на ${displayHost(site)}`, icon: "checkmark-16", disabled: !site },
    { id: "clear", label: "Не открывать", icon: "dismiss-16" },
  ];
  openMenu(
    "popups",
    popupsChip,
    items,
    (action) => {
      const current = state.blockedPopups.get(tab.id) ?? [];
      if (action?.startsWith("open:")) {
        const entry = recent[Number(action.slice(5))];
        if (entry) {
          open(entry.url, { opener: tab.id }).catch(() => {});
          state.blockedPopups.set(
            tab.id,
            current.filter((other) => other !== entry)
          );
        }
      } else if (action === "allow" && site) {
        const allowed = pref("popups_allowed_sites") ?? [];
        if (!allowed.includes(site)) setPref("popups_allowed_sites", [...allowed, site].sort()).catch(() => {});
        state.blockedPopups.delete(tab.id);
        hooks.toast(`Сайт ${displayHost(site)} может открывать новые окна`);
      } else if (action === "clear") {
        state.blockedPopups.delete(tab.id);
      }
      emitState();
    },
    { width: 340, align: "end" }
  );
}

/** Пузырь о сайте: соединение, блокировки, быстрые ссылки в настройки. */
async function openSiteInfo() {
  const tab = activeTab();
  if (!tab || tab.internal || isNewTabUrl(tab.url)) {
    if (tab?.internal) openSettings();
    return;
  }
  const blocking = await invoke("adblock_site", { url: tab.url }).catch(() => null);
  openPopup("site", site, {
    width: 340,
    payload: {
      url: tab.url,
      host: hostOf(tab.url),
      secure: tab.url.startsWith("https:") && !state.insecureHosts.has(hostOf(tab.url).toLowerCase()),
      blocked: tab.blocked ?? 0,
      adblock: state.adblockOn,
      site: blocking?.site ?? null,
      blocking: blocking?.blocking ?? true,
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

  inline = completable ? complete(value, history) : null;
  if (inline) {
    field.value = inline.text;
    field.setSelectionRange(inline.typed.length, inline.text.length);
  }
  rows = buildRows(value, history, []);
  // Пустое поле: Enter ничего не открывает, пока строку не выбрали стрелкой.
  selected = value ? 0 : -1;
  picked = false;
  if (field.hidden) return;
  paint();

  // Подсказки поисковика приходят из сети и опаздывают: список уже нарисован,
  // а они дописываются к нему, когда придут. В приватном окне их нет.
  if (!value) return;
  const words = await invoke("search_suggest", { query: value }).catch(() => []);
  if (token !== suggestToken || field.hidden || !words.length) return;
  rows = buildRows(value, history, words);
  selected = Math.min(selected, rows.length - 1);
  paint();
}

/**
 * Автодополнение адреса прямо в строке, как в Chrome: набрали «hab» — в поле
 * «habr.com», дописанное выделено. Enter открывает сайт, следующая буква
 * продолжает ввод, Backspace стирает дописанное. Кандидаты — история (она
 * уже отсортирована по частоте и свежести) и закладки. Дописывается сайт, а
 * если в набранном есть «/» — адрес целиком.
 */
function complete(typed, history) {
  if (!typed || /\s/.test(typed) || field.hidden || field.value !== typed) return null;
  if (field.selectionStart !== typed.length || field.selectionEnd !== typed.length) return null;
  const scheme = /^https?:\/\//i.exec(typed)?.[0] ?? "";
  let rest = typed.slice(scheme.length).toLowerCase();
  if (rest.startsWith("www.")) rest = rest.slice(4);
  if (!rest) return null;
  const marks = (state.bookmarks ?? []).filter((node) => node.kind === "url").map((node) => node.url);
  for (const url of [...history.map((entry) => entry.url), ...marks]) {
    let parsed;
    try {
      parsed = new URL(url);
    } catch {
      continue;
    }
    if (!/^https?:$/.test(parsed.protocol)) continue;
    // Сравниваем с тем, как адрес выглядит в строке: кириллицей, а не xn--/%D0.
    const host = (displayHost(parsed.hostname) + (parsed.port ? `:${parsed.port}` : "")).toLowerCase().replace(/^www\./, "");
    const path = displayPath(parsed.pathname + parsed.search);
    const whole = rest.includes("/");
    const target = whole ? host + (path === "/" ? "" : path) : host;
    if (target.length <= rest.length || !target.toLowerCase().startsWith(rest)) continue;
    return {
      typed,
      text: typed + target.slice(rest.length),
      url: whole ? url : `${parsed.protocol}//${parsed.host}/`,
    };
  }
  return null;
}

/**
 * Порядок строк фиксированный — пользователь не должен угадывать, что
 * окажется первым. Первая строка — то, что набрано: переход по адресу или
 * поиск, её и выполняет Enter, как в Chrome. Дальше закладки, история и
 * подсказки поисковика — до них доходят стрелками.
 */
function buildRows(value, history, words) {
  const out = [];
  if (!value) {
    for (const entry of history) {
      out.push({ text: entry.title || displayUrl(entry.url), hint: displayHost(hostOf(entry.url)), value: entry.url, iconId: "history-16" });
    }
    return out;
  }

  // Так же решает Rust (`normalize_url`): localhost и адреса с точкой — переход.
  const looksLikeUrl =
    /^[a-z][a-z0-9+.-]*:\/\/\S/i.test(value) ||
    (!/\s/.test(value) && (/\./.test(value) || /^localhost(:\d+)?(\/|$)/i.test(value)));
  // Адрес дописан в строке — первая строка ведёт на него, как и Enter.
  const filled = inline?.typed === value ? inline : null;
  out.push(
    filled
      ? { text: filled.text, hint: "перейти", value: filled.url, iconId: "globe-16" }
      : looksLikeUrl
        ? { text: displayUrl(value), hint: "перейти", value, iconId: "globe-16" }
        : { text: value, hint: "поиск", value, iconId: "search-16" }
  );

  const needle = value.toLowerCase();
  const seen = new Set();
  for (const node of state.bookmarks ?? []) {
    if (node.kind !== "url" || seen.size >= 3) continue;
    if (!node.title.toLowerCase().includes(needle) && !node.url.toLowerCase().includes(needle)) continue;
    seen.add(node.url);
    out.push({ text: node.title || node.url, hint: "закладка", value: node.url, iconId: "star-16", image: node.icon });
  }
  for (const entry of history) {
    if (seen.has(entry.url) || entry.url === value || entry.url === filled?.url) continue;
    out.push({ text: entry.title || displayUrl(entry.url), hint: displayHost(hostOf(entry.url)), value: entry.url, iconId: "history-16" });
  }
  for (const word of words.slice(0, 6)) {
    if (word.toLowerCase() === needle) continue;
    out.push({ text: word, hint: "поиск", value: word, iconId: "search-16" });
  }
  // Похоже на адрес, но это может быть и запрос: поиск набранного — последним.
  if (looksLikeUrl || filled) out.push({ text: value, hint: "поиск", value: `? ${value}`, iconId: "search-16" });
  return out;
}

/** Показать текущий список: в приложении — попапом, в макете — выпадашкой. */
function paint() {

  if (isNative) {
    if (rows.length) {
      suggestShown = true;
      invoke("popup_open", {
        kind: "suggest",
        anchor: anchorOf(omni),
        width: omni.getBoundingClientRect().width,
        align: "start",
        payload: { rows, selected },
      })
        .then((seq) => {
          if (suggestShown) suggestSeq = Number(seq) || 0;
          // Ввод закончился, пока подсказки открывались: иначе они остались бы
          // висеть над страницей.
          else if (seq) invoke("popup_hide", { seq }).catch(() => {});
        })
        .catch(() => {});
    } else {
      hideSuggestions();
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
  picked = true;
  selected = selected < 0 ? (delta > 0 ? 0 : rows.length - 1) : (selected + delta + rows.length) % rows.length;
  if (isNative) {
    emit("suggest-select", { selected });
    return;
  }
  [...suggest.children].forEach((node, index) => {
    node.dataset.selected = String(index === selected);
  });
}
