// Менеджер паролей 190x4: встраивается в каждый документ и в каждый фрейм до
// скриптов страницы (AddScriptToExecuteOnDocumentCreated).
//
// Скрипт ничего не хранит и ничего не решает. Он сообщает браузеру факты —
// «здесь форма входа», «отправлен логин с паролем», «выбрана учётка» — и
// показывает под полем список учёток, который прислал браузер. Пароль попадает
// на страницу только после выбора учётки или когда учётка сайта одна.
// Происхождение сообщения Rust берёт из движка, а не из текста сообщения.
(() => {
  "use strict";

  // Скрипт ставится при создании документа; повторно (DOMContentLoaded, см.
  // tab.rs) он не нужен. Метка скрытая и хранит этап установки и ошибку.
  const MARK = "__190x4Passwords";
  if (window[MARK]) return;
  const status = { stage: "start", asked: "", filled: "", accounts: 0, posts: 0, error: "" };
  try {
    Object.defineProperty(window, MARK, { value: status, enumerable: false });
  } catch (_) {
    return;
  }

  try {

  // Свои ссылки на DOM — до скриптов страницы. Список учёток строится ими, и
  // подменённые страницей методы не увидят ни логинов, ни пароля.
  const call = Function.prototype.call.bind(Function.prototype.call);
  const setter = (object, name) => Object.getOwnPropertyDescriptor(object, name).set;
  const dom = {
    createElement: Document.prototype.createElement,
    createElementNS: Document.prototype.createElementNS,
    attachShadow: Element.prototype.attachShadow,
    setAttribute: Element.prototype.setAttribute,
    remove: Element.prototype.remove,
    appendChild: Node.prototype.appendChild,
    listen: EventTarget.prototype.addEventListener,
    textContent: setter(Node.prototype, "textContent"),
    className: setter(Element.prototype, "className"),
    adopt: setter(ShadowRoot.prototype, "adoptedStyleSheets"),
    value: setter(HTMLInputElement.prototype, "value"),
    replaceSync: CSSStyleSheet.prototype.replaceSync,
    showPopover: HTMLElement.prototype.showPopover,
    hidePopover: HTMLElement.prototype.hidePopover,
  };
  const Sheet = CSSStyleSheet;
  const make = (tag, className, text) => {
    const node = call(dom.createElement, document, tag);
    if (className) call(dom.className, node, className);
    if (text != null) call(dom.textContent, node, text);
    return node;
  };
  const append = (parent, child) => call(dom.appendChild, parent, child);
  const on = (target, type, handler, options) => call(dom.listen, target, type, handler, options);

  // Мост WebView2 ищем при каждом обращении: скрипт запускается на создании
  // документа, и мост к этому мигу может ещё не появиться.
  const bridge = () => (window.chrome && window.chrome.webview) || null;

  const post = (message) => {
    const hook = bridge();
    if (!hook) return false;
    try {
      hook.postMessage(message);
      status.posts += 1;
      return true;
    } catch (_) {
      return false;
    }
  };

  const usable = (input) =>
    input instanceof HTMLInputElement &&
    !input.disabled &&
    !input.readOnly &&
    input.type !== "hidden" &&
    input.getClientRects().length > 0;

  const hasToken = (input, token) =>
    String(input.getAttribute("autocomplete") || "").toLowerCase().split(/\s+/).includes(token);
  const describe = (input) =>
    [input.name, input.id, input.getAttribute("aria-label"), input.placeholder].join(" ");

  const passwordsIn = (scope) =>
    Array.from((scope || document).querySelectorAll('input[type="password"]')).filter(usable);

  // Поле логина без пароля рядом — вход в два шага (Google, VK ID, Яндекс).
  // Разметка autocomplete="username" есть не везде: у VK ID это просто поле
  // email, поэтому смотрим и на тип, и на имя поля.
  const LOGIN_HINT = /e-?mail|login|user|account|identifier|phone|почт|логин|телефон/i;
  const SEARCH_HINT = /search|query|поиск/i;
  const loginsIn = (scope) =>
    Array.from((scope || document).querySelectorAll("input")).filter(
      (input) =>
        usable(input) &&
        /^(text|email|tel|)$/i.test(input.type) &&
        !SEARCH_HINT.test(describe(input)) &&
        (hasToken(input, "username") || input.type === "email" || LOGIN_HINT.test(describe(input)))
    );

  // Страница входа. Единственную учётку браузер подставит сам только здесь, а
  // не в любое поле email вроде подписки на рассылку.
  const LOGIN_PAGE = /log-?in|sign-?in|auth|passport|account|вход/i;
  const confident = (step) =>
    step === "password" ||
    loginsIn(document).some((input) => hasToken(input, "username")) ||
    LOGIN_PAGE.test(location.hostname + location.pathname);

  // Где искать форму входа вокруг элемента: сама форма, а без неё — ближайший
  // предок, в котором есть поле пароля. Иначе на странице с несколькими
  // блоками логин и пароль собирались бы из разных мест.
  const scopeFor = (element) => {
    if (element.form) return element.form;
    for (let node = element; node && node !== document.documentElement; node = node.parentElement) {
      if (node.querySelector && node.querySelector('input[type="password"]')) return node;
    }
    return document;
  };

  // Логин — ближайшее текстовое поле перед паролем. Явная разметка
  // autocomplete="username" важнее соседства, но только внутри той же формы.
  const usernameFor = (password, scope) => {
    const form = password.form;
    if (form) {
      const marked = form.querySelector('input[autocomplete~="username"], input[autocomplete~="email"]');
      if (usable(marked)) return marked;
    }
    const fields = Array.from((form || scope || document).querySelectorAll("input")).filter(usable);
    for (let i = fields.indexOf(password) - 1; i >= 0; i -= 1) {
      if (/^(text|email|tel|)$/i.test(fields[i].type)) return fields[i];
    }
    return null;
  };

  // Поле, у которого показывают список учёток: пароль, логин рядом с паролем
  // или поле логина на шаге без пароля.
  const accountField = (input) => {
    if (!usable(input)) return false;
    if (input.type === "password") return true;
    const password = passwordsIn(scopeFor(input))[0];
    if (password) return usernameFor(password, scopeFor(password)) === input;
    return loginsIn(document).includes(input);
  };

  /* ── Отправленный логин ─────────────────────────────────────── */

  let pending = "";
  let lastSubmit = null;
  let lastLogin = "";
  let watch = 0;

  const capture = (scope) => {
    const filled = passwordsIn(scope).filter((input) => input.value);
    if (!filled.length) {
      // Шаг с одним логином: на шаге с паролем поля логина уже не видно.
      const login = loginsIn(scope).find((input) => input.value);
      if (login) lastLogin = login.value.trim();
      return;
    }
    // На формах смены пароля их два-три: сохранять нужно последний, новый.
    const password = filled[filled.length - 1];
    const user = usernameFor(filled[0], scope);
    const username = user ? user.value.trim() : lastLogin;
    const key = username + String.fromCharCode(0) + password.value;
    if (key === pending) return;
    pending = key;
    lastSubmit = { evt: "password_submit", username, password: password.value };
    post(lastSubmit);

    // Вход без перезагрузки страницы: форма пропала — значит, пустили.
    // Если осталась на месте, скорее всего пароль неверный, и предлагать
    // сохранить его незачем.
    clearInterval(watch);
    const started = Date.now();
    watch = setInterval(() => {
      if (!password.isConnected || !usable(password)) {
        clearInterval(watch);
        post({ evt: "password_commit" });
      } else if (Date.now() - started > 8000) {
        clearInterval(watch);
      }
    }, 400);
  };

  on(document, "submit", (event) => capture(event.target), true);

  // Отправка формы сразу уводит страницу. Сообщение, ушедшее за миг до этого,
  // может не дойти — повторяем его, когда документ уже уходит.
  on(window, "pagehide", () => {
    hideMenu();
    if (lastSubmit) post(lastSubmit);
  });

  on(
    document,
    "click",
    (event) => {
      const target = event.target instanceof Element ? event.target : null;
      const button = target && target.closest('button, input[type="submit"], [role="button"]');
      if (button) capture(button.form || scopeFor(button));
    },
    true
  );

  on(
    document,
    "keydown",
    (event) => {
      if (event.key === "Enter" && event.target instanceof HTMLInputElement) {
        capture(scopeFor(event.target));
      }
    },
    true
  );

  /* ── Форма входа ────────────────────────────────────────────── */

  // Спрашиваем раз на шаг — шаг с логином и шаг с паролем. Одностраничные
  // сайты рисуют форму после загрузки, поэтому следим за DOM.
  let asked = "";
  const ask = () => {
    const step = passwordsIn(document).length ? "password" : loginsIn(document).length ? "login" : "";
    if (step && step !== asked && asked !== "password" && post({ evt: "password_form", step, confident: confident(step) })) {
      asked = step;
      status.asked = step;
    }
    return asked === "password";
  };

  const start = () => {
    if (ask()) return;
    let scheduled = false;
    const observer = new MutationObserver(() => {
      if (scheduled) return;
      scheduled = true;
      setTimeout(() => {
        scheduled = false;
        if (ask()) observer.disconnect();
      }, 300);
    });
    observer.observe(document.documentElement, { childList: true, subtree: true });
    // После шага с логином пароль появится, когда человек нажмёт «Далее».
    setTimeout(() => {
      if (asked === "login") setTimeout(() => observer.disconnect(), 120000);
      else observer.disconnect();
    }, 15000);
  };

  if (document.readyState === "loading") {
    on(document, "DOMContentLoaded", start, { once: true });
  } else {
    start();
  }

  /* ── Список учёток под полем ────────────────────────────────── */

  let accounts = [];
  let theme = "";
  let menu = null;
  let lastField = null;

  const CSS = `
    :host { all: initial; }
    .panel { box-sizing: border-box; display: flex; flex-direction: column; width: 100%; padding: 4px; border-radius: 10px; border: 1px solid rgba(255, 255, 255, 0.09); background: #151518; color: #f1f1f4; box-shadow: 0 12px 32px rgba(0, 0, 0, 0.45); font: 13px/1.35 "Segoe UI Variable Text", "Segoe UI", system-ui, sans-serif; user-select: none; }
    .panel.light { border-color: rgba(0, 0, 0, 0.1); background: #ffffff; color: #18181b; box-shadow: 0 12px 32px rgba(0, 0, 0, 0.16); }
    .list { position: relative; overflow-y: auto; overscroll-behavior: contain; scrollbar-width: thin; scrollbar-color: rgba(255, 255, 255, 0.18) transparent; }
    .light .list { scrollbar-color: rgba(0, 0, 0, 0.2) transparent; }
    .row { box-sizing: border-box; display: flex; align-items: center; gap: 10px; width: 100%; margin: 0; padding: 6px 8px; border: 0; border-radius: 7px; background: transparent; color: inherit; font: inherit; text-align: left; cursor: pointer; }
    .row:hover, .row.selected { background: rgba(255, 255, 255, 0.07); }
    .light .row:hover, .light .row.selected { background: rgba(0, 0, 0, 0.05); }
    .tile { flex: none; display: flex; align-items: center; justify-content: center; width: 30px; height: 30px; border-radius: 7px; background: rgba(222, 87, 114, 0.14); color: #de5772; }
    .light .tile { background: rgba(181, 47, 72, 0.1); color: #b52f48; }
    .tile svg { width: 16px; height: 16px; fill: currentColor; }
    .text { display: flex; flex-direction: column; min-width: 0; }
    .host { font-size: 11px; color: #9a9aa3; }
    .light .host { color: #71717a; }
    .user { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-weight: 600; }
    .sep { height: 1px; margin: 4px -4px; background: rgba(255, 255, 255, 0.08); }
    .light .sep { background: rgba(0, 0, 0, 0.08); }
    .manage { padding: 7px 8px; color: #9a9aa3; font-size: 12px; }
    .light .manage { color: #71717a; }
  `;

  /** Высота списка учёток — семь с половиной строк: край следующей подсказывает прокрутку. */
  const LIST_MAX = 330;
  const SVG = "http://www.w3.org/2000/svg";
  const LOCK = "M8 1a3 3 0 0 0-3 3v2h-.5A1.5 1.5 0 0 0 3 7.5v6A1.5 1.5 0 0 0 4.5 15h7a1.5 1.5 0 0 0 1.5-1.5v-6A1.5 1.5 0 0 0 11.5 6H11V4a3 3 0 0 0-3-3Zm2 5H6V4a2 2 0 1 1 4 0v2Z";
  const glyph = () => {
    const svg = call(dom.createElementNS, document, SVG, "svg");
    call(dom.setAttribute, svg, "viewBox", "0 0 16 16");
    const path = call(dom.createElementNS, document, SVG, "path");
    call(dom.setAttribute, path, "d", LOCK);
    append(svg, path);
    return svg;
  };

  // Тема браузера, а не страницы: список — часть браузера.
  const light = () =>
    theme === "shiro" || (theme === "system" && window.matchMedia("(prefers-color-scheme: light)").matches);

  const buildMenu = () => {
    const host = make("div");
    call(dom.setAttribute, host, "popover", "manual");
    // Стили хоста — поверх стилей страницы и умолчаний popover; содержимое
    // живёт в закрытом shadow DOM, куда страница не дотянется.
    const style = host.style;
    for (const [name, value] of [
      ["position", "fixed"],
      ["inset", "auto"],
      ["margin", "0"],
      ["padding", "0"],
      ["border", "0"],
      ["background", "transparent"],
      ["overflow", "visible"],
      ["max-width", "none"],
      ["max-height", "none"],
      ["z-index", "2147483647"],
    ]) {
      style.setProperty(name, value, "important");
    }
    const root = call(dom.attachShadow, host, { mode: "closed" });
    const sheet = new Sheet();
    call(dom.replaceSync, sheet, CSS);
    call(dom.adopt, root, [sheet]);
    const panel = make("div", "panel");
    append(root, panel);
    // Нажатие в списке не забирает фокус у поля: иначе список закрылся бы до щелчка.
    on(panel, "mousedown", (event) => event.preventDefault());
    // Колесо над списком прокручивает список, а не страницу. Прокрутка своя:
    // обработчики колеса сайта (свой скролл, модальные окна) её не перехватят.
    on(
      panel,
      "wheel",
      (event) => {
        event.preventDefault();
        event.stopPropagation();
        if (!menu.list) return;
        const step = event.deltaMode === 1 ? 40 : event.deltaMode === 2 ? menu.list.clientHeight : 1;
        menu.list.scrollTop += event.deltaY * step;
      },
      { passive: false }
    );
    menu = { host, panel, list: null, field: null, rows: [], selected: -1, shown: false };
  };

  const hideMenu = () => {
    if (!menu || !menu.shown) return;
    try {
      if (dom.hidePopover) call(dom.hidePopover, menu.host);
    } catch (_) {
      // Уже скрыт.
    }
    call(dom.remove, menu.host);
    menu.shown = false;
  };

  const pick = (event, id) => {
    // Выбор — только живым действием человека: программный щелчок страницы не в счёт.
    if (!event.isTrusted) return;
    hideMenu();
    post({ evt: "password_pick", id });
  };

  const select = (index) => {
    if (!menu.rows.length) return;
    menu.selected = (index + menu.rows.length) % menu.rows.length;
    menu.rows.forEach(({ row }, i) => call(dom.className, row, i === menu.selected ? "row selected" : "row"));
    // Выбранная стрелками учётка — в видимой части списка.
    const { row } = menu.rows[menu.selected];
    const list = menu.list;
    if (row.offsetTop < list.scrollTop) list.scrollTop = row.offsetTop;
    else if (row.offsetTop + row.offsetHeight > list.scrollTop + list.clientHeight) {
      list.scrollTop = row.offsetTop + row.offsetHeight - list.clientHeight;
    }
  };

  const render = () => {
    const field = menu.field;
    const typed = field.type === "password" ? "" : field.value.trim().toLowerCase();
    const shown = accounts.filter((account) => !typed || account.username.toLowerCase().startsWith(typed));
    call(dom.textContent, menu.panel, "");
    call(dom.className, menu.panel, light() ? "panel light" : "panel");
    menu.rows = [];
    menu.selected = -1;
    const list = make("div", "list");
    menu.list = list;
    append(menu.panel, list);
    for (const account of shown) {
      const row = make("button", "row");
      call(dom.setAttribute, row, "type", "button");
      const tile = make("span", "tile");
      append(tile, glyph());
      const text = make("span", "text");
      append(text, make("span", "host", account.host));
      append(text, make("span", "user", account.username || "без логина"));
      append(row, tile);
      append(row, text);
      on(row, "click", (event) => pick(event, account.id));
      append(list, row);
      menu.rows.push({ row, id: account.id });
    }
    append(menu.panel, make("div", "sep"));
    const manage = make("button", "row manage", "Управление паролями");
    call(dom.setAttribute, manage, "type", "button");
    on(manage, "click", (event) => {
      if (!event.isTrusted) return;
      hideMenu();
      post({ evt: "password_manage" });
    });
    append(menu.panel, manage);
    return shown.length > 0;
  };

  const place = () => {
    if (!menu || !menu.shown) return;
    const field = menu.field;
    if (!field.isConnected || !usable(field)) {
      hideMenu();
      return;
    }
    const rect = field.getBoundingClientRect();
    const width = Math.min(Math.max(rect.width, 260), 380);
    const style = menu.host.style;
    style.setProperty("width", `${width}px`, "important");
    // Список не выше семи с половиной строк и не больше места у поля: остальное
    // прокручивается. Вниз, если там хватает места или его больше, чем сверху.
    const list = menu.list;
    const frame = menu.panel.offsetHeight - list.offsetHeight;
    const wanted = Math.min(list.scrollHeight, LIST_MAX) + frame;
    const spaceBelow = window.innerHeight - rect.bottom - 8;
    const spaceAbove = rect.top - 8;
    const down = spaceBelow >= wanted || spaceBelow >= spaceAbove;
    const room = (down ? spaceBelow : spaceAbove) - frame;
    list.style.setProperty("max-height", `${Math.max(Math.min(LIST_MAX, room), 44)}px`);
    const height = menu.panel.offsetHeight;
    style.setProperty("left", `${Math.max(4, Math.min(rect.left, window.innerWidth - width - 4))}px`, "important");
    style.setProperty("top", `${down ? rect.bottom + 4 : Math.max(4, rect.top - 4 - height)}px`, "important");
  };

  const showMenu = (field) => {
    if (!accounts.length || !accountField(field)) return;
    if (!menu) buildMenu();
    menu.field = field;
    lastField = field;
    if (!render()) {
      hideMenu();
      return;
    }
    if (!menu.shown) {
      append(document.documentElement, menu.host);
      try {
        if (dom.showPopover) call(dom.showPopover, menu.host);
      } catch (_) {
        // Без верхнего слоя список всё равно виден: position: fixed и z-index.
      }
      menu.shown = true;
    }
    place();
  };

  // Щелчок по полю открывает список; щелчок мимо — закрывает.
  on(
    window,
    "mousedown",
    (event) => {
      if (!event.isTrusted) return;
      const target = event.target;
      if (menu && menu.shown && target === menu.host) return;
      if (target instanceof HTMLInputElement && accountField(target)) {
        setTimeout(() => showMenu(target), 0);
      } else {
        hideMenu();
      }
    },
    true
  );

  on(
    window,
    "keydown",
    (event) => {
      if (!event.isTrusted) return;
      const target = event.target;
      if (menu && menu.shown && target === menu.field) {
        if (event.key === "ArrowDown" || event.key === "ArrowUp") {
          event.preventDefault();
          select(menu.selected + (event.key === "ArrowDown" ? 1 : -1));
        } else if (event.key === "Enter" && menu.selected >= 0) {
          event.preventDefault();
          event.stopPropagation();
          pick(event, menu.rows[menu.selected].id);
        } else if (event.key === "Escape") {
          event.preventDefault();
          hideMenu();
        }
        return;
      }
      if (event.key === "ArrowDown" && target instanceof HTMLInputElement && accountField(target)) {
        event.preventDefault();
        showMenu(target);
      }
    },
    true
  );

  // Набор в поле логина сужает список, в поле пароля — закрывает его.
  on(
    window,
    "input",
    (event) => {
      if (!menu || !menu.shown || event.target !== menu.field) return;
      if (menu.field.type === "password" || !render()) hideMenu();
      else place();
    },
    true
  );

  on(
    window,
    "focusout",
    (event) => {
      if (!menu || !menu.shown || event.target !== menu.field) return;
      setTimeout(() => {
        if (menu.shown && document.activeElement !== menu.field) hideMenu();
      }, 0);
    },
    true
  );

  on(window, "scroll", () => place(), { capture: true, passive: true });
  on(window, "resize", () => place());

  /* ── Сообщения браузера ─────────────────────────────────────── */

  // React и компания держат значение поля у себя: простое присваивание они
  // перетрут на следующем рендере. Нативный сеттер плюс событие input — как
  // при вводе с клавиатуры.
  const setValue = (input, value) => {
    if (!input || typeof value !== "string") return;
    call(dom.value, input, value);
    input.dispatchEvent(new Event("input", { bubbles: true }));
    input.dispatchEvent(new Event("change", { bubbles: true }));
  };

  const onMessage = (event) => {
    const data = event.data;
    if (!data || data.origin !== location.origin) return;

    if (data.cmd === "password_accounts") {
      accounts = (Array.isArray(data.accounts) ? data.accounts : [])
        .filter((account) => account && typeof account.id === "number")
        .map((account) => ({ id: account.id, username: String(account.username || ""), host: String(account.host || "") }));
      theme = String(data.theme || "");
      status.accounts = accounts.length;
      if (menu && menu.shown) {
        if (!render()) hideMenu();
        else place();
      }
      return;
    }

    if (data.cmd !== "password_fill") return;
    // Форма — вокруг поля, у которого выбрали учётку; без него — первая на странице.
    const anchor = lastField && lastField.isConnected && usable(lastField) ? lastField : null;
    const password =
      (anchor && anchor.type === "password" && anchor) ||
      passwordsIn(anchor ? scopeFor(anchor) : document)[0] ||
      passwordsIn(document)[0];
    if (password) {
      setValue(usernameFor(password, scopeFor(password)), data.username);
      setValue(password, data.password);
      status.filled = "password";
      return;
    }
    const login = (anchor && anchor.type !== "password" && anchor) || loginsIn(document)[0];
    if (login) setValue(login, data.username);
    status.filled = login ? "login" : "no-form";
  };

  const subscribe = () => {
    const hook = bridge();
    if (!hook) return false;
    hook.addEventListener("message", onMessage);
    return true;
  };
  if (!subscribe()) on(document, "DOMContentLoaded", subscribe, { once: true });
    status.stage = "ready";
  } catch (error) {
    status.stage = "error";
    status.error = String((error && error.stack) || error).slice(0, 300);
  }
})();
