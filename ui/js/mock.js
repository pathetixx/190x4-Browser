/**
 * Mock-бэкенд: интерфейс без Rust.
 *
 * Нужен, чтобы править интерфейс и снимать скриншоты, не собирая
 * Windows-бинарь. Всё здесь — имитация контракта из `ipc.rs`; расходиться с
 * ним нельзя, иначе mock начнёт врать.
 */

const handlers = new Map();
let nextId = 1;
let seeded = false;

const NOW = Math.floor(Date.now() / 1000);

/** Значок сайта: цветная плашка с буквой. Сеть в макете не нужна. */
function badge(letter, background, color = "#fff") {
  const svg = `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><rect width="16" height="16" rx="3.5" fill="${background}"/><text x="8" y="11.6" font-family="Segoe UI,Arial" font-size="10" font-weight="700" fill="${color}" text-anchor="middle">${letter}</text></svg>`;
  return `data:image/svg+xml,${encodeURIComponent(svg)}`;
}

const ICONS = {
  kinopoisk: badge("К", "#ff6600"),
  habr: badge("Х", "#6e8ca0"),
  github: badge("G", "#24292f"),
  youtube: badge("▶", "#ff0033"),
  dzen: badge("Д", "#111", "#fff"),
  lenta: badge("Л", "#1c1c1c"),
  yandex: badge("Я", "#fc3f1d"),
  vk: badge("В", "#0077ff"),
  telegram: badge("T", "#2aabee"),
  rutube: badge("R", "#100943"),
};

const SEED = [
  { url: "https://www.kinopoisk.ru/film/1318972/", title: "Дюна: Часть вторая — смотреть онлайн", icon: ICONS.kinopoisk, blocked: 41 },
  { url: "https://habr.com/ru/articles/812345/", title: "WebView2: один Environment на все вкладки", icon: ICONS.habr },
  { url: "https://github.com/pathetixx/190x4-Ninety", title: "pathetixx/190x4-Ninety", icon: ICONS.github },
  { url: "https://www.youtube.com/watch?v=aqz-KE-bpKQ", title: "Big Buck Bunny 60fps 4K", icon: ICONS.youtube, audible: true },
  { url: "https://dzen.ru/news", title: "Дзен — Новости", icon: ICONS.dzen },
];

const MOCK_HISTORY = [
  { url: "https://habr.com/ru/articles/812345/", title: "WebView2: один Environment на все вкладки", host: "habr.com", visits: 4, visited_at: NOW - 600 },
  { url: "https://www.kinopoisk.ru/film/1318972/", title: "Дюна: Часть вторая", host: "www.kinopoisk.ru", visits: 2, visited_at: NOW - 5400 },
  { url: "https://github.com/pathetixx/190x4-Ninety", title: "pathetixx/190x4-Ninety", host: "github.com", visits: 11, visited_at: NOW - 86400 },
  { url: "https://dzen.ru/news", title: "Дзен — Новости", host: "dzen.ru", visits: 1, visited_at: NOW - 172800 },
];

