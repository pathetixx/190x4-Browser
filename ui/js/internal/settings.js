/**
 * Страница 190x4://settings.
 *
 * Разделы — как в Chrome и Edge: внешний вид, запуск, поиск, пароли,
 * закладки, конфиденциальность, загрузки, языки, расширения, о браузере.
 * Слева навигация и поиск по настройкам, справа — один раздел.
 */

import { invoke, listen } from "../bridge.js";
import { clock, dayLabel, el, favicon, formatDay, hostOf, icon, iconButton, plural, textButton } from "../dom.js";
import { applyTheme, onPref, pref, setPref } from "../prefs.js";
import { state } from "../state.js";
import { hooks, navigate } from "../actions.js";
import { checkUpdates, installUpdate, onUpdate, update } from "../updates.js";

const SECTIONS = [
  { id: "appearance", title: "Внешний вид", icon: "paint" },
  { id: "startup", title: "При запуске", icon: "power" },
  { id: "search", title: "Поисковая система", icon: "find" },
  { id: "passwords", title: "Пароли и автозаполнение", icon: "key" },
  { id: "bookmarks", title: "Закладки", icon: "favorites" },
  { id: "privacy", title: "Конфиденциальность и безопасность", nav: "Конфиденциальность", icon: "lock-shield" },
  { id: "downloads", title: "Загрузки", icon: "download" },
  { id: "languages", title: "Языки", icon: "language" },
  { id: "extensions", title: "Расширения", icon: "puzzle" },
  { id: "about", title: "О браузере 190x4", icon: "info" },
];

const ENGINES = [
  ["duckduckgo", "DuckDuckGo"],
  ["yandex", "Яндекс"],
  ["google", "Google"],
  ["bing", "Bing"],
];

const LANGUAGES = ["Русский", "English", "Deutsch", "Français", "Español", "Italiano", "Português", "Türkçe", "Українська", "Polski", "中文", "日本語", "한국어"];

export function createSettingsPage(root, { section, onSection }) {
  let current = SECTIONS.some((s) => s.id === section) ? section : "appearance";
  let query = "";

  const page = el("div", "page");
  const nav = el("nav", "page__nav");
  const main = el("div", "page__main");
  page.append(nav, main);
  root.append(page);

  /* ── Навигация ── */
  const brand = el("div", "page__brand");
  const mark = el("img");
  mark.src = "./assets/brand/mark-72.png";
  mark.alt = "";
  brand.append(mark, el("span", null, "Настройки"));
  nav.append(brand);

  const search = el("label", "search");
  search.append(icon("search-16", 16));
  const searchInput = el("input", "field");
  searchInput.type = "search";
  searchInput.placeholder = "Поиск настроек";
  searchInput.addEventListener("input", () => {
    query = searchInput.value.trim().toLowerCase();
    render();
  });
  search.append(searchInput);
  nav.append(search);

  const navItems = new Map();
  for (const item of SECTIONS) {
    const button = el("button", "nav__item");
    button.type = "button";
    button.append(icon(item.icon, 20), el("span", null, item.nav ?? item.title));
    button.addEventListener("click", () => {
      searchInput.value = "";
      query = "";
      show(item.id);
      onSection(item.id);
    });
    navItems.set(item.id, button);
    nav.append(button);
  }

  /* ── Содержимое ── */
  const views = createViews();

  function show(id) {
    current = SECTIONS.some((s) => s.id === id) ? id : "appearance";
    render();
    root.scrollTop = 0;
  }

  function render() {
    for (const [id, button] of navItems) button.setAttribute("aria-current", String(!query && id === current));

    if (query) {
      const nodes = [];
      for (const item of SECTIONS) {
        const node = views.section(item, { searchable: true });
        if (filterSection(node, query)) nodes.push(node);
      }
      main.replaceChildren(...(nodes.length ? nodes : [el("div", "empty", "Ничего не нашлось")]));
      return;
    }
    const item = SECTIONS.find((s) => s.id === current);
    main.replaceChildren(views.section(item, { searchable: false }));
  }

  const rerender = () => {
    if (!root.contains(document.activeElement) || document.activeElement === searchInput) render();
  };
  const offPref = onPref(rerender);
  const unlisten = [listen("passwords", () => views.refresh("passwords")), listen("bookmarks", () => views.refresh("bookmarks"))];

  render();

  return {
    show,
    destroy() {
      offPref();
      for (const promise of unlisten) promise.then((off) => off?.());
    },
  };

  /* ── Поиск: оставить только совпадающие строки ── */
  function filterSection(node, needle) {
    let any = false;
    for (const group of node.querySelectorAll(".group")) {
      let groupHit = false;
      for (const row of group.querySelectorAll("[data-search]")) {
        const hit = row.dataset.search.includes(needle);
        row.hidden = !hit;
        groupHit ||= hit;
      }
      group.hidden = !groupHit;
      any ||= groupHit;
    }
    return any;
  }

  /* ─────────────────────────────────────────────────────────── */

  function createViews() {
    const live = new Map();

    return {
      section(item, { searchable }) {
        const node = el("section", "section");
        node.id = `section-${item.id}`;
        node.append(el("h2", "section__title", item.title));
        const content = BUILDERS[item.id]({ searchable });
        node.append(...content);
        live.set(item.id, () => {
          const fresh = BUILDERS[item.id]({ searchable });
          node.replaceChildren(el("h2", "section__title", item.title), ...fresh);
        });
        return node;
      },
      refresh(id) {
        if (current === id && !query) live.get(id)?.();
      },
    };
  }
}

