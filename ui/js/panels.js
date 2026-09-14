/**
 * Боковые панели: блокировка рекламы, закладки, история, переводчик.
 * Каждая отжимает страницу, а не накрывает её — нативную поверхность нельзя
 * прикрыть полупрозрачным слоем.
 */

import { invoke } from "./bridge.js";
import { clock, dayLabel, el, favicon, hostOf, icon, plural } from "./dom.js";
import { syncDuring } from "./layout.js";
import { pref, setPref } from "./prefs.js";
import { activeTab, state } from "./state.js";
import { navigate, openSettings } from "./actions.js";

const panel = document.getElementById("panel");
const title = document.getElementById("panel-title");
const body = document.getElementById("panel-body");
const rail = document.querySelector(".rail");

const TITLES = {
  shield: "Блокировка рекламы",
  bookmarks: "Закладки",
  history: "История",
  translate: "Переводчик",
};

let current = null;
let renderToken = 0;
let bookmarkFolder = 1;
let historyQuery = "";

export function initPanels() {
  rail.addEventListener("click", (event) => {
    const button = event.target.closest(".rail__btn[data-panel]");
    if (button) toggle(button.dataset.panel);
  });
  document.getElementById("panel-close").addEventListener("click", () => toggle(current));
  body.classList.add("scroll");
}

export function toggle(name) {
  setPanel(current === name ? null : name);
}

export function openPanel(name) {
  if (current !== name) setPanel(name);
}

function setPanel(name) {
  current = name;
  const opening = Boolean(name);
  document.querySelector(".app").dataset.panel = String(opening);
  panel.dataset.open = String(opening);
  panel.setAttribute("aria-hidden", String(!opening));
  for (const button of rail.querySelectorAll(".rail__btn[data-panel]")) {
    button.setAttribute("aria-pressed", String(button.dataset.panel === name));
  }
  if (opening) renderPanel();
  // Панель едет 260 мс — всё это время страница должна ехать вместе с ней.
  syncDuring(340);
}

export async function renderPanel() {
  if (!current) return;
  const render = VIEWS[current];
  title.textContent = TITLES[current];
  const token = ++renderToken;
  const nodes = await render();
  if (token === renderToken && current) body.replaceChildren(...nodes);
}

export function isLivePanel() {
  return current === "shield";
}

export function isPanelOpen(name) {
  return current === name;
}

