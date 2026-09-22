/**
 * Поиск по странице.
 *
 * Сам поиск делает движок (`ICoreWebView2Find`), chrome только рисует строку
 * и показывает счётчик. Свой диалог движка подавлен — иначе на экране было бы
 * два поля ввода.
 */

import { invoke } from "./bridge.js";
import { activeTab, state, subscribe } from "./state.js";
import { syncDuring } from "./layout.js";

const bar = document.getElementById("findbar");
const field = document.getElementById("find-field");
const counter = document.getElementById("find-count");

let debounce = 0;
/** Вкладка, на которой идёт поиск: у каждой вкладки он свой, как в Chrome. */
let searched = null;

export function initFind() {
  // Переключили вкладку — строка поиска прежней закрывается вместе с подсветкой.
  subscribe(() => {
    if (!bar.hidden && searched !== null && searched !== state.activeId) closeFind();
  });

  field.addEventListener("input", () => {
    // Поиск на каждое нажатие перезапускает подсветку всей страницы —
    // на длинных документах это заметно, поэтому ждём паузу в наборе.
    clearTimeout(debounce);
    debounce = setTimeout(run, 220);
  });

  field.addEventListener("keydown", (event) => {
    if (event.key === "Enter") {
      event.preventDefault();
      step(event.shiftKey ? "prev" : "next");
    }
    if (event.key === "Escape") {
      event.preventDefault();
      closeFind();
    }
  });

  document.getElementById("find-next").addEventListener("click", () => step("next"));
  document.getElementById("find-prev").addEventListener("click", () => step("prev"));
  document.getElementById("find-close").addEventListener("click", closeFind);
}

export function openFind() {
  searched = state.activeId;
  bar.hidden = false;
  document.documentElement.dataset.find = "open";
  field.focus();
  field.select();
  invoke("chrome_focus")
    .then(() => {
      if (!bar.hidden) field.focus();
    })
    .catch(() => {});
  // Строка отняла высоту у страницы — подвинуть её нужно сразу.
  syncDuring(120);
  if (field.value) run();
}

export function closeFind() {
  bar.hidden = true;
  delete document.documentElement.dataset.find;
  counter.textContent = "0/0";
  counter.dataset.empty = "false";

  const id = searched ?? activeTab()?.id;
  if (id != null) invoke("tab_find_step", { id, action: "stop" }).catch(() => {});
  searched = null;
  syncDuring(120);
}

export function isFindOpen() {
  return !bar.hidden;
}

/**
 * F3 и Ctrl+G — следующее совпадение, Shift+F3 и Ctrl+Shift+G — предыдущее.
 * Строка поиска закрыта — открывается с прежним запросом, как в Chrome.
 */
export function findAgain(direction) {
  if (bar.hidden || searched !== state.activeId) {
    openFind();
    return;
  }
  step(direction);
}

/** Событие от движка: сколько нашли и на каком совпадении стоим. */
export function renderFindResult({ id, total, current }) {
  if (id !== searched) return;
  counter.textContent = `${total > 0 ? current + 1 : 0}/${total}`;
  counter.dataset.empty = String(total === 0 && field.value.length > 0);
}

function run() {
  const tab = activeTab();
  const query = field.value;
  if (!tab || !query) {
    counter.textContent = "0/0";
    return;
  }
  invoke("tab_find", { id: tab.id, query }).catch(() => {
    // Старый рантайм без Find API: честно говорим, что не умеем.
    counter.textContent = "—";
  });
}

function step(direction) {
  const tab = activeTab();
  if (tab) invoke("tab_find_step", { id: tab.id, action: direction }).catch(() => {});
}
