// Защищённое видео 190x4 (EME): встраивается в каждый документ и фрейм.
//
// Первое — какой модуль защиты видит сайт. Как в Chrome, по умолчанию только
// Widevine: PlayReady во встроенном движке работает хуже (видео стоит, идёт в
// худшем качестве или с артефактами), а сайт, которому доступны оба, нередко
// выбирает его. PlayReady сайт видит, только если Widevine выключен в
// настройках или для этого сайта — или если в настройках разрешено показывать
// оба. Спрятанная система для страницы как будто не существует: запрос её
// отклоняется так же, как в браузере без неё. Настройки браузер подставляет в
// CONFIG.
//
// Второе — заметить, что защищённое видео не пошло: модуль защиты не
// запустился, лицензию не дали, ключи не подошли или видео так и стоит на
// нуле. Об этом скрипт один раз за документ сообщает браузеру, а решает
// браузер (src-tauri/src/drm.rs): переключить сайт на другой модуль или
// сказать, что случилось. Сам скрипт ничего не чинит и видео не трогает.
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
  const playreadyHidden = !hidden && !CONFIG.playready;

  const isWidevine = (system) => /^com\.widevine\./i.test(String(system));
  const isPlayready = (system) => /playready/i.test(String(system));
  const isHidden = (system) => (hidden && isWidevine(system)) || (playreadyHidden && isPlayready(system));
  const nameOf = (system) => {
    const text = String(system);
    if (isWidevine(text)) return "widevine";
    if (isPlayready(text)) return "playready";
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

  /* ── Какой модуль видит сайт ──────────────────────────────── */

  /** Какие ключевые системы спрашивала страница — для сообщения о проблеме. */
  const asked = [];
  wrap(Navigator.prototype, "requestMediaKeySystemAccess", (target, self, args) => {
    const name = nameOf(args[0]);
    if (!asked.includes(name) && asked.length < 6) asked.push(name);
    return isHidden(args[0])
      ? Promise.reject(new DOMException("Unsupported keySystem or supportedConfigurations.", "NotSupportedError"))
      : Reflect.apply(target, self, args);
  });
  // Shaka Player и другие спрашивают о ключевой системе и здесь.
  wrap(proto("MediaCapabilities"), "decodingInfo", (target, self, args) => {
    const config = args[0];
    const system = config && config.keySystemConfiguration && config.keySystemConfiguration.keySystem;
    return system && isHidden(system)
      ? Promise.resolve({ supported: false, smooth: false, powerEfficient: false, keySystemAccess: null })
      : Reflect.apply(target, self, args);
  });

  /* ── Пошло ли видео ───────────────────────────────────────── */

  // Плеер при запуске нередко создаёт MediaKeys для нескольких систем, чтобы
  // проверить, какие есть. Играет та, чьи ключи подключены к видео
  // (`setMediaKeys`), — по ней и судим; счётчики сессий — по системам.
  const seen = { system: "", attached: "", pssh: "", media: null, reported: false };
  const systemOf = new WeakMap();
  const countOfSession = new WeakMap();
  const counts = new Map();
  const countsOf = (system) => {
    if (!counts.has(system)) counts.set(system, { sessions: 0, requests: 0, answers: 0, usable: false });
    return counts.get(system);
  };
  const current = () => seen.attached || seen.system;

  const report = (stage, detail = "") => {
    if (seen.reported) return;
    seen.reported = true;
    const context = `спрашивал ${asked.join("+") || "—"}${seen.pssh ? `, в видео ${seen.pssh}` : ""}`;
    post({
      evt: "drm_problem",
      system: current() || "none",
      stage,
      detail: (detail ? `${detail}; ${context}` : context).slice(0, 160),
      hidden,
      asked,
    });
  };

  /** Системы защиты, для которых в данных инициализации есть PSSH. */
  const PSSH = { edef8ba979d64acea3c827dcd51d21ed: "widevine", "9a04f07998404286ab92e65be0885f95": "playready" };
  const psshOf = (data) => {
    try {
      const bytes = ArrayBuffer.isView(data) ? new Uint8Array(data.buffer, data.byteOffset, data.byteLength) : new Uint8Array(data);
      const found = [];
      for (let i = 0; i + 28 <= bytes.length; i++) {
        if (bytes[i + 4] !== 0x70 || bytes[i + 5] !== 0x73 || bytes[i + 6] !== 0x73 || bytes[i + 7] !== 0x68) continue;
        let id = "";
        for (let j = i + 12; j < i + 28; j++) id += bytes[j].toString(16).padStart(2, "0");
        const name = PSSH[id] || "other";
        if (!found.includes(name)) found.push(name);
      }
      return found.join("+");
    } catch (_) {
      return "";
    }
  };

  wrap(proto("MediaKeySystemAccess"), "createMediaKeys", (target, self, args) => {
    const system = nameOf(self.keySystem);
    return Reflect.apply(target, self, args).then(
      (keys) => {
        systemOf.set(keys, system);
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

  wrap(proto("HTMLMediaElement"), "setMediaKeys", (target, self, args) => {
    const system = args[0] && systemOf.get(args[0]);
    if (system) seen.attached = system;
    return Reflect.apply(target, self, args);
  });

  wrap(proto("MediaKeys"), "createSession", (target, self, args) => {
    const session = Reflect.apply(target, self, args);
    const count = countsOf(systemOf.get(self) || "other");
    count.sessions += 1;
    countOfSession.set(session, count);
    session.addEventListener("message", () => {
      count.requests += 1;
    });
    session.addEventListener("keystatuseschange", () => {
      session.keyStatuses.forEach((status) => {
        if (status === "usable" || status === "output-downscaled") count.usable = true;
        else if (status === "output-restricted" || status === "internal-error") report("keys", status);
      });
    });
    return session;
  });

  wrap(proto("MediaKeySession"), "generateRequest", (target, self, args) => {
    const pssh = psshOf(args[1]);
    if (pssh) seen.pssh = pssh;
    return Reflect.apply(target, self, args).catch((error) => {
      report("request", `${args[0]}: ${errorText(error)}`);
      throw error;
    });
  });

  wrap(proto("MediaKeySession"), "update", (target, self, args) => {
    const count = countOfSession.get(self);
    if (count) count.answers += 1;
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
    const count = countsOf(current() || "none");
    if (!current()) report("no-keys-system");
    else if (!count.sessions || !count.requests) report("no-request");
    else if (!count.answers) report("no-license");
    else if (!count.usable) report("no-keys");
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
