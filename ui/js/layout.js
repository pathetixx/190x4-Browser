/**
 * Геометрия «дырки» под страницу.
 *
 * Единственный способ, которым CSS влияет на положение нативной вкладки:
 * chrome измеряет .stage и сообщает прямоугольник в Rust. Всё, что двигает
 * .stage (панель, ресайз, панель закладок), обязано пройти здесь — иначе
 * страница окажется не там, где её нарисовал интерфейс.
 */

import { invokeQuiet } from "./bridge.js";

let stage = null;
let last = "";
let animating = 0;

export function initLayout(stageElement) {
  stage = stageElement;

  const observer = new ResizeObserver(() => sync());
  observer.observe(stage);
  window.addEventListener("resize", sync);

  sync();
}

export function sync() {
  if (!stage) return;
  const rect = stage.getBoundingClientRect();
  const scale = window.devicePixelRatio || 1;

  // Одинаковый прямоугольник шлём один раз: SetBounds на каждый кадр
  // ресайза заметно дёргает композитор страницы.
  const key = `${rect.x}|${rect.y}|${rect.width}|${rect.height}|${scale}`;
  if (key === last) return;
  last = key;

  invokeQuiet("layout_set", {
    x: rect.x,
    y: rect.y,
    width: rect.width,
    height: rect.height,
    scale,
  });
}

/**
 * Пока панель едет (CSS transition ~260 мс), ResizeObserver даёт события
 * с отставанием в кадр, и страница «догоняет» интерфейс рывком. На время
 * анимации переходим на rAF.
 */
export function syncDuring(durationMs = 320) {
  const until = performance.now() + durationMs;
  if (animating) cancelAnimationFrame(animating);

  const step = () => {
    sync();
    animating = performance.now() < until ? requestAnimationFrame(step) : 0;
  };
  animating = requestAnimationFrame(step);
}

/**
 * Нативную страницу прячут по разным причинам одновременно: открыта
 * палитра, активна встроенная вкладка. Показать её можно, только когда не
 * осталось ни одной — иначе закрытие палитры вытащило бы сайт поверх
 * страницы настроек.
 */
const hiddenBy = new Set();

export function setPageHidden(reason, hidden) {
  const before = hiddenBy.size > 0;
  if (hidden) hiddenBy.add(reason);
  else hiddenBy.delete(reason);
  const after = hiddenBy.size > 0;
  if (before !== after) invokeQuiet("overlay_set", { on: after });
}

/** Палитра перекрывает страницу — страница уходит. */
export function setOverlay(on) {
  setPageHidden("overlay", on);
}
