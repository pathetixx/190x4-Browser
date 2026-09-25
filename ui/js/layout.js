/**
 * Геометрия «дырки» под страницу.
 *
 * Единственный способ, которым CSS влияет на положение нативной вкладки:
 * chrome измеряет .stage и сообщает прямоугольник в Rust. Всё, что двигает
 * .stage (панель, ресайз, панель закладок), обязано пройти здесь — иначе
 * страница окажется не там, где её нарисовал интерфейс.
 */

import { invoke, invokeQuiet } from "./bridge.js";

let stage = null;
let last = "";
let animating = 0;
let retryTimer = 0;
let retries = 0;

export function initLayout(stageElement) {
  stage = stageElement;

  const observer = new ResizeObserver(() => sync());
  observer.observe(stage);
  window.addEventListener("resize", sync);

  sync();
}

export function sync() {
  send(false);
}

/**
 * `force` — отправить, даже если прямоугольник тот же, что ушёл последним:
 * в конце перехода (полный экран, панель) страница должна стоять по итоговому
 * прямоугольнику, что бы ни случилось с промежуточными.
 */
function send(force) {
  if (!stage) return;
  const rect = stage.getBoundingClientRect();
  const scale = window.devicePixelRatio || 1;

  // Одинаковый прямоугольник шлём один раз: SetBounds на каждый кадр
  // ресайза заметно дёргает композитор страницы.
  const key = `${rect.x}|${rect.y}|${rect.width}|${rect.height}|${scale}`;
  if (key === last && !force) return;
  last = key;

  invoke("layout_set", {
    x: rect.x,
    y: rect.y,
    width: rect.width,
    height: rect.height,
    scale,
  }).then(
    () => {
      retries = 0;
    },
    () => {
      // Хост вкладок был занят (вызов движка внутри вложенного цикла
      // сообщений) — прямоугольник не дошёл, и страница осталась бы прежнего
      // размера. Пробуем снова.
      if (last === key) last = "";
      clearTimeout(retryTimer);
      if (retries++ < 20) retryTimer = setTimeout(sync, 60);
    }
  );
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
    const done = performance.now() >= until;
    send(done);
    animating = done ? 0 : requestAnimationFrame(step);
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
const hiddenListeners = new Set();

export function setPageHidden(reason, hidden) {
  const before = hiddenBy.size > 0;
  if (hidden) hiddenBy.add(reason);
  else hiddenBy.delete(reason);
  const after = hiddenBy.size > 0;
  if (before === after) return;
  invokeQuiet("overlay_set", { on: after });
  for (const fn of hiddenListeners) fn(after);
}

export function isPageHidden() {
  return hiddenBy.size > 0;
}

/** Страницу спрятали или вернули: окна страницы показываются только над ней. */
export function onPageHidden(fn) {
  hiddenListeners.add(fn);
  return () => hiddenListeners.delete(fn);
}

/** Палитра перекрывает страницу — страница уходит. */
export function setOverlay(on) {
  setPageHidden("overlay", on);
}
