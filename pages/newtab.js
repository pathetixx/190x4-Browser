/**
 * Новая вкладка 190x4 Browser.
 *
 * Страница живёт в обычной вкладке (http://190x4-pages.invalid) и о Tauri не
 * знает: запросы уходят в браузер через window.chrome.webview.postMessage,
 * ответы приходят событием message. Плитки, город и настройки хранит браузер,
 * здесь — только отрисовка. Открытая без движка (для ревью вёрстки) страница
 * показывает демонстрационные данные.
 */
(() => {
  "use strict";

  const webview = window.chrome?.webview ?? null;
  const SPRITE = "./icons.svg";
  const SVG = "http://www.w3.org/2000/svg";
  const MAX_TILES = 24;
  const WEATHER_EVERY = 20 * 60 * 1000;
  const ICON_KEY = "190x4:icon:";

  const $ = (id) => document.getElementById(id);
  const root = document.documentElement;
  const deck = $("deck");
  const params = new URLSearchParams(location.search);
  if (params.get("motion") === "off") root.dataset.motion = "off";

  const state = {
    tiles: [],
    weather: true,
    monitor: true,
    seconds: true,
    city: null,
    theme: "kurogane",
    loaded: false,
  };
  const icons = new Map();
  const iconsAsked = new Set();

  /* ── Мост ─────────────────────────────────────────────────── */

  function send(message) {
    if (webview) webview.postMessage(message);
    else Demo.handle(message);
  }

  function receive(message) {
    if (!message || typeof message !== "object") return;
    switch (message.type) {
      case "state":
        applyState(message);
        break;
      case "icon":
        storeIcon(message.url, message.icon ?? null);
        break;
      case "weather":
        renderWeather(message);
        break;
      case "cities":
        renderCities(message);
        break;
      case "resources":
        renderResources(message.report);
        break;
    }
  }

  if (webview) webview.addEventListener("message", (event) => receive(event.data));

  /* ── Мелочи ───────────────────────────────────────────────── */

  function h(tag, className, text) {
    const node = document.createElement(tag);
    if (className) node.className = className;
    if (text != null) node.textContent = text;
    return node;
  }

  function glyph(name, className) {
    const svg = document.createElementNS(SVG, "svg");
    if (className) svg.setAttribute("class", className);
    svg.setAttribute("aria-hidden", "true");
    const use = document.createElementNS(SVG, "use");
    use.setAttribute("href", `${SPRITE}#i-${name}`);
    svg.append(use);
    return svg;
  }

  function setGlyph(svg, name) {
    svg.querySelector("use").setAttribute("href", `${SPRITE}#i-${name}`);
  }

  const pad = (value) => String(value).padStart(2, "0");
  const oneDecimal = new Intl.NumberFormat("ru-RU", { minimumFractionDigits: 1, maximumFractionDigits: 1 });
  const twoDecimals = new Intl.NumberFormat("ru-RU", { minimumFractionDigits: 2, maximumFractionDigits: 2 });
  const whole = new Intl.NumberFormat("ru-RU", { maximumFractionDigits: 0 });

  function bytes(value) {
    const mb = value / 1024 / 1024;
    if (mb >= 1024) {
      const gb = mb / 1024;
      return { value: gb >= 10 ? oneDecimal.format(gb) : twoDecimals.format(gb), unit: "ГБ" };
    }
    return { value: whole.format(mb), unit: "МБ" };
  }

  const bytesText = (value) => {
    const { value: number, unit } = bytes(value);
    return `${number} ${unit}`;
  };

  const percent = (value) => (Number.isFinite(value) ? `${oneDecimal.format(value)}%` : "…");

  function temperature(value) {
    if (!Number.isFinite(value)) return "—";
    const rounded = Math.round(value);
    if (rounded > 0) return `+${rounded}°`;
    if (rounded < 0) return `−${Math.abs(rounded)}°`;
    return "0°";
  }

  function originOf(url) {
    try {
      const parsed = new URL(url);
      return /^https?:$/.test(parsed.protocol) && parsed.hostname !== "190x4-pages.invalid" ? parsed.origin : "";
    } catch {
      return "";
    }
  }

  function hostOf(url) {
    try {
      return new URL(url).hostname.replace(/^www\./, "");
    } catch {
      return url;
    }
  }

  /** Буква на плитке без значка: одно слово — одна буква, два — две. */
  function monogram(title) {
    const words = String(title).trim().split(/\s+/).filter(Boolean);
    const letters = words.length > 1 ? words[0][0] + words[1][0] : (words[0] ?? "").slice(0, 1);
    return letters.toUpperCase() || "·";
  }

  /* ── Настройки страницы ──────────────────────────────────── */

  function applyState(message) {
    state.tiles = Array.isArray(message.tiles) ? message.tiles : [];
    state.weather = message.weather !== false;
    state.monitor = message.monitor !== false;
    state.seconds = message.seconds !== false;
    state.city = message.city ?? null;
    state.theme = message.theme ?? "kurogane";
    applyTheme();
    applyVisibility();
    // Состояние приходит при каждом показе вкладки: плитки пересобираются,
    // только если они сменились, — иначе значки мигали подложкой.
    if (tilesKey() !== tilesShown) renderTiles();

    if (state.weather && (!weatherAt || Date.now() - weatherAt > WEATHER_EVERY)) requestWeather(false);
    state.loaded = true;
    reveal();
  }

  /**
   * Показать страницу целиком, когда есть плитки и загружены шрифты: без этого
   * часы, поиск и карточки появлялись по очереди, а текст перескакивал со
   * шрифта системы на свой. Дольше полусекунды не ждём.
   */
  function reveal() {
    if (!root.dataset.boot) return;
    const fonts = document.fonts?.ready ?? Promise.resolve();
    Promise.race([fonts, new Promise((resolve) => setTimeout(resolve, 250))]).then(() =>
      requestAnimationFrame(() => delete root.dataset.boot)
    );
  }
  setTimeout(() => delete root.dataset.boot, 500);

  function applyTheme() {
    const dark = matchMedia("(prefers-color-scheme: dark)").matches;
    root.dataset.theme = state.theme === "system" ? (dark ? "kurogane" : "shiro") : state.theme === "shiro" ? "shiro" : "kurogane";
    // Следующая новая вкладка встанет в эту тему с первого кадра (newtab.html).
    try {
      localStorage.setItem("190x4-theme", root.dataset.theme);
    } catch {}
  }
  matchMedia("(prefers-color-scheme: dark)").addEventListener("change", () => {
    applyTheme();
    // Подложка значка подбирается под тему.
    if (tilesKey() !== tilesShown) renderTiles();
  });

  function applyVisibility() {
    $("weather").hidden = !state.weather;
    $("monitor").hidden = !state.monitor;
    deck.dataset.weather = state.weather ? "on" : "off";
    deck.dataset.monitor = state.monitor ? "on" : "off";
    deck.dataset.seconds = state.seconds ? "on" : "off";
    for (const input of document.querySelectorAll("[data-pref]")) input.checked = state[input.dataset.pref];
    syncMonitor();
  }

  const gear = $("customize");
  const customize = $("customize-pop");
  gear.addEventListener("click", (event) => {
    event.stopPropagation();
    const open = customize.hidden;
    closePops();
    customize.hidden = !open;
    gear.setAttribute("aria-expanded", String(open));
  });
  for (const input of document.querySelectorAll("[data-pref]")) {
    input.addEventListener("change", () => {
      state[input.dataset.pref] = input.checked;
      applyVisibility();
      if (input.dataset.pref === "weather" && input.checked && !weatherAt) requestWeather(false);
      send({ evt: "newtab_prefs", [input.dataset.pref]: input.checked });
    });
  }

  function closePops() {
    customize.hidden = true;
    gear.setAttribute("aria-expanded", "false");
    closeTileMenu();
  }
  document.addEventListener("click", (event) => {
    if (!event.target.closest(".pop")) closePops();
  });
  document.addEventListener("keydown", (event) => {
    if (event.key === "Escape") {
      closePops();
      if (!$("picker").hidden) closePicker();
    }
    const typing = event.target.closest("input, textarea, [contenteditable]");
    if (event.key === "/" && !typing) {
      event.preventDefault();
      $("q").focus();
    }
  });

  /* ── Часы ─────────────────────────────────────────────────── */

  const ARC_RADIUS = 126;
  const ARC_LENGTH = 2 * Math.PI * ARC_RADIUS;

  function buildHud() {
    const at = (degrees, radius) => {
      const angle = (degrees * Math.PI) / 180;
      return [200 + Math.cos(angle) * radius, 200 + Math.sin(angle) * radius];
    };
    const f = (value) => value.toFixed(1);
    const line = (degrees, from, to, className) => {
      const [x1, y1] = at(degrees, from);
      const [x2, y2] = at(degrees, to);
      return `<line class="${className}" x1="${f(x1)}" y1="${f(y1)}" x2="${f(x2)}" y2="${f(y2)}"/>`;
    };
    const arc = (radius, from, to, className) => {
      const [x1, y1] = at(from, radius);
      const [x2, y2] = at(to, radius);
      return `<path class="${className}" d="M ${f(x1)} ${f(y1)} A ${radius} ${radius} 0 0 1 ${f(x2)} ${f(y2)}"/>`;
    };

    let outer = '<circle class="hud__line" cx="200" cy="200" r="194"/>';
    for (let i = 0; i < 72; i++) {
      const major = i % 6 === 0;
      outer += line(i * 5, major ? 182 : 187, 194, major ? "hud__tick hud__tick--major" : "hud__tick");
    }

    const segment = (2 * Math.PI * 164) / 5;
    const seg =
      `<circle class="hud__seg-ring" cx="200" cy="200" r="164" stroke-dasharray="${f(segment * 0.58)} ${f(segment * 0.42)}"/>` +
      '<circle class="hud__dots" cx="200" cy="200" r="174" stroke-dasharray="1.5 5"/>';

    let fixed = "";
    for (const degrees of [45, 135, 225, 315]) fixed += arc(190, degrees - 13, degrees + 13, "hud__bracket");
    for (const degrees of [0, 90, 180, 270]) fixed += line(degrees, 198, 212, "hud__cross");
    for (let i = 0; i < 60; i++) {
      const major = i % 5 === 0;
      fixed += line(i * 6 - 90, major ? 131 : 133, 137, major ? "hud__second hud__second--major" : "hud__second");
    }

    // Каждая движущаяся часть — свой слой: кольца крутит и маркер ведёт
    // композитор, не перерисовывая циферблат. Внутри одного SVG вращение
    // перерисовывало весь HUD с тенями 60 раз в секунду, пока вкладка открыта.
    const layer = (className, content, id = "") =>
      `<svg class="hud__layer ${className}"${id ? ` id="${id}"` : ""} viewBox="0 0 400 400">${content}</svg>`;
    $("hud").innerHTML =
      layer("", `${fixed}<g transform="rotate(-90 200 200)"><circle class="hud__track" cx="200" cy="200" r="${ARC_RADIUS}"/></g>`) +
      layer("hud__outer", outer) +
      layer("hud__seg", seg) +
      layer(
        "hud__sweep",
        `<circle class="hud__arc" id="hud-arc" cx="200" cy="200" r="${ARC_RADIUS}" transform="rotate(-90 200 200)"
          stroke-dasharray="${f(ARC_LENGTH)}" stroke-dashoffset="${f(ARC_LENGTH)}"/>`
      ) +
      layer("hud__marker", `<circle cx="200" cy="${200 - ARC_RADIUS}" r="3.2"/>`, "hud-marker") +
      layer(
        "",
        `<defs>
          <path id="hud-top" d="M 50 200 A 150 150 0 0 1 350 200"/>
          <path id="hud-bottom" d="M 50 200 A 150 150 0 0 0 350 200"/>
        </defs>
        <text class="hud__text"><textPath href="#hud-top" startOffset="50%" text-anchor="middle" id="hud-zone">МЕСТНОЕ ВРЕМЯ</textPath></text>
        <text class="hud__text hud__text--accent" dy="9"><textPath href="#hud-bottom" startOffset="50%" text-anchor="middle" id="hud-week"></textPath></text>`
      );
  }

  function isoWeek(date) {
    const day = new Date(Date.UTC(date.getFullYear(), date.getMonth(), date.getDate()));
    const weekday = day.getUTCDay() || 7;
    day.setUTCDate(day.getUTCDate() + 4 - weekday);
    const yearStart = new Date(Date.UTC(day.getUTCFullYear(), 0, 1));
    return Math.ceil(((day - yearStart) / 86400000 + 1) / 7);
  }

  function dayOfYear(date) {
    const start = new Date(date.getFullYear(), 0, 0);
    return Math.floor((date - start - (date.getTimezoneOffset() - start.getTimezoneOffset()) * 60000) / 86400000);
  }

  let lastMinute = -1;
  let tickTimer = 0;

  function tick() {
    const now = new Date();
    const seconds = now.getSeconds();
    $("hh").textContent = pad(now.getHours());
    $("mm").textContent = pad(now.getMinutes());
    $("sec").textContent = pad(seconds);

    const arc = $("hud-arc");
    const marker = $("hud-marker");
    const jump = seconds === 0;
    arc.classList.toggle("jump", jump);
    marker.classList.toggle("jump", jump);
    arc.style.strokeDashoffset = String(ARC_LENGTH * (1 - seconds / 60));
    marker.style.transform = `rotate(${seconds * 6}deg)`;

    if (now.getMinutes() !== lastMinute) {
      lastMinute = now.getMinutes();
      const offset = -now.getTimezoneOffset();
      const sign = offset >= 0 ? "+" : "−";
      const zone = `UTC${sign}${pad(Math.floor(Math.abs(offset) / 60))}:${pad(Math.abs(offset) % 60)}`;
      $("hud-zone").textContent = `МЕСТНОЕ ВРЕМЯ · ${zone}`;
      const leap = new Date(now.getFullYear(), 1, 29).getMonth() === 1;
      $("hud-week").textContent = `НЕДЕЛЯ ${isoWeek(now)} · ДЕНЬ ${dayOfYear(now)} ИЗ ${leap ? 366 : 365}`;
      $("date").textContent = now.toLocaleDateString("ru-RU", { weekday: "short", day: "numeric", month: "long" });
    }

    // Спрятанной вкладке (прогретой или фоновой) часы не нужны: при показе
    // они сразу встают на нужное время и идут дальше.
    tickTimer = document.visibilityState === "visible" ? setTimeout(tick, 1000 - new Date().getMilliseconds() + 8) : 0;
  }

  function glitchLater() {
    setTimeout(() => {
      const time = $("time");
      if (
        document.visibilityState === "visible" &&
        document.documentElement.dataset.hud === "live" &&
        !matchMedia("(prefers-reduced-motion: reduce)").matches
      ) {
        time.classList.add("glitch");
        setTimeout(() => time.classList.remove("glitch"), 260);
      }
      glitchLater();
    }, 6000 + Math.random() * 7000);
  }

  /* ── Живой и спокойный HUD ────────────────────────────────── */

  // Кольца, дуга секунд и мигание двоеточия перерисовывают страницу с частотой
  // экрана: на мониторе 165 Гц это больше половины ядра, пока вкладка на виду.
  // Поэтому HUD живёт, пока на него смотрят, — первые секунды после показа
  // вкладки и пока двигают мышью или печатают, — а потом затихает: кольца
  // замирают, где были, секунды меняются скачком раз в секунду (newtab.css,
  // `data-hud`).
  const LIVE_MS = 10000;
  let calmTimer = 0;

  function setHud(mode) {
    if (document.documentElement.dataset.hud !== mode) document.documentElement.dataset.hud = mode;
  }

  function wake() {
    setHud("live");
    clearTimeout(calmTimer);
    calmTimer = setTimeout(() => setHud("calm"), LIVE_MS);
  }

  for (const type of ["pointermove", "pointerdown", "wheel", "keydown"]) {
    addEventListener(type, wake, { passive: true });
  }

  /* ── Поиск ────────────────────────────────────────────────── */

  $("search").addEventListener("submit", (event) => {
    event.preventDefault();
    const value = $("q").value.trim();
    if (!value) return;
    // Разбор запроса — в браузере: правило «адрес или поиск» одно на всё.
    send({ evt: "navigate", url: value });
  });

  /* ── Плитки ───────────────────────────────────────────────── */

  const dials = $("dials");
  const tileMenu = $("tile-menu");
  let menuIndex = -1;
  let menuButton = null;
  let dragIndex = -1;

  /** Что нарисовано на плитках: сами плитки и тема, под которую подобраны подложки. */
  let tilesShown = "";
  const tilesKey = () => JSON.stringify([state.tiles, root.dataset.theme]);

  function renderTiles() {
    tilesShown = tilesKey();
    const count = state.tiles.length + (state.tiles.length < MAX_TILES ? 1 : 0);
    dials.style.setProperty("--cols", String(Math.max(3, Math.min(8, count))));
    dials.replaceChildren();
    state.tiles.forEach((tile, index) => dials.append(tileNode(tile, index)));
    if (state.tiles.length < MAX_TILES) dials.append(addNode());
  }

  function tileNode(tile, index) {
    const node = h("div", "dial");
    node.setAttribute("role", "listitem");
    node.dataset.index = String(index);
    node.dataset.origin = originOf(tile.url);
    node.draggable = true;

    const link = h("a", "dial__link");
    link.href = tile.url;
    link.draggable = false;
    link.title = `${tile.title}\n${hostOf(tile.url)}`;
    // Значок из кэша страницы — до первой отрисовки плитки: иначе плитка
    // рисовалась буквой, а пришедший следом такой же значок считался «без
    // изменений», и буква оставалась навсегда.
    requestIcon(tile.url);
    const plate = h("span", "dial__plate");
    fillPlate(plate, tile);
    link.append(plate, h("span", "dial__name", tile.title));

    const more = h("button", "dial__more");
    more.type = "button";
    more.title = "Изменить или удалить";
    more.setAttribute("aria-label", `Изменить или удалить ${tile.title}`);
    more.append(glyph("more"));
    more.addEventListener("click", (event) => {
      event.preventDefault();
      event.stopPropagation();
      openTileMenu(index, more);
    });

    node.append(link, more);
    node.addEventListener("contextmenu", (event) => {
      event.preventDefault();
      openTileMenu(index, more, event);
    });

    node.addEventListener("dragstart", (event) => {
      dragIndex = index;
      node.classList.add("dial--dragging");
      event.dataTransfer.effectAllowed = "move";
      event.dataTransfer.setData("text/plain", tile.url);
    });
    node.addEventListener("dragend", () => {
      dragIndex = -1;
      node.classList.remove("dial--dragging");
      for (const other of dials.querySelectorAll(".dial--over")) other.classList.remove("dial--over");
    });
    node.addEventListener("dragover", (event) => {
      if (dragIndex < 0 || dragIndex === index) return;
      event.preventDefault();
      node.classList.add("dial--over");
    });
    node.addEventListener("dragleave", () => node.classList.remove("dial--over"));
    node.addEventListener("drop", (event) => {
      event.preventDefault();
      if (dragIndex < 0 || dragIndex === index) return;
      const [moved] = state.tiles.splice(dragIndex, 1);
      state.tiles.splice(index, 0, moved);
      saveTiles();
    });

    return node;
  }

  /**
   * Подложка под значок по его средней яркости: чёрный логотип получает светлую
   * плашку в тёмной теме, белый — тёмную в светлой. Значки приходят data:-адресами
   * того же происхождения, так что canvas их читает.
   */
  function tuneBackdrop(node, image, prefix) {
    try {
      const canvas = document.createElement("canvas");
      canvas.width = 24;
      canvas.height = 24;
      const context = canvas.getContext("2d", { willReadFrequently: true });
      context.drawImage(image, 0, 0, 24, 24);
      const { data } = context.getImageData(0, 0, 24, 24);
      let sum = 0;
      let weight = 0;
      let opaque = 0;
      for (let i = 0; i < data.length; i += 4) {
        const alpha = data[i + 3] / 255;
        if (alpha > 0.9) opaque++;
        if (alpha < 0.2) continue;
        sum += ((0.2126 * data[i] + 0.7152 * data[i + 1] + 0.0722 * data[i + 2]) / 255) * alpha;
        weight += alpha;
      }
      // Непрозрачный значок — сам себе плашка, подложка ему не нужна.
      if (!weight || opaque / (data.length / 4) > 0.85) return;
      const luminance = sum / weight;
      const lightTheme = root.dataset.theme === "shiro";
      node.classList.toggle(`${prefix}--light`, !lightTheme && luminance < 0.16);
      node.classList.toggle(`${prefix}--dark`, lightTheme && luminance > 0.9);
    } catch {
      // Картинка не читается — остаётся обычная плашка.
    }
  }

  function fillPlate(plate, tile) {
    const origin = originOf(tile.url);
    const icon = origin ? icons.get(origin) : null;
    plate.replaceChildren();
    if (icon?.data) {
      const image = h("img", icon.size && icon.size < 32 ? "dial__icon dial__icon--small" : "dial__icon");
      image.alt = "";
      image.decoding = "async";
      image.src = icon.data;
      image.addEventListener("error", () => plate.replaceChildren(h("span", "dial__mono", monogram(tile.title))), { once: true });
      image.addEventListener("load", () => tuneBackdrop(plate, image, "dial__plate"), { once: true });
      plate.append(image);
    } else {
      plate.classList.remove("dial__plate--light", "dial__plate--dark");
      plate.append(h("span", "dial__mono", monogram(tile.title)));
    }
  }

  function addNode() {
    const button = h("button", "dial dial--add");
    button.type = "button";
    const plate = h("span", "dial__plate");
    plate.append(glyph("add"));
    button.append(plate, h("span", "dial__name", "Добавить"));
    button.addEventListener("click", () => openDialog(-1));
    return button;
  }

  function saveTiles() {
    renderTiles();
    send({ evt: "newtab_tiles", tiles: state.tiles });
  }

  function openTileMenu(index, button, pointer) {
    closePops();
    menuIndex = index;
    menuButton = button;
    button.setAttribute("aria-expanded", "true");
    tileMenu.hidden = false;
    const rect = button.getBoundingClientRect();
    const x = pointer ? pointer.clientX : rect.right - tileMenu.offsetWidth;
    const y = pointer ? pointer.clientY : rect.bottom + 6;
    tileMenu.style.left = `${Math.max(8, Math.min(x, innerWidth - tileMenu.offsetWidth - 8))}px`;
    tileMenu.style.top = `${Math.max(8, Math.min(y, innerHeight - tileMenu.offsetHeight - 8))}px`;
    tileMenu.querySelector("button").focus();
  }

  function closeTileMenu() {
    tileMenu.hidden = true;
    menuButton?.removeAttribute("aria-expanded");
    menuButton = null;
  }

  tileMenu.addEventListener("click", (event) => {
    const action = event.target.closest("button")?.dataset.action;
    const index = menuIndex;
    closeTileMenu();
    if (action === "edit") openDialog(index);
    if (action === "delete" && state.tiles[index]) {
      state.tiles.splice(index, 1);
      saveTiles();
    }
  });
  tileMenu.addEventListener("keydown", (event) => {
    if (!["ArrowDown", "ArrowUp"].includes(event.key)) return;
    event.preventDefault();
    const items = [...tileMenu.querySelectorAll("button")];
    const next = (items.indexOf(document.activeElement) + (event.key === "ArrowDown" ? 1 : -1) + items.length) % items.length;
    items[next].focus();
  });

  const dialog = $("tile-dialog");
  const urlInput = $("tile-url");
  const titleInput = $("tile-title");
  let editing = -1;

  function openDialog(index) {
    editing = index;
    const tile = state.tiles[index];
    $("tile-dialog-title").textContent = tile ? "Изменить сайт" : "Новый сайт";
    urlInput.value = tile?.url ?? "";
    titleInput.value = tile?.title ?? "";
    urlInput.removeAttribute("aria-invalid");
    dialog.showModal();
    urlInput.focus();
    urlInput.select();
  }

  $("tile-cancel").addEventListener("click", () => dialog.close());
  urlInput.addEventListener("input", () => urlInput.removeAttribute("aria-invalid"));
  $("tile-form").addEventListener("submit", (event) => {
    event.preventDefault();
    const url = urlInput.value.trim();
    const looksLikeUrl = /^[a-z][a-z0-9+.-]*:\/\/\S+$/i.test(url) || /^[^\s/]+\.[^\s]{2,}/.test(url);
    if (!url || !looksLikeUrl) {
      urlInput.setAttribute("aria-invalid", "true");
      urlInput.focus();
      return;
    }
    const tile = { title: titleInput.value.trim() || hostOf(/:\/\//.test(url) ? url : `https://${url}`), url };
    if (editing >= 0 && state.tiles[editing]) state.tiles[editing] = tile;
    else state.tiles.push(tile);
    dialog.close();
    saveTiles();
  });

  /* ── Значки сайтов ────────────────────────────────────────── */

  function requestIcon(url) {
    const origin = originOf(url);
    if (!origin || iconsAsked.has(origin)) return;
    iconsAsked.add(origin);
    try {
      const saved = JSON.parse(localStorage.getItem(ICON_KEY + origin) ?? "null");
      if (saved?.icon !== undefined) icons.set(origin, saved.icon);
    } catch {
      // Хранилище недоступно — значок придёт от браузера.
    }
    send({ evt: "newtab_icon", url });
  }

  function storeIcon(url, icon) {
    const origin = originOf(url);
    if (!origin) return;
    const known = icons.get(origin);
    icons.set(origin, icon);
    try {
      localStorage.setItem(ICON_KEY + origin, JSON.stringify({ icon }));
    } catch {
      // Переполнено — не страшно, в профиле браузера значок есть.
    }
    for (const node of dials.querySelectorAll(".dial[data-origin]")) {
      if (node.dataset.origin !== origin) continue;
      // Перерисовываем, только если плитка показывает не этот значок: так нет
      // мигания, а плитка с буквой получает картинку, даже когда значок в кэше
      // страницы уже был.
      const shown = node.querySelector(".dial__plate img")?.getAttribute("src") ?? null;
      if (shown === (icon?.data ?? null) && known?.data === icon?.data) continue;
      const tile = state.tiles[Number(node.dataset.index)];
      if (tile) fillPlate(node.querySelector(".dial__plate"), tile);
    }
    for (const row of document.querySelectorAll(".group[data-origin]")) {
      if (row.dataset.origin === origin) paintGroupIcon(row.querySelector(".group__icon"), row.dataset.kind, origin);
    }
  }

  /* ── Погода ───────────────────────────────────────────────── */

  const WMO = {
    0: ["Ясно", "sunny", "moon"],
    1: ["Преимущественно ясно", "partly-day", "partly-night"],
    2: ["Переменная облачность", "partly-day", "partly-night"],
    3: ["Пасмурно", "cloudy"],
    45: ["Туман", "fog"],
    48: ["Туман с изморозью", "fog"],
    51: ["Слабая морось", "drizzle"],
    53: ["Морось", "drizzle"],
    55: ["Сильная морось", "drizzle"],
    56: ["Ледяная морось", "drizzle"],
    57: ["Сильная ледяная морось", "drizzle"],
    61: ["Небольшой дождь", "rain"],
    63: ["Дождь", "rain"],
    65: ["Сильный дождь", "rain"],
    66: ["Ледяной дождь", "rain-snow"],
    67: ["Сильный ледяной дождь", "rain-snow"],
    71: ["Небольшой снег", "snow"],
    73: ["Снег", "snow"],
    75: ["Сильный снег", "snow"],
    77: ["Снежная крупа", "snow"],
    80: ["Небольшой ливень", "showers-day", "showers-night"],
    81: ["Ливень", "showers-day", "showers-night"],
    82: ["Сильный ливень", "showers-day", "showers-night"],
    85: ["Снегопад", "snow-day", "snow-night"],
    86: ["Сильный снегопад", "snow-day", "snow-night"],
    95: ["Гроза", "thunder"],
    96: ["Гроза с градом", "hail"],
    99: ["Гроза с сильным градом", "hail"],
  };

  function sky(code, day = true) {
    const [text, dayIcon, nightIcon] = WMO[code] ?? ["Нет данных", "cloudy"];
    return { text, icon: day ? dayIcon : nightIcon ?? dayIcon };
  }

  let weatherAt = 0;

  function requestWeather(force) {
    weatherAt = Date.now();
    send({ evt: "newtab_weather", force });
  }

  function renderWeather(message) {
    const status = $("weather-status");
    const body = $("weather-body");
    const place = message.place;

    if (place) {
      const approximate = message.source === "network";
      $("place-name").textContent = approximate ? `${place.name} ≈` : place.name;
      $("place").title = [
        [place.name, place.region, place.country].filter(Boolean).join(", "),
        approximate ? "Место определено по IP-адресу и может быть неточным — нажмите, чтобы выбрать город" : "Выбрать город",
      ].join("\n");
    } else {
      $("place-name").textContent = "Выбрать город";
    }

    if (!message.data) {
      body.hidden = true;
      status.hidden = false;
      status.replaceChildren(h("div", null, message.error ?? "Прогноз недоступен"));
      const actions = h("div", "weather__actions");
      const retry = h("button", "btn btn--sm", "Повторить");
      retry.type = "button";
      retry.addEventListener("click", () => {
        showWeatherLoading();
        requestWeather(true);
      });
      const choose = h("button", "btn btn--sm", "Выбрать город");
      choose.type = "button";
      choose.addEventListener("click", openPicker);
      actions.append(retry, choose);
      status.append(actions);
      return;
    }

    const { current, hourly = [], daily = [] } = message.data;
    const now = sky(current.code, current.day !== 0);
    setGlyph($("w-icon"), now.icon);
    $("w-temp").textContent = temperature(current.temp);
    $("w-desc").textContent = now.text;
    $("w-feels").textContent = `ощущается ${temperature(current.feels)}`;
    $("w-wind").textContent = Number.isFinite(current.wind) ? `${oneDecimal.format(current.wind)} м/с` : "—";
    $("w-humidity").textContent = Number.isFinite(current.humidity) ? `${Math.round(current.humidity)}%` : "—";

    renderHourly(hourly);

    const days = $("w-days");
    days.replaceChildren();
    daily.slice(0, 5).forEach((day, index) => {
      const item = h("li");
      const date = new Date(`${day.date}T12:00:00`);
      const weekday = date.toLocaleDateString("ru-RU", { weekday: "short" });
      const name =
        index === 0 ? "Сегодня" : index === 1 ? "Завтра" : `${weekday[0].toUpperCase()}${weekday.slice(1)}, ${date.getDate()}`;
      const info = sky(day.code);
      const icon = glyph(info.icon);
      const title = h("span", "day__name", name);
      title.title = info.text;
      item.append(
        title,
        icon,
        h("span", "day__rain", Number.isFinite(day.rain) && day.rain >= 20 ? `${Math.round(day.rain)}%` : ""),
        h("span", "day__max", temperature(day.max)),
        h("span", "day__min", temperature(day.min))
      );
      days.append(item);
    });

    status.hidden = true;
    body.hidden = false;
  }

  function showWeatherLoading() {
    const status = $("weather-status");
    status.replaceChildren(h("div", "skeleton skeleton--big"), h("div", "skeleton"), h("div", "skeleton skeleton--short"));
    status.hidden = false;
    $("weather-body").hidden = true;
  }

  /** Сглаженная линия через точки (Catmull-Rom). */
  function smoothPath(points) {
    if (points.length < 2) return "";
    let path = `M ${points[0][0].toFixed(1)} ${points[0][1].toFixed(1)}`;
    for (let i = 0; i < points.length - 1; i++) {
      const [x0, y0] = points[i - 1] ?? points[i];
      const [x1, y1] = points[i];
      const [x2, y2] = points[i + 1];
      const [x3, y3] = points[i + 2] ?? points[i + 1];
      const c1 = [x1 + (x2 - x0) / 6, y1 + (y2 - y0) / 6];
      const c2 = [x2 - (x3 - x1) / 6, y2 - (y3 - y1) / 6];
      path += ` C ${c1[0].toFixed(1)} ${c1[1].toFixed(1)}, ${c2[0].toFixed(1)} ${c2[1].toFixed(1)}, ${x2.toFixed(1)} ${y2.toFixed(1)}`;
    }
    return path;
  }

  function renderHourly(hourly) {
    const plot = $("w-plot");
    const marks = $("w-marks");
    const labels = $("w-hours");
    const values = hourly.map((hour) => hour.temp).filter(Number.isFinite);
    if (values.length < 2) {
      $("w-chart").hidden = true;
      return;
    }
    $("w-chart").hidden = false;

    const width = 280;
    const height = 56;
    const top = 14;
    const bottom = 10;
    const min = Math.min(...values);
    const max = Math.max(...values);
    const span = max - min || 1;
    const points = values.map((value, index) => [
      (index / (values.length - 1)) * width,
      top + (height - top - bottom) * (1 - (value - min) / span),
    ]);
    const line = smoothPath(points);
    plot.innerHTML = `
      <defs><linearGradient id="chart-fill" x1="0" y1="0" x2="0" y2="1">
        <stop offset="0" stop-color="var(--accent)" stop-opacity=".32"/>
        <stop offset="1" stop-color="var(--accent)" stop-opacity="0"/>
      </linearGradient></defs>
      <path class="chart__area" d="${line} L ${width} ${height} L 0 ${height} Z"/>
      <path class="chart__line" d="${line}"/>`;

    marks.replaceChildren();
    const mark = (index, className) => {
      const node = h("span", className, temperature(values[index]));
      node.style.left = `${Math.min(94, Math.max(6, (points[index][0] / width) * 100))}%`;
      node.style.top = `${(points[index][1] / height) * 100}%`;
      marks.append(node);
    };
    mark(values.indexOf(max), "chart__mark");
    if (max !== min) mark(values.indexOf(min), "chart__mark chart__mark--min");

    labels.replaceChildren();
    const hourLabel = (index) => (index === 0 ? "сейчас" : String(hourly[index]?.time ?? "").slice(11, 16));
    for (const index of [0, 6, 12, 18, values.length - 1]) labels.append(h("span", null, hourLabel(Math.min(index, values.length - 1))));
  }

  const picker = $("picker");
  const pickerInput = $("picker-q");
  const pickerList = $("picker-list");
  let searchTimer = 0;
  let pickerWasBody = false;

  function openPicker() {
    pickerWasBody = !$("weather-body").hidden;
    picker.hidden = false;
    $("weather-body").hidden = true;
    $("weather-status").hidden = true;
    pickerInput.value = "";
    pickerList.replaceChildren(h("li", "picker__note", "Начните вводить название"));
    pickerInput.focus();
  }

  function closePicker() {
    picker.hidden = true;
    if (pickerWasBody) $("weather-body").hidden = false;
    else $("weather-status").hidden = false;
  }

  $("place").addEventListener("click", () => (picker.hidden ? openPicker() : closePicker()));
  $("picker-close").addEventListener("click", closePicker);
  $("picker-auto").addEventListener("click", () => chooseCity(null));

  pickerInput.addEventListener("input", () => {
    clearTimeout(searchTimer);
    const query = pickerInput.value.trim();
    if (query.length < 2) {
      pickerList.replaceChildren(h("li", "picker__note", "Начните вводить название"));
      return;
    }
    searchTimer = setTimeout(() => send({ evt: "newtab_city_search", query }), 280);
  });
  pickerInput.addEventListener("keydown", (event) => {
    if (event.key === "Enter") pickerList.querySelector("button")?.click();
    if (event.key === "ArrowDown") {
      event.preventDefault();
      pickerList.querySelector("button")?.focus();
    }
  });

  function renderCities(message) {
    if (message.query !== pickerInput.value.trim()) return;
    pickerList.replaceChildren();
    if (message.error) {
      pickerList.append(h("li", "picker__note", message.error));
      return;
    }
    if (!message.places?.length) {
      pickerList.append(h("li", "picker__note", "Ничего не нашлось"));
      return;
    }
    for (const place of message.places) {
      const item = h("li");
      const button = h("button", "picker__row");
      button.type = "button";
      const text = h("span");
      text.append(h("span", null, place.name), h("small", null, [place.region, place.country].filter(Boolean).join(", ")));
      button.append(glyph("location"), text);
      button.addEventListener("click", () => chooseCity(place));
      item.append(button);
      pickerList.append(item);
    }
  }

  function chooseCity(place) {
    picker.hidden = true;
    pickerWasBody = false;
    $("place-name").textContent = place ? place.name : "Ищем, где вы…";
    showWeatherLoading();
    weatherAt = Date.now();
    send({ evt: "newtab_city_set", city: place });
  }

  /* ── Ресурсы ──────────────────────────────────────────────── */

  let monitorTimer = 0;
  const MONITOR_EVERY = 2000;

  function syncMonitor() {
    const on = state.monitor && document.visibilityState === "visible";
    if (on && !monitorTimer) {
      send({ evt: "newtab_resources" });
      // Раз в две секунды: замер опрашивает счётчики видеокарты по всем
      // процессам системы, и сам монитор не должен заметно грузить машину.
      monitorTimer = setInterval(() => send({ evt: "newtab_resources" }), MONITOR_EVERY);
    } else if (!on && monitorTimer) {
      clearInterval(monitorTimer);
      monitorTimer = 0;
    }
  }

  function sparkline(svg, values, ceiling) {
    const width = 120;
    const height = 34;
    if (values.length < 2) {
      svg.innerHTML = `<line class="spark__base" x1="0" y1="${height}" x2="${width}" y2="${height}"/>`;
      return;
    }
    const top = Math.max(ceiling, ...values) * 1.15;
    const points = values.map((value, index) => [
      (index / (values.length - 1)) * width,
      height - (Math.max(0, value) / top) * (height - 2),
    ]);
    const line = smoothPath(points);
    svg.innerHTML = `
      <line class="spark__base" x1="0" y1="${height}" x2="${width}" y2="${height}"/>
      <path class="spark__area" d="${line} L ${width} ${height} L 0 ${height} Z"/>
      <path class="spark__line" d="${line}"/>`;
  }

  const GROUP_GLYPHS = { interface: "window", gpu: "gpu", services: "pulse", background: "tab", tab: "globe", tabs: "tab" };

  function paintGroupIcon(slot, kind, origin) {
    const icon = origin ? icons.get(origin) : null;
    if (icon?.data) {
      if (slot.querySelector("img")?.src === icon.data) return;
      const image = h("img");
      image.alt = "";
      image.addEventListener("load", () => tuneBackdrop(slot, image, "group__icon"), { once: true });
      image.src = icon.data;
      slot.replaceChildren(image);
    } else if (!slot.firstChild) {
      slot.replaceChildren(glyph(GROUP_GLYPHS[kind] ?? "tab"));
    }
  }

  function renderResources(report) {
    if (!report) return;
    const { browser, system, history = [] } = report;

    const memory = bytes(browser.memory);
    $("m-mem").textContent = memory.value;
    $("m-mem-unit").textContent = memory.unit;

    const total = system.memory_total || 1;
    const browserShare = (browser.memory / total) * 100;
    const otherShare = Math.max(0, ((system.memory_used - browser.memory) / total) * 100);
    $("m-bar-browser").style.width = `${Math.min(100, browserShare)}%`;
    $("m-bar-other").style.width = `${Math.min(100 - browserShare, otherShare)}%`;
    $("m-share").textContent = `браузер ${percent(browserShare)} памяти`;
    $("m-system").textContent = `занято ${bytesText(system.memory_used)} из ${bytesText(system.memory_total)}`;
    $("m-bar").setAttribute(
      "aria-label",
      `Браузер занимает ${percent(browserShare)} памяти, всего занято ${bytesText(system.memory_used)} из ${bytesText(system.memory_total)}`
    );

    $("m-cpu").textContent = percent(browser.cpu);
    sparkline($("s-cpu"), history.map((point) => point.cpu), 5);

    const hasGpu = browser.gpu != null;
    $("g-gpu").hidden = !hasGpu;
    $("m-gauges").dataset.gpu = hasGpu ? "on" : "off";
    if (hasGpu) {
      $("m-gpu").textContent = percent(browser.gpu);
      $("g-gpu").title = browser.gpu_memory != null ? `Видеопамять браузера: ${bytesText(browser.gpu_memory)}` : "";
      sparkline($("s-gpu"), history.map((point) => point.gpu), 5);
    }

    const list = $("m-groups");
    const rows = new Map([...list.children].map((row) => [row.dataset.key, row]));
    const groups = report.groups.slice(0, 6);
    const keep = new Set();
    groups.forEach((group, index) => {
      const key = `${group.kind}:${group.tabs.join(",")}`;
      keep.add(key);
      let row = rows.get(key);
      if (!row) {
        row = h("li", "group");
        row.dataset.key = key;
        row.dataset.kind = group.kind;
        row.append(h("span", "group__icon"), h("span", "group__title"), h("span", "group__mem"), h("span", "group__cpu"));
      }
      const origin = group.url ? originOf(group.url) : "";
      if (origin) {
        row.dataset.origin = origin;
        requestIcon(group.url);
      } else {
        delete row.dataset.origin;
      }
      paintGroupIcon(row.querySelector(".group__icon"), group.kind, origin);
      const title = row.querySelector(".group__title");
      title.textContent = group.title || "Без названия";
      row.title = `${group.title}\nПроцессов: ${group.processes}${group.gpu != null ? `\nВидеокарта: ${percent(group.gpu)}` : ""}`;
      row.querySelector(".group__mem").textContent = bytesText(group.memory);
      row.querySelector(".group__cpu").textContent = percent(group.cpu);
      row.style.setProperty("--share", `${Math.max(2, (group.memory / (browser.memory || 1)) * 100)}%`);
      if (list.children[index] !== row) list.insertBefore(row, list.children[index] ?? null);
    });
    for (const [key, row] of rows) if (!keep.has(key)) row.remove();

    $("m-processes").textContent = `процессов: ${browser.processes}`;
    $("m-system-cpu").textContent = `ЦП системы ${percent(system.cpu)}`;
  }

  /* ── Жизненный цикл ───────────────────────────────────────── */

  document.addEventListener("visibilitychange", () => {
    syncMonitor();
    if (document.visibilityState !== "visible") {
      clearTimeout(calmTimer);
      setHud("calm");
      return;
    }
    wake();
    if (!tickTimer) tick();
    // Тему и плитки могли поменять, пока вкладка была в фоне.
    send({ evt: "newtab_init" });
    if (state.weather && Date.now() - weatherAt > WEATHER_EVERY) requestWeather(false);
  });

  /* ── Демонстрация без браузера ────────────────────────────── */

  const Demo = {
    history: [],
    handle(message) {
      setTimeout(() => {
        switch (message.evt) {
          case "newtab_init":
            receive({
              type: "state",
              tiles: state.loaded ? state.tiles : Demo.tiles,
              city: null,
              weather: params.get("weather") !== "off",
              monitor: params.get("monitor") !== "off",
              seconds: true,
              theme: params.get("theme") ?? "kurogane",
            });
            break;
          case "newtab_tiles":
            receive({ type: "state", tiles: message.tiles, weather: state.weather, monitor: state.monitor, seconds: state.seconds, theme: state.theme });
            break;
          case "newtab_icon":
            Demo.icon(message.url);
            break;
          case "newtab_weather":
          case "newtab_city_set":
            receive(Demo.weather(message.city));
            break;
          case "newtab_city_search":
            receive({
              type: "cities",
              query: message.query,
              places: [
                { name: "Казань", region: "Татарстан", country: "Россия", lat: 55.79, lon: 49.12 },
                { name: "Казань", region: "Кировская область", country: "Россия", lat: 59.16, lon: 49.93 },
              ],
            });
            break;
          case "newtab_resources":
            receive({ type: "resources", report: Demo.report() });
            break;
          case "navigate":
            console.info("переход:", message.url);
            break;
        }
      }, 40);
    },
    tiles: [
      { title: "Кинопоиск", url: "https://www.kinopoisk.ru/" },
      { title: "YouTube", url: "https://www.youtube.com/" },
      { title: "GitHub", url: "https://github.com/" },
      { title: "Хабр", url: "https://habr.com/ru/" },
      { title: "Telegram", url: "https://web.telegram.org/" },
      { title: "Дзен", url: "https://dzen.ru/" },
    ],
    async icon(url) {
      // Для ревью значки можно подложить файлом demo-icons.json рядом со страницей.
      Demo.icons ??= fetch("./demo-icons.json").then((response) => (response.ok ? response.json() : {})).catch(() => ({}));
      const known = await Demo.icons;
      receive({ type: "icon", url, icon: known[originOf(url)] ?? null });
    },
    weather(city) {
      const base = 17;
      return {
        type: "weather",
        place: city ?? { name: "Казань", region: "Татарстан", country: "Россия" },
        source: city ? "city" : "device",
        data: {
          current: { temp: 18.4, feels: 16.1, humidity: 45, code: 2, wind: 2.6, day: 1 },
          hourly: Array.from({ length: 24 }, (_, i) => ({
            time: `2026-09-14T${pad((14 + i) % 24)}:00`,
            temp: base + 4 * Math.sin((i - 2) / 3.6) - i * 0.18,
            rain: i > 14 ? 40 : 5,
          })),
          daily: [
            { date: "2026-09-14", code: 2, max: 18.5, min: 10.0, rain: 5 },
            { date: "2026-09-15", code: 61, max: 14.7, min: 10.7, rain: 72 },
            { date: "2026-09-16", code: 63, max: 13.3, min: 10.9, rain: 64 },
            { date: "2026-09-17", code: 3, max: 15.1, min: 8.2, rain: 12 },
            { date: "2026-09-18", code: 0, max: 19.8, min: 9.4, rain: 0 },
          ],
        },
      };
    },
    report() {
      const wave = Date.now() / 4000;
      const cpu = 3 + 2.5 * Math.abs(Math.sin(wave)) + Math.random();
      const gpu = 1.2 + Math.random() * 1.5;
      const memory = 1.31e9 + Math.sin(wave / 3) * 4e7;
      Demo.history.push({ cpu, gpu, memory });
      if (Demo.history.length > 60) Demo.history.shift();
      const group = (kind, title, url, mem, share, tabs = []) => ({
        kind, title, url, tabs, memory: mem, cpu: cpu * share, gpu: kind === "gpu" ? gpu : 0, processes: 1,
      });
      return {
        cpus: 16,
        browser: { cpu, memory, gpu, gpu_memory: 3.1e8, processes: 14 },
        system: { cpu: 11.8, memory_total: 34.2e9, memory_used: 14.6e9 },
        history: Demo.history,
        groups: [
          group("tab", "YouTube", "https://www.youtube.com/", 4.12e8, 0.45, [2]),
          group("interface", "Окно браузера", null, 2.96e8, 0.2),
          group("gpu", "Видеокарта", null, 2.2e8, 0.12),
          group("tabs", "Вкладки: GitHub, Хабр", "https://github.com/", 1.78e8, 0.1, [3, 4]),
          group("services", "Сеть, звук и другие службы", null, 1.21e8, 0.08),
          group("tab", "Новая вкладка", "http://190x4-pages.invalid/newtab.html", 0.83e8, 0.05, [5]),
        ],
      };
    },
  };

  // Запуск — после всех объявлений: ответ демонстрации приходит синхронно
  // по цепочке и обращается к тому, что объявлено ниже вызова.
  buildHud();
  tick();
  glitchLater();
  if (document.visibilityState === "visible") wake();
  else setHud("calm");
  send({ evt: "newtab_init" });
})();