/* ── Строительные блоки ─────────────────────────────────────── */

function group(children, { title, hint, actions } = {}) {
  const node = el("div", "group");
  if (title || actions) {
    const head = el("div", "group__head");
    head.append(el("div", "group__title", title ?? ""));
    if (actions) head.append(...actions);
    node.append(head);
  }
  if (hint) node.append(el("div", "group__hint", hint));
  node.append(...children.filter(Boolean));
  return node;
}

function setting(label, hint, control, { iconId } = {}) {
  const row = el("div", "setting");
  row.dataset.search = `${label} ${hint ?? ""}`.toLowerCase();
  if (iconId) {
    const tile = el("div", "setting__icon");
    tile.append(icon(iconId, 20));
    row.append(tile);
  }
  const text = el("div", "setting__text");
  text.append(el("div", "setting__label", label));
  if (hint) text.append(el("div", "setting__hint", hint));
  row.append(text);
  if (control) row.append(control);
  return row;
}

function toggle(key, { onChange } = {}) {
  const node = el("button", "switch");
  node.type = "button";
  node.setAttribute("role", "switch");
  node.setAttribute("aria-checked", String(Boolean(pref(key))));
  node.addEventListener("click", async () => {
    const value = !pref(key);
    node.setAttribute("aria-checked", String(value));
    await setPref(key, value);
    onChange?.(value);
  });
  return node;
}

function switchSetting(key, label, hint, options) {
  const row = setting(label, hint, toggle(key, options));
  row.style.cursor = "pointer";
  row.addEventListener("click", (event) => {
    if (!event.target.closest(".switch")) row.querySelector(".switch").click();
  });
  return row;
}

function select(key, options, { onChange } = {}) {
  const node = el("select", "field");
  for (const [value, label] of options) {
    const option = el("option", null, label);
    option.value = value;
    option.selected = String(pref(key)) === value;
    node.append(option);
  }
  node.addEventListener("change", async () => {
    await setPref(key, node.value);
    onChange?.(node.value);
  });
  return node;
}

function choices(key, options, { onChange } = {}) {
  const node = el("div", "choices");
  node.setAttribute("role", "radiogroup");
  for (const [value, label, hint] of options) {
    const choice = el("button", "choice");
    choice.type = "button";
    choice.setAttribute("role", "radio");
    choice.dataset.search = `${label} ${hint ?? ""}`.toLowerCase();
    choice.setAttribute("aria-checked", String(pref(key) === value));
    choice.append(el("span", null, label));
    if (hint) choice.append(el("span", "choice__hint", hint));
    choice.addEventListener("click", async () => {
      await setPref(key, value);
      onChange?.(value);
    });
    node.append(choice);
  }
  return node;
}

function button(label, onClick, { iconId, kind = "btn" } = {}) {
  const node = el("button", kind);
  node.type = "button";
  if (iconId) node.append(icon(iconId, 16));
  node.append(el("span", null, label));
  node.addEventListener("click", onClick);
  return node;
}

/** Диалог внутри страницы. Возвращает выбранное действие или null. */
function modal({ title, text, body = [], actions }) {
  return new Promise((resolve) => {
    const scrim = el("div", "modal-scrim");
    const card = el("div", "modal");
    card.setAttribute("role", "dialog");
    const content = el("div", "modal__body");
    content.append(el("h3", "modal__title", title));
    if (text) content.append(el("p", "modal__text", text));
    content.append(...body);
    const foot = el("div", "modal__foot");
    const close = (value) => {
      scrim.remove();
      document.removeEventListener("keydown", onKey, true);
      resolve(value);
    };
    for (const [value, label, kind] of actions) {
      foot.append(textButton(label, () => close(value), kind ?? "btn"));
    }
    card.append(content, foot);
    scrim.append(card);
    scrim.addEventListener("mousedown", (event) => {
      if (event.target === scrim) close(null);
    });
    const onKey = (event) => {
      if (event.key === "Escape") {
        event.stopPropagation();
        close(null);
      }
      if (event.key === "Enter" && event.target.tagName !== "BUTTON") {
        event.preventDefault();
        const primary = actions.find(([, , kind]) => kind?.includes("primary"));
        if (primary) close(primary[0]);
      }
    };
    document.addEventListener("keydown", onKey, true);
    document.getElementById("internal").append(scrim);
    (content.querySelector("input") ?? foot.lastElementChild)?.focus();
  });
}

