/**
 * Всплывающее окно браузера: меню, пузыри, расширение «Загрузчик видео».
 *
 * Окно одно и живёт всё время работы браузера, скрываясь между показами.
 * Окно браузера присылает `popup-render` с видом и данными; попап рисует,
 * меряет себя и просит Rust показать его нужной высоты. Выбор пользователя
 * уходит обратно событием `popup-action`.
 */

import { emit, invoke, isNative, listen } from "../bridge.js";
import {
  el,
  favicon,
  fileIcon,
  formatBytes,
  formatDay,
  hostOf,
  icon,
  iconButton,
  plural,
  textButton,
} from "../dom.js";
import * as model from "../downloads-model.js";
import { ONCE, PERMISSIONS } from "../permissions.js";
import { applyTheme, loadPrefs, onPref } from "../prefs.js";

const root = document.getElementById("popup");
const ZOOM_STEPS = [0.25, 0.33, 0.5, 0.67, 0.75, 0.8, 0.9, 1, 1.1, 1.25, 1.5, 1.75, 2, 2.5, 3, 4, 5];

let current = null;
let visible = false;

await loadPrefs();
applyTheme();
// Тема — по подписке на настройки: она срабатывает, когда значение уже
// обновлено. Свой слушатель события мог успеть раньше, и попап отставал от
// окна браузера на одну смену темы.
onPref((key) => {
  if (key === "theme") applyTheme();
});
matchMedia("(prefers-color-scheme: dark)").addEventListener("change", applyTheme);

listen("popup-render", (message) => render(message));
listen("popup-closed", () => {
  visible = false;
  current?.cleanup?.();
  current = null;
});

const pending = await invoke("popup_pending").catch(() => null);
if (pending) render(pending);

document.addEventListener("contextmenu", (event) => {
  if (!event.target.closest("input, textarea")) event.preventDefault();
});
document.addEventListener("keydown", onKey);

if (!isNative) demo();

/* ── Жизненный цикл ────────────────────────────────────────── */

function render({ kind, payload, reuse = false }) {
  // Тот же попап на прежнем месте (подсказки на каждую клавишу): окно уже на
  // экране, нужно только подогнать высоту, а не показывать его заново.
  const keep = reuse && visible && current?.kind === kind;
  current?.cleanup?.();
  const view = VIEWS[kind];
  if (!view) return;
  root.replaceChildren();
  root.className = `popup popup--${kind}`;
  current = { kind, payload, cleanup: null };
  current.cleanup = view(payload ?? {}) ?? null;
  visible = keep;
  requestAnimationFrame(fit);
}

/**
 * Подогнать окно под содержимое; первый вызов после рендера — показать.
 *
 * Меряем естественную высоту без ограничений: окно в этот момент ещё
 * прежнего размера, и всё, что привязано к нему (100vh), дало бы его высоту,
 * а не высоту содержимого. Rust возвращает высоту, которая досталась окну, —
 * по ней прокручиваемые части сжимаются, а не обрезаются.
 */
// Показ и подгонка — отдельные команды, и их ответы приходят в любом порядке.
// Ответ на устаревший замер, пришедший последним, оставлял окно прежней высоты:
// содержимое, дорисованное после загрузки (папка закладок), обрезалось.
// Поэтому замеры идут очередью, а из скопившихся выполняется только последний.
let fitting = Promise.resolve();
let fitRequest = 0;

function fit() {
  const request = ++fitRequest;
  fitting = fitting.then(() => (request === fitRequest ? fitNow() : undefined));
}

async function fitNow() {
  root.style.maxHeight = "";
  const height = Math.ceil(root.getBoundingClientRect().height);
  if (!isNative) return;
  try {
    let applied;
    if (visible) {
      applied = await invoke("popup_resize", { height });
    } else {
      visible = true;
      // Подсказки адресной строки не забирают фокус: курсор остаётся в строке.
      applied = await invoke("popup_show", { height, focus: current?.kind !== "suggest" });
      root.querySelector("[autofocus]")?.focus();
    }
    if (Number.isFinite(applied)) root.style.maxHeight = `${applied}px`;
  } catch {
    // Окно попапа уже закрыто — подгонять нечего.
  }
}

function close() {
  current?.cleanup?.();
  current = null;
  visible = false;
  if (isNative) invoke("popup_hide").catch(() => {});
}

/** Сообщить окну браузера о выборе. */
function act(kind, action, extra = {}, { keepOpen = false } = {}) {
  emit("popup-action", { kind, action, ...extra });
  if (!keepOpen) close();
}

function onKey(event) {
  // Меню поверх списка (правый клик по закладке): клавиши сначала ему.
  const float = root.querySelector(".menu--float");
  if (event.key === "Escape") {
    event.preventDefault();
    if (float) float.dispatchEvent(new Event("dismiss"));
    // Окно страницы ждёт ответа: Escape — это «Отмена», а не просто закрыть.
    else if (current?.onEscape) current.onEscape();
    else close();
    return;
  }
  const items = [...(float ?? root).querySelectorAll(".menu__item:not([disabled])")];
  if (!items.length || !["ArrowDown", "ArrowUp", "Enter"].includes(event.key)) return;
  if (event.target.matches("input, select")) return;

  const index = items.findIndex((item) => item.dataset.selected === "true");
  if (event.key === "Enter") {
    if (index >= 0) {
      event.preventDefault();
      items[index].click();
    }
    return;
  }
  event.preventDefault();
  const next = event.key === "ArrowDown" ? (index + 1) % items.length : (index - 1 + items.length) % items.length;
  items.forEach((item, i) => (item.dataset.selected = String(i === next)));
  items[next].scrollIntoView({ block: "nearest" });
}

function sizeOf(id) {
  return /-(16|12)(-filled)?$/.test(id) ? 16 : 20;
}

/* ── Виды ──────────────────────────────────────────────────── */

