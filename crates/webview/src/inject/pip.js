// Мини-плеер 190x4: вкладка уже переехала в маленькое окно поверх всех, а этот
// скрипт разворачивает главное видео страницы на всё окно и рисует поверх него
// свои кнопки — пауза, перемотка, время, громкость, «вернуть во вкладку» и
// «закрыть». Браузер выполняет его как действие человека (с жестом), иначе
// страница не пустила бы видео во весь экран.
//
// Результат — строка: `ok` (видео во весь экран), `css` (экран не дали, видео
// растянуто стилем), `none` (видео нет). Выход — `window[Symbol.for("x4pip")].exit`.
(async () => {
  "use strict";

  const KEY = Symbol.for("x4pip");
  if (window[KEY]) return "ok";

  /* ── Какое видео ─────────────────────────────────────────── */

  // Видео ищется и во фреймах своего сайта: плеер часто живёт во фрейме.
  const found = [];
  const collect = (doc, chain) => {
    for (const video of doc.querySelectorAll("video")) found.push({ video, chain });
    for (const frame of doc.querySelectorAll("iframe")) {
      let inner = null;
      try {
        inner = frame.contentDocument;
      } catch (_) {
        inner = null;
      }
      if (inner) collect(inner, [...chain, frame]);
    }
  };
  collect(document, []);
  const area = ({ video }) => {
    const rect = video.getBoundingClientRect();
    return rect.width * rect.height;
  };
  const playable = found.filter(({ video }) => video.readyState > 0 || video.currentSrc);
  playable.sort((a, b) => Number(!b.video.paused) - Number(!a.video.paused) || area(b) - area(a));
  const target = playable[0];
  if (!target) return "none";
  const { video, chain } = target;

  /* ── Видео на всё окно ───────────────────────────────────── */

  // Во весь экран уходит само видео, а если оно во фрейме — фрейм верхнего
  // документа; внутри фреймов видео растягивается стилем.
  const saved = [];
  const pin = (node) => {
    saved.push([node, node.style.cssText]);
    const set = (name, value) => node.style.setProperty(name, value, "important");
    set("position", "fixed");
    set("inset", "0");
    set("width", "100%");
    set("height", "100%");
    set("max-width", "none");
    set("max-height", "none");
    set("margin", "0");
    set("transform", "none");
    set("z-index", "2147483646");
    set("background", "#000");
    if (node === video) set("object-fit", "contain");
  };
  const unpin = () => {
    for (const [node, css] of saved.reverse()) node.style.cssText = css;
    saved.length = 0;
  };
  for (const [index, frame] of chain.entries()) {
    if (index > 0) pin(frame);
  }
  if (chain.length) pin(video);

  const screenTarget = chain[0] || video;
  let mode = "ok";
  try {
    await screenTarget.requestFullscreen({ navigationUI: "hide" });
  } catch (_) {
    // Экран не дали — растягиваем стилем и сам фрейм (или видео), а
    // прокрутку страницы убираем.
    mode = "css";
    pin(screenTarget);
    saved.push([document.documentElement, document.documentElement.style.cssText]);
    document.documentElement.style.setProperty("overflow", "hidden", "important");
  }

  /* ── Кнопки ──────────────────────────────────────────────── */

  const SVG = "http://www.w3.org/2000/svg";
  const PATHS = {
    play: ["M6.5 4.25v11.5L15.75 10z"],
    pause: ["M5.75 4.5H8.5v11H5.75zM11.5 4.5h2.75v11H11.5z"],
    volume: ["M3 7.5h3l4-3.25v11.5L6 12.5H3z", "M12.75 7.5 14.5 10l-1.75 2.5M15.25 5.25 17.5 10l-2.25 4.75"],
    mute: ["M3 7.5h3l4-3.25v11.5L6 12.5H3z", "M13 8l4 4M17 8l-4 4"],
    back: ["M9 3.75H3.75v12.5h12.5V11", "M16.25 3.75 9.75 10.25M9.75 5.75v4.5h4.5"],
    close: ["M5 5l10 10M15 5 5 15"],
  };
  const glyph = (name) => {
    const svg = document.createElementNS(SVG, "svg");
    svg.setAttribute("viewBox", "1 1 18 18");
    svg.setAttribute("aria-hidden", "true");
    for (const d of PATHS[name]) {
      const path = document.createElementNS(SVG, "path");
      path.setAttribute("d", d);
      svg.append(path);
    }
    return svg;
  };
  const make = (tag, className, text) => {
    const node = document.createElement(tag);
    if (className) node.className = className;
    if (text != null) node.textContent = text;
    return node;
  };
  const button = (name, title, onClick) => {
    const node = make("button", "btn");
    node.type = "button";
    node.title = title;
    node.setAttribute("aria-label", title);
    node.append(glyph(name));
    node.addEventListener("click", (event) => {
      event.stopPropagation();
      if (event.isTrusted) onClick();
    });
    return node;
  };

  const CSS = `
    :host { all: initial; position: fixed; inset: 0; width: 100%; height: 100%; margin: 0; padding: 0; border: 0;
      background: transparent; overflow: hidden; pointer-events: none; color: #f1f1f4;
      font: 500 12.5px/1.2 "Segoe UI Variable Text", "Segoe UI", system-ui, sans-serif; }
    .ui { position: absolute; inset: 0; display: flex; flex-direction: column; justify-content: space-between;
      opacity: 1; transition: opacity 0.18s ease; }
    .ui[data-hidden="true"] { opacity: 0; }
    .ui[data-hidden="true"] * { pointer-events: none !important; }
    .top, .bottom { pointer-events: auto; display: flex; align-items: center; gap: 4px; padding: 6px 8px; }
    .top { background: linear-gradient(rgba(0, 0, 0, 0.72), transparent); }
    .bottom { flex-direction: column; align-items: stretch; gap: 2px; padding-top: 18px;
      background: linear-gradient(transparent, rgba(0, 0, 0, 0.8)); }
    .title { flex: 1; min-width: 0; overflow: hidden; white-space: nowrap; text-overflow: ellipsis; opacity: 0.85; }
    .row { display: flex; align-items: center; gap: 4px; }
    .time { font-variant-numeric: tabular-nums; opacity: 0.9; white-space: nowrap; }
    .live { color: #de5772; font-weight: 700; letter-spacing: 0.6px; }
    .spacer { flex: 1; }
    .btn { display: grid; place-items: center; width: 30px; height: 30px; padding: 0; border: 0; background: transparent;
      color: inherit; cursor: pointer; clip-path: polygon(0 0, calc(100% - 6px) 0, 100% 6px, 100% 100%, 0 100%); }
    .btn:hover { background: rgba(255, 255, 255, 0.14); color: #fff; }
    .btn:focus-visible { outline: 2px solid #de5772; outline-offset: -2px; }
    .btn svg { width: 18px; height: 18px; fill: none; stroke: currentColor; stroke-width: 1.4; }
    input[type="range"] { -webkit-appearance: none; appearance: none; margin: 0; height: 16px; background: transparent; cursor: pointer; }
    input[type="range"]::-webkit-slider-runnable-track { height: 3px;
      background: linear-gradient(90deg, #de5772 var(--fill, 0%), rgba(255, 255, 255, 0.28) var(--fill, 0%)); }
    input[type="range"]::-webkit-slider-thumb { -webkit-appearance: none; width: 11px; height: 11px; margin-top: -4px;
      background: #fff; border: 0; clip-path: polygon(50% 0, 100% 50%, 50% 100%, 0 50%); }
    .seek { width: 100%; }
    .volume { width: 72px; }
  `;

  const host = document.createElement("div");
  host.setAttribute("popover", "manual");
  const root = host.attachShadow({ mode: "closed" });
  const sheet = new CSSStyleSheet();
  sheet.replaceSync(CSS);
  root.adoptedStyleSheets = [sheet];

  const ui = make("div", "ui");
  const top = make("div", "top");
  const title = make("span", "title", document.title || location.hostname);
  top.append(
    title,
    button("back", "Вернуть во вкладку (Esc)", () => post("pip_back")),
    button("close", "Закрыть мини-плеер", () => post("pip_close"))
  );

  const seek = make("input", "seek");
  seek.type = "range";
  seek.min = "0";
  seek.max = "1000";
  seek.step = "1";
  seek.title = "Перемотка";
  const playButton = button("pause", "Пауза (пробел)", () => togglePlay());
  const time = make("span", "time");
  const muteButton = button("volume", "Выключить звук (M)", () => {
    video.muted = !video.muted;
    if (!video.muted && video.volume === 0) video.volume = 0.5;
  });
  const volume = make("input", "volume");
  volume.type = "range";
  volume.min = "0";
  volume.max = "100";
  volume.step = "1";
  volume.title = "Громкость";
  const row = make("div", "row");
  row.append(playButton, time, make("span", "spacer"), muteButton, volume);
  const bottom = make("div", "bottom");
  bottom.append(seek, row);
  ui.append(top, bottom);
  root.append(ui);
  document.documentElement.append(host);
  try {
    host.showPopover();
  } catch (_) {
    // Нет верхнего слоя — остаётся поверх стилем.
  }

  /* ── Состояние ───────────────────────────────────────────── */

  const clock = (seconds) => {
    if (!Number.isFinite(seconds)) return "0:00";
    const total = Math.max(0, Math.floor(seconds));
    const h = Math.floor(total / 3600);
    const m = Math.floor((total % 3600) / 60);
    const s = String(total % 60).padStart(2, "0");
    return h ? `${h}:${String(m).padStart(2, "0")}:${s}` : `${m}:${s}`;
  };
  const setIcon = (node, name, label) => {
    node.replaceChildren(glyph(name));
    node.title = label;
    node.setAttribute("aria-label", label);
  };

  let seeking = false;
  const render = () => {
    const live = !Number.isFinite(video.duration);
    seek.hidden = live;
    if (live) {
      time.replaceChildren(make("span", "live", "В ЭФИРЕ"));
    } else {
      time.textContent = `${clock(video.currentTime)} / ${clock(video.duration)}`;
      if (!seeking && video.duration > 0) {
        seek.value = String(Math.round((video.currentTime / video.duration) * 1000));
      }
      seek.style.setProperty("--fill", `${Number(seek.value) / 10}%`);
    }
    setIcon(playButton, video.paused ? "play" : "pause", video.paused ? "Играть (пробел)" : "Пауза (пробел)");
    const silent = video.muted || video.volume === 0;
    setIcon(muteButton, silent ? "mute" : "volume", silent ? "Включить звук (M)" : "Выключить звук (M)");
    volume.value = String(silent ? 0 : Math.round(video.volume * 100));
    volume.style.setProperty("--fill", `${volume.value}%`);
  };

  seek.addEventListener("input", () => {
    seeking = true;
    if (video.duration > 0) video.currentTime = (Number(seek.value) / 1000) * video.duration;
    render();
  });
  seek.addEventListener("change", () => {
    seeking = false;
  });
  volume.addEventListener("input", () => {
    const value = Number(volume.value) / 100;
    video.volume = value;
    video.muted = value === 0;
  });

  const togglePlay = () => {
    if (video.paused) video.play().catch(() => {});
    else video.pause();
  };
  const nudgeVolume = (delta) => {
    video.muted = false;
    video.volume = Math.min(1, Math.max(0, video.volume + delta));
  };
  const skip = (seconds) => {
    if (Number.isFinite(video.duration)) video.currentTime = Math.min(video.duration, Math.max(0, video.currentTime + seconds));
  };

  const MEDIA = ["play", "pause", "timeupdate", "durationchange", "loadedmetadata", "volumechange", "seeked"];
  for (const type of MEDIA) video.addEventListener(type, render);
  render();

  /* ── Кнопки прячутся, пока мышь стоит ────────────────────── */

  let hideTimer = 0;
  const wake = () => {
    ui.dataset.hidden = "false";
    video.style.removeProperty("cursor");
    clearTimeout(hideTimer);
    hideTimer = setTimeout(() => {
      if (video.paused || seeking) return;
      ui.dataset.hidden = "true";
      video.style.setProperty("cursor", "none");
    }, 2500);
  };
  wake();

  /* ── Мышь и клавиатура ───────────────────────────────────── */

  const bridge = () => (window.chrome && window.chrome.webview) || null;
  const post = (evt) => {
    const hook = bridge();
    if (!hook) return;
    try {
      hook.postMessage({ evt });
    } catch (_) {
      // Мост закрыт — документ уходит.
    }
  };

  // Щелчок по видео — пауза, а потянули — окно едет за мышью.
  let press = null;
  let dragged = false;
  const onDown = (event) => {
    wake();
    if (event.button !== 0 || !event.isTrusted) return;
    press = { x: event.screenX, y: event.screenY };
    dragged = false;
  };
  const onMove = (event) => {
    wake();
    if (!press || !(event.buttons & 1)) return;
    if (Math.abs(event.screenX - press.x) + Math.abs(event.screenY - press.y) < 5) return;
    press = null;
    dragged = true;
    post("pip_drag");
  };
  const onClick = (event) => {
    if (!event.isTrusted) return;
    event.preventDefault();
    event.stopPropagation();
    if (!dragged) togglePlay();
    press = null;
    dragged = false;
  };
  const onWheel = (event) => {
    event.preventDefault();
    wake();
    nudgeVolume(event.deltaY < 0 ? 0.05 : -0.05);
  };
  const onKey = (event) => {
    if (event.ctrlKey || event.altKey || event.metaKey) return;
    const actions = {
      " ": togglePlay,
      k: togglePlay,
      ArrowLeft: () => skip(-5),
      ArrowRight: () => skip(5),
      j: () => skip(-10),
      l: () => skip(10),
      ArrowUp: () => nudgeVolume(0.1),
      ArrowDown: () => nudgeVolume(-0.1),
      m: () => (video.muted = !video.muted),
      Escape: () => post("pip_back"),
    };
    const action = actions[event.key.length === 1 ? event.key.toLowerCase() : event.key];
    if (!action) return;
    event.preventDefault();
    event.stopImmediatePropagation();
    wake();
    action();
  };

  // Слушаем и документ страницы, и документ видео, если оно во фрейме.
  const windows = [...new Set([window, video.ownerDocument.defaultView])];
  const listeners = [
    ["mousedown", onDown],
    ["mousemove", onMove],
    ["click", onClick],
    ["wheel", onWheel],
    ["keydown", onKey],
  ];
  const inVideo = (handler) => (event) => {
    // Свои кнопки обрабатывают щелчки сами.
    if (event.composedPath().includes(host)) {
      if (event.type === "mousemove") wake();
      return;
    }
    handler(event);
  };
  const wrapped = listeners.map(([type, handler]) => [type, type === "keydown" ? handler : inVideo(handler)]);
  for (const target of windows) {
    for (const [type, handler] of wrapped) {
      target.addEventListener(type, handler, { capture: true, passive: type === "mousedown" || type === "mousemove" });
    }
  }

  // Видео свернули из экрана не мы (Esc, страница сама) или оно пропало со
  // страницы — вкладка возвращается на место.
  let leaving = false;
  const onFullscreen = () => {
    if (!leaving && mode === "ok" && !document.fullscreenElement) post("pip_back");
  };
  document.addEventListener("fullscreenchange", onFullscreen);
  const watch = setInterval(() => {
    if (!video.isConnected) post("pip_back");
  }, 1000);

  const exit = (pause) => {
    leaving = true;
    clearInterval(watch);
    clearTimeout(hideTimer);
    document.removeEventListener("fullscreenchange", onFullscreen);
    for (const type of MEDIA) video.removeEventListener(type, render);
    for (const target of windows) {
      for (const [type, handler] of wrapped) target.removeEventListener(type, handler, { capture: true });
    }
    video.style.removeProperty("cursor");
    host.remove();
    unpin();
    if (document.fullscreenElement) document.exitFullscreen().catch(() => {});
    if (pause) video.pause();
    delete window[KEY];
  };
  Object.defineProperty(window, KEY, { value: { exit }, configurable: true, enumerable: false });
  return mode;
})();
