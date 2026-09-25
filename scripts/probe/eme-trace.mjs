// Шаги защищённого видео (EME) во вкладке пробы — по порту отладки.
//
//   node scripts/probe/eme-trace.mjs <подстрока адреса вкладки> [секунды] [--no-reload] [--play]
//
// Встраивает в вкладку трассировку EME: какие ключевые системы спрашивает
// страница и что получает, MediaKeys, сессии, generateRequest (с системами из
// PSSH), запросы лицензии и ответы, статусы ключей, ошибки и ход видео. Сеть —
// запросы, похожие на лицензию: адрес без параметров и статус. Затем
// перезагружает вкладку и пишет всё по порядку.
import { connect, targets } from "./cdp.mjs";

const args = process.argv.slice(2).filter((arg) => !arg.startsWith("--"));
const reload = !process.argv.includes("--no-reload");
const [needle = "", seconds = "60"] = args;

const TRACE = `(() => {
  if (window.__x4eme) return;
  window.__x4eme = true;
  const log = (...parts) => console.debug("[EME]", location.host, ...parts);
  const hex = (bytes) => [...bytes].map((b) => b.toString(16).padStart(2, "0")).join("");
  const SYSTEMS = {
    edef8ba979d64acea3c827dcd51d21ed: "widevine",
    "9a04f07998404286ab92e65be0885f95": "playready",
    "1077efecc0b24d02ace33c1e52e2fb4b": "clearkey",
    "94ce86fb07ff4f43adb893d2fa968ca2": "fairplay",
  };
  const psshSystems = (data) => {
    try {
      const bytes = new Uint8Array(data.buffer ? data.buffer.slice(data.byteOffset, data.byteOffset + data.byteLength) : data);
      const found = [];
      for (let i = 0; i + 28 <= bytes.length; i++) {
        if (bytes[i + 4] === 0x70 && bytes[i + 5] === 0x73 && bytes[i + 6] === 0x73 && bytes[i + 7] === 0x68) {
          const id = hex(bytes.slice(i + 12, i + 28));
          found.push(SYSTEMS[id] || id);
        }
      }
      return found.join(",") || "no pssh (" + bytes.length + " bytes)";
    } catch (e) {
      return "?" + e;
    }
  };
  const brief = (configs) => {
    try {
      return JSON.stringify((configs || []).map((c) => ({
        init: c.initDataTypes,
        video: (c.videoCapabilities || []).map((v) => (v.robustness || "-") + " " + (v.contentType || "").slice(0, 40)),
        audio: (c.audioCapabilities || []).map((a) => a.robustness || "-"),
        distinctive: c.distinctiveIdentifier,
        persistent: c.persistentState,
      })));
    } catch (e) {
      return "?";
    }
  };
  const wrap = (proto, name, apply) => {
    if (!proto || typeof proto[name] !== "function") return;
    proto[name] = new Proxy(proto[name], { apply });
  };
  const names = new WeakMap();
  let n = 0;
  wrap(Navigator.prototype, "requestMediaKeySystemAccess", (target, self, args) => {
    const id = ++n;
    log("request#" + id, args[0], brief(args[1]));
    return Reflect.apply(target, self, args).then(
      (access) => { log("request#" + id, "granted", access.keySystem, brief([access.getConfiguration()])); return access; },
      (error) => { log("request#" + id, "rejected", error && error.name, error && error.message); throw error; }
    );
  });
  wrap(MediaKeySystemAccess.prototype, "createMediaKeys", (target, self, args) =>
    Reflect.apply(target, self, args).then(
      (keys) => { names.set(keys, self.keySystem); log("createMediaKeys", self.keySystem, "ok"); return keys; },
      (error) => { log("createMediaKeys", self.keySystem, "failed", error && error.name, error && error.message); throw error; }
    )
  );
  wrap(HTMLMediaElement.prototype, "setMediaKeys", (target, self, args) => {
    log("setMediaKeys", args[0] ? names.get(args[0]) || "?" : "null");
    return Reflect.apply(target, self, args).then(
      (value) => { log("setMediaKeys ok"); return value; },
      (error) => { log("setMediaKeys failed", error && error.name, error && error.message); throw error; }
    );
  });
  wrap(MediaKeys.prototype, "setServerCertificate", (target, self, args) => {
    log("setServerCertificate", names.get(self), args[0] && args[0].byteLength);
    return Reflect.apply(target, self, args).then(
      (value) => { log("setServerCertificate ->", value); return value; },
      (error) => { log("setServerCertificate failed", error && error.message); throw error; }
    );
  });
  wrap(MediaKeys.prototype, "createSession", (target, self, args) => {
    const session = Reflect.apply(target, self, args);
    const system = names.get(self);
    log("createSession", system, args[0] || "temporary");
    session.addEventListener("message", (e) => log("message", system, e.messageType, e.message.byteLength));
    session.addEventListener("keystatuseschange", () => {
      const statuses = [];
      session.keyStatuses.forEach((status) => statuses.push(status));
      log("keystatuses", system, statuses.join(","));
    });
    session.closed.then((reason) => log("session closed", system, reason));
    return session;
  });
  wrap(MediaKeySession.prototype, "generateRequest", (target, self, args) => {
    log("generateRequest", args[0], psshSystems(args[1]));
    return Reflect.apply(target, self, args).then(
      (value) => { log("generateRequest ok"); return value; },
      (error) => { log("generateRequest failed", error && error.name, error && error.message); throw error; }
    );
  });
  wrap(MediaKeySession.prototype, "update", (target, self, args) => {
    log("update", args[0] && args[0].byteLength);
    return Reflect.apply(target, self, args).then(
      (value) => { log("update ok"); return value; },
      (error) => { log("update failed", error && error.name, error && error.message); throw error; }
    );
  });
  document.addEventListener("encrypted", (e) => log("encrypted", e.initDataType, psshSystems(e.initData)), true);
  document.addEventListener("error", (e) => {
    const m = e.target;
    if (m instanceof HTMLMediaElement && m.error) log("media error", m.error.code, m.error.message);
  }, true);
  for (const type of ["playing", "waiting", "stalled"]) {
    document.addEventListener(type, (e) => {
      if (e.target instanceof HTMLMediaElement) log(type, e.target.currentTime.toFixed(1), "rs" + e.target.readyState);
    }, true);
  }
})();`;

