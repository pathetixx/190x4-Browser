/**
 * Единственная точка контакта интерфейса с Rust.
 *
 * Здесь же живёт mock-режим: если `window.__TAURI__` нет, интерфейс
 * поднимается на фальшивых данных. Это рабочий инструмент — chrome браузера
 * можно открыть в обычном браузере, править CSS и снимать скриншоты, не
 * собирая Windows-бинарь.
 */

const tauri = globalThis.__TAURI__;

export const isNative = Boolean(tauri?.core?.invoke);

let mock = null;
if (!isNative) {
  mock = await import("./mock.js");
}

/**
 * Команды, где важен порядок: «поставить тему A», сразу «тему B». Rust
 * выполняет их в пуле потоков, и два вызова подряд могли бы применяться в
 * обратном порядке — тогда в базе осталось бы старое значение. Вызовы одной
 * такой команды идут друг за другом.
 */
const ORDERED = new Set(["settings_set", "session_save", "adblock_set_enabled", "adblock_site_set", "zoom_site_set"]);
const chains = new Map();

export async function invoke(command, args = {}) {
  if (!isNative) return mock.invoke(command, args);
  if (!ORDERED.has(command)) return tauri.core.invoke(command, args);
  const previous = chains.get(command) ?? Promise.resolve();
  const call = previous.catch(() => {}).then(() => tauri.core.invoke(command, args));
  chains.set(command, call);
  return call;
}

export async function listen(event, handler) {
  if (isNative) return tauri.event.listen(event, ({ payload }) => handler(payload));
  return mock.listen(event, handler);
}

/** Событие другим окнам приложения (попап ↔ окно браузера). */
export async function emit(event, payload) {
  if (isNative) return tauri.event.emit(event, payload);
  return mock.emit(event, payload);
}

/** Вызовы раскладки летят пачками при ресайзе — ошибки тут не новость. */
export function invokeQuiet(command, args = {}) {
  invoke(command, args).catch(() => {});
}
