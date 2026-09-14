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

let openKind = null;
let openAnchor = null;
let closedKind = null;
let closedAt = 0;

export function initPopups() {
  listen("popup-closed", () => {
    closedKind = openKind;
    closedAt = performance.now();
    openAnchor?.removeAttribute("aria-expanded");
    openKind = null;
    openAnchor = null;
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

  const rect = anchor instanceof Element ? anchorOf(anchor) : anchor;
  try {
    await invoke("popup_open", { kind, anchor: rect, width, align, payload });
  } catch (error) {
    openAnchor?.removeAttribute("aria-expanded");
    openKind = null;
    openAnchor = null;
    throw error;
  }
  return true;
}

/** Меню — самый частый попап: список пунктов, выбор приходит в `onPick`. */
export function openMenu(menu, anchor, items, onPick, { width = 280, align = "start" } = {}) {
  onPopupAction(`menu:${menu}`, (message) => onPick(message.action, message));
  return openPopup("menu", anchor, { width, align, payload: { menu, items } });
}

export function closePopup() {
  invoke("popup_hide").catch(() => {});
}
