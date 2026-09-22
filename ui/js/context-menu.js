/**
 * Меню страницы по правому щелчку.
 *
 * Пункты и цель щелчка присылает движок (событие вкладки `context_menu`), а
 * рисует меню всплывающее окно — то же, что у остальных меню. Команды движка
 * («Копировать», «Сохранить картинку как», подсказки орфографии) выполняет сам
 * движок: попап отвечает ему номером команды. Свои пункты — поиск, перевод,
 * загрузчик видео, блокировка рекламы на сайте — выполняет окно браузера.
 *
 * Движок ждёт ответа на каждое меню. Закрытое без выбора меню закрывается и
 * для него, иначе следующий правый щелчок он бы не прислал.
 */

import { invoke, listen } from "./bridge.js";
import { hooks, openInNewWindow, openInPrivateWindow, openInSplit, openMediaExtension } from "./actions.js";
import { onPopupAction, openPopup } from "./popups.js";
import { pref } from "./prefs.js";
import { rightPaneId, state, tabIndex } from "./state.js";
import { open } from "./tabs.js";

/** Сочетания в нашей записи: у движка они длиннее («Alt+Стрелка влево»). */
const KEYS = {
  back: "Alt+←",
  forward: "Alt+→",
  reload: "Ctrl+R",
  saveAs: "Ctrl+S",
  print: "Ctrl+P",
  undo: "Ctrl+Z",
  redo: "Ctrl+Y",
  cut: "Ctrl+X",
  copy: "Ctrl+C",
  paste: "Ctrl+V",
  pasteAndMatchStyle: "Ctrl+Shift+V",
  selectAll: "Ctrl+A",
  inspectElement: "Ctrl+Shift+I",
  emoji: "Win+.",
};

const LABELS = {
  // Новое окно движка у нас открывается вкладкой.
  openLinkInNewWindow: "Открыть ссылку в новой вкладке",
  inspectElement: "Просмотреть код",
};

/** Пункты, которые ведут в окна и службы Edge, а не в браузер 190x4. */
const HIDDEN = new Set(["createQrCode", "openLinkInNewPrivateWindow", "share", "readAloud"]);
const HIDDEN_LABEL = /InPrivate|Microsoft|Edge|Copilot|QR/i;

let current = null;
let translate = () => {};
/**
 * «Открыть ссылку в новой вкладке» открывает её в фоне — как в Chrome.
 * Команду выполняет движок, вкладка приходит событием `popup`, поэтому
 * помечаем ожидание: ближайшее такое событие не переключает вкладку.
 */
let backgroundUntil = 0;

export function initContextMenu(options) {
  translate = options.translate;
  listen("popup-closed", () => answer(null));
  onPopupAction("context", ({ action, token }) => {
    if (!current || token !== current.menu) return;
    // Команду «Открыть ссылку в новой вкладке» выполняет движок, а попап лишь
    // предупреждает: вкладка, которая сейчас придёт, открыта из меню и
    // переключать на неё не нужно — так же ведёт себя Chrome.
    if (action === "background-tab") {
      backgroundUntil = performance.now() + 2000;
      return;
    }
    runAction(action, current);
  });
}

/** Движок прислал правый щелчок. */
export async function openContextMenu({ id, menu, x, y, target, items }) {
  answer(null);
  current = { tab: id, menu, target, site: null, answered: false };
  const menuState = current;

  // Меню бывает только у страницы на экране: активной или второй половины
  // разделённого экрана.
  if (id !== state.activeId && id !== state.splitId) {
    answer(null);
    return;
  }

  if (isPlainPage(target) && state.adblockOn) {
    menuState.site = await invoke("adblock_site", { url: target.page_url }).catch(() => null);
    if (current !== menuState) return;
  }

  const rows = buildRows(target, items, menuState.site);
  if (!rows.some((row) => !row.separator)) {
    answer(null);
    return;
  }

  const stage = document.getElementById("stage").getBoundingClientRect();
  const scale = window.devicePixelRatio || 1;
  // Точка щелчка — от левого края своей вкладки и уже в CSS-пикселях: движок
  // отдаёт её в логических единицах (на 150% делёная ещё раз на масштаб, она
  // уезжала к углу страницы). Правая половина разделённого экрана начинается
  // там же, где её ставит Rust (`TabHost::bounds_for`), — в физических.
  const offset = id === rightPaneId() ? paneOffset(stage.width * scale) / scale : 0;
  const point = { x: stage.left + offset + x, y: stage.top + y, width: 0, height: 0 };
  const opened = await openPopup("context", point, {
    width: 292,
    align: "point",
    // Своё имя у каждого меню: защита от повторного открытия того же попапа
    // сразу после закрытия приняла бы второй правый щелчок за «закрыть».
    payload: { menu: `page:${menu}`, tab: id, token: menu, rows },
  }).catch(() => false);
  if (!opened && current === menuState) answer(null);
}