function field(value = "", { placeholder = "", type = "text", mono = false } = {}) {
  const input = el("input", mono ? "field field--mono" : "field");
  input.type = type;
  input.value = value;
  input.placeholder = placeholder;
  input.spellcheck = false;
  input.autocomplete = "off";
  return input;
}

function label(text) {
  return el("label", "label", text);
}

async function attempt(promise, success) {
  try {
    const result = await promise;
    if (success) hooks.toast(typeof success === "function" ? success(result) : success);
    return result;
  } catch (error) {
    hooks.toast(String(error?.message ?? error));
    return undefined;
  }
}

/* ── Разделы ────────────────────────────────────────────────── */

const BUILDERS = {
  appearance() {
    return [
      group(
        [
          choices(
            "theme",
            [
              ["kurogane", "Тёмная", "Kurogane"],
              ["shiro", "Светлая", "Shiro"],
              ["system", "Как в системе", ""],
            ],
            { onChange: applyTheme }
          ),
        ],
        { title: "Тема" }
      ),
      group(
        [
          setting(
            "Панель закладок",
            "Строка с закладками под адресной строкой. Быстро переключается сочетанием Ctrl+Shift+B.",
            select("bookmarks_bar", [
              ["always", "Всегда показывать"],
              ["newtab", "Только на новой вкладке"],
              ["never", "Не показывать"],
            ])
          ),
          switchSetting("show_home", "Кнопка «Домой»", "Кнопка слева от адресной строки"),
          pref("show_home")
            ? choices("home_page", [
                ["newtab", "Открывать новую вкладку", ""],
                ["url", "Открывать свою страницу", pref("home_url") || "адрес не задан"],
              ])
            : null,
          pref("show_home") && pref("home_page") === "url"
            ? (() => {
                const input = field(pref("home_url"), { placeholder: "https://…" });
                input.addEventListener("change", () => setPref("home_url", input.value.trim()));
                return setting("Адрес домашней страницы", "", input);
              })()
            : null,
          setting(
            "Кнопка загрузок",
            "Кнопка со списком загрузок справа на панели инструментов",
            select("downloads_button", [
              ["auto", "Когда есть загрузки"],
              ["always", "Всегда"],
            ])
          ),
          switchSetting("statusbar", "Строка состояния", "Состояние страницы, число блокировок и вкладок внизу окна"),
        ],
        { title: "Панели" }
      ),
    ];
  },

  startup() {
    const pages = Array.isArray(pref("startup_pages")) ? pref("startup_pages") : [];
    const list = [];
    if (pref("startup") === "pages") {
      for (const url of pages) {
        const row = el("div", "row row--simple");
        row.dataset.search = url.toLowerCase();
        row.append(icon("globe-16", 16), el("span", "row__primary", url));
        const actions = el("div", "row__actions");
        actions.append(
          iconButton("delete-16", "Удалить", () =>
            setPref(
              "startup_pages",
              pages.filter((page) => page !== url)
            )
          )
        );
        row.append(actions);
        list.push(row);
      }
      const add = el("div", "setting");
      add.dataset.search = "добавить страницу";
      const input = field("", { placeholder: "Адрес страницы, например ya.ru" });
      input.style.width = "100%";
      const addButton = button("Добавить", () => {
        const value = input.value.trim();
        if (!value) return;
        const url = /^[a-z]+:\/\//i.test(value) ? value : `https://${value}`;
        setPref("startup_pages", [...pages, url]);
      });
      input.addEventListener("keydown", (event) => {
        if (event.key === "Enter") addButton.click();
      });
      const useOpen = button("Использовать открытые вкладки", () => {
        const urls = [...state.tabs.values()]
          .filter((tab) => !tab.internal && tab.url && !tab.url.includes("190x4-pages.invalid"))
          .map((tab) => tab.url);
        setPref("startup_pages", urls);
      }, { kind: "btn btn--ghost" });
      add.append(input, addButton, useOpen);
      list.push(add);
    }

    return [
      group(
        [
          choices("startup", [
            ["newtab", "Открывать новую вкладку", ""],
            ["restore", "Продолжать работу с того же места", "вкладки прошлого сеанса"],
            ["pages", "Открывать заданные страницы", ""],
          ]),
          ...(list.length ? [el("div", "rows"), ...list] : []),
        ],
        { title: "Что открывать при запуске браузера" }
      ),
    ];
  },

  search() {
    return [
      group([
        setting(
          "Поисковая система в адресной строке",
          "Запросы, набранные в адресной строке и на новой вкладке, уходят сюда",
          select("search_engine", ENGINES)
        ),
      ]),
    ];
  },

  passwords({ searchable }) {
    const out = [
      group(
        [
          switchSetting("passwords_offer", "Предлагать сохранять пароли", "Браузер спросит после входа на сайт"),
          switchSetting(
            "passwords_autofill",
            "Автоматически заполнять формы входа",
            "Логин и пароль подставляются, когда сайт показывает форму входа"
          ),
        ],
        { title: "Менеджер паролей" }
      ),
    ];
    if (searchable) return out;

    const list = el("div", "rows");
    const filterInput = field("", { placeholder: "Поиск паролей", type: "search" });
    filterInput.style.width = "200px";

    const saved = group([list], {
      title: "Сохранённые пароли",
      actions: [filterInput, button("Добавить", () => editPassword(null), { iconId: "add-16" })],
    });
    const notice = el("div", "notice");
    notice.append(
      icon("lock-16", 16),
      el(
        "span",
        null,
        "Пароли зашифрованы средствами Windows: прочитать их может только ваша учётная запись на этом компьютере."
      )
    );
    saved.append(notice);
    out.push(saved);

    out.push(
      group(
        [
          setting(
            "Импорт паролей",
            "CSV-файл из Chrome, Edge, Яндекс Браузера, Firefox или Bitwarden",
            button("Выбрать файл", importPasswords, { iconId: "import" })
          ),
          setting(
            "Экспорт паролей",
            "CSV-файл в формате Chrome — его принимают другие браузеры и менеджеры паролей",
            button("Экспортировать", exportPasswords, { iconId: "export" })
          ),
        ],
        { title: "Импорт и экспорт" }
      )
    );

    const never = el("div", "rows");
    const neverGroup = group([never], { title: "Сайты, для которых пароли не сохраняются" });
    out.push(neverGroup);

    let entries = [];
    const draw = () => {
      const needle = filterInput.value.trim().toLowerCase();
      const visible = entries.filter(
        (entry) => !needle || entry.origin.toLowerCase().includes(needle) || entry.username.toLowerCase().includes(needle)
      );
      list.replaceChildren();
      if (!visible.length) {
        list.append(el("div", "empty", entries.length ? "Ничего не нашлось" : "Сохранённых паролей пока нет"));
        return;
      }
      for (const entry of visible) list.append(passwordRow(entry));
    };
    filterInput.addEventListener("input", draw);

    invoke("passwords_list")
      .then((items) => {
        entries = items;
        draw();
      })
      .catch(() => list.replaceChildren(el("div", "empty", "Пароли недоступны")));

    invoke("password_never_list")
      .then((sites) => {
        neverGroup.hidden = !sites.length;
        never.replaceChildren(
          ...sites.map((site) => {
            const row = el("div", "row row--simple");
            row.append(favicon(`${site.origin}/favicon.ico`), el("span", "row__primary", hostOf(site.origin)));
            const actions = el("div", "row__actions");
            actions.append(
              iconButton("dismiss-16", "Убрать из списка", () => invoke("password_never_forget", { origin: site.origin }))
            );
            row.append(actions);
            return row;
          })
        );
      })
      .catch(() => {
        neverGroup.hidden = true;
      });

    return out;
  },

  bookmarks({ searchable }) {
    const out = [
      group(
        [
          setting(
            "Панель закладок",
            "Ctrl+Shift+B",
            select("bookmarks_bar", [
              ["always", "Всегда показывать"],
              ["newtab", "Только на новой вкладке"],
              ["never", "Не показывать"],
            ])
          ),
          setting(
            "Импорт закладок",
            "HTML-файл из Chrome, Edge, Яндекс Браузера, Firefox или Opera",
            button("Выбрать файл", importBookmarks, { iconId: "import" })
          ),
          setting(
            "Экспорт закладок",
            "Сохранить все закладки в HTML-файл, который понимает любой браузер",
            button("Экспортировать", exportBookmarks, { iconId: "export" })
          ),
        ],
        { title: "Закладки" }
      ),
    ];
    if (searchable) return out;
    out.push(bookmarkManager());
    return out;
  },

  privacy() {
    const lists = el("div");
    const adblockGroup = group(
      [
        switchSetting("adblock_enabled", "Блокировать рекламу и трекеры", "Встроенный фильтр 190x4 на всех сайтах", {
          onChange: (on) => {
            state.adblockOn = on;
          },
        }),
        lists,
      ],
      { title: "Блокировка рекламы" }
    );

    const NOTES = {
      easylist: "Основной список рекламы",
      easyprivacy: "Счётчики, пиксели и трекеры",
      ruadlist: "Реклама на русскоязычных сайтах",
    };
    invoke("adblock_lists")
      .then((items) => {
        lists.replaceChildren(
          ...items.map((item) => {
            const node = el("button", "switch");
            node.type = "button";
            node.setAttribute("aria-checked", String(item.enabled));
            node.addEventListener("click", async () => {
              item.enabled = !item.enabled;
              node.setAttribute("aria-checked", String(item.enabled));
              const enabled = items.filter((list) => list.enabled).map((list) => list.id);
              await setPref("adblock_lists", enabled);
              hooks.toast("Списки фильтров пересобираются");
            });
            const row = setting(item.title, NOTES[item.id] ?? "", node);
            return row;
          })
        );
      })
      .catch(() => {});

    const clear = el("button", "setting setting--action");
    clear.dataset.search = "удалить данные история cookie кэш очистить";
    const tile = el("div", "setting__icon");
    tile.append(icon("broom", 20));
    const text = el("div", "setting__text");
    text.append(
      el("div", "setting__label", "Удалить данные о работе в браузере"),
      el("div", "setting__hint", "История, файлы cookie и данные сайтов, кэш, список загрузок")
    );
    clear.append(tile, text, icon("chevron-right-16", 16, "setting__chevron"));
    clear.addEventListener("click", clearBrowsingData);

    return [group([clear], { title: "Данные браузера" }), adblockGroup];
  },

  downloads() {
    const dir = pref("download_dir");
    const change = button("Изменить", async () => {
      await attempt(invoke("download_folder_pick"));
    });
    const openFolder = iconButton("folder-open", "Открыть папку", () => invoke("downloads_folder_open"), {
      size: 20,
      className: "btn btn--ghost btn--icon",
    });
    const location = setting("Папка для загрузок", dir || "Загрузки (по умолчанию Windows)", el("div"));
    location.lastElementChild.replaceWith(openFolder, change);

    return [
      group(
        [
          location,
          switchSetting("download_ask", "Всегда указывать место для скачивания", "Перед каждой загрузкой откроется окно «Сохранить как»"),
          switchSetting("download_bubble", "Показывать загрузки, когда скачивание начинается", "Список загрузок откроется под кнопкой на панели"),
        ],
        { title: "Загрузки" }
      ),
    ];
  },

  languages() {
    const status = state.services.translate ? "подключён" : "не настроен";
    return [
      group(
        [
          setting(
            "Язык перевода",
            "На этот язык переводится выделенный текст",
            select(
              "translate_lang",
              LANGUAGES.map((name) => [name, name])
            )
          ),
          switchSetting("translate_button", "Кнопка перевода в адресной строке", `Сервис перевода 190x4 ${status}`),
        ],
        { title: "Перевод" }
      ),
    ];
  },

  extensions() {
    const mediaReady = state.services.media;
    const translateReady = state.services.translate;
    return [
      group(
        [
          setting(
            "Загрузчик видео 190x4",
            `Скачивает видео и звук с YouTube, VK, Rutube и других сайтов через сервер 190x4. Ctrl+Shift+D. Сервис ${mediaReady ? "подключён" : "не настроен"}.`,
            toggle("ext_media_enabled"),
            { iconId: "video" }
          ),
          switchSetting("ext_media_pinned", "Показывать значок на панели инструментов", "Иначе расширение доступно из меню расширений"),
        ],
        { title: "Установленные расширения" }
      ),
      group(
        [
          setting(
            "Переводчик 190x4",
            `Переводит выделенный текст из контекстного меню страницы. Сервис ${translateReady ? "подключён" : "не настроен"}.`,
            toggle("translate_button"),
            { iconId: "translate" }
          ),
        ],
        { title: "Встроенные" }
      ),
    ];
  },

  about() {
    const card = el("div", "about");
    const mark = el("img");
    mark.src = "./assets/brand/mark-72.png";
    mark.alt = "";
    const text = el("div");
    const meta = el("div", "about__meta", "Версия …");
    text.append(el("div", "about__name", "190x4 Browser"), meta);
    card.append(mark, text);

    const profile = setting("Папка профиля", "…", el("div"));
    profile.lastElementChild.replaceWith(button("Открыть", () => invoke("profile_open"), { kind: "btn btn--ghost" }));

    invoke("about_info")
      .then((info) => {
        meta.replaceChildren(
          document.createTextNode(`Версия ${info.version}`),
          el("br"),
          document.createTextNode(`Движок Microsoft Edge WebView2 ${info.webview || "—"}`),
          el("br"),
          document.createTextNode("© 2026 pathetixx · 190x4")
        );
        profile.querySelector(".setting__hint").textContent = info.profile;
      })
      .catch(() => {});

    // Обновления: состояние живёт в updates.js, строка перерисовывается по нему.
    const updates = setting("Обновления", "…", el("div"));
    const updatesHint = updates.querySelector(".setting__hint");
    const updatesSlot = updates.lastElementChild;
    const drawUpdates = () => {
      let text = "Браузер проверяет обновления сам";
      let control = button("Проверить обновления", () => checkUpdates(), { kind: "btn btn--ghost" });
      if (update.installing) {
        text = `Устанавливается версия ${update.info?.version ?? ""} — браузер перезапустится сам`;
        control = null;
      } else if (update.info) {
        text = `Доступна версия ${update.info.version}${update.info.date ? ` от ${formatDay(update.info.date)}` : ""}`;
        control = button("Обновить и перезапустить", () => installUpdate(), { kind: "btn btn--primary" });
      } else if (update.checking) {
        text = "Проверка…";
        control = null;
      } else if (update.error) {
        text = `Проверить не удалось: ${update.error}`;
        control = button("Проверить снова", () => checkUpdates(), { kind: "btn btn--ghost" });
      } else if (update.checked) {
        text = "Установлена последняя версия";
      }
      updatesHint.textContent = text;
      updatesSlot.replaceChildren(...(control ? [control] : []));
    };
    drawUpdates();
    const stopUpdates = onUpdate(() => (updates.isConnected ? drawUpdates() : stopUpdates()));
    const autoUpdates = switchSetting(
      "updates_auto",
      "Проверять обновления автоматически",
      "После запуска и раз в шесть часов, пока браузер открыт"
    );

    const licenses = setting(
      "Сторонние компоненты",
      "Значки интерфейса — Fluent UI System Icons (Microsoft, MIT). Блокировка — adblock-rust (Brave, MPL-2.0).",
      null
    );

    return [group([card]), group([updates, autoUpdates], { title: "Обновления" }), group([profile, licenses])];
  },
};

