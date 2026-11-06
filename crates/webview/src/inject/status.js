// Строка состояния 190x4: адрес ссылки под курсором — плашкой в углу страницы,
// вместо плашки движка WebView2 (её браузер выключает). Текст присылает
// браузер по событию движка `StatusBarTextChanged`, скрипт только рисует его.
//
// Плашка — в закрытом shadow DOM, стили — через `style` элементов: `<style>`
// внутри страницы со строгим CSP не применился бы.
(() => {
  "use strict";

  if (window.top !== window) return;
  const MARK = Symbol.for("x4status");
  if (window[MARK]) return;
  try {
    Object.defineProperty(window, MARK, { value: true, enumerable: false });
  } catch (_) {
    return;
  }

  /** Плашка у края и отступ от него: как в Chrome, слева внизу. */
  const EDGE = 0;
  /** Сколько плашка ещё видна, когда курсор ушёл со ссылки. */
  const HIDE_MS = 160;
  const MAX_TEXT = 2048;

  let host = null;
  let label = null;
  let hideTimer = 0;
  let pointer = { x: -1, y: -1 };

  const build = () => {
    host = document.createElement("x4-status");
    host.style.cssText =
      "all: initial; position: fixed; bottom: 0; left: 0; z-index: 2147483647; display: block; pointer-events: none;";
    const root = host.attachShadow({ mode: "closed" });
    label = document.createElement("div");
    label.style.cssText = [
      "box-sizing: border-box",
      "max-width: min(560px, calc(100vw - 24px))",
      "padding: 3px 8px",
      "background: #16171b",
      "color: #e9e9ee",
      "border: 1px solid #2c2e35",
      "border-radius: 0 4px 0 0",
      "box-shadow: 0 2px 10px rgba(0, 0, 0, 0.35)",
      'font: 12px/18px "Segoe UI", system-ui, sans-serif',
      "white-space: nowrap",
      "overflow: hidden",
      "text-overflow: ellipsis",
      "opacity: 0",
      "transition: opacity 90ms linear",
    ].join(";");
    root.append(label);
  };

  /** Курсор над плашкой — она уходит в правый угол, чтобы не закрывать ссылку. */
  const place = () => {
    const rect = label.getBoundingClientRect();
    const covered = pointer.y >= innerHeight - rect.height - 24 && pointer.x <= rect.width + 24;
    host.style.left = covered ? "auto" : `${EDGE}px`;
    host.style.right = covered ? `${EDGE}px` : "auto";
    label.style.borderRadius = covered ? "4px 0 0 0" : "0 4px 0 0";
  };

  const show = (text) => {
    // Видео во весь экран — без плашки, как в Chrome.
    if (document.fullscreenElement) return;
    if (!host) build();
    const parent = document.documentElement;
    if (!parent) return;
    if (!host.isConnected) parent.append(host);
    clearTimeout(hideTimer);
    label.textContent = text;
    place();
    label.style.opacity = "1";
  };

  const hide = () => {
    if (!label) return;
    clearTimeout(hideTimer);
    hideTimer = setTimeout(() => {
      label.style.opacity = "0";
    }, HIDE_MS);
  };

  addEventListener(
    "mousemove",
    (event) => {
      pointer = { x: event.clientX, y: event.clientY };
    },
    { capture: true, passive: true }
  );

  const onMessage = (event) => {
    const data = event.data;
    if (!data || data.cmd !== "x4_status" || typeof data.text !== "string") return;
    const text = data.text.slice(0, MAX_TEXT);
    if (text) show(text);
    else hide();
  };
  const bridge = () => (window.chrome && window.chrome.webview) || null;
  const subscribe = () => {
    const hook = bridge();
    if (!hook) return false;
    hook.addEventListener("message", onMessage);
    return true;
  };
  if (!subscribe()) document.addEventListener("DOMContentLoaded", subscribe, { once: true });
})();
