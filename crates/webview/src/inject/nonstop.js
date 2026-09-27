// YouTube NonStop 190x4: встраивается в каждый документ, но работает только на
// YouTube и YouTube Music.
//
// Если долго ничего не нажимать, YouTube ставит видео на паузу и спрашивает
// «Видео приостановлено. Продолжить просмотр?», а YouTube Music — «Вы ещё
// здесь?». Скрипт замечает это окно и спрашивает браузер; включено расширение
// (src-tauri/src/nonstop.rs) — окно закрывается, а видео играет дальше. Паузу,
// которую человек поставил сам, скрипт не трогает.
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

  const onMessage = (event) => {
    const data = event.data;
    if (data && data.cmd === "nonstop_continue" && data.origin === location.origin) resume();
  };
  const subscribe = () => {
    const hook = bridge();
    if (!hook) return false;
    hook.addEventListener("message", onMessage);
    return true;
  };
  if (!subscribe()) document.addEventListener("DOMContentLoaded", subscribe, { once: true });
})();