/* ── Пароли ─────────────────────────────────────────────────── */

function passwordRow(entry) {
  const row = el("div", "row");
  const host = hostOf(entry.origin);
  row.append(favicon(`${entry.origin}/favicon.ico`));

  const siteCell = el("div", "row__primary");
  siteCell.append(el("span", null, host));
  const sub = el("span", "row__sub", entry.used_at ? `вход ${dayLabel(entry.used_at).toLowerCase()} в ${clock(entry.used_at)}` : entry.origin);
  siteCell.append(sub);

  const user = el("div", "row__secondary", entry.username || "без логина");
  const secret = el("div", "row__secret", "••••••••••");
  let revealed = false;

  const actions = el("div", "row__actions");
  const eye = iconButton("eye-16", "Показать пароль", async () => {
    if (revealed) {
      secret.textContent = "••••••••••";
      eye.replaceChildren(icon("eye-16", 16));
      eye.title = "Показать пароль";
      revealed = false;
      return;
    }
    const password = await attempt(invoke("password_reveal", { id: entry.id }));
    if (password == null) return;
    secret.textContent = password;
    eye.replaceChildren(icon("eye-off-16", 16));
    eye.title = "Скрыть пароль";
    revealed = true;
  });
  actions.append(
    eye,
    iconButton("copy-16", "Копировать пароль", async () => {
      const password = await attempt(invoke("password_reveal", { id: entry.id }));
      if (password == null) return;
      await navigator.clipboard.writeText(password).catch(() => {});
      hooks.toast("Пароль скопирован");
    }),
    iconButton("edit-16", "Изменить", () => editPassword(entry)),
    iconButton("delete-16", "Удалить", async () => {
      const answer = await modal({
        title: "Удалить пароль?",
        text: `${host} · ${entry.username || "без логина"}`,
        actions: [
          [null, "Отмена", "btn btn--ghost"],
          ["delete", "Удалить", "btn btn--primary"],
        ],
      });
      if (answer === "delete") attempt(invoke("password_delete", { id: entry.id }), "Пароль удалён");
    })
  );

  row.append(siteCell, user, secret, actions);
  return row;
}