let BOOKMARKS = [
  { id: 1, parent_id: null, kind: "folder", title: "Панель закладок", url: null, icon: "", position: 0, added_at: NOW },
  { id: 2, parent_id: null, kind: "folder", title: "Другие закладки", url: null, icon: "", position: 1, added_at: NOW },
  { id: 3, parent_id: 1, kind: "url", title: "Кинопоиск", url: "https://www.kinopoisk.ru/", icon: ICONS.kinopoisk, position: 0, added_at: NOW },
  { id: 4, parent_id: 1, kind: "url", title: "YouTube", url: "https://www.youtube.com/", icon: ICONS.youtube, position: 1, added_at: NOW },
  { id: 5, parent_id: 1, kind: "url", title: "Хабр", url: "https://habr.com/ru/feed/", icon: ICONS.habr, position: 2, added_at: NOW },
  { id: 6, parent_id: 1, kind: "folder", title: "Работа", url: null, icon: "", position: 3, added_at: NOW },
  { id: 7, parent_id: 1, kind: "url", title: "GitHub", url: "https://github.com/", icon: ICONS.github, position: 4, added_at: NOW },
  { id: 8, parent_id: 1, kind: "url", title: "Telegram", url: "https://web.telegram.org/", icon: ICONS.telegram, position: 5, added_at: NOW },
  { id: 9, parent_id: 1, kind: "url", title: "VK Видео", url: "https://vkvideo.ru/", icon: ICONS.vk, position: 6, added_at: NOW },
  { id: 10, parent_id: 1, kind: "folder", title: "Кино", url: null, icon: "", position: 7, added_at: NOW },
  { id: 11, parent_id: 6, kind: "url", title: "pathetixx/190x4-Ninety", url: "https://github.com/pathetixx/190x4-Ninety", icon: ICONS.github, position: 0, added_at: NOW },
  { id: 12, parent_id: 6, kind: "url", title: "Яндекс Трекер", url: "https://tracker.yandex.ru/", icon: ICONS.yandex, position: 1, added_at: NOW },
  { id: 13, parent_id: 10, kind: "url", title: "Rutube", url: "https://rutube.ru/", icon: ICONS.rutube, position: 0, added_at: NOW },
  { id: 14, parent_id: 2, kind: "url", title: "Лента.ру", url: "https://lenta.ru/", icon: ICONS.lenta, position: 0, added_at: NOW },
];

const PASSWORDS = [
  { id: 1, origin: "https://github.com", username: "pathetixx", created_at: NOW - 900000, used_at: NOW - 3600 },
  { id: 2, origin: "https://habr.com", username: "alex.petrov@example.com", created_at: NOW - 400000, used_at: null },
  { id: 3, origin: "https://www.kinopoisk.ru", username: "190x4", created_at: NOW - 200000, used_at: NOW - 86400 },
  { id: 4, origin: "https://vk.com", username: "+7 999 000-00-00", created_at: NOW - 100000, used_at: NOW - 7200 },
];

const DOWNLOADS = [
  { id: 1, kind: "web", url: "https://github.com/pathetixx/190x4-Ninety/releases/download/v3.4.0/Ninety_3.4.0_x64-setup.exe", path: "C:/Users/me/Downloads/Ninety_3.4.0_x64-setup.exe", state: "running", bytes: 38_211_724, total_bytes: 61_388_608, started_at: NOW - 40, error: "" },
  { id: 2, kind: "media", url: "https://www.youtube.com/watch?v=aqz-KE-bpKQ", path: "Big Buck Bunny 60fps 4K - Official Blender Foundation Short Film.mp4", state: "running", bytes: 21_000_000, total_bytes: 85_670_000, started_at: NOW - 80, error: "" },
  { id: 3, kind: "web", url: "https://example.com/report-q3.pdf", path: "C:/Users/me/Downloads/report-q3.pdf", state: "paused", bytes: 2_411_724, total_bytes: 8_388_608, started_at: NOW - 400, error: "" },
  { id: 4, kind: "web", url: "https://www.7-zip.org/a/7z2408-x64.exe", path: "C:/Users/me/Downloads/7z2408-x64.exe", state: "done", bytes: 1_611_000, total_bytes: 1_611_000, started_at: NOW - 3600, error: "" },
  { id: 5, kind: "web", url: "https://mirror.example.org/ubuntu-24.04-desktop-amd64.iso", path: "C:/Users/me/Downloads/ubuntu-24.04-desktop-amd64.iso", state: "failed", bytes: 912_000_000, total_bytes: 6_114_656_256, started_at: NOW - 5400, error: "network" },
  { id: 6, kind: "web", url: "https://images.example.com/wallpaper-190x4.png", path: "C:/Users/me/Downloads/wallpaper-190x4.png", state: "done", bytes: 4_200_000, total_bytes: 4_200_000, started_at: NOW - 90000, error: "" },
  { id: 7, kind: "web", url: "https://example.com/archive.zip", path: "C:/Users/me/Downloads/archive.zip", state: "cancelled", bytes: 0, total_bytes: 80_000_000, started_at: NOW - 95000, error: "" },
];