const VIEWS = {
  shield() {
    const tab = activeTab();
    const blocked = tab?.blocked ?? 0;

    const page = card("На этой странице", blocked, plural(blocked, "запрос заблокирован", "запроса заблокировано", "запросов заблокировано"));
    const total = card("За сеанс", state.blockedTotal, plural(state.blockedTotal, "запрос", "запроса", "запросов"));

    const latency = card("Задержка фильтра", state.latencyMicros.toFixed(1), "мкс на запрос");
    const meter = el("div", "meter");
    const fill = el("div", "meter__fill");
    // 100 мкс — порог из спайка, за которым синхронный матчинг перестаёт
    // быть бесплатным.
    fill.style.width = `${Math.min(100, state.latencyMicros)}%`;
    meter.append(fill);
    latency.append(meter);

    const toggleRow = el("button", "prow");
    toggleRow.append(
      textBlock("Блокировка рекламы и трекеров", state.adblockOn ? "Включена для всех сайтов" : "Выключена")
    );
    const switchNode = el("span", "switch");
    switchNode.setAttribute("aria-checked", String(state.adblockOn));
    toggleRow.append(switchNode);
    toggleRow.addEventListener("click", async () => {
      state.adblockOn = !state.adblockOn;
      await invoke("adblock_set_enabled", { on: state.adblockOn }).catch(() => {});
      renderPanel();
    });

    const settings = el("button", "prow");
    settings.append(icon("settings", 20), textBlock("Списки фильтров", "Настройки блокировки"));
    settings.addEventListener("click", () => openSettings("privacy"));

    return [page, total, latency, toggleRow, settings];
  },

  async bookmarks() {
    const nodes = state.bookmarks?.length ? state.bookmarks : await invoke("bookmarks_tree").catch(() => []);
    const byId = new Map(nodes.map((node) => [node.id, node]));
    if (!byId.has(bookmarkFolder)) bookmarkFolder = 1;
    const folder = byId.get(bookmarkFolder);

    const out = [];
    const head = el("div", "phead");
    if (folder?.parent_id != null) {
      const back = el("button", "piconbtn");
      back.title = "Назад";
      back.append(icon("back", 20));
      back.addEventListener("click", () => {
        bookmarkFolder = folder.parent_id;
        renderPanel();
      });
      head.append(back);
    }
    head.append(el("span", "phead__title", folder?.title ?? "Закладки"));
    const manage = el("button", "plink", "Управление");
    manage.addEventListener("click", () => openSettings("bookmarks"));
    head.append(manage);
    out.push(head);

    // В корне показываем обе корневые папки, внутри — содержимое папки.
    const children =
      bookmarkFolder === 1
        ? [...nodes.filter((n) => n.parent_id === 1), ...nodes.filter((n) => n.id === 2)]
        : nodes.filter((n) => n.parent_id === bookmarkFolder);
    children.sort((a, b) => (a.id === 2 ? 1 : b.id === 2 ? -1 : a.position - b.position));

    if (!children.length) {
      out.push(el("div", "pempty", "В этой папке пока пусто"));
      return out;
    }

    for (const node of children) {
      const row = el("button", "prow");
      if (node.kind === "folder") {
        row.append(icon("folder", 20), textBlock(node.title, `${nodes.filter((n) => n.parent_id === node.id).length} шт.`));
        row.addEventListener("click", () => {
          bookmarkFolder = node.id;
          renderPanel();
        });
      } else {
        row.append(favicon(node.icon), textBlock(node.title || node.url, hostOf(node.url)));
        row.title = node.url;
        row.addEventListener("click", (event) => navigate(node.url, { newTab: event.ctrlKey }));
        row.addEventListener("auxclick", (event) => {
          if (event.button === 1) navigate(node.url, { newTab: true, background: true });
        });
      }
      out.push(row);
    }
    return out;
  },

  async history() {
    const out = [];
    const search = el("div", "psearch");
    search.append(icon("search-16", 16));
    const input = el("input", "field");
    input.type = "search";
    input.placeholder = "Поиск в истории";
    input.value = historyQuery;
    input.addEventListener("input", () => {
      historyQuery = input.value;
      clearTimeout(input._timer);
      input._timer = setTimeout(async () => {
        const nodes = await VIEWS.history();
        body.replaceChildren(...nodes);
        const next = body.querySelector("input");
        next?.focus();
        next?.setSelectionRange(next.value.length, next.value.length);
      }, 200);
    });
    search.append(input);
    out.push(search);

    const items = historyQuery.trim()
      ? await invoke("history_search", { query: historyQuery.trim(), limit: 80 }).catch(() => [])
      : await invoke("history_recent", { limit: 120 }).catch(() => []);

    if (!items.length) {
      out.push(el("div", "pempty", historyQuery ? "Ничего не нашлось" : "История пуста"));
      return out;
    }

    let day = "";
    for (const entry of items) {
      const label = dayLabel(entry.visited_at);
      if (label !== day) {
        day = label;
        out.push(el("div", "pday", label));
      }
      const row = el("button", "prow");
      row.title = entry.url;
      row.append(favicon(null), textBlock(entry.title || entry.url, entry.host));
      row.append(el("span", "prow__num", clock(entry.visited_at)));
      row.addEventListener("click", (event) => navigate(entry.url, { newTab: event.ctrlKey }));
      out.push(row);
    }

    const clear = el("button", "prow prow--footer");
    clear.append(icon("broom", 20), textBlock("Удалить данные о работе в браузере", "История, файлы cookie, кэш"));
    clear.addEventListener("click", () => openSettings("privacy"));
    out.push(clear);
    return out;
  },

  translate() {
    const out = [];
    const tr = state.translate;

    if (!state.services.translate) {
      out.push(el("div", "pempty", "Переводчик недоступен: сервис 190x4 не настроен"));
      return out;
    }

    const langCard = el("div", "pcard");
    langCard.append(el("div", "pcard__kicker", "Язык перевода"));
    const chips = el("div", "chips");
    for (const name of ["Русский", "English", "Deutsch", "Français", "Español", "Türkçe", "中文"]) {
      const chip = el("button", "chip", name);
      chip.dataset.active = String(pref("translate_lang") === name);
      chip.addEventListener("click", async () => {
        await setPref("translate_lang", name);
        renderPanel();
      });
      chips.append(chip);
    }
    langCard.append(chips);
    out.push(langCard);

    if (tr.busy) {
      out.push(el("div", "pempty", "Переводим…"));
      return out;
    }
    if (tr.error) {
      const error = el("div", "pcard");
      error.append(el("div", "pcard__kicker", "Не получилось"), el("div", "quote", tr.error));
      out.push(error);
      return out;
    }
    if (!tr.result) {
      out.push(el("div", "pempty", "Выделите текст на странице и выберите «Перевести выделенное» в контекстном меню"));
      return out;
    }

    const source = el("div", "pcard");
    source.append(
      el("div", "pcard__kicker", tr.detected ? `Оригинал · ${tr.detected}` : "Оригинал"),
      el("div", "quote", tr.text)
    );
    const result = el("div", "pcard");
    result.append(el("div", "pcard__kicker", `Перевод · ${pref("translate_lang")}`), el("div", "quote quote--accent", tr.result));
    out.push(source, result);
    return out;
  },
};

function card(kicker, value, unit) {
  const node = el("div", "pcard");
  const valueNode = el("div", "pcard__value", String(value));
  valueNode.append(el("small", null, unit));
  node.append(el("div", "pcard__kicker", kicker), valueNode);
  return node;
}

function textBlock(primary, secondary) {
  const node = el("div", "prow__main");
  node.append(el("div", "prow__title", primary));
  if (secondary) node.append(el("div", "prow__meta", secondary));
  return node;
}