async function editPassword(entry) {
  const site = field(entry?.origin ?? "", { placeholder: "https://example.com" });
  site.disabled = Boolean(entry);
  const username = field(entry?.username ?? "", { placeholder: "Логин или почта" });
  const password = field("", { placeholder: entry ? "Оставьте пустым, чтобы не менять" : "Пароль", type: "password" });
  const secretBox = el("div", "secret");
  const eye = el("button");
  eye.type = "button";
  eye.title = "Показать";
  eye.append(icon("eye-16", 16));
  eye.addEventListener("click", () => {
    password.type = password.type === "password" ? "text" : "password";
  });
  secretBox.append(password, eye);

  const answer = await modal({
    title: entry ? "Изменить пароль" : "Добавить пароль",
    body: [label("Сайт"), site, label("Имя пользователя"), username, label("Пароль"), secretBox],
    actions: [
      [null, "Отмена", "btn btn--ghost"],
      ["save", "Сохранить", "btn btn--primary"],
    ],
  });
  if (answer !== "save") return;

  if (entry) {
    attempt(
      invoke("password_update", { id: entry.id, username: username.value, password: password.value || null }),
      "Пароль сохранён"
    );
  } else {
    attempt(
      invoke("password_add", { url: site.value, username: username.value, password: password.value }),
      "Пароль сохранён"
    );
  }
}

