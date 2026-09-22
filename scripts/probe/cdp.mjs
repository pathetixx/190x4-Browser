// Мини-клиент CDP для живой проверки 190x4 Browser через проброшенный порт.
export const BASE = process.env.CDP ?? "http://127.0.0.1:19333";
export const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

export async function targets() {
  const res = await fetch(`${BASE}/json`);
  return res.json();
}

export async function waitTarget(pred, timeout = 15000) {
  const until = Date.now() + timeout;
  while (Date.now() < until) {
    try {
      const list = await targets();
      const found = list.find(pred);
      if (found) return found;
    } catch {}
    await sleep(300);
  }
  throw new Error("target not found");
}

export async function connect(target) {
  const ws = new WebSocket(target.webSocketDebuggerUrl);
  await new Promise((resolve, reject) => {
    ws.onopen = resolve;
    ws.onerror = reject;
  });
  let seq = 0;
  const pending = new Map();
  ws.onmessage = (msg) => {
    const data = JSON.parse(msg.data);
    if (data.id && pending.has(data.id)) {
      pending.get(data.id)(data);
      pending.delete(data.id);
    }
  };
  const send = (method, params = {}) =>
    new Promise((resolve) => {
      const id = ++seq;
      pending.set(id, resolve);
      ws.send(JSON.stringify({ id, method, params }));
    });
  const evaluate = async (expression, { gesture = false } = {}) => {
    const r = await send("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true, userGesture: gesture });
    if (r.error) throw new Error(r.error.message);
    if (r.result?.exceptionDetails) throw new Error(r.result.exceptionDetails.exception?.description ?? r.result.exceptionDetails.text);
    return r.result?.result?.value;
  };
  return { ws, send, evaluate, close: () => ws.close() };
}

/** Окна браузера (интерфейс), без попапов. */
export async function chromeWindows() {
  return (await targets()).filter((t) => t.type === "page" && /^http:\/\/tauri\.localhost\/(index\.html)?$/.test(t.url));
}

let failures = 0;
export function check(ok, label, extra = "") {
  console.log(`${ok ? "OK  " : "FAIL"} ${label}${extra ? ` — ${extra}` : ""}`);
  if (!ok) failures++;
}
export function summary() {
  console.log(failures ? `\n${failures} FAIL` : "\nALL OK");
  return failures;
}
