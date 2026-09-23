/**
 * Окна, которые просит страница: alert, confirm, prompt, «Покинуть сайт?»,
 * запрос разрешения, вход на сайт по паролю, открытие приложения по ссылке.
 *
 * Своё окно движок не показывает и ждёт ответа. Окно рисует всплывающее окно
 * браузера над страницей той вкладки, что его просила; пока вкладка в фоне,
 * окно ждёт в очереди. Щелчок мимо окно не закрывает: страница ждёт ответа.
 * Если окно сменило меню, переключили вкладку или свернули окно браузера, оно
 * появится снова, когда станет можно. Ответ записывает Rust и сообщает
 * `dialog-done`.
 */

import { invoke, listen } from "./bridge.js";
import { isPageHidden, onPageHidden } from "./layout.js";
import { closePopup, onPopupAction, onPopupClosed, openPopup, openPopupKey } from "./popups.js";
import { rightPaneId, state, subscribe } from "./state.js";

/** Окно, закрытое без ответа, возвращается с задержкой: меню, ради которого оно
 *  закрылось, должно успеть открыться, а окно браузера — встать на место. */
const RETURN_MS = 250;
const WIDTH = 440;

const queue = [];
/** Окна страницы с последней навигации: со второго их можно запретить. */
const counts = new Map();

let shown = null;
let timer = 0;
let seq = 0;
let minimized = false;
/** Какие вкладки были на экране при прошлой проверке: активная и вторая половина. */
let onScreen = "";

export function initDialogs() {
  listen("dialog-done", ({ id, tokens }) => forget(id, tokens));
  onPopupAction("dialog", ({ tab, tokens }) => forget(tab, tokens));
  // Окно страницы закрыл ресайз — оно вернётся, когда станет можно. Его место
  // занял другой попап (меню, подсказки адресной строки) — вернётся, когда
  // закроется тот: иначе они вытесняли бы друг друга.
  onPopupClosed((seq, replaced) => {
    if (shown?.seq && seq === shown.seq) {
      shown = null;
      if (replaced) return;
    }
    if (!shown && queue.length) later();
  });
  listen("window-state", ({ minimized: now = false }) => {
    minimized = now;
    // Окно браузера меняет размер: окно страницы вернётся, когда размер встанет.
    if (!now && queue.length) later();
  });
  onPageHidden((hidden) => (hidden ? hide() : later()));
  subscribe(() => {
    // Закрытая вкладка: её окна движок закрыл вместе со страницей.
    for (let i = queue.length - 1; i >= 0; i--) {
      if (!state.tabs.has(queue[i].tab)) queue.splice(i, 1);
    }
    const now = `${state.activeId}|${state.splitId}`;
    if (now === onScreen) return;
    onScreen = now;
    hide();
    later();
  });
}

/** Движок просит окно. */
export function onDialog({ id, token, request }) {
  if (!request?.type) return;
  let repeat = false;
  if (request.type === "script") {
    const count = (counts.get(id) ?? 0) + 1;
    counts.set(id, count);
    repeat = count > 1;
  }
  queue.push({ tab: id, token, request, repeat });
  later();
}

/** Окна, которых больше не ждут: страница ушла. */
export function onDialogsClosed({ id, tokens }) {
  forget(id, tokens);
}

/** Ждёт ли вкладка ответа на «Покинуть сайт?» — показанного или в очереди. */
export function hasLeaveDialog(id) {
  const leave = (item) => item.tab === id && item.request.type === "script" && item.request.kind === "beforeunload";
  return queue.some(leave);
}

/**
 * Окно браузера остаётся открытым: страницам, которые ещё спрашивают «Покинуть
 * сайт?», отвечаем «Остаться» — человек уже ответил на такой вопрос.
 */
