// SponsorBlock 190x4: встраивается в каждый документ и фрейм, но работает
// только в плеере YouTube (и во встроенном в чужие страницы).
//
// Скрипт сообщает браузеру номер видео и получает его сегменты: спонсорские
// вставки, просьбы подписаться, заставки. Сегменты режима «пропускать» он
// пропускает сам и предлагает вернуться, режима «спрашивать» — показывает
// кнопку «Пропустить», а все — отмечает на полосе прокрутки. Сеть и настройки
// живут в браузере (src-tauri/src/sponsorblock.rs).
(() => {
  "use strict";

  if (!/(^|\.)youtube(-nocookie)?\.com$/.test(location.hostname)) return;
  const MARK = Symbol.for("sb");
  if (window[MARK]) return;
  try {
    Object.defineProperty(window, MARK, { value: true, enumerable: false });
  } catch (_) {
    return;
  }

  const LABELS = {
    sponsor: "Спонсорская вставка",
    selfpromo: "Реклама автора",
    interaction: "Просьба подписаться",
    intro: "Заставка",
    outro: "Концовка",
    preview: "Анонс",
    filler: "Отступление",
    music_offtopic: "Не музыка",
  };
  // Цвета категорий — как у SponsorBlock: по ним сегменты узнают на полосе.
  const COLORS = {
    sponsor: "#00d400",
    selfpromo: "#ffff00",
    interaction: "#cc00ff",
    intro: "#00ffff",
    outro: "#0202ed",
    preview: "#008fd6",
    filler: "#7300ff",
    music_offtopic: "#ff9900",
  };

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

  /** Номер видео по адресу: просмотр, короткие видео, встроенный плеер. */
  const videoOf = () => {
    if (location.pathname === "/watch") return new URLSearchParams(location.search).get("v") || "";
    const match = /^\/(?:embed|shorts|live)\/([\w-]{11})/.exec(location.pathname);
    return match ? match[1] : "";
  };

  let video = "";
  let segments = [];
  /** Сегменты, которые человек смотрит сам: вернулся по «Вернуть» или перемотал внутрь. */
  const watched = new Set();
  let player = null;
  let media = null;
  let timer = 0;

  const sync = () => {
    const next = /^[\w-]{11}$/.test(videoOf()) ? videoOf() : "";
    if (next === video) return;
    video = next;
    segments = [];
    watched.clear();
    clearTimeout(timer);
    hideNotice();
    drawMarkers();
    if (video) post({ evt: "sponsorblock_video", video });
  };

  /** Главное видео плеера: у YouTube на странице бывают и превью в ленте. */
  const isMain = (node) =>
    node instanceof HTMLVideoElement &&
    (node.classList.contains("html5-main-video") || !document.querySelector("video.html5-main-video"));

  const playerOf = (node) => node.closest(".html5-video-player") || node.parentElement;
  // Пока идёт реклама YouTube, время плеера — время ролика рекламы.
  const inAd = () => Boolean(player && player.classList.contains("ad-showing"));

  const segmentAt = (time, modes) =>
    segments.find((segment) => modes.includes(segment.mode) && time >= segment.start && time < segment.end - 0.3 && !watched.has(segment.uuid));

  const check = () => {
    clearTimeout(timer);
    if (!media || !segments.length || inAd() || media.seeking) return;
    const time = media.currentTime;
    const skip = segmentAt(time, ["skip"]);
    if (skip) {
      media.currentTime = skip.end;
      showNotice(skip, true);
      return;
    }
    const ask = segmentAt(time, ["ask"]);
    if (ask) showNotice(ask, false);
    else if (notice.segment && !notice.skipped) hideNotice();
    // Следующий сегмент скоро — проверяем точно в его начало, а не на
    // ближайшем timeupdate (он бывает раз в четверть секунды).
    const next = segments.find((segment) => segment.mode === "skip" && segment.start > time && !watched.has(segment.uuid));
    if (next && !media.paused) {
      const wait = (next.start - time) / (media.playbackRate || 1);
      if (wait < 1) timer = setTimeout(check, Math.max(0, wait * 1000));
    }
  };

  const onMedia = (event) => {
    const node = event.target;
    if (node !== media && !isMain(node)) return;
    if (node !== media) {
      media = node;
      player = playerOf(node);
    }
    if (event.type === "loadedmetadata" || event.type === "durationchange") {
      sync();
      drawMarkers();
      return;
    }
    if (event.type === "seeked") {
      // Перемотали внутрь сегмента — человек хочет его посмотреть.
      const inside = segments.find((segment) => media.currentTime >= segment.start && media.currentTime < segment.end);
      if (inside) watched.add(inside.uuid);
    }
    check();
  };
  for (const type of ["timeupdate", "seeked", "play", "loadedmetadata", "durationchange"]) {
    // События медиа не всплывают, но проходят через document при захвате.
    document.addEventListener(type, onMedia, true);
  }
  // Переходы внутри YouTube не перезагружают страницу. Первый запрос — как
  // только документ готов, не дожидаясь, пока плеер загрузит видео.
  for (const type of ["yt-navigate-finish", "popstate"]) window.addEventListener(type, sync);
  document.addEventListener("DOMContentLoaded", sync, { once: true });

  /* ── Сегменты от браузера ─────────────────────────────────── */

  const onMessage = (event) => {
    const data = event.data;
    if (!data || data.cmd !== "sponsorblock_segments" || data.origin !== location.origin || data.video !== video) return;
    segments = (Array.isArray(data.segments) ? data.segments : [])
      .filter((segment) => segment && Number.isFinite(segment.start) && Number.isFinite(segment.end) && LABELS[segment.category])
      .map((segment) => ({
        start: segment.start,
        end: segment.end,
        category: segment.category,
        mode: String(segment.mode),
        uuid: String(segment.uuid || `${segment.start}`),
        duration: Number(segment.duration) || 0,
      }));
    drawMarkers();
    check();
  };
  const subscribe = () => {
    const hook = bridge();
    if (!hook) return false;
    hook.addEventListener("message", onMessage);
    return true;
  };
  if (!subscribe()) document.addEventListener("DOMContentLoaded", subscribe, { once: true });

  /* ── Плашка в плеере ──────────────────────────────────────── */

  const CSS = `
    :host { all: initial; position: absolute; left: 12px; bottom: 72px; z-index: 70; }
    .notice { display: flex; align-items: center; gap: 10px; padding: 8px 8px 8px 12px; background: rgba(13, 13, 16, 0.92); color: #f1f1f4; border: 1px solid rgba(255, 255, 255, 0.12); box-shadow: inset 2px 0 0 #de5772, 0 8px 24px rgba(0, 0, 0, 0.45); font: 500 13px/1.3 "Inter", "Segoe UI Variable Text", "Segoe UI", system-ui, sans-serif; }
    .dot { width: 8px; height: 8px; flex: none; }
    .text small { display: block; margin-top: 1px; font-size: 11px; font-weight: 400; color: #a3a3a8; }
    button { margin: 0; padding: 5px 10px; border: 1px solid rgba(255, 255, 255, 0.16); background: rgba(255, 255, 255, 0.08); color: inherit; font: inherit; font-size: 12.5px; cursor: pointer; }
    button:hover { background: rgba(255, 255, 255, 0.16); }
    button.primary { border-color: transparent; background: #c0304a; color: #fff; }
    button.primary:hover { background: #de5772; }
  `;
  const notice = { host: null, root: null, segment: null, skipped: false, hideTimer: 0 };

  const hideNotice = () => {
    clearTimeout(notice.hideTimer);
    if (notice.host) notice.host.remove();
    notice.segment = null;
    notice.skipped = false;
  };

  const showNotice = (segment, skipped) => {
    if (!player) return;
    if (notice.segment === segment && notice.skipped === skipped && notice.host && notice.host.isConnected) return;
    if (!notice.host) {
      notice.host = document.createElement("div");
      notice.root = notice.host.attachShadow({ mode: "closed" });
      const style = new CSSStyleSheet();
      style.replaceSync(CSS);
      notice.root.adoptedStyleSheets = [style];
    }
    clearTimeout(notice.hideTimer);
    notice.segment = segment;
    notice.skipped = skipped;

    const box = document.createElement("div");
    box.className = "notice";
    const dot = document.createElement("span");
    dot.className = "dot";
    dot.style.background = COLORS[segment.category];
    const text = document.createElement("span");
    text.className = "text";
    text.textContent = skipped ? "Пропущено" : LABELS[segment.category];
    const hint = document.createElement("small");
    hint.textContent = skipped ? LABELS[segment.category] : "SponsorBlock";
    text.append(hint);
    const action = document.createElement("button");
    action.type = "button";
    if (skipped) {
      action.textContent = "Вернуть";
      action.addEventListener("click", (event) => {
        if (!event.isTrusted || !media) return;
        watched.add(segment.uuid);
        media.currentTime = segment.start;
        hideNotice();
      });
      notice.hideTimer = setTimeout(hideNotice, 6000);
    } else {
      action.className = "primary";
      action.textContent = "Пропустить";
      action.addEventListener("click", (event) => {
        if (!event.isTrusted || !media) return;
        media.currentTime = segment.end;
        showNotice(segment, true);
      });
    }
    // Щелчок по плашке не должен ставить видео на паузу.
    for (const type of ["click", "mousedown", "pointerdown", "dblclick"]) {
      box.addEventListener(type, (event) => event.stopPropagation());
    }
    box.append(dot, text, action);
    notice.root.replaceChildren(box);
    if (notice.host.parentNode !== player) player.append(notice.host);
  };

  /* ── Отметки на полосе прокрутки ──────────────────────────── */

  const bar = { host: null, root: null };

  const drawMarkers = () => {
    // Во время рекламы YouTube длина плеера — длина ролика рекламы.
    const track = player && !inAd() && player.querySelector(".ytp-progress-bar");
    const duration = media && Number.isFinite(media.duration) && media.duration > 0 ? media.duration : (segments[0] && segments[0].duration) || 0;
    if (!track || !segments.length || !duration) {
      if (bar.host) bar.host.remove();
      return;
    }
    if (!bar.host) {
      bar.host = document.createElement("div");
      bar.host.style.cssText = "position:absolute;left:0;right:0;bottom:0;height:100%;pointer-events:none;z-index:32";
      bar.root = bar.host.attachShadow({ mode: "closed" });
    }
    const marks = segments.map((segment) => {
      const mark = document.createElement("div");
      const left = Math.min(100, (segment.start / duration) * 100);
      const width = Math.max(0.2, Math.min(100 - left, ((segment.end - segment.start) / duration) * 100));
      mark.style.cssText = `position:absolute;top:0;bottom:0;left:${left}%;width:${width}%;background:${COLORS[segment.category]};opacity:${segment.mode === "show" ? 0.55 : 0.8}`;
      mark.title = LABELS[segment.category];
      return mark;
    });
    bar.root.replaceChildren(...marks);
    if (bar.host.parentNode !== track) track.append(bar.host);
  };
})();
