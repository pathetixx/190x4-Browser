/**
 * Обновления браузера в интерфейсе: кнопка «Обновить» на панели, пункт в
 * меню, пузырь с заметками и прогрессом, блок на странице «О браузере».
 *
 * Проверяет и устанавливает Rust (`updates.rs`); здесь — только состояние
 * для показа и команды.
 */

import { invoke, isNative, listen } from "./bridge.js";
import { hooks } from "./actions.js";
import { closePopup, onPopupAction, openPopup } from "./popups.js";

const button = document.getElementById("update-btn");
const listeners = new Set();

export const update = {
  /** Найденная версия: `{ version, current, notes, date }`. */
  info: null,
  checking: false,
  /** Проверка уже была в этом запуске — можно сказать «последняя версия». */
  checked: false,
  installing: false,
  error: "",
};

export function initUpdates() {
  if (!isNative) return;
  listen("update-available", (info) => set({ info, error: "" }));
  invoke("update_state")
    .then((info) => info && set({ info }))
    .catch(() => {});
  button.addEventListener("click", () => openUpdateBubble());
  onPopupAction("update", ({ action }) => {
    if (action === "install") installUpdate();
  });
  renderButton();
}

export function onUpdate(fn) {
  listeners.add(fn);
  return () => listeners.delete(fn);
}

function set(patch) {
  Object.assign(update, patch);
  renderButton();
  for (const fn of listeners) fn(update);
}

function renderButton() {
  button.hidden = !update.info;
  if (update.info) button.title = `Обновить 190x4 до версии ${update.info.version}`;
}

export async function checkUpdates() {
  if (update.checking) return update.info;
  set({ checking: true, error: "" });
  try {
    const info = await invoke("update_check");
    set({ checking: false, checked: true, info: info ?? null });
  } catch (error) {
    set({ checking: false, checked: true, error: String(error) });
  }
  return update.info;
}

export function openUpdateBubble() {
  if (!update.info) return false;
  const anchor = button.hidden ? document.getElementById("open-menu") : button;
  return openPopup("update", anchor, {
    width: 360,
    align: "end",
    payload: { ...update.info, installing: update.installing },
  });
}

/** Скачать и поставить. На Windows браузер закроется и откроется уже новым. */
export async function installUpdate() {
  if (update.installing || !update.info) return;
  set({ installing: true, error: "" });
  await hooks.saveSession();
  try {
    await invoke("update_install");
  } catch (error) {
    set({ installing: false, error: String(error) });
    closePopup();
    hooks.toast(`Обновление не установлено: ${error}`);
  }
}
