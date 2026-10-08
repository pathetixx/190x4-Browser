// «Скрыть элемент» (src-tauri/src/picker.rs): человек показывает блок на
// странице, скрипт предлагает для него селектор, а браузер сохраняет правило
// `сайт##селектор` в «Мои правила». Выполняется по пункту меню страницы, не
// встраивается; настройки подставляет браузер вместо __X4_PICKER__:
// `{ token, x, y }` — номер выбора и точка щелчка в CSS-пикселях страницы.
//
// Всё своё — в закрытом shadow DOM: стили страницы его не трогают, а скрипты
// страницы до него не дотягиваются. Пока идёт выбор, щелчки по странице не
// доходят до её обработчиков: выбор не должен переходить по ссылкам.
(() => {
  "use strict";

  const config = __X4_PICKER__;
  const KEY = Symbol.for("x4picker");
  if (typeof window[KEY] === "function") window[KEY]();
  const bridge = window.chrome && window.chrome.webview;
  if (!bridge) return;
  const post = (message) => {
    try {
      bridge.postMessage({ token: config.token, ...message });
    } catch (_) {
      // Канал закрыт — выбор просто закончится без правила.
    }
  };

  const host = document.createElement("x4-picker");
  host.style.cssText = "all: initial; position: fixed; inset: 0; z-index: 2147483647; pointer-events: none;";
  const root = host.attachShadow({ mode: "closed" });
  root.innerHTML = `
    <style>
      :host { all: initial; }
      .box { position: fixed; pointer-events: none; border: 2px solid #c0304a; background: rgba(192, 48, 74, 0.16);
        box-shadow: 0 0 0 1px rgba(0, 0, 0, 0.4), 0 0 18px rgba(192, 48, 74, 0.35); border-radius: 2px; display: none; }
      .panel { position: fixed; right: 16px; bottom: 16px; width: min(420px, calc(100vw - 32px)); box-sizing: border-box;
        pointer-events: auto; padding: 14px 16px; background: #16171b; color: #e9e9ee; border: 1px solid #2c2e35;
        border-left: 3px solid #c0304a; border-radius: 6px; box-shadow: 0 12px 32px rgba(0, 0, 0, 0.45);
        font: 13px/1.4 "Inter", "Segoe UI", system-ui, sans-serif; }
      .title { font-weight: 600; font-size: 14px; margin-bottom: 6px; }
      .hint { color: #9a9ca6; font-size: 12px; margin-bottom: 8px; }
      .selector { font: 12px/1.4 "JetBrains Mono", Consolas, monospace; color: #f2c4cd; background: #0e0f12;
        border: 1px solid #2c2e35; border-radius: 4px; padding: 6px 8px; word-break: break-all; max-height: 72px; overflow: auto; }
      .count { color: #9a9ca6; font-size: 12px; margin: 6px 0 10px; }
      .row { display: flex; gap: 8px; flex-wrap: wrap; }
      button { font: inherit; color: #e9e9ee; background: #23252b; border: 1px solid #34363e; border-radius: 4px;
        padding: 6px 12px; cursor: pointer; }
      button:hover { background: #2c2e35; }
      button:disabled { opacity: 0.45; cursor: default; }
      .primary { background: #c0304a; border-color: #c0304a; color: #fff; margin-left: auto; }
      .primary:hover { background: #d23a56; }
    </style>
    <div class="box"></div>
    <div class="panel" role="dialog" aria-label="Скрыть элемент">
      <div class="title">Скрыть элемент</div>
      <div class="hint">Наведите на рекламу и щёлкните. «Шире» берёт блок вокруг, Esc — отмена.</div>
      <div class="selector"></div>
      <div class="count"></div>
      <div class="row">
        <button type="button" data-act="wider">Шире</button>
        <button type="button" data-act="narrower">Уже</button>
        <button type="button" data-act="cancel">Отмена</button>
        <button type="button" class="primary" data-act="hide">Скрыть</button>
      </div>
    </div>`;
  const box = root.querySelector(".box");
  const selectorView = root.querySelector(".selector");
  const countView = root.querySelector(".count");
  const narrowerButton = root.querySelector('[data-act="narrower"]');
  const hideButton = root.querySelector('[data-act="hide"]');

  /* ── Селектор ───────────────────────────────────────────────── */

  // Имя, которое сайт явно собрал сборщиком (хеш, случайные буквы, числа), до
  // следующей сборки не доживёт — по нему правило не пишется.
  const stableName = (name) =>
    name.length >= 2 && name.length <= 40 && /^[A-Za-z_][\w-]*$/.test(name) && !/\d{3,}/.test(name) && !/[A-Z].*[A-Z].*[0-9]|[0-9].*[A-Z]/.test(name);

  const ATTRIBUTES = ["data-testid", "data-test-id", "data-qa", "data-ad", "aria-label", "role", "name"];

  const own = (el) => {
    const tag = el.tagName.toLowerCase();
    if (el.id && stableName(el.id)) return `#${CSS.escape(el.id)}`;
    const classes = Array.from(el.classList).filter(stableName).slice(0, 2);
    if (classes.length) return tag + classes.map((name) => `.${CSS.escape(name)}`).join("");
    for (const name of ATTRIBUTES) {
      const value = el.getAttribute(name);
      if (value && value.length <= 60 && !/["\\\n]/.test(value)) return `${tag}[${name}="${value}"]`;
    }
    const source = el.getAttribute("src") || el.getAttribute("href");
    if (source && /^https?:\/\//.test(source)) {
      try {
        const url = new URL(source);
        return `${tag}[${el.hasAttribute("src") ? "src" : "href"}^="${url.origin}${url.pathname.split("/").slice(0, 2).join("/")}"]`;
      } catch (_) {
        // Кривой адрес — без него.
      }
    }
    return tag;
  };

  const count = (selector) => {
    try {
      return document.querySelectorAll(selector).length;
    } catch (_) {
      return 0;
    }
  };

  const bare = (step) => /^[a-z][a-z0-9-]*$/.test(step);

  /**
   * Селектор для элемента: свой признак, а если он слишком общий — путь от
   * предков, не длиннее пяти шагов. Голый тег уточняется номером среди братьев.
   */
  const selectorFor = (el) => {
    const parts = [];
    let node = el;
    for (let depth = 0; depth < 5 && usable(node); depth += 1) {
      let step = own(node);
      if (bare(step) && node.parentElement) {
        const same = Array.from(node.parentElement.children).filter((child) => child.tagName === node.tagName);
        if (same.length > 1) step += `:nth-of-type(${same.indexOf(node) + 1})`;
      }
      parts.unshift(step);
      const selector = parts.join(" > ");
      const found = count(selector);
      if (found >= 1 && found <= 20 && !bare(parts[0].split(":")[0])) return selector;
      node = node.parentElement;
    }
    return parts.join(" > ");
  };

  /* ── Выбор ──────────────────────────────────────────────────── */

  let target = null;
  let locked = false;
  /** Элементы, от которых шли «Шире»: «Уже» возвращает к ним. */
  const narrower = [];

  const usable = (el) => el && el !== host && el !== document.documentElement && el !== document.body && el.nodeType === 1;
  const at = (x, y) => {
    const el = document.elementFromPoint(x, y);
    return usable(el) ? el : null;
  };

  const render = () => {
    if (!target || !target.isConnected) {
      box.style.display = "none";
      selectorView.textContent = "Наведите на блок на странице";
      countView.textContent = "";
      hideButton.disabled = true;
      narrowerButton.disabled = true;
      return;
    }
    const rect = target.getBoundingClientRect();
    Object.assign(box.style, {
      display: "block",
      left: `${rect.left}px`,
      top: `${rect.top}px`,
      width: `${rect.width}px`,
      height: `${rect.height}px`,
    });
    const selector = selectorFor(target);
    selectorView.textContent = selector;
    const found = count(selector);
    countView.textContent = found === 1 ? "На странице: 1 элемент" : `На странице: ${found} элемент(ов)`;
    hideButton.disabled = found === 0;
    narrowerButton.disabled = !narrower.length;
  };

  const inside = (event) => event.target === host;

  const onMove = (event) => {
    if (locked || inside(event)) return;
    const el = at(event.clientX, event.clientY);
    if (el && el !== target) {
      target = el;
      narrower.length = 0;
      render();
    }
  };

  // Щелчок по странице выбирает элемент и ничего больше не делает: ни
  // переходов по ссылкам, ни обработчиков сайта.
  const swallow = (event) => {
    if (inside(event)) return;
    event.preventDefault();
    event.stopImmediatePropagation();
  };
  const onClick = (event) => {
    if (inside(event)) return;
    swallow(event);
    const el = at(event.clientX, event.clientY);
    if (el) {
      target = el;
      narrower.length = 0;
      locked = true;
      render();
    }
  };

  const onKey = (event) => {
    if (event.key !== "Escape") return;
    swallow(event);
    finish({ evt: "picker_cancel" });
  };

  const onScroll = () => render();

  const EVENTS = [
    ["mousemove", onMove],
    ["click", onClick],
    ["mousedown", swallow],
    ["mouseup", swallow],
    ["pointerdown", swallow],
    ["pointerup", swallow],
    ["auxclick", swallow],
    ["contextmenu", swallow],
    ["keydown", onKey],
  ];

  const finish = (message) => {
    for (const [type, handler] of EVENTS) window.removeEventListener(type, handler, true);
    window.removeEventListener("scroll", onScroll, true);
    window.removeEventListener("resize", onScroll, true);
    host.remove();
    if (window[KEY] === stop) delete window[KEY];
    if (message) post(message);
  };
  const stop = () => finish({ evt: "picker_cancel" });

  root.querySelector(".panel").addEventListener("click", (event) => {
    const action = event.target && event.target.closest && event.target.closest("button")?.dataset.act;
    if (!action) return;
    if (action === "cancel") {
      finish({ evt: "picker_cancel" });
    } else if (action === "wider" && target && usable(target.parentElement)) {
      narrower.push(target);
      target = target.parentElement;
      locked = true;
      render();
    } else if (action === "narrower" && narrower.length) {
      target = narrower.pop();
      render();
    } else if (action === "hide" && target) {
      const selector = selectorFor(target);
      // Сразу, не дожидаясь пересборки фильтра: правило подхватит следующая загрузка.
      try {
        for (const el of document.querySelectorAll(selector)) el.style.setProperty("display", "none", "important");
      } catch (_) {
        // Селектор не разобрался — правило браузер всё равно проверит сам.
      }
      finish({ evt: "picker_rule", selector });
    }
  });

  for (const [type, handler] of EVENTS) window.addEventListener(type, handler, true);
  window.addEventListener("scroll", onScroll, true);
  window.addEventListener("resize", onScroll, true);
  try {
    Object.defineProperty(window, KEY, { value: stop, configurable: true });
  } catch (_) {
    // Отметка не встала — повторный выбор просто откроет вторую панель.
  }
  document.documentElement.append(host);
  target = at(config.x, config.y);
  locked = Boolean(target);
  render();
})();