const settings = {};
let defaultBrowser = false;

export async function invoke(command, args = {}) {
  switch (command) {
    case "tab_open": {
      const id = nextId++;
      queueMicrotask(() => {
        emit("tab", { kind: "started", id, url: args.url });
        emit("tab", { kind: "url", id, url: args.url });
        emit("tab", { kind: "title", id, title: "Новая вкладка" });
        emit("tab", { kind: "finished", id, ok: true, http_status: 200 });
      });
      return id;
    }
    case "tab_navigate":
      // Переход в макете: вкладка получает адрес, как от движка.
      queueMicrotask(() => {
        emit("tab", { kind: "started", id: args.id, url: args.url });
        emit("tab", { kind: "finished", id: args.id, ok: true, http_status: 200, url: args.url });
      });
      return null;
    case "tab_close":
      return null;
    case "adblock_stats":
      return { checked: 2480, blocked: 143, rewritten: 12, avg_micros: 18.4, max_micros: 92.1 };
    case "adblock_lists":
      return [
        { id: "easylist", title: "EasyList", enabled: true },
        { id: "easyprivacy", title: "EasyPrivacy", enabled: true },
        { id: "ruadlist", title: "RU AdList", enabled: true },
      ];
    case "settings_get":
      return { ...settings };
    case "settings_set":
      settings[args.key] = args.value;
      emit("settings", { key: args.key, value: args.value });
      return null;
    case "window_state":
      return { maximized: false };
    case "window_info":
      return { label: "chrome", private: false, session: 0, windows: 1 };
    case "window_open":
    case "tab_split":
    case "tab_zoom_set":
    case "zoom_site_set":
    case "history_forget_visit":
    case "history_clear_period":
      return null;
    case "zoom_sites":
      return {};
    case "site_icon":
      return null;
    case "session_restore":
      return [];
    case "history_recent":
    case "history_search":
      return MOCK_HISTORY.slice(0, args.limit ?? 6);
    case "history_page":
      // Страница истории в макете: те же адреса, но каждое посещение — строкой.
      return MOCK_HISTORY.flatMap((entry, index) =>
        [0, 1].map((step) => ({
          id: index * 10 + step,
          url: entry.url,
          title: entry.title,
          host: entry.host,
          visited_at: entry.visited_at - step * 5400,
        }))
      ).filter((visit) =>
        !args.query || `${visit.title} ${visit.url}`.toLowerCase().includes(String(args.query).toLowerCase())
      );
    case "search_suggest":
      return [`${args.query} перевод`, `${args.query} скачать`, `${args.query} 2026`];
    case "bookmarks_tree":
      return BOOKMARKS;
    case "bookmark_find":
      return BOOKMARKS.find((node) => node.url === args.url) ?? null;
    case "bookmark_add": {
      const node = { id: 100 + BOOKMARKS.length, parent_id: args.parent ?? 1, kind: "url", title: args.title, url: args.url, icon: args.icon ?? "", position: 99, added_at: NOW };
      BOOKMARKS = [...BOOKMARKS, node];
      emit("bookmarks", null);
      return node;
    }
    case "bookmark_remove":
      BOOKMARKS = BOOKMARKS.filter((node) => node.id !== args.id);
      emit("bookmarks", null);
      return null;
    case "passwords_list":
      return PASSWORDS;
    case "password_reveal":
      return "correct-horse-battery";
    case "site_permissions":
      return [
        { permission: "camera", origin: "https://meet.jit.si", allowed: true },
        { permission: "microphone", origin: "https://meet.jit.si", allowed: true },
        { permission: "notifications", origin: "https://vk.com", allowed: false },
      ];
    case "site_permission_reset":
    case "tab_dialog":
      return null;
    case "password_never_list":
      return [{ origin: "https://bank.example.ru", added_at: NOW - 5000 }];
    case "downloads_list":
      return DOWNLOADS;
    case "services_state":
      return { translate: true, media: true };
    case "launch_take":
    case "launch_adopt_take":
      return [];
    case "tab_history":
      // Меню «Назад»/«Вперёд» в макете: три страницы, текущая — средняя.
      return {
        currentIndex: 1,
        entries: [
          { id: 1, url: "https://habr.com/ru/feed/", title: "Хабр" },
          { id: 2, url: "https://github.com/pathetixx/190x4-Browser", title: "190x4 Browser" },
          { id: 3, url: "https://www.kinopoisk.ru/", title: "Кинопоиск" },
        ],
      };
    case "default_browser_state":
      return { is_default: defaultBrowser };
    case "default_browser_set":
      defaultBrowser = true;
      return null;
    case "about_info":
      return { version: "0.1.0", webview: "131.0.2903.70", profile: "C:\\Users\\me\\AppData\\Local\\190x4 Browser" };
    case "translate_text":
      return { result: "Перевод приходит с сервера 190x4.", detected: "English" };
    case "media_probe":
      return {
        title: "Big Buck Bunny 60fps 4K — Official Blender Foundation Short Film",
        uploader: "Blender",
        duration_str: "10:35",
        thumbnail: "",
        formats: [
          { spec: "video:2160", label: "2160p", approx_mb: 604.2 },
          { spec: "video:1080", label: "1080p", approx_mb: 300.4 },
          { spec: "video:720", label: "720p", approx_mb: 153.3 },
          { spec: "video:480", label: "480p", approx_mb: 71.9 },
          { spec: "audio:320", label: "MP3 320 кбит/с", approx_mb: 24.3 },
          { spec: "audio:192", label: "MP3 192 кбит/с", approx_mb: 14.9 },
        ],
      };
    case "media_download":
      return "mock-job";
    default:
      return null;
  }
}

