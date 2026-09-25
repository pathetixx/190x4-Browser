// Расширение Twitch 190x4: встраивается в каждый документ, но работает только
// на twitch.tv.
//
// Бонусы баллов канала: кнопку «Получить бонус» скрипт нажимает сам, как
// только она появилась. Смайлы 7TV, BetterTTV и FrankerFaceZ: браузер
// присылает список смайлов канала (src-tauri/src/twitch.rs), в новых
// сообщениях чата их коды становятся картинками, а кнопка под чатом открывает
// их список. Лучшее качество трансляции — не здесь: его делает браузер,
// отвечая плееру на запрос плейлиста.
(() => {
  "use strict";

  if (!/(^|\.)twitch\.tv$/.test(location.hostname) || window.top !== window) return;
  const MARK = Symbol.for("x4twitch");
  if (window[MARK]) return;
  try {
    Object.defineProperty(window, MARK, { value: true, enumerable: false });
  } catch (_) {
    return;
  }

  const bridge = () => (window.chrome && window.chrome.webview) || null;
  const post = (message) => {
    const hook = bridge();
    if (!hook) return;
    try {
      hook.postMessage(message);
    } catch (_) {
      // Мост закрыт — документ уходит.
    }
  };

  // Первые части пути, которые не логин канала.
  const RESERVED = new Set([
    "directory", "videos", "settings", "search", "p", "downloads", "jobs", "turbo", "subscriptions", "wallet",
    "inventory", "drops", "friends", "messages", "payments", "prime", "u", "legal", "store", "following", "login",
    "signup", "bits", "clips", "privacy", "security", "user", "team", "event", "broadcast",
  ]);

  /** Канал страницы: трансляция, всплывающий чат, режим модератора. */
  const channelOf = () => {
    const parts = location.pathname.split("/").filter(Boolean);
    if (["popout", "embed", "moderator"].includes(parts[0])) return (parts[1] || "").toLowerCase();
    const first = (parts[0] || "").toLowerCase();
    return RESERVED.has(first) || !/^[a-z0-9_]{1,25}$/.test(first) ? "" : first;
  };

  let config = { points: false, emotes: false, token: false };
  /** Смайлы по коду; позднее перекрывает раннее (канал — общие). */
  let emotes = new Map();
  let channel = null;

  const sync = () => {
    const next = channelOf();
    if (next === channel) return;
    channel = next;
    emotes = new Map();
    post({ evt: "twitch_channel", channel });
  };

  /* ── Сообщения от браузера ────────────────────────────────── */

  const onMessage = (event) => {
    const data = event.data;
    if (!data || data.origin !== location.origin) return;
    // Настройки сменились: на следующем такте канал уйдёт браузеру заново, и
    // тот ответит новыми настройками и смайлами.
    if (data.cmd === "twitch_refresh") {
      channel = null;
      return;
    }
    if (data.cmd === "twitch_config") {
      config = { points: Boolean(data.points), emotes: Boolean(data.emotes), token: Boolean(data.token) };
      if (!data.enabled) fixes.remove();
      else if (!fixes.isConnected) (document.head || document.documentElement).append(fixes);
      if (config.token) sendToken();
      if (!config.emotes) emotes = new Map();
      return;
    }
    if (data.cmd === "twitch_emotes" && data.channel === channel && Array.isArray(data.emotes)) {
      const next = new Map();
      for (const item of data.emotes) {
        if (!Array.isArray(item) || typeof item[0] !== "string" || typeof item[1] !== "string") continue;
        next.set(item[0], {
          src: item[1],
          src2: String(item[2] || item[1]),
          provider: String(item[3] || ""),
          channel: item[4] === "channel",
        });
      }
      emotes = next;
      // Сообщения, что уже в чате, тоже получают смайлы.
      if (observed) scan(observed);
      if (panel.isConnected) fillPanel();
    }
  };
  const subscribe = () => {
    const hook = bridge();
    if (!hook) return false;
    hook.addEventListener("message", onMessage);
    return true;
  };
  if (!subscribe()) document.addEventListener("DOMContentLoaded", subscribe, { once: true });

  /** Токен входа — браузеру, только если человек включил его передачу. */
  const sendToken = () => {
    const match = /(?:^|;\s*)auth-token=([^;]+)/.exec(document.cookie);
    if (match) post({ evt: "twitch_token", token: decodeURIComponent(match[1]) });
  };

  /* ── Исправления Twitch ───────────────────────────────────── */

  // Зрителю без входа Twitch показывает внизу баннер «Зарегистрируйтесь». Он
  // укорачивает область канала, а плеер во весь экран лежит внутри неё, и
  // меню настроек (шестерёнка) считает кнопку обрезанной — открывается
  // невидимым. Во весь экран баннер не нужен.
  const fixes = document.createElement("style");
  fixes.textContent = ":fullscreen footer{display:none!important}";

  /* ── Бонусы баллов канала ─────────────────────────────────── */

  const claim = () => {
    if (!config.points) return;
    const icon = document.querySelector(".community-points-summary .claimable-bonus__icon");
    const button = icon && icon.closest("button");
    if (button && !button.disabled) button.click();
  };

  /* ── Смайлы в чате ────────────────────────────────────────── */

  // Чат прямого эфира и чат записи.
  const CHAT = ".chat-scrollable-area__message-container, .video-chat__message-list-wrapper";
  const DONE = "x4Emotes";
  let observed = null;

  const style = document.createElement("style");
  style.textContent =
    ".x4-emote{display:inline-block;vertical-align:middle;margin:-5px 0}" +
    ".x4-emote img{height:28px;width:auto;vertical-align:middle}" +
    ".x4-emotes-button{display:inline-flex;align-items:center;justify-content:center;width:30px;height:30px;" +
    "padding:0;border:0;border-radius:4px;background:none;color:var(--color-fill-button-icon,#adadb8);cursor:pointer}" +
    ".x4-emotes-button:hover,.x4-emotes-button[aria-expanded=true]{background:var(--color-background-button-text-hover,rgba(83,83,95,.48));" +
    "color:var(--color-fill-button-icon-hover,#efeff1)}" +
    ".x4-emotes-panel{position:fixed;z-index:5000;width:320px;max-height:min(420px,70vh);display:flex;flex-direction:column;" +
    "background:var(--color-background-base,#18181b);color:var(--color-text-base,#efeff1);" +
    "border:1px solid var(--color-border-base,#2f2f35);border-radius:6px;box-shadow:0 6px 16px rgba(0,0,0,.4);font-size:13px}" +
    ".x4-emotes-panel input{margin:10px 10px 6px;padding:6px 10px;border-radius:4px;border:1px solid var(--color-border-input,#464649);" +
    "background:var(--color-background-input,#0e0e10);color:inherit;font:inherit;outline:none}" +
    ".x4-emotes-panel input:focus{border-color:var(--color-border-input-focus,#a970ff)}" +
    ".x4-emotes-list{overflow-y:auto;padding:0 6px 8px}" +
    ".x4-emotes-title{padding:8px 4px 4px;font-weight:600;color:var(--color-text-alt-2,#adadb8)}" +
    ".x4-emotes-grid{display:flex;flex-wrap:wrap}" +
    ".x4-emotes-grid button{display:inline-flex;align-items:center;justify-content:center;min-width:36px;height:36px;padding:2px;" +
    "border:0;border-radius:4px;background:none;cursor:pointer}" +
    ".x4-emotes-grid button:hover{background:var(--color-background-button-text-hover,rgba(83,83,95,.48))}" +
    ".x4-emotes-grid img{max-height:28px;max-width:64px}" +
    ".x4-emotes-empty{padding:16px 8px;text-align:center;color:var(--color-text-alt-2,#adadb8)}";

  /** Текст сообщения: коды смайлов — картинками, остальное как было. */
  const decorate = (fragment) => {
    if (fragment.dataset[DONE]) return;
    fragment.dataset[DONE] = "1";
    const text = fragment.textContent;
    if (!text || !emotes.size) return;
    const words = text.split(/(\s+)/);
    if (!words.some((word) => emotes.has(word))) return;
    const out = document.createDocumentFragment();
    let plain = "";
    for (const word of words) {
      const emote = emotes.get(word);
      if (!emote) {
        plain += word;
        continue;
      }
      if (plain) out.append(plain);
      plain = "";
      const wrap = document.createElement("span");
      wrap.className = "x4-emote";
      const image = document.createElement("img");
      image.className = "chat-image chat-line__message--emote";
      image.src = emote.src;
      image.srcset = `${emote.src} 1x, ${emote.src2} 2x`;
      image.alt = word;
      image.title = emote.provider ? `${word} · ${emote.provider}` : word;
      image.loading = "lazy";
      wrap.append(image);
      out.append(wrap);
    }
    if (plain) out.append(plain);
    fragment.replaceChildren(out);
  };

  const scan = (root) => {
    if (!emotes.size) return;
    if (root.matches && root.matches(".text-fragment")) decorate(root);
    else if (root.querySelectorAll) root.querySelectorAll(".text-fragment").forEach(decorate);
  };

  const observer = new MutationObserver((records) => {
    if (!emotes.size) return;
    for (const record of records) {
      for (const node of record.addedNodes) if (node.nodeType === 1) scan(node);
    }
  });

  /** Чат пересоздаётся при переходах: следим за тем, что сейчас на странице. */
  const watchChat = () => {
    const box = config.emotes ? document.querySelector(CHAT) : null;
    if (box === observed) return;
    observer.disconnect();
    observed = box;
    if (!box) return;
    if (!style.isConnected) (document.head || document.documentElement).append(style);
    observer.observe(box, { childList: true, subtree: true });
    scan(box);
  };

  /* ── Кнопка смайлов в чате ────────────────────────────────── */

  // Кнопка стоит в нижней строке чата, перед настройками чата. Панель
  // показывает смайлы канала и общие, щелчок вставляет код в поле сообщения.
  const SVG = "http://www.w3.org/2000/svg";
  const toggle = document.createElement("button");
  toggle.type = "button";
  toggle.className = "x4-emotes-button";
  toggle.title = "Смайлы 7TV, BetterTTV и FrankerFaceZ";
  toggle.setAttribute("aria-label", toggle.title);
  toggle.setAttribute("aria-expanded", "false");
  {
    const svg = document.createElementNS(SVG, "svg");
    svg.setAttribute("width", "20");
    svg.setAttribute("height", "20");
    svg.setAttribute("viewBox", "0 0 20 20");
    svg.setAttribute("fill", "none");
    svg.setAttribute("stroke", "currentColor");
    svg.setAttribute("stroke-width", "1.6");
    svg.setAttribute("stroke-linecap", "round");
    const face = [
      "M10 2.5a7.5 7.5 0 1 0 0 15 7.5 7.5 0 0 0 0-15Z",
      "M6.8 11.6c.8 1.2 1.9 1.8 3.2 1.8s2.4-.6 3.2-1.8",
      "M7.4 7.8v.4",
      "M12.6 7.8v.4",
    ];
    for (const d of face) {
      const path = document.createElementNS(SVG, "path");
      path.setAttribute("d", d);
      svg.append(path);
    }
    toggle.append(svg);
  }

  const panel = document.createElement("div");
  panel.className = "x4-emotes-panel";
  panel.setAttribute("role", "dialog");
  panel.setAttribute("aria-label", toggle.title);
  const search = document.createElement("input");
  search.type = "search";
  search.placeholder = "Поиск смайлов";
  search.spellcheck = false;
  const list = document.createElement("div");
  list.className = "x4-emotes-list";
  panel.append(search, list);

  /** Код смайла — в поле сообщения чата, через пробел от набранного. */
  const insert = (code) => {
    const input = document.querySelector('[data-a-target="chat-input"]');
    if (!input) return;
    input.focus();
    const text = input.textContent || "";
    const gap = text && !/\s$/.test(text) ? " " : "";
    document.execCommand("insertText", false, `${gap}${code} `);
  };

  const fillPanel = () => {
    const needle = search.value.trim().toLowerCase();
    const groups = [];
    for (const channel of [true, false]) {
      for (const [provider, name] of [["7TV", "7TV"], ["BTTV", "BetterTTV"], ["FFZ", "FrankerFaceZ"]]) {
        groups.push([channel ? `Канал · ${name}` : name, (e) => e.channel === channel && e.provider === provider]);
      }
    }
    const out = document.createDocumentFragment();
    for (const [title, fits] of groups) {
      const grid = document.createElement("div");
      grid.className = "x4-emotes-grid";
      for (const [code, emote] of emotes) {
        if (!fits(emote) || (needle && !code.toLowerCase().includes(needle))) continue;
        const cell = document.createElement("button");
        cell.type = "button";
        cell.title = code;
        const image = document.createElement("img");
        image.src = emote.src;
        image.srcset = `${emote.src} 1x, ${emote.src2} 2x`;
        image.alt = code;
        image.loading = "lazy";
        cell.append(image);
        cell.addEventListener("click", () => insert(code));
        grid.append(cell);
      }
      if (!grid.childElementCount) continue;
      const head = document.createElement("div");
      head.className = "x4-emotes-title";
      head.textContent = title;
      out.append(head, grid);
    }
    if (!out.childNodes.length) {
      const empty = document.createElement("div");
      empty.className = "x4-emotes-empty";
      empty.textContent = emotes.size ? "Ничего не нашлось" : "Смайлы ещё загружаются";
      out.append(empty);
    }
    list.replaceChildren(out);
  };
  search.addEventListener("input", fillPanel);

  /** Панель — над полем сообщения, правым краем к кнопке. */
  const place = () => {
    const rect = toggle.getBoundingClientRect();
    const box = document.querySelector(".chat-input__textarea") || toggle;
    const right = Math.min(window.innerWidth - 8, Math.max(328, rect.right));
    panel.style.left = `${Math.max(8, right - 320)}px`;
    panel.style.bottom = `${Math.max(8, window.innerHeight - box.getBoundingClientRect().top + 6)}px`;
  };
  const closePanel = () => {
    panel.remove();
    toggle.setAttribute("aria-expanded", "false");
  };
  const openPanel = () => {
    search.value = "";
    fillPanel();
    document.body.append(panel);
    place();
    toggle.setAttribute("aria-expanded", "true");
    search.focus({ preventScroll: true });
  };
  toggle.addEventListener("click", () => (panel.isConnected ? closePanel() : openPanel()));
  document.addEventListener(
    "pointerdown",
    (event) => {
      if (panel.isConnected && !panel.contains(event.target) && !toggle.contains(event.target)) closePanel();
    },
    true
  );
  panel.addEventListener("keydown", (event) => {
    if (event.key !== "Escape") return;
    event.stopPropagation();
    closePanel();
    document.querySelector('[data-a-target="chat-input"]')?.focus();
  });
  window.addEventListener("resize", () => panel.isConnected && place());

  /** Кнопка — в строке под полем сообщения, пока смайлы включены. */
  const placeButton = () => {
    const settings = config.emotes ? document.querySelector('[data-a-target="chat-settings"]') : null;
    if (!settings) {
      if (toggle.isConnected) toggle.remove();
      if (panel.isConnected) closePanel();
      return;
    }
    // Кнопка настроек обёрнута в несколько слоёв; ставим рядом с внешним из
    // них, чтобы не попасть внутрь всплывающей подсказки Twitch.
    let anchor = settings;
    const single = (node) => node && node.childElementCount === 1 && !node.matches(".chat-input__buttons-container");
    while (single(anchor.parentElement)) anchor = anchor.parentElement;
    if (toggle.nextElementSibling === anchor) return;
    anchor.before(toggle);
    if (!style.isConnected) (document.head || document.documentElement).append(style);
  };

  /* ── Жизнь страницы ───────────────────────────────────────── */

  // Twitch — одностраничный: канал меняется без загрузки документа. Раз в
  // секунду — сверить канал и чат, раз в пять — бонус; это пара запросов к DOM.
  let ticks = 0;
  const tick = () => {
    sync();
    watchChat();
    placeButton();
    ticks += 1;
    if (ticks % 5 === 0) claim();
  };
  const start = () => {
    tick();
    setInterval(tick, 1000);
  };
  if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", start, { once: true });
  else start();
})();