async function importPasswords() {
  const report = await attempt(invoke("passwords_import"));
  if (!report) return;
  hooks.toast(
    `Импортировано ${report.imported} ${plural(report.imported, "пароль", "пароля", "паролей")}` +
      (report.skipped ? `, пропущено ${report.skipped}` : "")
  );
}

async function exportPasswords() {
  const answer = await modal({
    title: "Экспортировать пароли?",
    text: "Пароли будут сохранены в CSV-файл открытым текстом. Любой, у кого окажется этот файл, увидит их. Удалите файл после переноса.",
    actions: [
      [null, "Отмена", "btn btn--ghost"],
      ["export", "Экспортировать", "btn btn--primary"],
    ],
  });
  if (answer !== "export") return;
  const path = await attempt(invoke("passwords_export"));
  if (path) hooks.toast(`Пароли сохранены: ${path}`);
}

/* ── Закладки ───────────────────────────────────────────────── */

async function importBookmarks() {
  const report = await attempt(invoke("bookmarks_import"));
  if (!report) return;
  hooks.toast(
    `Импортировано ${report.links} ${plural(report.links, "закладка", "закладки", "закладок")}` +
      (report.folders ? ` и ${report.folders} ${plural(report.folders, "папка", "папки", "папок")}` : "") +
      (report.skipped ? `, повторы пропущены: ${report.skipped}` : "")
  );
}

