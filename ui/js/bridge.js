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

/**
 * Ярлык этого окна. Подписка Tauri по умолчанию получает события, адресованные
 * *любому* окну: при двух окнах меню, открытое во втором, рисовал и показывал
 * ещё и попап первого — и забирал себе фокус. Поэтому каждое окно слушает
 * только свои события и общие (`app.emit`), а попап разговаривает только со
 * своим окном браузера.
 */
const currentLabel = isNative ? tauri.webviewWindow?.getCurrentWebviewWindow?.().label ?? null : null;

/** С кем говорит это окно: попап — со своим окном браузера, окно — со своим попапом. */
function partnerLabel() {
  if (!currentLabel) return null;
  return currentLabel.startsWith("popup--") ? currentLabel.slice("popup--".length) : `popup--${currentLabel}`;
}

export async function listen(event, handler) {
  if (!isNative) return mock.listen(event, handler);
  const options = currentLabel ? { target: { kind: "AnyLabel", label: currentLabel } } : undefined;
  return tauri.event.listen(event, ({ payload }) => handler(payload), options);
}

/** Событие парному окну: попап ↔ его окно браузера. */
export async function emit(event, payload) {
  if (!isNative) return mock.emit(event, payload);
  const target = partnerLabel();
  if (target) return tauri.event.emitTo(target, event, payload);
  return tauri.event.emit(event, payload);
}

/** Вызовы раскладки летят пачками при ресайзе — ошибки тут не новость. */
export function invokeQuiet(command, args = {}) {
  invoke(command, args).catch(() => {});
}
