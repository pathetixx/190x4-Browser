// Защищённое видео 190x4 (EME): встраивается в каждый документ и фрейм.
//
// Первое — Widevine. Выключенный в настройках или для этого сайта, он для
// страницы как будто не существует: запрос ключевой системы Widevine
// отклоняется так же, как в браузере без неё, и сайт, который умеет другое,
// берёт PlayReady. Какие сайты без Widevine, браузер подставляет в CONFIG.
//
// Второе — заметить, что защищённое видео не пошло: модуль защиты не
// запустился, лицензию не дали, ключи не подошли или видео так и стоит на
// нуле. Об этом скрипт один раз за документ сообщает браузеру, а решает
// браузер (src-tauri/src/drm.rs): переключить сайт на PlayReady или сказать,
// что случилось. Сам скрипт ничего не чинит и видео не трогает.
(() => {
  "use strict";

  const CONFIG = __X4_DRM__;
  if (!window.isSecureContext || typeof Navigator === "undefined" || !Navigator.prototype.requestMediaKeySystemAccess) return;
  const MARK = Symbol.for("x4drm");
  if (window[MARK]) return;
  try {
    Object.defineProperty(window, MARK, { value: true, enumerable: false });
  } catch (_) {
    return;
  }

  /** Сайт вкладки. Плеер часто живёт во фрейме чужого домена — важен верхний документ. */
  const topHost = (() => {
    try {
      const origins = location.ancestorOrigins;
      return new URL(origins && origins.length ? origins[origins.length - 1] : location.origin).hostname;
    } catch (_) {
      return location.hostname;
    }
  })();
  const onSite = (site) => topHost === site || topHost.endsWith(`.${site}`);
  const hidden = !CONFIG.widevine || CONFIG.sites.some(onSite);

  const isWidevine = (system) => /^com\.widevine\./i.test(String(system));
  const nameOf = (system) => {
    const text = String(system);
    if (isWidevine(text)) return "widevine";
    if (/playready/i.test(text)) return "playready";
    if (/clearkey/i.test(text)) return "clearkey";
    return "other";
  };
  const errorText = (error) => String((error && (error.message || error.name)) || error || "").slice(0, 160);

  const post = (message) => {
    try {
      if (window.chrome && window.chrome.webview) window.chrome.webview.postMessage(message);
    } catch (_) {
      // Мост закрыт — документ уходит.
    }
  };

  /** Прототип встроенного класса — или null, если его здесь нет. */
  const proto = (name) => (typeof window[name] === "function" ? window[name].prototype : null);

  /** Обёртка метода, которую страница не отличит от родного (`toString` тот же). */
  const wrap = (target, method, apply) => {
    const original = target && target[method];
    if (typeof original !== "function") return;
    try {
      target[method] = new Proxy(original, { apply });
    } catch (_) {
      // Прототип заморожен — остаёмся без обёртки.
    }
  };

  /* ── Widevine для сайта ───────────────────────────────────── */

  if (hidden) {
    wrap(Navigator.prototype, "requestMediaKeySystemAccess", (target, self, args) =>
      isWidevine(args[0])
        ? Promise.reject(new DOMException("Unsupported keySystem or supportedConfigurations.", "NotSupportedError"))
        : Reflect.apply(target, self, args)
    );
    // Shaka Player и другие спрашивают о ключевой системе и здесь.
    wrap(proto("MediaCapabilities"), "decodingInfo", (target, self, args) => {
      const config = args[0];
      const system = config && config.keySystemConfiguration && config.keySystemConfiguration.keySystem;
      return isWidevine(system)
        ? Promise.resolve({ supported: false, smooth: false, powerEfficient: false, keySystemAccess: null })
        : Reflect.apply(target, self, args);
    });
  }

  /* ── Пошло ли видео ───────────────────────────────────────── */

  const seen = { system: "", sessions: 0, requests: 0, answers: 0, usable: false, media: null, reported: false };

  const report = (stage, detail = "") => {
    if (seen.reported) return;
    seen.reported = true;
    post({ evt: "drm_problem", system: seen.system || "none", stage, detail: String(detail).slice(0, 160), hidden });
  };

  wrap(proto("MediaKeySystemAccess"), "createMediaKeys", (target, self, args) => {
    const system = nameOf(self.keySystem);
    return Reflect.apply(target, self, args).then(
      (keys) => {
        seen.system = system;
        return keys;
      },
      (error) => {
        seen.system = system;
        report("cdm", errorText(error));
        throw error;
      }
    );
  });

  wrap(proto("MediaKeys"), "createSession", (target, self, args) => {
    const session = Reflect.apply(target, self, args);
    seen.sessions += 1;
    session.addEventListener("message", () => {
      seen.requests += 1;
    });
    session.addEventListener("keystatuseschange", () => {
      session.keyStatuses.forEach((status) => {
        if (status === "usable" || status === "output-downscaled") seen.usable = true;
        else if (status === "output-restricted" || status === "internal-error") report("keys", status);
      });
    });
    return session;
  });

  wrap(proto("MediaKeySession"), "update", (target, self, args) => {
    seen.answers += 1;
    return Reflect.apply(target, self, args).catch((error) => {
      report("license", errorText(error));
      throw error;
    });
  });

  /**
   * Через 15 секунд после первого зашифрованного фрагмента видео должно идти.
   * Стоит на нуле, хотя его не ставили на паузу, — смотрим, на каком шаге
   * застряло: у плеера нет сессии, сайт не ответил на запрос лицензии, ключи
   * не пришли или всё есть, а кадры не идут.
   */
  const check = () => {
    const media = seen.media;
    if (seen.reported || !media || !media.isConnected || media.currentTime > 0.5) return;
    if (media.paused) {
      media.addEventListener("play", () => setTimeout(check, 15000), { once: true });
      return;
    }
    if (!seen.system) report("no-keys-system");
    else if (!seen.sessions || !seen.requests) report("no-request");
    else if (!seen.answers) report("no-license");
    else if (!seen.usable) report("no-keys");
    else report("stalled", `readyState ${media.readyState}`);
  };

  // События медиа не всплывают, но проходят через document при захвате.
  document.addEventListener(
    "encrypted",
    (event) => {
      if (seen.media || !(event.target instanceof HTMLMediaElement)) return;
      seen.media = event.target;
      setTimeout(check, 15000);
    },
    true
  );
  document.addEventListener(
    "error",
    (event) => {
      const media = event.target;
      if (!(media instanceof HTMLMediaElement) || media !== seen.media || !media.error) return;
      report("media", `${media.error.code} ${media.error.message || ""}`.trim());
    },
    true
  );
})();
