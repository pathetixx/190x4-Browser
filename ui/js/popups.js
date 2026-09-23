/**
 * Всплывающие окна со стороны окна браузера: открыть под кнопкой, получить
 * выбор пользователя.
 *
 * Сам попап — отдельное окно (popup.html) поверх нативной страницы. Он
 * закрывается, как только теряет фокус. Отсюда тонкость: клик по той же
 * кнопке, что открыла попап, сначала закрывает его (фокус ушёл в окно
 * браузера), а потом приходит как клик — и открыл бы его снова. Поэтому
 * повторное открытие того же попапа в первые мгновения после закрытия
 * считается «закрыть».
 */

import { invoke, listen } from "./bridge.js";
import { anchorOf } from "./dom.js";

const REOPEN_GUARD_MS = 300;
const handlers = new Map();
const closers = new Set();

let openKind = null;
let openAnchor = null;
/**
 * Номер показа попапа, открытого отсюда (его выдаёт Rust); 0 — попап ещё
 * открывается. Закрытие прежнего меню приходит отдельным путём и может
 * опоздать — по номеру оно не спутается с закрытием нового попапа.
 */
let openSeq = 0;
let opening = 0;
let closedKind = null;
let closedAt = 0;

export function initPopups() {
  listen("popup-closed", (payload) => {
    const seq = Number(payload?.seq) || 0;
    const replaced = Boolean(payload?.replaced);
    if (seq && seq === openSeq) {
      closedKind = openKind;
      closedAt = performance.now();
      openAnchor?.removeAttribute("aria-expanded");
      openKind = null;
      openAnchor = null;
      openSeq = 0;
    }
    for (const fn of closers) fn(seq, replaced);
  });

  listen("popup-action", (message) => {
    if (!message?.kind) return;
    handlers.get(message.kind)?.(message);
  });
}

/** Обработчик действий из попапа определённого вида. */
export function onPopupAction(kind, handler) {
  handlers.set(kind, handler);
}

/**
 * Попап закрылся или его сменил другой: `fn(seq, replaced)` получает номер
 * закрытого показа — тот, что вернул `openPopup` (или `popup_open`), — и
 * `replaced`, если окно не пряталось, а его занял следующий попап.
 */
export function onPopupClosed(fn) {
  closers.add(fn);
  return () => closers.delete(fn);
}

/**
 * Возвращает номер показа (для `onPopupClosed`) или `false`, если попап не
 * открылся: повторный щелчок по кнопке только что закрытого попапа — это
 * «закрыть».
 *
 * @param {string} kind    вид попапа: menu, downloads, bookmark, media, password…
 * @param {Element|{x,y,width,height}} anchor  кнопка или прямоугольник
 */
export async function openPopup(kind, anchor, { width = 320, align = "start", payload = null } = {}) {
  const key = `${kind}:${payload?.menu ?? ""}`;
  if (closedKind === key && performance.now() - closedAt < REOPEN_GUARD_MS) {
    closedKind = null;
    return false;
  }

  openAnchor?.removeAttribute("aria-expanded");
  openKind = key;
  openAnchor = anchor instanceof Element ? anchor : null;
  openAnchor?.setAttribute("aria-expanded", "true");
  openSeq = 0;
  const request = ++opening;

  const rect = anchor instanceof Element ? anchorOf(anchor) : anchor;
  let seq;
  try {
    seq = await invoke("popup_open", { kind, anchor: rect, width, align, payload });
  } catch (error) {
    if (request === opening) {
      openAnchor?.removeAttribute("aria-expanded");
      openKind = null;
      openAnchor = null;
    }
    throw error;
  }
  if (request === opening) openSeq = Number(seq) || 0;
  // В макете без Rust номера нет — попап всё равно «открыт».
  return Number(seq) || true;
}

/** Меню — самый частый попап: список пунктов, выбор приходит в `onPick`. */
export function openMenu(menu, anchor, items, onPick, { width = 280, align = "start" } = {}) {
  onPopupAction(`menu:${menu}`, (message) => onPick(message.action, message));
  return openPopup("menu", anchor, { width, align, payload: { menu, items } });
}

/** Закрыть попап, открытый отсюда, — но не тот, что успел его сменить. */
export function closePopup() {
  invoke("popup_hide", { seq: openSeq || null }).catch(() => {});
}

/** Какой попап открыт: `вид:меню` или null. */
export function openPopupKey() {
  return openKind;
}
