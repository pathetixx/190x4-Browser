/**
 * Список загрузок: общий для кнопки на панели инструментов, пузыря загрузок
 * и страницы 190x4://downloads.
 *
 * Основа — записи из базы; поверх них живые события `download` двигают
 * прогресс без походов в SQLite. Скорость считается здесь же, по разнице
 * байтов между событиями, со сглаживанием — иначе цифра скачет.
 */

import { invoke, listen } from "./bridge.js";
import { fileName, formatBytes, formatDuration, hostOf } from "./dom.js";

const items = new Map();
const rates = new Map();
const listeners = new Set();
let started = false;

export function initDownloads() {
  if (!started) {
    started = true;
    listen("download", onEvent);
    listen("downloads", () => loadDownloads());
  }
  return loadDownloads();
}

export async function loadDownloads() {
  const list = await invoke("downloads_list", { limit: 300 }).catch(() => []);
  const live = new Map(items);
  items.clear();
  for (const item of list) {
    // Запись в базе отстаёт от событий на секунду — свежий прогресс не теряем.
    const known = live.get(item.id);
    items.set(item.id, known && known.state === item.state ? { ...item, bytes: Math.max(item.bytes, known.bytes) } : item);
  }
  notify(null);
}

const PHASE_STATE = {
  started: "running",
  progress: "running",
  paused: "paused",
  done: "done",
  failed: "failed",
  cancelled: "cancelled",
};

function onEvent(event) {
  const current = items.get(event.id) ?? {
    id: event.id,
    kind: event.kind,
    url: event.url,
    path: event.path,
    state: "running",
    bytes: 0,
    total_bytes: null,
    started_at: Math.floor(Date.now() / 1000),
    error: "",
  };
  const next = {
    ...current,
    url: event.url || current.url,
    path: event.path || current.path,
    bytes: event.bytes || current.bytes,
    total_bytes: event.total ?? current.total_bytes,
    state: PHASE_STATE[event.phase] ?? current.state,
    error: event.error ?? "",
  };

  if (next.state === "running") {
    const now = performance.now();
    const previous = rates.get(event.id);
    if (!previous) {
      rates.set(event.id, { bytes: next.bytes, at: now, rate: 0 });
    } else if (now - previous.at >= 500) {
      const instant = ((next.bytes - previous.bytes) * 1000) / (now - previous.at);
      const rate = previous.rate ? previous.rate * 0.65 + instant * 0.35 : instant;
      rates.set(event.id, { bytes: next.bytes, at: now, rate: Math.max(0, rate) });
    }
  } else {
    rates.delete(event.id);
  }

  items.set(event.id, next);
  notify(event);
}

export function onDownloads(fn) {
  listeners.add(fn);
  return () => listeners.delete(fn);
}

function notify(event) {
  for (const fn of listeners) fn(event);
}

export function downloads() {
  return [...items.values()].sort((a, b) => b.started_at - a.started_at || b.id - a.id);
}

export function isActive(item) {
  return item.state === "running" || item.state === "paused";
}

/** Сводка для кнопки на панели: сколько идёт и общий прогресс. */
export function summary() {
  let active = 0;
  let bytes = 0;
  let total = 0;
  let unknown = false;
  for (const item of items.values()) {
    if (!isActive(item)) continue;
    active += 1;
    if (item.total_bytes) {
      bytes += item.bytes;
      total += item.total_bytes;
    } else {
      unknown = true;
    }
  }
  return { active, progress: total ? bytes / total : unknown ? null : 0, count: items.size };
}

export function displayName(item) {
  const name = fileName(item.path);
  if (name) return name;
  return item.kind === "media" ? "Видео — готовится на сервере" : hostOf(item.url) || "Файл";
}

const ERRORS = {
  network: "нет связи с сервером",
  no_space: "на диске закончилось место",
  access_denied: "нет доступа к папке",
  blocked: "файл заблокирован системой безопасности",
  server: "сервер отказал в загрузке",
  shutdown: "браузер был закрыт",
  failed: "не удалось скачать",
};

export function errorText(code) {
  if (!code) return ERRORS.failed;
  return ERRORS[code] ?? code;
}

/** Строка состояния под именем файла — как в списке загрузок Chrome. */
export function statusText(item) {
  const total = item.total_bytes;
  switch (item.state) {
    case "running": {
      const rate = rates.get(item.id)?.rate ?? 0;
      const parts = [total ? `${formatBytes(item.bytes)} из ${formatBytes(total)}` : formatBytes(item.bytes)];
      if (rate > 0) parts.push(`${formatBytes(rate)}/с`);
      if (rate > 0 && total) {
        const left = formatDuration((total - item.bytes) / rate);
        if (left) parts.push(`осталось ${left}`);
      }
      if (item.kind === "media" && !item.bytes) return "Сервер готовит файл…";
      return parts.join(" · ");
    }
    case "paused":
      return `Приостановлено · ${total ? `${formatBytes(item.bytes)} из ${formatBytes(total)}` : formatBytes(item.bytes)}`;
    case "done": {
      const size = total || item.bytes;
      return [size ? formatBytes(size) : "", hostOf(item.url)].filter(Boolean).join(" · ");
    }
    case "cancelled":
      return "Отменено";
    default:
      return `Ошибка: ${errorText(item.error)}`;
  }
}

export function progressOf(item) {
  return item.total_bytes ? Math.min(1, item.bytes / item.total_bytes) : null;
}

export function control(id, action) {
  return invoke("download_control", { id, action });
}