/** Где начинается правая половина разделённого экрана, в физических пикселях. */
export function paneOffset(physicalWidth) {
  const GAP = 2;
  return Math.floor((Math.round(physicalWidth) - GAP) / 2) + GAP;
}

function answer(command) {
  if (!current || current.answered) return;
  current.answered = true;
  invoke("tab_context_menu", { id: current.tab, menu: current.menu, command }).catch(() => {});
}

function isPlainPage(target) {
  return (
    target.kind === "page" &&
    !target.link_url &&
    !target.editable &&
    /^https?:/.test(target.page_url) &&
    !target.page_url.startsWith("http://190x4-pages.invalid/")
  );
}

function buildRows(target, items, site) {
  const rows = [];
  for (const item of items) {
    if (item.kind === "separator") {
      rows.push({ separator: true });
      continue;
    }
    // Подменю движка («Другие инструменты») открывают окна Edge.
    if (item.kind === "submenu" || HIDDEN.has(item.name) || HIDDEN_LABEL.test(item.label)) continue;
    rows.push({
      id: `cmd:${item.command}`,
      name: item.name,
      command: item.command,
      label: LABELS[item.name] ?? cleanLabel(item.label),
      keys: KEYS[item.name] ?? item.shortcut ?? "",
      disabled: !item.enabled,
      checked: item.checked && (item.kind === "checkbox" || item.kind === "radio"),
    });
  }

  // Ссылку можно открыть не только вкладкой: окно, разделённый экран и
  // приватное окно — наши, движок про них не знает.
  const link = target.link_url ?? "";
  if (/^https?:/.test(link)) {
    const extra = [
      { id: "link-window", label: "Открыть ссылку в новом окне" },
      { id: "link-split", label: "Открыть ссылку в режиме разделения экрана" },
      { id: "link-private", label: "Открыть ссылку в приватном окне" },
    ];
    const after = rows.findIndex((row) => row.name === "openLinkInNewWindow");
    rows.splice(after >= 0 ? after + 1 : 0, 0, ...extra);
  }

  const source = target.source_url ?? "";
  if ((target.kind === "video" || target.kind === "audio") && /^https?:/.test(source) && pref("ext_media_enabled")) {
    rows.unshift({ id: "media", label: "Скачать через загрузчик 190x4" }, { separator: true });
  }

  const selection = (target.selection ?? "").trim();
  if (selection && !target.editable) {
    const short = selection.length > 24 ? `${selection.slice(0, 24).trimEnd()}…` : selection;
    const extra = [
      { id: "search", label: `Найти «${short.replace(/\s+/g, " ")}»` },
      { id: "translate", label: "Перевести выделенное" },
    ];
    const copy = rows.findIndex((row) => row.name === "copy");
    rows.splice(copy >= 0 ? copy + 1 : 0, 0, ...extra);
  }

  if (site?.site) {
    const row = {
      id: "adblock",
      label: site.blocking ? "Не блокировать рекламу на сайте" : "Блокировать рекламу на сайте",
    };
    const inspect = rows.findIndex((row) => row.name === "inspectElement");
    rows.splice(inspect >= 0 ? inspect : rows.length, 0, { separator: true }, row);
  }

  return tidySeparators(rows);
}

/** Амперсанд в подписи движка — подчёркнутая буква для клавиатуры, в нашем меню её нет. */
function cleanLabel(label) {
  return String(label ?? "").replace(/&(&?)/g, "$1");
}

function tidySeparators(rows) {
  const out = [];
  for (const row of rows) {
    if (row.separator && (!out.length || out[out.length - 1].separator)) continue;
    out.push(row);
  }
  while (out.length && out[out.length - 1].separator) out.pop();
  return out;
}

/** Ждёт ли интерфейс вкладку, которую открывать нужно в фоне. */
export function wantsBackgroundTab() {
  if (performance.now() > backgroundUntil) return false;
  backgroundUntil = 0;
  return true;
}

async function runAction(action, menu) {
  const { target } = menu;
  answer(null);

  const link = target.link_url ?? "";
  if (action === "link-window" && link) {
    openInNewWindow(link);
  } else if (action === "link-private" && link) {
    openInPrivateWindow(link);
  } else if (action === "link-split" && link) {
    openInSplit(link);
  } else if (action === "translate" && target.selection) {
    translate(target.selection);
  } else if (action === "search" && target.selection) {
    open(target.selection.trim().slice(0, 500), { index: tabIndex(menu.tab) + 1 });
  } else if (action === "media" && target.source_url) {
    openMediaExtension(target.source_url);
  } else if (action === "adblock" && menu.site?.site) {
    const blocking = !menu.site.blocking;
    try {
      await invoke("adblock_site_set", { url: target.page_url, blocking });
      hooks.toast(
        blocking ? `Реклама на ${menu.site.site} снова блокируется` : `Реклама на ${menu.site.site} больше не блокируется`
      );
      invoke("tab_action", { id: menu.tab, action: "reload" }).catch(() => {});
    } catch (error) {
      hooks.toast(String(error?.message ?? error));
    }
  }
}
