/**
 * Во весь экран: видео, которое страница развернула сама (кнопка плеера), и
 * F11. Окно браузера занимает экран, интерфейс прячется, страница ложится на
 * всё окно. Движок об этом не заботится: он только сообщает, что у страницы
 * появился полноэкранный элемент.
 */

import { invoke } from "./bridge.js";
import { syncDuring } from "./layout.js";
import { closePopup, openPopup, openPopupKey } from "./popups.js";
import { state, subscribe } from "./state.js";
import { endSplit, splitWith } from "./tabs.js";

/** Половина разделённого экрана, которая была рядом до разворота видео. */
let splitPartner = null;

export function initFullscreen() {
  subscribe(() => {
    // Вкладку переключили или закрыли — развёрнутое видео прежней сворачивается.
    const page = state.fullscreen.page;
    if (page !== null && page !== state.activeId) {
      invoke("tab_action", { id: page, action: "exit_fullscreen" }).catch(() => {});
      splitPartner = null;
      setPage(null);
    }
  });
}

/** Страница развернула элемент на весь экран или свернула его. */
export async function onPageFullscreen(id, on) {
  if (!on) {
    if (state.fullscreen.page !== id) return;
    setPage(null);
    // Разделённый экран возвращается, как был до видео.
    const partner = splitPartner;
    splitPartner = null;
    if (partner !== null && state.activeId === id && state.tabs.has(partner)) await splitWith(partner);
    return;
  }
  // Развернуть можно только то, что на экране: фоновую вкладку сворачиваем.
  if (id !== state.activeId) {
    invoke("tab_action", { id, action: "exit_fullscreen" }).catch(() => {});
    return;
  }
  // Видео занимает окно целиком — вторая половина экрана ему не нужна.
  if (state.splitId !== null) {
    splitPartner = state.splitId;
    await endSplit();
  }
  setPage(id);
}

/** F11: окно во весь экран без интерфейса и обратно. */
export function toggleWindowFullscreen() {
  state.fullscreen.window = !state.fullscreen.window;
  apply();
}

export function isFullscreen() {
  return state.fullscreen.page !== null || state.fullscreen.window;
}

function setPage(id) {
  state.fullscreen.page = id;
  apply();
}

let applied = false;

function apply() {
  const on = isFullscreen();
  const root = document.documentElement;
  if (on) root.dataset.fullscreen = "true";
  else delete root.dataset.fullscreen;
  if (on !== applied) {
    applied = on;
    invoke("window_command", { action: on ? "fullscreen" : "unfullscreen" }).catch(() => {});
    if (on) showHint();
    else hideHint();
  }
  // Окно меняет размер не сразу: страница догоняет его по кадрам.
  syncDuring(500);
}

/**
 * Подсказка, как выйти, — как в Chrome: несколько секунд сверху по центру.
 * Интерфейс спрятан, а страница закрыла бы HTML собой, поэтому подсказка —
 * всплывающее окно, и фокус оно не забирает: Escape остаётся у видео.
 */
const HINT_WIDTH = 440;
let hintTimer = 0;

function showHint() {
  clearTimeout(hintTimer);
  // Окно ещё разворачивается, а ресайз закрывает всплывающие окна — ждём.
  hintTimer = setTimeout(() => {
    if (!isFullscreen() || openPopupKey() !== null) return;
    const key = state.fullscreen.page !== null ? "Esc" : "F11";
    const anchor = { x: Math.max(8, (innerWidth - HINT_WIDTH) / 2), y: 20, width: HINT_WIDTH, height: 0 };
    openPopup("hint", anchor, { width: HINT_WIDTH, payload: { key } }).catch(() => {});
    hintTimer = setTimeout(hideHint, 3500);
  }, 450);
}

function hideHint() {
  clearTimeout(hintTimer);
  if (openPopupKey() === "hint:") closePopup();
}
