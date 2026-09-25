// Расширение Twitch 190x4: встраивается в каждый документ, но работает только
// на twitch.tv.
//
// Бонусы баллов канала: кнопку «Получить бонус» скрипт нажимает сам, как
// только она появилась. Смайлы BetterTTV и FrankerFaceZ: браузер присылает
// список смайлов канала (src-tauri/src/twitch.rs), и в новых сообщениях чата
// их коды становятся картинками. Лучшее качество трансляции — не здесь: его
// делает браузер, отвечая плееру на запрос плейлиста.
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
      if (config.token) sendToken();
      if (!config.emotes) emotes = new Map();
      return;
    }
    if (data.cmd === "twitch_emotes" && data.channel === channel && Array.isArray(data.emotes)) {
      const next = new Map();
      for (const item of data.emotes) {
        if (!Array.isArray(item) || typeof item[0] !== "string" || typeof item[1] !== "string") continue;
        next.set(item[0], { src: item[1], src2: String(item[2] || item[1]), provider: String(item[3] || "") });
      }
      emotes = next;
      // Сообщения, что уже в чате, тоже получают смайлы.
      if (observed) scan(observed);
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
    ".x4-emote img{height:28px;width:auto;vertical-align:middle}";

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

  /* ── Жизнь страницы ───────────────────────────────────────── */

  // Twitch — одностраничный: канал меняется без загрузки документа. Раз в
  // секунду — сверить канал и чат, раз в пять — бонус; это пара запросов к DOM.
  let ticks = 0;
  const tick = () => {
    sync();
    watchChat();
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