const LICENSE = /drm|licen[cs]e|widevine|playready|wv|cenc|keyserver|acquire/i;

const all = await targets();
const tab = all.find((target) => target.type === "page" && target.url.includes(needle) && !target.url.includes("tauri.localhost"));
if (!tab) {
  console.error(`вкладки с «${needle}» нет:`, all.map((target) => target.url).join("\n  "));
  process.exit(1);
}
console.log(`вкладка: ${tab.url.split("?")[0]}`);
const cdp = await connect(tab);
const on = (method, handler) =>
  cdp.ws.addEventListener("message", (message) => {
    const data = JSON.parse(message.data);
    if (data.method === method) handler(data.params);
  });
const start = Date.now();
const stamp = () => `${((Date.now() - start) / 1000).toFixed(1).padStart(6)}s`;

on("Runtime.consoleAPICalled", ({ args }) => {
  const text = args.map((arg) => arg.value ?? arg.description ?? "").join(" ");
  if (text.startsWith("[EME]")) console.log(stamp(), text.slice(6));
});
const requests = new Map();
on("Network.requestWillBeSent", ({ requestId, request, type }) => {
  const url = request.url.split("?")[0];
  if (!LICENSE.test(url) || type === "Image" || type === "Script" || type === "Stylesheet") return;
  requests.set(requestId, url);
  console.log(stamp(), "net →", request.method, url);
});
on("Network.responseReceived", ({ requestId, response }) => {
  const url = requests.get(requestId);
  if (url) console.log(stamp(), "net ←", response.status, url, response.headers["content-type"] || "");
});
on("Network.loadingFailed", ({ requestId, errorText, blockedReason }) => {
  const url = requests.get(requestId);
  if (url) console.log(stamp(), "net ✗", errorText, blockedReason || "", url);
});

await cdp.send("Runtime.enable");
await cdp.send("Network.enable");
await cdp.send("Page.enable");
// --hide=playready|widevine: страница не видит эту ключевую систему, как в браузере без неё.
const hide = process.argv.find((arg) => arg.startsWith("--hide="))?.slice(7);
if (hide) {
  await cdp.send("Page.addScriptToEvaluateOnNewDocument", {
    source: `(() => {
      const hidden = (system) => String(system).toLowerCase().includes(${JSON.stringify(hide)});
      const request = Navigator.prototype.requestMediaKeySystemAccess;
      Navigator.prototype.requestMediaKeySystemAccess = new Proxy(request, { apply: (t, self, args) => hidden(args[0])
        ? Promise.reject(new DOMException("Unsupported keySystem or supportedConfigurations.", "NotSupportedError"))
        : Reflect.apply(t, self, args) });
      const info = MediaCapabilities.prototype.decodingInfo;
      MediaCapabilities.prototype.decodingInfo = new Proxy(info, { apply: (t, self, args) => {
        const system = args[0] && args[0].keySystemConfiguration && args[0].keySystemConfiguration.keySystem;
        return system && hidden(system) ? Promise.resolve({ supported: false, smooth: false, powerEfficient: false, keySystemAccess: null }) : Reflect.apply(t, self, args);
      } });
    })();`,
  });
  console.log("спрятано:", hide);
}
await cdp.send("Page.addScriptToEvaluateOnNewDocument", { source: TRACE });
console.log("userAgent:", await cdp.evaluate("navigator.userAgent"));
if (reload) await cdp.send("Page.reload", {});
else await cdp.evaluate(TRACE);

// --play: через 5 секунд запустить видео «щелчком» — у браузера звук только после жеста.
if (process.argv.includes("--play")) {
  await new Promise((resolve) => setTimeout(resolve, 5000));
  const started = await cdp.evaluate(
    `Promise.race([Promise.all([...document.querySelectorAll("video")].map((m) => m.play().then(() => "ok", (e) => e.name))).then((r) => r.join(",")), new Promise((r) => setTimeout(() => r("ещё ждёт"), 3000))])`,
    { gesture: true }
  );
  console.log(stamp(), "play():", started);
}
await new Promise((resolve) => setTimeout(resolve, Number(seconds) * 1000));
const media = await cdp.evaluate(
  `JSON.stringify([...document.querySelectorAll("video,audio")].map((m) => ({ t: m.currentTime, paused: m.paused, rs: m.readyState, keys: Boolean(m.mediaKeys), err: m.error && m.error.code })))`
);
console.log(stamp(), "медиа на странице:", media);
cdp.close();