export async function listen(event, handler) {
  handlers.set(event, [...(handlers.get(event) ?? []), handler]);
  if (!seeded && event === "tab") {
    seeded = true;
    setTimeout(seed, 30);
  }
  return () => {};
}

export function emit(event, payload) {
  for (const handler of handlers.get(event) ?? []) handler(payload);
}

/** Наполняем интерфейс так, как он выглядит в обычный рабочий день. */
function seed() {
  SEED.forEach((entry, index) => {
    const id = nextId++;
    emit("tab", { kind: "started", id, url: entry.url });
    emit("tab", { kind: "url", id, url: entry.url });
    emit("tab", { kind: "title", id, title: entry.title });
    emit("tab", { kind: "favicon", id, page: entry.url, url: entry.icon });
    emit("tab", { kind: "finished", id, ok: true, http_status: 200 });
    emit("tab", { kind: "history", id, can_back: index > 0, can_forward: false });
    if (entry.audible) emit("tab", { kind: "audio", id, audible: true, muted: false });
  });
}

/** Данные для скриншотов всплывающего окна: popup.html?kind=… */
export function popupDemo(kind) {
  switch (kind) {
    case "context":
      return {
        tab: 1,
        token: 1,
        rows: [
          { id: "cmd:1", command: 1, name: "back", label: "Назад", keys: "Alt+←", disabled: true },
          { id: "cmd:2", command: 2, name: "reload", label: "Обновить", keys: "Ctrl+R" },
          { separator: true },
          { id: "cmd:3", command: 3, name: "saveAs", label: "Сохранить как", keys: "Ctrl+S" },
          { id: "cmd:4", command: 4, name: "print", label: "Печать", keys: "Ctrl+P" },
          { separator: true },
          { id: "adblock", label: "Не блокировать рекламу на сайте" },
          { id: "cmd:5", command: 5, name: "inspectElement", label: "Просмотреть код", keys: "Ctrl+Shift+I" },
        ],
      };
    case "group":
      return {
        group: { id: 1, title: "Работа", color: "teal", collapsed: false },
        tabs: 4,
        colors: [
          ["rose", "Багровый"],
          ["amber", "Янтарный"],
          ["lime", "Лаймовый"],
          ["teal", "Бирюзовый"],
          ["sky", "Небесный"],
          ["violet", "Фиолетовый"],
          ["slate", "Серый"],
        ],
      };
    case "update":
      return {
        version: "0.2.0",
        current: "0.1.0",
        date: "2026-09-20",
        notes:
          "Пароли сохраняются и подставляются на сайтах со входом во встроенном окне.\nСтраница целиком переводится одной кнопкой в адресной строке.",
      };
    case "menu":
      return {
        menu: "main",
        items: [
          { id: "new-tab", label: "Новая вкладка", icon: "tab-add", keys: "Ctrl+T" },
          { id: "reopen", label: "Открыть закрытую вкладку", icon: "history", keys: "Ctrl+Shift+T" },
          { separator: true },
          { id: "history", label: "История", icon: "history", keys: "Ctrl+H" },
          { id: "downloads", label: "Загрузки", icon: "download", keys: "Ctrl+J" },
          { id: "bookmarks", label: "Закладки", icon: "favorites", keys: "Ctrl+Shift+O" },
          { id: "passwords", label: "Пароли", icon: "key" },
          { id: "extensions", label: "Расширения", icon: "puzzle" },
          { separator: true },
          { type: "zoom", id: "zoom", label: "Масштаб", value: 1 },
          { separator: true },
          { id: "print", label: "Печать…", icon: "print", keys: "Ctrl+P" },
          { id: "find", label: "Найти на странице", icon: "find", keys: "Ctrl+F" },
          { id: "devtools", label: "Инструменты разработчика", icon: "code", keys: "F12" },
          { separator: true },
          { id: "settings", label: "Настройки", icon: "settings" },
          { id: "about", label: "О браузере 190x4", icon: "info" },
          { separator: true },
          { id: "exit", label: "Закрыть браузер", icon: "exit" },
        ],
      };
    case "extensions":
      return {
        kind: "menu",
        payload: {
          menu: "extensions",
          items: [
            { type: "header", label: "Расширения" },
            { id: "media", label: "Загрузчик видео 190x4", icon: "video", trailing: "pinned" },
            { separator: true },
            { id: "manage", label: "Управление расширениями", icon: "settings" },
          ],
        },
      };
    case "bookmark":
      return {
        mode: "edit",
        created: true,
        node: { id: 5, parent_id: 1, kind: "url", title: "Хабр — лучшие публикации за сутки", url: "https://habr.com/ru/feed/" },
        folders: [
          { id: 1, title: "Панель закладок" },
          { id: 6, title: "Панель закладок / Работа" },
          { id: 10, title: "Панель закладок / Кино" },
          { id: 2, title: "Другие закладки" },
        ],
      };
    case "bookmark-folder":
      return { folder: 6, title: "Работа" };
    case "media":
      return { url: "https://www.youtube.com/watch?v=aqz-KE-bpKQ", services: { media: true } };
    case "password":
      return { tab: 1, origin: "https://github.com", username: "pathetixx", update: false };
    case "accounts":
      return { tab: 1, origin: "https://github.com", accounts: [{ id: 1, username: "pathetixx" }, { id: 2, username: "work@190x4.pw" }] };
    case "site":
      return { url: "https://habr.com/ru/", host: "habr.com", secure: true, blocked: 41, adblock: true, site: "habr.com", blocking: true, passwords: 1 };
    case "dialog-alert":
      return { kind: "dialog", payload: { tab: 1, tokens: [1], request: { type: "script", kind: "alert", url: "https://habr.com/ru/", message: "Сессия истекла. Войдите снова, чтобы не потерять черновик.", default_text: "" } } };
    case "dialog-confirm":
      return { kind: "dialog", payload: { tab: 1, tokens: [2], repeat: true, request: { type: "script", kind: "confirm", url: "https://e.mail.ru/inbox/", message: "Удалить 3 письма без возможности восстановления?", default_text: "" } } };
    case "dialog-prompt":
      return { kind: "dialog", payload: { tab: 1, tokens: [3], request: { type: "script", kind: "prompt", url: "https://github.com/", message: "Название новой ветки", default_text: "feature/dialogs" } } };
    case "dialog-leave":
      return { kind: "dialog", payload: { tab: 1, tokens: [4], request: { type: "script", kind: "beforeunload", url: "https://docs.google.com/", message: "", default_text: "" } } };
    case "dialog-permission":
      return { kind: "dialog", payload: { tab: 1, tokens: [5, 6], permissions: ["camera", "microphone"], request: { type: "permission", permission: "camera", url: "https://meet.jit.si/190x4", user_initiated: true } } };
    case "dialog-auth":
      return { kind: "dialog", payload: { tab: 1, tokens: [7], request: { type: "auth", url: "http://192.168.1.1/" } } };
    case "dialog-external":
      return { kind: "dialog", payload: { tab: 1, tokens: [8], request: { type: "external", scheme: "tg", origin: "https://t.me", app: "TG 190x4 EDITION", remember: true } } };
    case "suggest":
      return {
        selected: 1,
        rows: [
          { text: "habr.com", hint: "перейти", value: "habr.com", iconId: "globe-16" },
          { text: "Хабр", hint: "закладка", value: "https://habr.com/ru/feed/", iconId: "star-16", image: ICONS.habr },
          { text: "WebView2: один Environment на все вкладки", hint: "habr.com", value: "https://habr.com/ru/articles/812345/", iconId: "history-16" },
          { text: "habr", hint: "поиск", value: "habr", iconId: "search-16" },
        ],
      };
    default:
      return {};
  }
}

