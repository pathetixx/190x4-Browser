// Менеджер паролей 190x4: встраивается в каждый документ и в каждый фрейм до
// скриптов страницы (AddScriptToExecuteOnDocumentCreated).
//
// Скрипт ничего не хранит и ничего не решает. Он сообщает браузеру факты —
// «здесь форма входа», «отправлен логин с паролем», «человек встал в поле
// логина» — и подставляет учётку, которую человек выбрал в окне браузера.
// Списка учёток страница не получает: канал chrome.webview слышит любой скрипт
// документа, и всё, что пришло бы сюда, увидела бы и сама страница.
// Происхождение сообщения Rust берёт из движка, а не из текста сообщения.
(() => {
  "use strict";

  // Скрипт ставится при создании документа; повторно он не нужен. Метка —
  // безымянный символ: по говорящему имени в window сайт узнавал бы браузер,
  // а это лишняя строчка в отпечатке.
  const MARK = Symbol.for("pm");
  if (window[MARK]) return;
  const status = { stage: "start", asked: "", filled: "", posts: 0, error: "" };
  try {
    Object.defineProperty(window, MARK, { value: status, enumerable: false });
  } catch (_) {
    return;
  }

  try {

  // Свои ссылки на DOM — до скриптов страницы: подменённые страницей методы
  // не увидят ни логина, ни пароля, которые браузер подставляет в форму.
  const call = Function.prototype.call.bind(Function.prototype.call);
  const setter = (object, name) => Object.getOwnPropertyDescriptor(object, name).set;
  const dom = {
    listen: EventTarget.prototype.addEventListener,
    value: setter(HTMLInputElement.prototype, "value"),
  };
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
  // Похоже ли поле на логин. Видимость (`usable`) проверяется последней: она
  // пересчитывает раскладку страницы, а полей ввода на странице бывают сотни.
  const loginLike = (input) =>
    input instanceof HTMLInputElement &&
    /^(text|email|tel|)$/i.test(input.type) &&
    !SEARCH_HINT.test(describe(input)) &&
    (hasToken(input, "username") || input.type === "email" || LOGIN_HINT.test(describe(input)));
  const loginsIn = (scope) =>
    Array.from((scope || document).querySelectorAll("input")).filter((input) => loginLike(input) && usable(input));

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

  // Поле, у которого браузер показывает список учёток: пароль, логин рядом с
  // паролем или поле логина на шаге без пароля.
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

  // Средняя кнопка по ссылке открывает вкладку в фоне, как в Chrome. Движок не
  // сообщает, какой кнопкой открыто новое окно, — об этом говорит нажатие.
  on(
    window,
    "mousedown",
    (event) => {
      if (event.button !== 1 || !event.isTrusted || !(event.target instanceof Element)) return;
      if (event.target.closest("a[href]")) post({ evt: "middle_click" });
    },
    true
  );

  // Разбирать щелчки и Enter стоит только там, где есть вход: поиск формы
  // вокруг кнопки обходит DOM и пересчитывает раскладку, а на странице без
  // формы входа (почти на любой) это была бы цена каждого щелчка.
  const hasLogin = () => asked !== "" || document.querySelector('input[type="password"]') !== null;

  // Отправка формы сразу уводит страницу. Сообщение, ушедшее за миг до этого,
  // может не дойти — повторяем его, когда документ уже уходит.
  on(window, "pagehide", () => {
    closeMenu();
    if (lastSubmit) post(lastSubmit);
  });

  on(
    document,
    "click",
    (event) => {
      if (!hasLogin()) return;
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
      if (event.key === "Enter" && event.target instanceof HTMLInputElement && hasLogin()) {
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
    if (step && step !== asked && asked !== "password" && post({ evt: "password_form", step })) {
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

  // Форма входа, открытая позже (окно «Войти» через минуту чтения), за DOM
  // уже не следят: её узнаём, когда человек встаёт в поле логина или пароля.
  on(
    document,
    "focusin",
    (event) => {
      const target = event.target;
      if (asked === "password" || !(target instanceof HTMLInputElement)) return;
      if (target.type === "password" || loginLike(target)) ask();
    },
    true
  );

  /* ── Список учёток у поля ───────────────────────────────────── */

  // Список рисует окно браузера, а выбор в нём делает человек: страница только
  // говорит, где поле. Есть ли у сайта учётки, она так и не узнаёт — список
  // появится или нет, но не здесь.
  let lastField = null;
  let menuAsked = false;

  const rectOf = (field) => {
    const rect = field.getBoundingClientRect();
    return { x: rect.left, y: rect.top, width: rect.width, height: rect.height };
  };

  const askMenu = (field, keyboard) => {
    lastField = field;
    if (post({ evt: "password_menu", rect: rectOf(field), keyboard })) menuAsked = true;
  };

  const closeMenu = () => {
    if (!menuAsked) return;
    menuAsked = false;
    post({ evt: "password_menu_close" });
  };

  // Щелчок по полю — показать список; щелчок мимо — убрать.
  on(
    window,
    "mousedown",
    (event) => {
      if (!event.isTrusted) return;
      const target = event.target;
      if (target instanceof HTMLInputElement && accountField(target)) {
        setTimeout(() => askMenu(target, false), 0);
      } else {
        closeMenu();
      }
    },
    true
  );

  // В поле перешли клавишей Tab из другого поля — тоже показать. Фокус,
  // вернувшийся вместе с окном (список браузера закрыли), — нет: иначе список
  // открывался бы снова, едва его закрыли.
  on(
    document,
    "focusin",
    (event) => {
      if (!event.isTrusted) return;
      const target = event.target;
      if (target === lastField && menuAsked) return;
      if (menuAsked) closeMenu();
      if (event.relatedTarget instanceof Element && target instanceof HTMLInputElement && accountField(target)) {
        askMenu(target, false);
      }
    },
    true
  );

  // Стрелка вниз в поле открывает список с клавиатуры; Escape — убирает.
  on(
    window,
    "keydown",
    (event) => {
      if (!event.isTrusted) return;
      const target = event.target;
      if (!(target instanceof HTMLInputElement) || !accountField(target)) return;
      if (event.key === "ArrowDown" && !event.altKey && !event.ctrlKey && !event.metaKey) askMenu(target, true);
      else if (event.key === "Escape") closeMenu();
    },
    true
  );

  // Человек печатает сам — список больше не нужен.
  on(
    window,
    "input",
    (event) => {
      if (event.isTrusted && event.target === lastField) closeMenu();
    },
    true
  );

  // Поле уехало вместе со страницей — список остался бы висеть в стороне.
  on(window, "scroll", () => closeMenu(), { capture: true, passive: true });
  on(window, "resize", () => closeMenu());

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

  // Учётку браузер присылает только после выбора человеком в своём окне.
  const onMessage = (event) => {
    const data = event.data;
    if (!data || data.cmd !== "password_fill" || data.origin !== location.origin) return;
    menuAsked = false;
    applyFill(data);
  };

  const applyFill = (data) => {
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
