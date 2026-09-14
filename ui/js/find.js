/**
 * Поиск по странице.
 *
 * Сам поиск делает движок (`ICoreWebView2Find`), chrome только рисует строку
 * и показывает счётчик. Свой диалог движка подавлен — иначе на экране было бы
 * два поля ввода.
 */

import { invoke } from "./bridge.js";
import { activeTab } from "./state.js";
import { syncDuring } from "./layout.js";

const bar = document.getElementById("findbar");
const field = document.getElementById("find-field");
const counter = document.getElementById("find-count");

let debounce = 0;

export function initFind() {
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

  const tab = activeTab();
  if (tab) invoke("tab_find_step", { id: tab.id, action: "stop" }).catch(() => {});
  syncDuring(120);
}

export function isFindOpen() {
  return !bar.hidden;
}

/** Событие от движка: сколько нашли и на каком совпадении стоим. */
export function renderFindResult({ total, current }) {
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