/**
 * Макет страницы на месте нативной поверхности: нейтральный контент, чтобы
 * было видно, как интерфейс обрамляет реальную страницу.
 */
export function paintStage() {
  const stage = document.getElementById("stage-fallback");
  if (!stage) return;

  stage.innerHTML = `
    <style>
      .mockpage { height: 100%; overflow: hidden; background: #0B0B0E; font-family: var(--font-body); color: #EDEDE9; }
      .mockpage__hero { padding: 56px 64px 32px; background: radial-gradient(120% 100% at 12% 0%, rgba(192,48,74,.16) 0%, transparent 62%), #0B0B0E; }
      .mockpage__kicker { font-size: 12px; color: rgba(255,255,255,.4); }
      .mockpage__title { margin: 14px 0 10px; font-family: var(--font-display); font-size: 40px; font-weight: 700; letter-spacing: -.03em; line-height: 1.05; max-width: 16ch; }
      .mockpage__lead { max-width: 62ch; font-size: 14.5px; line-height: 1.65; color: rgba(255,255,255,.55); }
      .mockpage__grid { display: grid; grid-template-columns: repeat(auto-fill, minmax(168px, 1fr)); gap: 14px; padding: 8px 64px 64px; }
      .mockcard { border-radius: 12px; overflow: hidden; background: #131318; border: 1px solid rgba(255,255,255,.05); }
      .mockcard__art { height: 104px; background: linear-gradient(135deg, rgba(255,255,255,.07), rgba(255,255,255,.02)); }
      .mockcard__body { padding: 10px 12px 14px; }
      .mockcard__line { height: 7px; border-radius: 4px; background: rgba(255,255,255,.10); }
      .mockcard__line + .mockcard__line { margin-top: 7px; width: 58%; background: rgba(255,255,255,.06); }
    </style>
    <div class="mockpage">
      <div class="mockpage__hero">
        <div class="mockpage__kicker">Область страницы · нативный WebView2</div>
        <h1 class="mockpage__title">Здесь живёт сайт</h1>
        <p class="mockpage__lead">В собранном приложении этот прямоугольник занимает нативная поверхность вкладки. Интерфейс обрамляет её и никогда не рисует поверх: меню и пузыри живут в отдельном окне.</p>
      </div>
      <div class="mockpage__grid">
        ${Array.from({ length: 12 }).map(() => `<div class="mockcard"><div class="mockcard__art"></div><div class="mockcard__body"><div class="mockcard__line"></div><div class="mockcard__line"></div></div></div>`).join("")}
      </div>
    </div>
  `;
}
