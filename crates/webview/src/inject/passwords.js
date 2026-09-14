// Менеджер паролей 190x4: встраивается в каждую страницу до её собственных
// скриптов (AddScriptToExecuteOnDocumentCreated).
//
// Скрипт ничего не хранит и ничего не решает. Он сообщает браузеру две вещи:
// «на странице есть форма входа» и «пользователь отправил логин с паролем» —
// а сохраняет, спрашивает и заполняет уже Rust. Происхождение сообщения Rust
// берёт из движка (адрес документа), а не из текста сообщения: страница может
// прислать что угодно, но только от своего имени.
(() => {
  "use strict";
  if (window.top !== window) return;

  // Скрипт ставится двумя путями — при создании документа и повторно после
  // DOMContentLoaded (см. tab.rs). Второй раз он не нужен. Метка скрытая, а
  // заодно хранит этап установки и ошибку, если она случилась.
  const MARK = "__190x4Passwords";
  if (window[MARK]) return;
  const status = { stage: "start", asked: false, posts: 0, error: "" };
  try {
    Object.defineProperty(window, MARK, { value: status, enumerable: false });
  } catch (_) {
    return;
  }

  try {

  // Мост WebView2 ищем при каждом обращении, а не один раз при встраивании:
  // скрипт запускается на создании документа, и мост к этому мигу может ещё
  // не появиться. Ранний выход здесь молча выключал весь менеджер паролей.
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

  const passwordsIn = (scope) =>
    Array.from((scope || document).querySelectorAll('input[type="password"]')).filter(usable);

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

  let pending = "";
  let lastSubmit = null;
  let watch = 0;

  const capture = (scope) => {
    const filled = passwordsIn(scope).filter((input) => input.value);
    if (!filled.length) return;
    // На формах смены пароля их два-три: сохранять нужно последний, новый.
    const password = filled[filled.length - 1];
    const user = usernameFor(filled[0], scope);
    const username = user ? user.value.trim() : "";
    const key = username + "\u0000" + password.value;
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

  document.addEventListener("submit", (event) => capture(event.target), true);

  // Отправка формы сразу уводит страницу. Сообщение, ушедшее за миг до этого,
  // может не дойти — повторяем его, когда документ уже уходит.
  window.addEventListener("pagehide", () => {
    if (lastSubmit) post(lastSubmit);
  });

  document.addEventListener(
    "click",
    (event) => {
      const target = event.target instanceof Element ? event.target : null;
      const button = target && target.closest('button, input[type="submit"], [role="button"]');
      if (button) capture(button.form || scopeFor(button));
    },
    true
  );

  document.addEventListener(
    "keydown",
    (event) => {
      if (event.key === "Enter" && event.target instanceof HTMLInputElement) {
        capture(scopeFor(event.target));
      }
    },
    true
  );

  // Форма входа: спрашиваем один раз на документ. Одностраничные сайты
  // рисуют её после загрузки, поэтому ещё несколько секунд следим за DOM.
  let asked = false;
  const ask = () => {
    if (asked || !passwordsIn(document).length) return false;
    asked = post({ evt: "password_form" });
    status.asked = asked;
    return asked;
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
    setTimeout(() => observer.disconnect(), 15000);
  };

  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", start, { once: true });
  } else {
    start();
  }

  // React и компания держат значение поля у себя: простое присваивание они
  // перетрут на следующем рендере. Нативный сеттер плюс событие input — как
  // при вводе с клавиатуры.
  const setValue = (input, value) => {
    if (!input || typeof value !== "string") return;
    const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value").set;
    setter.call(input, value);
    input.dispatchEvent(new Event("input", { bubbles: true }));
    input.dispatchEvent(new Event("change", { bubbles: true }));
  };

  const onMessage = (event) => {
    const data = event.data;
    if (!data || data.cmd !== "password_fill" || data.origin !== location.origin) return;
    const password = passwordsIn(document)[0];
    if (!password) return;
    setValue(usernameFor(password, scopeFor(password)), data.username);
    setValue(password, data.password);
  };

  const subscribe = () => {
    const hook = bridge();
    if (!hook) return false;
    hook.addEventListener("message", onMessage);
    return true;
  };
  if (!subscribe()) document.addEventListener("DOMContentLoaded", subscribe, { once: true });
    status.stage = "ready";
  } catch (error) {
    status.stage = "error";
    status.error = String((error && error.stack) || error).slice(0, 300);
  }
})();
