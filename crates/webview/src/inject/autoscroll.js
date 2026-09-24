// Автопролистывание 190x4: встраивается в каждый документ, но работает только
// в лентах коротких видео — YouTube Shorts, Reels в Instagram и TikTok.
//
// Ролики там зациклены и события конца не дают, поэтому скрипт смотрит, сколько
// осталось главному ролику на экране, и за миг до конца спрашивает браузер,
// листать ли дальше. Решает браузер (настройки расширения, кнопка на панели,
// src-tauri/src/autoscroll.rs); сказал «дальше» — скрипт листает ленту так же,
// как человек: кнопкой «Следующее видео», прокруткой или стрелкой вниз, и
// проверяет, что ролик сменился.
(() => {
  "use strict";

  const host = location.hostname;
  const SITE = /(^|\.)youtube\.com$/.test(host)
    ? "youtube"
    : /(^|\.)instagram\.com$/.test(host)
      ? "instagram"
      : /(^|\.)tiktok\.com$/.test(host)
        ? "tiktok"
        : "";
  if (!SITE || window.top !== window) return;
  const MARK = Symbol.for("as");
  if (window[MARK]) return;
  try {
    Object.defineProperty(window, MARK, { value: true, enumerable: false });
  } catch (_) {
    return;
  }

  /** Лента ли это: у YouTube и Instagram короткие видео живут по своим адресам. */
  const onFeed = () =>
    SITE === "youtube" ? location.pathname.startsWith("/shorts/") : SITE === "instagram" ? /^\/reels?\//.test(location.pathname) : true;

  const bridge = () => (window.chrome && window.chrome.webview) || null;

  /** Сколько от ролика видно на экране, 0…1. Мелкие превью роликом ленты не считаются. */
  const shown = (video) => {
    const rect = video.getBoundingClientRect();
    if (rect.height < innerHeight * 0.4 || !rect.width) return 0;
    const height = Math.min(rect.bottom, innerHeight) - Math.max(rect.top, 0);
    const width = Math.min(rect.right, innerWidth) - Math.max(rect.left, 0);
    return height > 0 && width > 0 ? (height * width) / (rect.height * rect.width) : 0;
  };

  /** Главный ролик ленты: играет и виден больше всех. */
  const current = () => {
    let best = null;
    let share = 0.6;
    for (const video of document.querySelectorAll("video")) {
      if (video.paused || !(video.duration > 0)) continue;
      const part = shown(video);
      if (part > share) {
        best = video;
        share = part;
      }
    }
    return best;
  };

  /** Человек пишет комментарий или ищет — ленту из-под него не уводим. */
  const typing = () => {
    const active = document.activeElement;
    return Boolean(active && (active.isContentEditable || /^(INPUT|TEXTAREA|SELECT)$/.test(active.tagName)));
  };

  // Вопрос уходит один раз за проигрыш: следующий — когда ролик снова далеко
  // от конца (пошёл на новый круг, потому что листать не велели) или сменился.
  let asked = null;
  let armed = true;

  const onTime = (event) => {
    const video = event.target;
    if (!(video instanceof HTMLVideoElement) || video.paused || !onFeed()) return;
    const duration = video.duration;
    // Прямой эфир и совсем короткое — не ролики ленты.
    if (!Number.isFinite(duration) || duration < 3) return;
    const left = (duration - video.currentTime) / (video.playbackRate || 1);
    if (left > 1.5) {
      armed = true;
      return;
    }
    if (!armed || left > 0.35 || document.visibilityState !== "visible" || typing()) return;
    if (video !== current()) return;
    armed = false;
    asked = { video, href: location.href, src: video.currentSrc };
    const hook = bridge();
    if (hook) {
      try {
        hook.postMessage({ evt: "autoscroll_end" });
      } catch (_) {
        // Мост закрыт — документ уходит.
      }
    }
  };
  // События медиа не всплывают, но проходят через document при захвате.
  document.addEventListener("timeupdate", onTime, true);

  /* ── Как листать ──────────────────────────────────────────── */

  /** Кнопка «Следующее видео» самого сайта — самый честный способ. */
  const clickNext = () => {
    if (SITE !== "youtube") return false;
    const button = document.querySelector("#navigation-button-down button");
    if (!button || button.disabled) return false;
    button.click();
    return true;
  };

  /** Прокрутить ленту к следующему ролику: к соседнему элементу той же высоты. */
  const scrollNext = (video) => {
    const height = video.getBoundingClientRect().height;
    let node = video.parentElement;
    for (let depth = 0; node && node !== document.body && depth < 14; depth++, node = node.parentElement) {
      const next = node.nextElementSibling;
      const own = node.getBoundingClientRect().height;
      if (next && own >= height * 0.6) {
        const other = next.getBoundingClientRect().height;
        if (other > own * 0.65 && other < own * 1.5) {
          next.scrollIntoView({ block: "start", behavior: "smooth" });
          return true;
        }
      }
      const style = getComputedStyle(node);
      if (/(auto|scroll)/.test(style.overflowY) && node.scrollHeight > node.clientHeight + 10) {
        node.scrollBy({ top: node.clientHeight, behavior: "smooth" });
        return true;
      }
    }
    return false;
  };

  /** Стрелка вниз — её понимают все три ленты. */
  const pressDown = () => {
    const target = document.activeElement && document.activeElement !== document.body ? document.activeElement : document;
    for (const type of ["keydown", "keyup"]) {
      target.dispatchEvent(new KeyboardEvent(type, { key: "ArrowDown", code: "ArrowDown", keyCode: 40, which: 40, bubbles: true, cancelable: true }));
    }
    return true;
  };

  /**
   * Ролик сменился: другой адрес, прежний ушёл с экрана или в нём уже другое
   * видео. Новый ролик может ещё не заиграть — ждать его нельзя, иначе
   * следующий способ пролистал бы ещё один.
   */
  const moved = (from) => {
    if (location.href !== from.href || !from.video.isConnected || shown(from.video) < 0.5) return true;
    const now = current();
    return Boolean(now && (now !== from.video || now.currentSrc !== from.src));
  };

  const advance = (from) => {
    const ways = [clickNext, scrollNext, pressDown];
    const step = (index) => {
      if (index >= ways.length || moved(from)) return;
      if (!ways[index](from.video)) {
        step(index + 1);
        return;
      }
      setTimeout(() => step(index + 1), 1200);
    };
    step(0);
  };

  const onMessage = (event) => {
    const data = event.data;
    if (!data || data.cmd !== "autoscroll_next" || data.origin !== location.origin) return;
    const from = asked;
    asked = null;
    // Ответ опоздал: человек уже пролистал сам.
    if (!from || moved(from)) return;
    advance(from);
  };
  const subscribe = () => {
    const hook = bridge();
    if (!hook) return false;
    hook.addEventListener("message", onMessage);
    return true;
  };
  if (!subscribe()) document.addEventListener("DOMContentLoaded", subscribe, { once: true });
})();