async function exportBookmarks() {
  const path = await attempt(invoke("bookmarks_export"));
  if (path) hooks.toast(`Закладки сохранены: ${path}`);
}

/** 0 — корень диспетчера: обе корневые папки рядом. */
let managerFolder = 0;

function bookmarkManager() {
  const list = el("div", "rows");
  const crumbs = el("div", "crumbs");
  const filterInput = field("", { placeholder: "Поиск закладок", type: "search" });
  filterInput.style.width = "200px";

  const container = group([crumbs, list], {
    title: "Диспетчер закладок",
    actions: [filterInput, button("Новая папка", () => newFolder(), { iconId: "folder-16" })],
  });

  let nodes = [];
  const byId = () => new Map(nodes.map((node) => [node.id, node]));

  const draw = () => {
    const map = byId();
    if (managerFolder !== 0 && !map.has(managerFolder)) managerFolder = 0;
    const needle = filterInput.value.trim().toLowerCase();

    crumbs.replaceChildren();
    crumbs.hidden = Boolean(needle);
    const trail = [];
    for (let cursor = map.get(managerFolder); cursor; cursor = map.get(cursor.parent_id)) trail.unshift(cursor);
    const rootButton = el("button", null, "Закладки");
    rootButton.setAttribute("aria-current", String(managerFolder === 0));
    rootButton.addEventListener("click", () => {
      managerFolder = 0;
      draw();
    });
    crumbs.append(rootButton);
    for (const node of trail) {
      crumbs.append(icon("chevron-right-12", 12));
      const crumb = el("button", null, node.title);
      crumb.setAttribute("aria-current", String(node.id === managerFolder));
      crumb.addEventListener("click", () => {
        managerFolder = node.id;
        draw();
      });
      crumbs.append(crumb);
    }

    let items;
    if (needle) {
      items = nodes.filter(
        (node) => node.kind === "url" && (node.title.toLowerCase().includes(needle) || node.url.toLowerCase().includes(needle))
      );
    } else if (managerFolder === 0) {
      // Корень диспетчера: «Панель закладок» и «Другие закладки» рядом.
      items = nodes.filter((node) => node.parent_id == null).sort((a, b) => a.position - b.position);
    } else {
      items = nodes.filter((node) => node.parent_id === managerFolder).sort((a, b) => a.position - b.position);
    }

    list.replaceChildren();
    if (!items.length) {
      list.append(el("div", "empty", needle ? "Ничего не нашлось" : "Папка пуста"));
      return;
    }
    for (const node of items) list.append(bookmarkRow(node, map));
  };

  const bookmarkRow = (node, map) => {
    const row = el("div", "row row--simple");
    const main = el("div", "row__primary");
    if (node.kind === "folder") {
      row.append(icon("folder", 20));
      const count = nodes.filter((child) => child.parent_id === node.id).length;
      main.append(el("span", null, node.title), el("span", "row__sub", `${count} ${plural(count, "элемент", "элемента", "элементов")}`));
      row.style.cursor = "pointer";
      row.addEventListener("click", (event) => {
        if (event.target.closest(".row__actions")) return;
        managerFolder = node.id;
        filterInput.value = "";
        draw();
      });
    } else {
      row.append(favicon(node.icon));
      main.append(el("span", null, node.title || node.url), el("span", "row__sub", node.url));
      main.style.cursor = "pointer";
      main.addEventListener("click", () => navigate(node.url, { newTab: true }));
    }
    row.append(main);

    const actions = el("div", "row__actions");
    const root = node.parent_id == null;
    if (!root) {
      actions.append(
        iconButton("edit-16", node.kind === "folder" ? "Переименовать" : "Изменить", () => editNode(node, map)),
        iconButton("delete-16", "Удалить", async () => {
          if (node.kind === "folder") {
            const answer = await modal({
              title: "Удалить папку?",
              text: `«${node.title}» и всё, что в ней лежит.`,
              actions: [
                [null, "Отмена", "btn btn--ghost"],
                ["delete", "Удалить", "btn btn--primary"],
              ],
            });
            if (answer !== "delete") return;
          }
          attempt(invoke("bookmark_remove", { id: node.id }));
        })
      );
    }
    row.append(actions);
    return row;
  };

  const folderOptions = (map, exclude) =>
    nodes
      .filter((node) => node.kind === "folder" && node.id !== exclude)
      .map((node) => {
        const parts = [];
        for (let cursor = node; cursor; cursor = map.get(cursor.parent_id)) parts.unshift(cursor.title);
        return [node.id, parts.join(" / ")];
      });

  const editNode = async (node, map) => {
    const title = field(node.title);
    const url = node.kind === "url" ? field(node.url) : null;
    const folder = el("select", "field");
    for (const [id, path] of folderOptions(map, node.id)) {
      const option = el("option", null, path);
      option.value = id;
      option.selected = id === node.parent_id;
      folder.append(option);
    }
    const answer = await modal({
      title: node.kind === "folder" ? "Переименовать папку" : "Изменить закладку",
      body: [label("Название"), title, ...(url ? [label("Адрес"), url] : []), label("Папка"), folder],
      actions: [
        [null, "Отмена", "btn btn--ghost"],
        ["save", "Сохранить", "btn btn--primary"],
      ],
    });
    if (answer !== "save") return;
    attempt(
      invoke("bookmark_update", {
        id: node.id,
        title: title.value.trim(),
        url: url ? url.value.trim() : null,
        parent: Number(folder.value),
      })
    );
  };

  const newFolder = async () => {
    const title = field("Новая папка");
    const answer = await modal({
      title: "Новая папка",
      body: [label("Название"), title],
      actions: [
        [null, "Отмена", "btn btn--ghost"],
        ["save", "Создать", "btn btn--primary"],
      ],
    });
    if (answer !== "save" || !title.value.trim()) return;
    attempt(invoke("bookmark_folder_add", { parent: managerFolder || 1, title: title.value.trim() }));
  };

  filterInput.addEventListener("input", draw);
  invoke("bookmarks_tree")
    .then((items) => {
      nodes = items;
      draw();
    })
    .catch(() => list.replaceChildren(el("div", "empty", "Закладки недоступны")));

  return container;
}