export function dismissLeaveDialogs(tabs) {
  const ids = new Set(tabs);
  for (let i = queue.length - 1; i >= 0; i--) {
    const item = queue[i];
    if (!ids.has(item.tab) || item.request.type !== "script" || item.request.kind !== "beforeunload") continue;
    queue.splice(i, 1);
    if (shown?.tab === item.tab && shown.tokens.includes(item.token)) hide();
    invoke("tab_dialog", { id: item.tab, tokens: [item.token], answer: { action: "cancel" } }).catch(() => {});
  }
}

/** Новая страница во вкладке — счёт её окон заново. */
export function onNavigation(id) {
  counts.delete(id);
}

function forget(tab, tokens = []) {
  const gone = new Set(tokens);
  for (let i = queue.length - 1; i >= 0; i--) {
    if (queue[i].tab === tab && gone.has(queue[i].token)) queue.splice(i, 1);
  }
  if (shown?.tab === tab && shown.tokens.some((token) => gone.has(token))) hide();
  later();
}

/** Убрать окно с экрана без ответа: оно вернётся, когда станет можно. */
function hide() {
  if (shown && openPopupKey() === shown.key) closePopup();
}

function later() {
  clearTimeout(timer);
  timer = setTimeout(show, RETURN_MS);
}

function show() {
  // Поверх другого попапа (меню, подсказки адресной строки) окно не встаёт:
  // оно дождётся, пока тот закроется.
  if (minimized || isPageHidden() || openPopupKey() !== null) return;
  // Окно страницы встаёт над своей вкладкой, если она на экране: активной или
  // второй половиной разделённого экрана.
  const visible = [state.activeId, state.splitId].filter((id) => id !== null && !state.tabs.get(id)?.internal);
  const first = queue.find((item) => visible.includes(item.tab));
  if (!first) return;
  const tab = state.tabs.get(first.tab);

  // Камера и микрофон приходят двумя запросами подряд — спрашиваем одним окном.
  const group =
    first.request.type === "permission"
      ? queue.filter(
          (item) =>
            item.tab === tab.id &&
            item.request.type === "permission" &&
            originOf(item.request.url) === originOf(first.request.url)
        )
      : [first];
  const menu = `dialog-${++seq}`;
  const tokens = group.map((item) => item.token);
  const mine = { tab: tab.id, key: `dialog:${menu}`, tokens };
  shown = mine;
  const payload = {
    menu,
    tab: tab.id,
    tokens,
    request: first.request,
    repeat: first.repeat,
    permissions: [...new Set(group.map((item) => item.request.permission))],
  };

  // Разрешение — пузырём у сведений о сайте, как в Chrome (если сайт —
  // активная вкладка); остальное — над страницей своей вкладки по центру.
  const site = document.getElementById("omni-site");
  const pane = paneRect(tab.id);
  const opening =
    first.request.type === "permission" && tab.id === state.activeId && site?.offsetParent
      ? openPopup("dialog", site, { width: 360, payload })
      : openPopup("dialog", stageAnchor(pane), { width: stageWidth(pane), payload });
  opening
    .then((opened) => {
      if (!opened && shown === mine) shown = null;
      else if (typeof opened === "number") mine.seq = opened;
    })
    .catch(() => {
      if (shown === mine) shown = null;
    });
}

/** Где на экране страница вкладки: всё место или своя половина разделённого экрана. */
function paneRect(id) {
  const rect = document.getElementById("stage").getBoundingClientRect();
  if (state.splitId === null) return { left: rect.left, top: rect.top, width: rect.width };
  const half = rect.width / 2;
  return { left: id === rightPaneId() ? rect.left + half : rect.left, top: rect.top, width: half };
}

function stageWidth(pane) {
  return Math.round(Math.max(280, Math.min(WIDTH, pane.width - 32)));
}

function stageAnchor(pane) {
  const width = stageWidth(pane);
  return { x: pane.left + (pane.width - width) / 2, y: pane.top + 8, width, height: 0 };
}

function originOf(url) {
  try {
    return new URL(url).origin;
  } catch {
    return url;
  }
}
