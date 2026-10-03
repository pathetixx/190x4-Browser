// YouTube NonStop 190x4: встраивается в каждый документ, но работает только на
// YouTube и YouTube Music.
//
// Если долго ничего не нажимать, YouTube ставит видео на паузу и спрашивает
// «Видео приостановлено. Продолжить просмотр?», а YouTube Music — «Вы ещё
// здесь?». Вопрос появляется, когда YouTube считает, что человек давно ничего
// не нажимал (время последнего действия — `window._lact`). Включено расширение
// (src-tauri/src/nonstop.rs) — скрипт держит это время свежим, и вопроса нет
// даже в фоновой вкладке, где YouTube не рисует окно, пока вкладку не
// откроют, а видео уже стоит. Если окно всё же появилось, оно закрывается, а
// видео играет дальше. Паузу, которую человек поставил сам, скрипт не трогает.
(() => {
  "use strict";

  const host = location.hostname;
  const MUSIC = host === "music.youtube.com";
  if (!(MUSIC || /(^|\.)youtube\.com$/.test(host)) || window.top !== window) return;
  const MARK = Symbol.for("x4nonstop");
  if (window[MARK]) return;
  try {
    Object.defineProperty(window, MARK, { value: true, enumerable: false });
  } catch (_) {
    return;
  }

  const DIALOG = MUSIC ? "YTMUSIC-YOU-THERE-RENDERER" : "YT-CONFIRM-DIALOG-RENDERER";
  const CONTAINER = MUSIC ? "ytmusic-popup-container" : "ytd-popup-container";
  /** Столько без щелчков и клавиш — и окно спросил YouTube, а не человек своим действием. */
  const IDLE_MS = 5000;
  /** Как часто освежать время последнего действия: YouTube спрашивает после десятков минут. */
  const KEEP_ALIVE_MS = 60_000;

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

  let lastInput = Date.now();
  for (const type of ["mousedown", "keydown", "touchstart", "wheel"]) {
    addEventListener(
      type,
      (event) => {
        if (event.isTrusted) lastInput = Date.now();
      },
      { capture: true, passive: true }
    );
  }

  /** Окно «Продолжить просмотр?», которое сейчас на экране. */
  let pending = null;

  document.addEventListener("yt-popup-opened", (event) => {
    const popup = event.detail;
    if (!popup || popup.nodeName !== DIALOG || Date.now() - lastInput < IDLE_MS) return;
    pending = popup;
    post({ evt: "nonstop_ask" });
  });

  const resume = () => {
    const popup = pending;
    pending = null;
    if (!popup || !popup.isConnected) return;
    const container = document.querySelector(CONTAINER);
    if (container && typeof container.handleClosePopupAction_ === "function") {
      container.handleClosePopupAction_();
    } else {
      const button = popup.querySelector("#confirm-button button, #confirm-button, button");
      if (button) button.click();
    }
    const video = document.querySelector("video.html5-main-video, video");
    if (video && video.paused) video.play().catch(() => {});
  };

  // Время последнего действия держится свежим, только пока играет видео:
  // поставленная на паузу вкладка живёт по правилам YouTube.
  let keepAlive = 0;
  const touch = () => {
    const video = document.querySelector("video.html5-main-video, video");
    if (video && !video.paused) window._lact = Date.now();
  };

  const onMessage = (event) => {
    const data = event.data;
    if (!data || data.origin !== location.origin) return;
    if (data.cmd === "nonstop_continue") resume();
    if (data.cmd === "nonstop_on" && !keepAlive) {
      touch();
      keepAlive = setInterval(touch, KEEP_ALIVE_MS);
    }
  };
  const subscribe = () => {
    const hook = bridge();
    if (!hook) return false;
    hook.addEventListener("message", onMessage);
    // Включено ли расширение, решает браузер: ответом будет nonstop_on.
    post({ evt: "nonstop_hello" });
    return true;
  };
  if (!subscribe()) document.addEventListener("DOMContentLoaded", subscribe, { once: true });
})();
