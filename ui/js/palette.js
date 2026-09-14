/**
 * Командная палитра (Ctrl+K).
 *
 * Перекрывает страницу, поэтому поднимает overlay-режим: пока она открыта,
 * нативная вкладка скрыта.
 */

import { invoke } from "./bridge.js";
import { el, icon } from "./dom.js";
import { setOverlay } from "./layout.js";

const palette = document.getElementById("palette");
const input = document.getElementById("palette-input");
const list = document.getElementById("palette-list");
const scrim = document.getElementById("scrim");

let commands = [];
let filtered = [];
let selected = 0;

export function initPalette(registry) {
  commands = registry;

  input.addEventListener("input", () => render(input.value));
  input.addEventListener("keydown", (event) => {
    if (event.key === "Escape") return closePalette();
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      move(event.key === "ArrowDown" ? 1 : -1);
      return;
    }
    if (event.key === "Enter") {
      event.preventDefault();
      run(filtered[selected]);
    }
  });

  list.addEventListener("click", (event) => {
    const row = event.target.closest(".palette__row");
    if (row) run(filtered[Number(row.dataset.index)]);
  });

  scrim.addEventListener("click", closePalette);
}

export function openPalette() {
  scrim.hidden = false;
  palette.hidden = false;
  input.value = "";
  render("");
  input.focus();
  setOverlay(true);
  invoke("chrome_focus")
    .then(() => {
      if (!palette.hidden) input.focus();
    })
    .catch(() => {});
}

export function closePalette() {
  palette.hidden = true;
  scrim.hidden = true;
  setOverlay(false);
}

export function isPaletteOpen() {
  return !palette.hidden;
}

function run(command) {
  if (!command) return;
  closePalette();
  command.run();
}

/**
 * Нечёткий поиск по подпоследовательности: «нвк» находит «новая вкладка».
 * Ранжируем по позиции совпадений — команда, чьё название начинается с
 * запроса, выше.
 */
function match(command, query) {
  if (!query) return 0;
  const haystack = `${command.title} ${command.group} ${command.keywords ?? ""}`.toLowerCase();
  let index = 0;
  let score = 0;
  for (const char of query.toLowerCase()) {
    const found = haystack.indexOf(char, index);
    if (found === -1) return -1;
    score += found - index;
    index = found + 1;
  }
  return score;
}

function render(query) {
  filtered = commands
    .filter((command) => !command.when || command.when())
    .map((command) => ({ command, score: match(command, query.trim()) }))
    .filter((item) => item.score >= 0)
    .sort((a, b) => a.score - b.score)
    .map((item) => item.command);

  selected = 0;
  list.replaceChildren();

  let group = null;
  filtered.forEach((command, index) => {
    if (!query && command.group !== group) {
      group = command.group;
      list.append(el("div", "palette__group", group));
    }

    const row = el("button", "palette__row");
    row.dataset.index = index;
    row.dataset.selected = String(index === selected);
    row.append(icon(command.icon ?? "find", 20, "palette__icon"), el("span", "palette__label", command.title));
    if (command.keys) row.append(el("span", "kbd", command.keys));
    list.append(row);
  });

  if (!filtered.length) list.append(el("div", "empty-palette", "Ничего не нашлось"));
}

function move(delta) {
  if (!filtered.length) return;
  selected = (selected + delta + filtered.length) % filtered.length;
  for (const row of list.querySelectorAll(".palette__row")) {
    row.dataset.selected = String(Number(row.dataset.index) === selected);
  }
  list.querySelector('[data-selected="true"]')?.scrollIntoView({ block: "nearest" });
}