const VIEWS = {
  /** Обычное меню: пункты, разделители, строка масштаба, флажки. */
  menu({ menu, items = [] }) {
    const kind = `menu:${menu}`;
    const list = el("div", "menu scroll");

    for (const item of items) {
      if (item.separator) {
        list.append(el("div", "menu__sep"));
        continue;
      }
      if (item.type === "header") {
        list.append(el("div", "menu__head", item.label));
        continue;
      }
      if (item.type === "zoom") {
        list.append(zoomRow(kind, item));
        continue;
      }

      const row = el("button", "menu__item");
      row.type = "button";
      row.disabled = Boolean(item.disabled);
      const slot = el("span", "menu__icon");
      if (item.image) slot.append(favicon(item.image));
      else if (item.icon) slot.append(icon(item.icon, sizeOf(item.icon)));
      row.append(slot, el("span", "menu__label", item.label));

      if (item.checked) row.append(icon("checkmark-16", 16, "menu__check"));
      else if (item.keys) row.append(el("span", "menu__keys", item.keys));

      if (item.trailing) {
        const pinned = item.trailing === "pinned";
        const pin = iconButton(pinned ? "pin-16-filled" : "pin-16", pinned ? "Открепить от панели" : "Закрепить на панели", () => {
          act(kind, `${item.id}:pin`);
        });
        if (pinned) pin.style.color = "var(--accent-bright)";
        row.append(pin);
      }

      row.addEventListener("mouseenter", () => {
        for (const other of list.querySelectorAll(".menu__item")) other.dataset.selected = "false";
        row.dataset.selected = "true";
      });
      row.addEventListener("click", () => act(kind, item.id));
      list.append(row);
    }
    root.append(list);
  },

  /** Пузырь загрузок под кнопкой на панели инструментов. */
  downloads() {
    const head = el("div", "panel-head");
    head.append(
      el("span", "panel-head__title", "Загрузки"),
      iconButton("folder-open", "Открыть папку загрузок", () => invoke("downloads_folder_open").catch(() => {}), {
        size: 20,
        className: "btn btn--ghost btn--icon",
      }),
      iconButton("more", "Страница загрузок", () => act("downloads", "open-page"), {
        size: 20,
        className: "btn btn--ghost btn--icon",
      })
    );

    const list = el("div", "dl-list scroll");
    const foot = el("div", "panel-foot");
    const all = textButton("Все загрузки", () => act("downloads", "open-page"), "btn btn--ghost btn--sm");
    foot.append(el("span"), all);
    root.append(head, list, foot);

    const rows = new Map();
    let order = "";

    const draw = () => {
      const items = model.downloads().slice(0, 8);
      const key = items.map((item) => `${item.id}:${item.state}`).join(",");
      if (key !== order) {
        order = key;
        rows.clear();
        list.replaceChildren();
        if (!items.length) list.append(el("div", "empty", "Здесь появятся файлы, которые вы скачаете"));
        for (const item of items) {
          const row = downloadRow(item);
          rows.set(item.id, row);
          list.append(row.node);
        }
        fit();
      }
      for (const item of items) updateDownloadRow(rows.get(item.id), item);
    };

    const off = model.onDownloads(() => requestAnimationFrame(draw));
    model.initDownloads().then(draw);
    draw();
    return off;
  },

  /** Пузырь обновления: версия, заметки к ней, установка с прогрессом. */
  update({ version, current, notes = "", date, installing = false }) {
    const bubble = el("div", "bubble");
    const head = el("div", "bubble__head");
    head.append(
      el("h2", "bubble__title", "Доступно обновление"),
      iconButton("dismiss-16", "Закрыть", close, { className: "bubble__close" })
    );
    bubble.append(head);
    bubble.append(
      el("p", "bubble__text", `190x4 Browser ${version}${date ? ` от ${formatDay(date)}` : ""}. Сейчас установлена версия ${current}.`)
    );

    const lines = notes
      .split("\n")
      .map((line) => line.trim())
      .filter((line) => line && !/^#{1,6}\s/.test(line));
    if (lines.length) {
      const list = el("div", "update-notes scroll");
      for (const line of lines) list.append(el("p", null, line.replace(/^[-*]\s+/, "")));
      bubble.append(list);
    }

    const status = el("div", "update-status");
    const meter = el("div", "meter");
    const fill = el("div", "meter__fill");
    meter.append(fill);
    const label = el("div", "update-status__label", "Скачивание…");
    status.append(meter, label);
    status.hidden = !installing;
    bubble.append(status);

    const actions = el("div", "bubble__actions");
    const later = textButton("Позже", close, "btn");
    const start = () => {
      primary.disabled = true;
      later.disabled = true;
      status.hidden = false;
      fit();
    };
    const primary = textButton(
      "Обновить и перезапустить",
      () => {
        start();
        act("update", "install", {}, { keepOpen: true });
      },
      "btn btn--primary"
    );
    primary.setAttribute("autofocus", "");
    actions.append(later, primary);
    bubble.append(actions);
    root.append(bubble);
    if (installing) start();

    const off = listen("update-progress", ({ phase, downloaded, total }) => {
      if (status.hidden) start();
      if (phase === "install") {
        fill.style.width = "100%";
        label.textContent = "Установка — браузер перезапустится";
        return;
      }
      fill.style.width = total ? `${Math.round(Math.min(1, downloaded / total) * 100)}%` : "30%";
      label.textContent = total
        ? `Скачано ${formatBytes(downloaded)} из ${formatBytes(total)}`
        : `Скачано ${formatBytes(downloaded)}`;
    });
    return () => off.then((stop) => stop?.());
  },

  /** Пузырь закладки: название, папка, удалить — как в Chrome по Ctrl+D. */
  bookmark({ mode, node, created, folders = [], parent }) {
    const bubble = el("div", "bubble");
    const head = el("div", "bubble__head");
    const title =
      mode === "folder" ? "Новая папка" : node?.kind === "folder" ? "Переименовать папку" : created ? "Закладка добавлена" : "Изменить закладку";
    head.append(el("h2", "bubble__title", title), iconButton("dismiss-16", "Закрыть", close, { className: "bubble__close" }));
    bubble.append(head);

    const nameField = el("input", "field");
    nameField.value = mode === "folder" ? "Новая папка" : node?.title ?? "";
    nameField.setAttribute("autofocus", "");
    const nameRow = el("div", "form__row");
    nameRow.append(el("label", "label", "Название"), nameField);
    bubble.append(nameRow);

    const folderSelect = el("select", "field");
    const selectedParent = mode === "folder" ? parent : node?.parent_id;
    for (const folder of folders) {
      if (node?.kind === "folder" && folder.id === node.id) continue;
      const option = el("option", null, folder.title);
      option.value = folder.id;
      option.selected = folder.id === selectedParent;
      folderSelect.append(option);
    }
    const folderRow = el("div", "form__row");
    folderRow.append(el("label", "label", "Папка"), folderSelect);
    bubble.append(folderRow);

    let finished = false;
    const save = async () => {
      finished = true;
      const titleValue = nameField.value.trim();
      if (mode === "folder") {
        if (titleValue) await invoke("bookmark_folder_add", { parent: Number(folderSelect.value), title: titleValue }).catch(() => {});
      } else if (node) {
        await invoke("bookmark_update", {
          id: node.id,
          title: titleValue || node.title,
          url: null,
          parent: Number(folderSelect.value),
        }).catch(() => {});
      }
      close();
    };

    const actions = el("div", "bubble__actions");
    if (mode !== "folder" && node) {
      actions.append(
        textButton(
          "Удалить",
          async () => {
            finished = true;
            await invoke("bookmark_remove", { id: node.id }).catch(() => {});
            close();
          },
          "btn btn--ghost"
        ),
        textButton("Ещё…", () => act("bookmark", "manage"), "btn")
      );
    } else {
      actions.append(textButton("Отмена", close, "btn btn--ghost"));
    }
    actions.append(textButton(mode === "folder" ? "Создать" : "Готово", save, "btn btn--primary"));
    bubble.append(actions);

    bubble.addEventListener("keydown", (event) => {
      if (event.key === "Enter" && event.target.tagName !== "BUTTON") {
        event.preventDefault();
        save();
      }
    });
    root.append(bubble);

    // Пузырь закрыли щелчком мимо — правки не теряем, как в Chrome.
    return () => {
      if (finished || mode === "folder" || !node) return;
      const titleValue = nameField.value.trim();
      const parentValue = Number(folderSelect.value);
      if ((titleValue && titleValue !== node.title) || parentValue !== node.parent_id) {
        invoke("bookmark_update", { id: node.id, title: titleValue || node.title, url: null, parent: parentValue }).catch(() => {});
      }
    };
  },

  /** Расширение «Загрузчик видео 190x4». */
  media(payload) {
    return mediaView(payload);
  },

  /** «Сохранить пароль?» после входа на сайт. */
  password({ tab, origin, username, update }) {
    const bubble = el("div", "bubble");
    const head = el("div", "bubble__head");
    head.append(
      el("h2", "bubble__title", update ? "Обновить пароль?" : "Сохранить пароль?"),
      iconButton("dismiss-16", "Закрыть", close, { className: "bubble__close" })
    );
    bubble.append(head);
    bubble.append(
      el(
        "p",
        "bubble__text",
        update
          ? "Пароль для этого логина изменился. Сохранённый будет заменён новым."
          : "Пароль будет зашифрован и подставится при следующем входе."
      )
    );

    const site = el("div", "site");
    site.append(favicon(`${origin}/favicon.ico`), el("span", null, hostOf(origin)));
    bubble.append(site);

    const userRow = el("div", "form__row");
    const userField = el("input", "field");
    userField.value = username || "без логина";
    userField.readOnly = true;
    const secret = el("input", "field field--mono");
    secret.value = "••••••••••";
    secret.readOnly = true;
    userRow.append(el("label", "label", "Имя пользователя"), userField);
    const secretRow = el("div", "form__row");
    secretRow.append(el("label", "label", "Пароль"), secret);
    bubble.append(el("div", "form__row"), userRow, secretRow);

    const answer = async (action) => {
      await invoke("password_offer_answer", { tab, action }).catch(() => {});
      act("password", action);
    };
    const actions = el("div", "bubble__actions");
    if (!update) {
      const never = textButton("Никогда", () => answer("never"), "btn btn--ghost");
      never.title = "Никогда не сохранять пароли для этого сайта";
      actions.append(never);
    }
    actions.append(textButton("Не сейчас", () => answer("dismiss"), "btn"));
    const primary = textButton(update ? "Обновить" : "Сохранить", () => answer("save"), "btn btn--primary");
    primary.setAttribute("autofocus", "");
    actions.append(primary);
    bubble.append(actions);
    root.append(bubble);
  },

  /** Ключ в адресной строке: выбрать учётку для входа. */
  accounts({ tab, origin, accounts = [] }) {
    const head = el("div", "panel-head");
    head.append(el("span", "panel-head__title", `Пароли · ${hostOf(origin)}`));
    const list = el("div", "menu");
    for (const account of accounts) {
      const row = el("button", "menu__item");
      const slot = el("span", "menu__icon");
      slot.append(icon("person-16", 16));
      // Учётка другого адреса того же сайта подписана своим адресом.
      const from = account.origin && account.origin !== origin ? hostOf(account.origin) : "заполнить";
      row.append(slot, el("span", "menu__label", account.username || "без логина"), el("span", "menu__keys", from));
      row.addEventListener("click", async () => {
        await invoke("password_fill", { tab, id: account.id }).catch(() => {});
        close();
      });
      list.append(row);
    }
    list.append(el("div", "menu__sep"));
    const manage = el("button", "menu__item");
    const slot = el("span", "menu__icon");
    slot.append(icon("key", 20));
    manage.append(slot, el("span", "menu__label", "Управление паролями"));
    manage.addEventListener("click", () => act("accounts", "manage"));
    list.append(manage);
    root.append(head, list);
  },

  /** Сведения о сайте из значка слева в адресной строке. */
  site({ url, host, secure, blocked, adblock, site, blocking = true, passwords }) {
    const bubble = el("div", "bubble");
    const head = el("div", "bubble__head");
    head.append(el("h2", "bubble__title", host), iconButton("dismiss-16", "Закрыть", close, { className: "bubble__close" }));
    bubble.append(head);

    const list = el("div", "menu");
    list.style.padding = "0";
    list.style.margin = "0 -8px";
    const info = (iconId, label, hint, action) => {
      const row = el(action ? "button" : "div", "menu__item");
      row.style.height = "auto";
      row.style.padding = "8px 10px";
      const slot = el("span", "menu__icon");
      slot.append(icon(iconId, sizeOf(iconId)));
      const text = el("span", "menu__label");
      text.append(el("div", null, label));
      if (hint) {
        const hintNode = el("div", null, hint);
        hintNode.style.cssText = "font-size:12px;color:var(--text-lo);white-space:normal";
        text.append(hintNode);
      }
      row.append(slot, text);
      if (action) {
        row.append(icon("chevron-right-16", 16));
        row.addEventListener("click", () => act("site", action));
      }
      list.append(row);
    };

    info(
      secure ? "lock-16" : "warning-16",
      secure ? "Подключение защищено" : "Подключение не защищено",
      secure
        ? "Данные, которые вы вводите, передаются в зашифрованном виде."
        : "Не вводите здесь пароли и данные карт: их могут перехватить."
    );
    if (adblock && site) {
      list.append(siteBlockingRow({ url, site, blocked, blocking }));
    } else {
      info("shield-16", "Блокировка рекламы выключена", "На всех сайтах — включается в настройках", "privacy");
    }
    info(
      "key-16",
      passwords ? `Сохранено паролей: ${passwords}` : "Паролей для сайта нет",
      null,
      "passwords"
    );
    bubble.append(list);
    root.append(bubble);
  },
};

/** Строка «блокировка на этом сайте» с переключателем: исключение хранится в
 *  настройках, страница перезагружается уже с новым правилом. */
function siteBlockingRow({ url, site, blocked, blocking }) {
  const row = el("div", "menu__item");
  row.style.height = "auto";
  row.style.padding = "8px 10px";
  const slot = el("span", "menu__icon");
  slot.append(icon("shield-16", 16));
  const text = el("span", "menu__label");
  const title = el("div");
  const hint = el("div");
  hint.style.cssText = "font-size:12px;color:var(--text-lo);white-space:normal";
  text.append(title, hint);

  const toggleNode = el("button", "switch");
  toggleNode.type = "button";
  toggleNode.setAttribute("aria-label", "Блокировать рекламу на этом сайте");
  let on = blocking;
  const paint = () => {
    title.textContent = on
      ? `Заблокировано: ${blocked} ${plural(blocked, "запрос", "запроса", "запросов")}`
      : "Реклама на сайте не блокируется";
    hint.textContent = on ? `Реклама и трекеры на ${site}` : `Исключение для ${site} и его поддоменов`;
    toggleNode.setAttribute("aria-checked", String(on));
  };
  paint();
  toggleNode.addEventListener("click", async () => {
    toggleNode.disabled = true;
    try {
      const result = await invoke("adblock_site_set", { url, blocking: !on });
      on = result.blocking;
      paint();
      act("site", "reload", {}, { keepOpen: true });
    } catch {
      // Страница не сайт или настройки не записались — переключатель остаётся как был.
    } finally {
      toggleNode.disabled = false;
    }
  });
  row.append(slot, text, toggleNode);
  return row;
}

/** Меню страницы по правому щелчку: пункты собирает окно браузера
 *  (`context-menu.js`). Команду движка попап отдаёт движку сам и только потом
 *  закрывается — иначе закрытие успело бы ответить «без выбора». */
/** Пузырь группы вкладок: имя, цвет и действия над всей группой. */
VIEWS.group = function group({ group: data = {}, tabs = 0, colors = [] }) {
  const box = el("div", "bubble");
  box.append(el("div", "bubble__title", "Группа вкладок"));

  const name = el("input", "field");
  name.type = "text";
  name.placeholder = "Название группы";
  name.value = data.title ?? "";
  name.maxLength = 40;
  name.setAttribute("autofocus", "true");
  // Имя применяется по ходу набора: отдельной кнопки «Сохранить» в Chrome нет.
  name.addEventListener("input", () =>
    act("group", "rename", { id: data.id, title: name.value }, { keepOpen: true })
  );
  name.addEventListener("keydown", (event) => {
    if (event.key === "Enter") close();
  });
  box.append(name);

  const palette = el("div", "palette");
  for (const [id, label] of colors) {
    const dot = el("button", "palette__dot");
    dot.type = "button";
    dot.dataset.color = id;
    dot.title = label;
    dot.setAttribute("aria-pressed", String(id === data.color));
    dot.addEventListener("click", () => {
      for (const other of palette.children) other.setAttribute("aria-pressed", String(other === dot));
      act("group", "color", { id: data.id, color: id }, { keepOpen: true });
    });
    palette.append(dot);
  }
  box.append(palette);

  const actions = el("div", "menu");
  for (const [action, label, glyph] of [
    ["collapse", data.collapsed ? "Развернуть группу" : "Свернуть группу", "chevron-down-16"],
    ["ungroup", "Разгруппировать", "dismiss-16"],
    ["close", `Закрыть группу (${tabs})`, "delete-16"],
  ]) {
    const item = el("button", "menu__item");
    item.type = "button";
    item.append(icon(glyph, sizeOf(glyph)), el("span", "menu__label", label));
    item.addEventListener("click", () => act("group", action, { id: data.id }));
    actions.append(item);
  }
  box.append(actions);
  return box;
};

VIEWS.context = function context({ tab, token, rows = [] }) {
  const list = el("div", "menu menu--context scroll");
  for (const row of rows) {
    if (row.separator) {
      list.append(el("div", "menu__sep"));
      continue;
    }
    const node = el("button", "menu__item");
    node.type = "button";
    node.disabled = Boolean(row.disabled);
    node.append(el("span", "menu__label", row.label));
    if (row.checked) node.append(icon("checkmark-16", 16, "menu__check"));
    else if (row.keys) node.append(el("span", "menu__keys", row.keys));
    node.addEventListener("mouseenter", () => {
      for (const other of list.querySelectorAll(".menu__item")) other.dataset.selected = "false";
      node.dataset.selected = "true";
    });
    node.addEventListener("click", async () => {
      if (row.command == null) {
        act("context", row.id, { tab, token });
        return;
      }
      if (isNative) await invoke("tab_context_menu", { id: tab, menu: token, command: row.command }).catch(() => {});
      close();
    });
    list.append(node);
  }
  root.append(list);
};

/** Подсказки адресной строки. Клавиатура — у окна браузера, сюда приходит
 *  только выбранная строка. */
VIEWS.suggest = function suggest({ rows = [], selected = 0 }) {
  const list = el("div", "suggest");
  rows.forEach((row, index) => {
    const node = el("button", "suggest__row");
    node.type = "button";
    node.dataset.selected = String(index === selected);
    const iconNode = row.image ? favicon(row.image, "favicon suggest__icon") : icon(row.iconId, 16, "suggest__icon");
    node.append(iconNode, el("span", "suggest__text", row.text), el("span", "suggest__hint", row.hint));
    // mousedown, а не click: на клике фокус уже ушёл бы из адресной строки.
    node.addEventListener("mousedown", (event) => {
      event.preventDefault();
      act("suggest", "pick", { value: row.value });
    });
    list.append(node);
  });
  root.append(list);

  const off = listen("suggest-select", ({ selected: index }) => {
    [...list.children].forEach((node, i) => (node.dataset.selected = String(i === index)));
  });
  return () => off.then((stop) => stop?.());
};

/** Содержимое папки закладок: меню с переходом во вложенные папки. */
VIEWS["bookmark-folder"] = function bookmarkFolder({ folder, skip = 0, title }) {
  const stack = [{ id: folder, skip, title }];
  let nodes = [];
  // Меню закладки по правому клику. Второе всплывающее окно поверх этого не
  // открыть, поэтому меню рисуется внутри списка, у курсора.
  let menu = null;
  let swallowClick = false;

  const closeMenu = () => {
    if (!menu) return;
    menu.remove();
    menu = null;
    root.style.minHeight = "";
    fit();
  };

  const pick = async (node, action) => {
    closeMenu();
    if (["open", "open-new", "open-window", "open-split", "open-private"].includes(action)) {
      act("bookmark-folder", action, { url: node.url });
    } else if (action === "edit") {
      act("bookmark-folder", "edit", { id: node.id });
    } else if (action === "remove") {
      await invoke("bookmark_remove", { id: node.id }).catch(() => {});
      nodes = await invoke("bookmarks_tree").catch(() => nodes);
      draw();
    }
  };

  const openMenu = (node, event) => {
    closeMenu();
    const items =
      node.kind === "url"
        ? [
            // Тот же порядок, что и в меню панели закладок: «Открыть» здесь
            // не нужно — для этого достаточно щелчка по самой закладке.
            ["open-new", "Открыть в новой вкладке", "tab-add"],
            ["open-window", "Открыть в новом окне", "window-16"],
            ["open-split", "Открыть в режиме разделения экрана", "split-16"],
            ["open-private", "Открыть в приватном окне", "private-16"],
            null,
            ["edit", "Изменить…", "edit-16"],
            ["remove", "Удалить", "delete-16"],
          ]
        : [["edit", "Переименовать…", "edit-16"], ["remove", "Удалить", "delete-16"]];
    menu = el("div", "menu menu--float");
    menu.setAttribute("role", "menu");
    for (const item of items) {
      if (!item) {
        menu.append(el("div", "menu__sep"));
        continue;
      }
      const [id, label, iconId] = item;
      const row = el("button", "menu__item");
      row.type = "button";
      const slot = el("span", "menu__icon");
      slot.append(icon(iconId, sizeOf(iconId)));
      row.append(slot, el("span", "menu__label", label));
      row.addEventListener("mouseenter", () => {
        for (const other of menu.querySelectorAll(".menu__item")) other.dataset.selected = "false";
        row.dataset.selected = "true";
      });
      row.addEventListener("click", () => pick(node, id));
      menu.append(row);
    }
    menu.addEventListener("dismiss", closeMenu);
    root.append(menu);

    // Короткий список ниже меню — окно подрастает под него.
    const { offsetWidth: width, offsetHeight: height } = menu;
    const need = height + 8;
    if (root.clientHeight < need) {
      root.style.minHeight = `${need}px`;
      fit();
    }
    const room = Math.max(root.clientHeight, need);
    const left = Math.max(4, Math.min(event.clientX, root.clientWidth - width - 4));
    const top = event.clientY + height + 4 <= room ? event.clientY : Math.max(4, event.clientY - height);
    menu.style.left = `${left}px`;
    menu.style.top = `${top}px`;
  };

  // Щелчок мимо меню только закрывает его и не открывает закладку под курсором.
  root.addEventListener(
    "mousedown",
    (event) => {
      if (!menu || menu.contains(event.target)) return;
      swallowClick = event.button === 0;
      closeMenu();
    },
    true
  );
  root.addEventListener(
    "click",
    (event) => {
      if (!swallowClick) return;
      swallowClick = false;
      event.preventDefault();
      event.stopPropagation();
    },
    true
  );

  const draw = () => {
    const level = stack[stack.length - 1];
    const children = nodes
      .filter((node) => node.parent_id === level.id)
      .sort((a, b) => a.position - b.position)
      .slice(level.skip);

    menu = null;
    root.style.minHeight = "";
    root.replaceChildren();
    const list = el("div", "menu scroll");
    list.style.maxHeight = "480px";
    list.addEventListener("scroll", closeMenu);

    if (stack.length > 1) {
      const headRow = el("div", "menu__head");
      const back = iconButton("back", "Назад", () => {
        stack.pop();
        draw();
      }, { size: 20 });
      headRow.append(back, el("span", null, level.title));
      list.append(headRow);
    }

    if (!children.length) list.append(el("div", "empty", "Папка пуста"));

    for (const node of children) {
      const row = el("button", "menu__item");
      row.type = "button";
      const slot = el("span", "menu__icon");
      if (node.kind === "folder") {
        slot.append(icon("folder-16", 16));
        row.append(slot, el("span", "menu__label", node.title), icon("chevron-right-16", 16));
        row.addEventListener("click", () => {
          stack.push({ id: node.id, skip: 0, title: node.title });
          draw();
        });
      } else {
        slot.append(favicon(node.icon));
        row.append(slot, el("span", "menu__label", node.title || node.url));
        row.title = node.url;
        row.addEventListener("click", (event) =>
          act("bookmark-folder", event.ctrlKey ? "open-new" : "open", { url: node.url })
        );
        row.addEventListener("auxclick", (event) => {
          if (event.button === 1) act("bookmark-folder", "open-new", { url: node.url }, { keepOpen: true });
        });
      }
      row.addEventListener("contextmenu", (event) => {
        event.preventDefault();
        openMenu(node, event);
      });
      row.addEventListener("mouseenter", () => {
        for (const other of list.querySelectorAll(".menu__item")) other.dataset.selected = "false";
        row.dataset.selected = "true";
      });
      list.append(row);
    }

    const urls = children.filter((node) => node.kind === "url").map((node) => node.url);
    if (urls.length > 1) {
      list.append(el("div", "menu__sep"));
      const all = el("button", "menu__item");
      const slot = el("span", "menu__icon");
      slot.append(icon("tab-add", 20));
      all.append(slot, el("span", "menu__label", `Открыть все (${urls.length})`));
      all.addEventListener("click", () => act("bookmark-folder", "open-all", { urls }));
      list.append(all);
    }
    root.append(list);
    fit();
  };

  invoke("bookmarks_tree")
    .then((items) => {
      nodes = items;
      draw();
    })
    .catch(() => {});
};

/* ── Окна страниц ──────────────────────────────────────────── */

/** Кнопки окон, которые страница может подсунуть под щелчок, оживают не сразу:
 *  иначе сайт открыл бы окно ровно под курсором, и двойной щелчок по странице
 *  разрешил бы камеру или запустил программу. Столько же ждёт Chrome. */
const GUARD_MS = 500;

/** Окно, которое просит страница. Ответ уходит движку через Rust; Escape —
 *  «Отмена». Закрытое без ответа окно (переключили вкладку, открыли меню) окно
 *  браузера покажет снова (`dialogs.js`). */
VIEWS.dialog = function dialog({ tab, tokens = [], request = {}, permissions = [], repeat = false }) {
  const bubble = el("div", "bubble dialog");
  const shownAt = performance.now();
  let answered = false;
  const answer = async (action, extra = {}, { guard = false } = {}) => {
    if (answered || (guard && performance.now() - shownAt < GUARD_MS)) return;
    answered = true;
    if (isNative) await invoke("tab_dialog", { id: tab, tokens, answer: { action, ...extra } }).catch(() => {});
    act("dialog", "answered", { tab, tokens });
  };
  const build = DIALOGS[request.type];
  if (!build) {
    answer("cancel");
    return;
  }
  current.onEscape = build(bubble, request, { answer, permissions, repeat });
  root.append(bubble);
};

const DIALOGS = {
  /** alert, confirm, prompt и «Покинуть сайт?». */
  script(bubble, { kind, url = "", message = "", default_text: defaultText = "" }, { answer, repeat }) {
    const leave = kind === "beforeunload";
    const host = hostOf(url);
    bubble.append(dialogHead(leave ? "Покинуть сайт?" : host ? `Сайт ${host} сообщает` : "Страница сообщает"));
    if (leave) bubble.append(el("p", "bubble__text", "Изменения, которые вы внесли, могут не сохраниться."));
    else if (message) bubble.append(el("p", "dialog__message scroll", message));

    let input = null;
    if (kind === "prompt") {
      input = el("input", "field");
      input.value = defaultText;
      input.spellcheck = false;
      input.setAttribute("autofocus", "");
      bubble.append(input);
    }
    const suppress = repeat && !leave ? checkbox("Запретить этой странице показывать новые окна") : null;
    if (suppress) bubble.append(suppress.node);

    const reply = (action) => answer(action, { text: input?.value ?? "", suppress: suppress?.input.checked ?? false });
    const actions = el("div", "bubble__actions");
    if (kind !== "alert") actions.append(textButton(leave ? "Остаться" : "Отмена", () => reply("cancel"), "btn"));
    const ok = textButton(leave ? "Покинуть" : "ОК", () => reply("accept"), "btn btn--primary");
    if (!input) ok.setAttribute("autofocus", "");
    actions.append(ok);
    bubble.append(actions);
    input?.addEventListener("keydown", (event) => {
      if (event.key === "Enter") {
        event.preventDefault();
        reply("accept");
      }
    });
    // У alert одна кнопка: Escape его просто закрывает.
    return () => reply(kind === "alert" ? "accept" : "cancel");
  },

  /** Запрос разрешения. Несколько запросов одного сайта — одним окном. */
  permission(bubble, { url = "" }, { answer, permissions }) {
    const host = hostOf(url);
    const head = dialogHead(host ? `Сайт ${host} запрашивает разрешение` : "Страница запрашивает разрешение");
    head.append(iconButton("dismiss-16", "Закрыть", () => answer("cancel"), { className: "bubble__close" }));
    bubble.append(head);

    const list = el("ul", "dialog__permissions");
    for (const name of permissions) {
      const info = PERMISSIONS[name];
      if (!info) continue;
      const item = el("li");
      item.append(icon(info.icon, 20), el("span", null, info.ask));
      list.append(item);
    }
    bubble.append(list);

    const stack = el("div", "dialog__stack");
    const option = (label, action) => stack.append(textButton(label, () => answer(action, {}, { guard: true }), "btn"));
    option("Разрешить при посещении сайта", "allow");
    if (permissions.every((name) => ONCE.has(name))) option("Разрешить в этот раз", "allow_once");
    option("Никогда не разрешать", "deny");
    bubble.append(stack);
    return () => answer("cancel");
  },

  /** Сайт или прокси требует имя и пароль. */
  auth(bubble, { url = "" }, { answer }) {
    bubble.append(dialogHead("Вход на сайт"));
    const secure = url.startsWith("https:");
    const site = el("div", "site");
    site.append(icon(secure ? "lock-16" : "warning-16", 16), el("span", null, hostOf(url) || url));
    bubble.append(site);
    if (!secure) bubble.append(el("p", "dialog__note", "Подключение не защищено: имя и пароль можно перехватить."));

    const user = el("input", "field");
    user.spellcheck = false;
    user.autocomplete = "off";
    user.setAttribute("autofocus", "");
    const secret = el("input", "field");
    secret.type = "password";
    const userRow = el("div", "form__row");
    userRow.append(el("label", "label", "Имя пользователя"), user);
    const secretRow = el("div", "form__row");
    secretRow.append(el("label", "label", "Пароль"), secret);
    bubble.append(el("div", "form__row"), userRow, secretRow);

    const submit = () => {
      if (!user.value && !secret.value) {
        user.focus();
        return;
      }
      answer("accept", { username: user.value, password: secret.value });
    };
    for (const input of [user, secret]) {
      input.addEventListener("keydown", (event) => {
        if (event.key === "Enter") {
          event.preventDefault();
          submit();
        }
      });
    }
    const actions = el("div", "bubble__actions");
    actions.append(textButton("Отмена", () => answer("cancel"), "btn"), textButton("Войти", submit, "btn btn--primary"));
    bubble.append(actions);
    return () => answer("cancel");
  },

  /** Ссылка на приложение: tg:, mailto:, zoommtg:… */
  external(bubble, { scheme = "", origin = "", app = "", remember = false }, { answer }) {
    bubble.append(dialogHead(app ? `Открыть приложение «${app}»?` : "Открыть приложение?"));
    const host = hostOf(origin);
    bubble.append(
      el(
        "p",
        "bubble__text",
        host ? `Сайт ${host} хочет открыть ссылку ${scheme}: в приложении на компьютере.` : `Ссылка ${scheme}: откроется в приложении на компьютере.`
      )
    );
    const always = remember && host ? checkbox(`Всегда разрешать ${host} открывать такие ссылки`) : null;
    if (always) bubble.append(always.node);

    const actions = el("div", "bubble__actions");
    const cancel = textButton("Отмена", () => answer("cancel"), "btn");
    // Фокус — на «Отмене»: Enter, нажатый в странице за миг до окна, не должен
    // запускать программу.
    cancel.setAttribute("autofocus", "");
    const open = textButton("Открыть", () => answer("accept", { remember: always?.input.checked ?? false }, { guard: true }), "btn btn--primary");
    actions.append(cancel, open);
    bubble.append(actions);
    return () => answer("cancel");
  },
};

function dialogHead(title) {
  const head = el("div", "bubble__head");
  head.append(el("h2", "bubble__title", title));
  return head;
}

/** Флажок: подпись щёлкается вместе с ним. */
function checkbox(label) {
  const node = el("label", "check");
  const input = el("input");
  input.type = "checkbox";
  node.append(input, el("span", null, label));
  return { node, input };
}

/* ── Строка масштаба ───────────────────────────────────────── */

function zoomRow(kind, item) {
  let value = item.value ?? 1;
  const row = el("div", "menu__zoom");
  const label = el("span", "menu__label", item.label);
  const valueNode = el("span", "menu__zoom-value", `${Math.round(value * 100)}%`);

  const step = (direction) => {
    const next =
      direction > 0
        ? ZOOM_STEPS.find((z) => z > value + 0.001) ?? value
        : [...ZOOM_STEPS].reverse().find((z) => z < value - 0.001) ?? value;
    value = next;
    valueNode.textContent = `${Math.round(value * 100)}%`;
    act(kind, direction > 0 ? "zoom_in" : "zoom_out", {}, { keepOpen: true });
  };

  const minus = iconButton("zoom-out", "Уменьшить (Ctrl+−)", () => step(-1), { size: 20, className: "btn btn--ghost btn--icon" });
  const plus = iconButton("zoom-in", "Увеличить (Ctrl++)", () => step(1), { size: 20, className: "btn btn--ghost btn--icon" });
  minus.disabled = plus.disabled = Boolean(item.disabled);
  row.append(label, minus, valueNode, plus);
  return row;
}

/* ── Строки загрузок ───────────────────────────────────────── */

function downloadRow(item) {
  const node = el("div", "dl");
  node.dataset.state = item.state;
  const tile = el("div", "dl__icon");
  tile.append(icon(fileIcon(item.path), 20));

  const body = el("div");
  body.style.minWidth = "0";
  const name = el("div", "dl__name", model.displayName(item));
  const status = el("div", "dl__status");
  body.append(name, status);

  let meter = null;
  let fill = null;
  if (model.isActive(item)) {
    meter = el("div", "meter");
    fill = el("div", "meter__fill");
    meter.append(fill);
    body.append(meter);
  }

  const actions = el("div", "dl__actions");
  const control = (action) => model.control(item.id, action).catch(() => {});
  const media = item.kind === "media";
  switch (item.state) {
    case "running":
      if (!media) actions.append(iconButton("pause-16", "Приостановить", () => control("pause")));
      actions.append(iconButton("dismiss-16", "Отменить", () => control("cancel")));
      break;
    case "paused":
      actions.append(iconButton("play-16", "Продолжить", () => control("resume")), iconButton("dismiss-16", "Отменить", () => control("cancel")));
      break;
    case "done":
      actions.append(iconButton("folder-open-16", "Показать в папке", () => control("show").then(close)));
      break;
    default:
      if (!media) actions.append(iconButton("retry-16", "Повторить", () => control("retry")));
      actions.append(iconButton("dismiss-16", "Убрать из списка", () => control("remove")));
  }

  if (item.state === "done") {
    node.style.cursor = "pointer";
    node.title = "Открыть файл";
    node.addEventListener("click", (event) => {
      if (event.target.closest(".dl__actions")) return;
      control("open").then(close);
    });
  }

  node.append(tile, body, actions);
  return { node, status, meter, fill };
}

function updateDownloadRow(row, item) {
  if (!row) return;
  row.status.textContent = model.statusText(item);
  if (row.meter) {
    const share = model.progressOf(item);
    row.meter.dataset.paused = String(item.state === "paused");
    row.meter.dataset.indeterminate = String(share == null);
    row.fill.style.width = `${Math.round((share ?? 0) * 100)}%`;
  }
}

/* ── Расширение: загрузчик видео ───────────────────────────── */

/** Состояние расширения переживает закрытие окна: загрузка идёт дальше. */
const media = { url: "", info: null, busy: false, error: null, job: null, progress: null, done: null };

function mediaView({ url, services }) {
  const head = el("div", "ext-head");
  const logo = el("div", "ext-head__logo");
  logo.append(icon("video-filled", 20));
  const name = el("div", "ext-head__name");
  name.append(document.createTextNode("Загрузчик видео"), el("small", null, "Расширение 190x4"));
  head.append(
    logo,
    name,
    iconButton("settings", "Настройки расширения", () => act("media", "settings"), { size: 20, className: "btn btn--ghost btn--icon" })
  );

  const body = el("div", "media scroll");
  body.style.maxHeight = "520px";
  root.append(head, body);

  if (url && url !== media.url && !media.job) {
    Object.assign(media, { url, info: null, error: null, done: null });
  }

  const draw = () => {
    body.replaceChildren();

    if (!services?.media) {
      body.append(el("div", "error-card", "Сервис загрузки 190x4 не настроен на этом компьютере."));
      fit();
      return;
    }

    if (media.progress) {
      const card = el("div", "progress-card");
      const top = el("div", "progress-card__row");
      top.append(
        icon("download-16", 16),
        el("span", "progress-card__name", media.progress.file_name || "Сервер готовит файл…"),
        textButton("Отменить", () => invoke("media_cancel", { job: media.job }).catch(() => {}), "btn btn--ghost btn--sm")
      );
      const meter = el("div", "meter");
      const fill = el("div", "meter__fill");
      const percent = Number(media.progress.percent) || 0;
      meter.dataset.indeterminate = String(!percent);
      fill.style.width = `${Math.min(100, percent)}%`;
      meter.append(fill);
      const meta = el(
        "div",
        "progress-card__meta",
        media.progress.total > 0
          ? `${formatBytes(media.progress.downloaded)} из ${formatBytes(media.progress.total)} · ${Math.round(percent)}%`
          : percent
            ? `${Math.round(percent)}%`
            : "Ожидание сервера"
      );
      card.append(top, meter, meta);
      body.append(card);
    }

    if (media.done) {
      const card = el("div", "progress-card");
      const top = el("div", "progress-card__row");
      top.append(icon("checkmark-16", 16), el("span", "progress-card__name", media.done.name));
      const buttons = el("div", "bubble__actions");
      buttons.style.marginTop = "8px";
      buttons.append(
        textButton("Показать в папке", () => media.done.id && model.control(media.done.id, "show").then(close), "btn btn--sm"),
        textButton("Открыть", () => media.done.id && model.control(media.done.id, "open").then(close), "btn btn--primary btn--sm")
      );
      card.append(top, el("div", "progress-card__meta", "Скачано в папку загрузок"), buttons);
      body.append(card);
    }

    if (media.error) body.append(el("div", "error-card", media.error));

    if (!media.url) {
      body.append(el("div", "empty", "Откройте страницу с видео — YouTube, VK Видео, Rutube, Дзен — и нажмите на значок ещё раз."));
      fit();
      return;
    }

    if (media.busy) {
      body.append(el("div", "spinner"), el("div", "empty", "Ищем видео на странице…"));
      fit();
      return;
    }

    if (media.info) {
      const card = el("div", "media__card");
      const thumb = el("img", "media__thumb");
      if (/^https:/.test(media.info.thumbnail ?? "")) thumb.src = media.info.thumbnail;
      thumb.alt = "";
      const text = el("div");
      text.append(
        el("div", "media__title", media.info.title || media.url),
        el("div", "media__meta", [media.info.uploader, media.info.duration_str].filter(Boolean).join(" · ") || hostOf(media.url))
      );
      card.append(thumb, text);
      body.append(card);

      const video = (media.info.formats ?? []).filter((format) => format.spec.startsWith("video"));
      const audio = (media.info.formats ?? []).filter((format) => format.spec.startsWith("audio"));
      if (video.length) body.append(el("div", "formats__label", "Видео"), ...video.map(formatRow));
      if (audio.length) body.append(el("div", "formats__label", "Только звук"), ...audio.map(formatRow));
      if (!video.length && !audio.length) body.append(el("div", "empty", "Сервер не нашёл доступных форматов"));
    }
    fit();
  };

  const formatRow = (format) => {
    const row = el("button", "format");
    row.type = "button";
    row.disabled = Boolean(media.job);
    row.append(icon("download-16", 16), el("span", "format__label", format.label || format.spec));
    row.append(el("span", "format__size", format.approx_mb ? `≈ ${String(format.approx_mb).replace(".", ",")} МБ` : ""));
    row.addEventListener("click", () => start(format.spec));
    return row;
  };

  const probe = async () => {
    media.busy = true;
    media.error = null;
    draw();
    try {
      media.info = await invoke("media_probe", { url: media.url });
    } catch (error) {
      media.error = String(error?.message ?? error);
    } finally {
      media.busy = false;
      draw();
    }
  };

  const start = async (spec) => {
    if (media.job) return;
    media.error = null;
    media.done = null;
    media.progress = { percent: 0, file_name: "" };
    draw();
    try {
      media.job = await invoke("media_download", { url: media.url, format: spec });
    } catch (error) {
      media.progress = null;
      media.error = String(error?.message ?? error);
    }
    draw();
  };

  if (services?.media && media.url && !media.info && !media.busy && !media.job) probe();
  else draw();

  const offMedia = listen("media", (event) => {
    if (event.job !== media.job) return;
    if (event.phase === "progress") {
      media.progress = event;
    } else {
      media.progress = null;
      media.job = null;
      if (event.phase === "done") {
        const item = model.downloads().find((d) => d.kind === "media" && d.path === event.path);
        media.done = { name: event.path.split(/[\\/]/).pop(), id: item?.id ?? null };
      } else if (event.phase === "failed") {
        media.error = event.error || "Загрузка прервалась";
      }
    }
    draw();
  });
  model.initDownloads();

  return () => offMedia.then((off) => off?.());
}

/* ── Демо для ревью вёрстки без Rust ───────────────────────── */

async function demo() {
  const params = new URLSearchParams(location.search);
  if (params.get("motion") === "off") document.documentElement.dataset.motion = "off";
  const mock = await import("../mock.js");
  const kind = params.get("kind") ?? "menu";
  const payload = mock.popupDemo(kind);
  render({ kind: payload.kind ?? kind, payload: payload.payload ?? payload });
}