/* ── Удаление данных ────────────────────────────────────────── */

async function clearBrowsingData() {
  const options = [
    ["history", "История посещений", "адреса и заголовки страниц"],
    ["site_data", "Файлы cookie и данные сайтов", "выход из аккаунтов на сайтах"],
    ["cache", "Кэшированные изображения и файлы", ""],
    ["downloads", "Список загрузок", "сами файлы останутся на диске"],
  ];
  const selected = new Set(["history", "cache"]);
  const checks = options.map(([key, text, hint]) => {
    const check = el("button", "check");
    check.type = "button";
    check.setAttribute("role", "checkbox");
    check.setAttribute("aria-checked", String(selected.has(key)));
    check.append(el("span", null, text));
    if (hint) check.append(el("span", "check__hint", hint));
    check.addEventListener("click", () => {
      if (selected.has(key)) selected.delete(key);
      else selected.add(key);
      check.setAttribute("aria-checked", String(selected.has(key)));
    });
    return check;
  });

  const answer = await modal({
    title: "Удалить данные о работе в браузере",
    text: "Данные удаляются за всё время на этом компьютере.",
    body: checks,
    actions: [
      [null, "Отмена", "btn btn--ghost"],
      ["clear", "Удалить данные", "btn btn--primary"],
    ],
  });
  if (answer !== "clear" || !selected.size) return;

  attempt(
    invoke("browsing_data_clear", {
      history: selected.has("history"),
      downloads: selected.has("downloads"),
      siteData: selected.has("site_data"),
      cache: selected.has("cache"),
    }),
    "Данные удалены"
  );
}
