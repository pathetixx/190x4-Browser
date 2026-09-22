/**
 * Мелкие помощники DOM и форматирования. Общие для окна браузера,
 * встроенных страниц и всплывающего окна.
 */

export const SPRITE = "./assets/icons.svg";

export function el(tag, className, text) {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text != null) node.textContent = text;
  return node;
}

/** Иконка из спрайта Fluent. `id` — без префикса: "back", "star-16". */
export function icon(id, size = 16, className = "") {
  const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  svg.setAttribute("width", size);
  svg.setAttribute("height", size);
  svg.setAttribute("aria-hidden", "true");
  if (className) svg.setAttribute("class", className);
  const use = document.createElementNS("http://www.w3.org/2000/svg", "use");
  use.setAttribute("href", `${SPRITE}#i-${id}`);
  svg.append(use);
  return svg;
}

/** Кнопка-иконка с подсказкой. */
export function iconButton(id, title, onClick, { size = 16, className = "btn btn--ghost btn--icon btn--sm" } = {}) {
  const button = el("button", className);
  button.type = "button";
  button.title = title;
  button.setAttribute("aria-label", title);
  button.append(icon(id, size));
  if (onClick) {
    button.addEventListener("click", (event) => {
      event.stopPropagation();
      onClick(event);
    });
  }
  return button;
}

export function textButton(label, onClick, className = "btn") {
  const button = el("button", className, label);
  button.type = "button";
  if (onClick) button.addEventListener("click", onClick);
  return button;
}

/**
 * Иконка сайта. Если картинка не загрузилась (сайт без favicon, http на
 * https-странице), на её месте остаётся глобус — пустое место в списке
 * выглядит как поломка.
 */
export function favicon(src, className = "favicon") {
  if (!src || !/^(https:|data:image\/)/.test(src)) return icon("globe-16", 16, className);
  const img = el("img", className);
  img.alt = "";
  img.decoding = "async";
  img.referrerPolicy = "no-referrer";
  img.src = src;
  img.addEventListener("error", () => img.replaceWith(icon("globe-16", 16, className)), { once: true });
  return img;
}

/**
 * Значок сайта для списков браузера (пароли, история).
 *
 * Берётся из кэша профиля, а не с самого сайта: иначе открытый список
 * паролей означал бы запрос на каждый сайт, где у вас есть пароль, — и по
 * этим запросам виден весь список. Пока значка нет, стоит глобус.
 */
export function siteIcon(url, className = "favicon") {
  const node = icon("globe-16", 16, className);
  const holder = el("span", null);
  holder.append(node);
  import("./bridge.js")
    .then(({ invoke }) => invoke("site_icon", { url }))
    .then((data) => {
      if (!data) return;
      holder.replaceChildren(favicon(data, className));
    })
    .catch(() => {});
  return holder;
}

export function hostOf(url) {
  try {
    return new URL(url).host;
  } catch {
    return "";
  }
}

/* ── Адрес для глаз ────────────────────────────────────────── */

/**
 * Имя сайта так, как его пишут люди: `xn--e1afmkfd.xn--p1ai` → `пример.рф`.
 * Часть имени показывается буквами, только если она целиком на одном алфавите:
 * «аpple» с кириллической «а» остаётся в виде xn--, иначе поддельный адрес
 * не отличить от настоящего.
 */
export function displayHost(host) {
  const labels = String(host ?? "").split(".");
  const tld = labels[labels.length - 1]?.toLowerCase() ?? "";
  // Кириллица, неотличимая от латиницы («аррӏе.com»), честна только в
  // кириллических зонах — как у Chrome.
  const cyrillicZone = /^xn--/.test(tld) || CYRILLIC_ZONES.has(tld);
  return labels
    .map((label) => {
      if (!/^xn--/i.test(label)) return label;
      const decoded = punycodeDecode(label.slice(4).toLowerCase());
      if (!decoded) return label;
      if (/^[\p{Script=Latin}\d-]+$/u.test(decoded)) return decoded;
      if (!/^[\p{Script=Cyrillic}\d-]+$/u.test(decoded)) return label;
      const lookalike = [...decoded].every((ch) => LATIN_LOOKALIKES.includes(ch) || /[\d-]/.test(ch));
      return lookalike && !cyrillicZone ? label : decoded;
    })
    .join(".");
}

/** Кириллические буквы, которые выглядят как латинские. */
const LATIN_LOOKALIKES = "асԁеһіјӏорԛѕԝхуүьпгѵѡ";
/** Зоны стран с кириллицей: там кириллическое имя сайта ожидаемо. */
const CYRILLIC_ZONES = new Set(["ru", "su", "by", "ua", "kz", "bg", "rs", "mk", "mn", "kg", "tj", "uz", "me", "ba"]);

/** Знаки, которые в адресе остаются закодированными: служебные, пробелы и невидимые. */
const KEEP_ENCODED =
  /[\u0000-\u00a0\u00ad\u034f\u061c\u115f\u1160\u17b4\u17b5\u180b-\u180e\u2000-\u200f\u2028-\u202f\u205f-\u206f\u3000\u3164\ufe00-\ufe0f\ufeff\uffa0\ufff0-\ufffb]/gu;

/**
 * Путь адреса буквами: `/wiki/%D0%9C%D0%BE%D1%81%D0%BA%D0%B2%D0%B0` →
 * `/wiki/Москва`, как в Chrome. Служебные знаки (`%2F`, `%20`) и невидимые
 * символы остаются закодированными: они меняют смысл адреса.
 */
export function displayPath(text) {
  return String(text ?? "").replace(/(?:%[0-9a-f]{2})+/gi, (run) => {
    let decoded;
    try {
      decoded = decodeURIComponent(run);
    } catch {
      return run;
    }
    return decoded.replace(KEEP_ENCODED, (ch) => encodeURIComponent(ch));
  });
}

/** Адрес целиком для показа: имя сайта и путь буквами. */
export function displayUrl(url) {
  try {
    const parsed = new URL(url);
    if (!/^https?:$/.test(parsed.protocol)) return displayPath(url);
    const host = displayHost(parsed.hostname) + (parsed.port ? `:${parsed.port}` : "");
    return `${parsed.protocol}//${host}${displayPath(parsed.pathname + parsed.search + parsed.hash)}`;
  } catch {
    return String(url ?? "");
  }
}

/** Punycode (RFC 3492): метка без `xn--` → строка. `null` — метка испорчена. */
function punycodeDecode(input) {
  const base = 36;
  const tMin = 1;
  const tMax = 26;
  const adapt = (delta, points, first) => {
    let d = first ? Math.floor(delta / 700) : delta >> 1;
    d += Math.floor(d / points);
    let k = 0;
    while (d > ((base - tMin) * tMax) >> 1) {
      d = Math.floor(d / (base - tMin));
      k += base;
    }
    return k + Math.floor(((base - tMin + 1) * d) / (d + 38));
  };

  const output = [];
  const delimiter = input.lastIndexOf("-");
  for (let j = 0; j < Math.max(0, delimiter); j++) {
    const code = input.charCodeAt(j);
    if (code >= 0x80) return null;
    output.push(code);
  }
  let n = 128;
  let i = 0;
  let bias = 72;
  for (let index = delimiter > 0 ? delimiter + 1 : 0; index < input.length; ) {
    const old = i;
    for (let w = 1, k = base; ; k += base) {
      if (index >= input.length) return null;
      const code = input.charCodeAt(index++);
      const digit =
        code >= 48 && code <= 57 ? code - 22 : code >= 97 && code <= 122 ? code - 97 : code >= 65 && code <= 90 ? code - 65 : base;
      if (digit >= base) return null;
      i += digit * w;
      const t = k <= bias ? tMin : k >= bias + tMax ? tMax : k - bias;
      if (digit < t) break;
      w *= base - t;
    }
    bias = adapt(i - old, output.length + 1, old === 0);
    n += Math.floor(i / (output.length + 1));
    i %= output.length + 1;
    if (n > 0x10ffff) return null;
    output.splice(i, 0, n);
    i += 1;
  }
  try {
    return String.fromCodePoint(...output);
  } catch {
    return null;
  }
}

export function fileName(path) {
  return String(path ?? "").split(/[\\/]/).pop() || String(path ?? "");
}

/** Русские числительные: «1 файл», «3 файла», «11 файлов». */
export function plural(count, one, few, many) {
  const mod10 = count % 10;
  const mod100 = count % 100;
  if (mod10 === 1 && mod100 !== 11) return one;
  if (mod10 >= 2 && mod10 <= 4 && (mod100 < 12 || mod100 > 14)) return few;
  return many;
}

export function formatBytes(bytes) {
  if (!bytes || bytes < 0) return "0 Б";
  const units = ["Б", "КБ", "МБ", "ГБ", "ТБ"];
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  const digits = unit === 0 ? 0 : value < 10 ? 1 : 0;
  return `${value.toFixed(digits).replace(".", ",")} ${units[unit]}`;
}

export function formatDuration(seconds) {
  if (!Number.isFinite(seconds) || seconds <= 0) return "";
  if (seconds < 60) return `${Math.ceil(seconds)} с`;
  if (seconds < 3600) return `${Math.round(seconds / 60)} мин`;
  const hours = Math.floor(seconds / 3600);
  return `${hours} ч ${Math.round((seconds - hours * 3600) / 60)} мин`;
}

/** «Сегодня», «Вчера», «12 сентября» — заголовки групп по дням. */
export function dayLabel(stampSeconds) {
  const date = new Date(stampSeconds * 1000);
  const today = new Date();
  const yesterday = new Date(today);
  yesterday.setDate(today.getDate() - 1);
  if (date.toDateString() === today.toDateString()) return "Сегодня";
  if (date.toDateString() === yesterday.toDateString()) return "Вчера";
  return date.toLocaleDateString("ru-RU", {
    day: "numeric",
    month: "long",
    year: date.getFullYear() === today.getFullYear() ? undefined : "numeric",
  });
}

/** День из даты «ГГГГ-ММ-ДД» — как пишут по-русски: «20 сентября 2026». */
export function formatDay(isoDate) {
  const date = new Date(`${isoDate}T00:00:00`);
  if (Number.isNaN(date.getTime())) return isoDate;
  return date.toLocaleDateString("ru-RU", { day: "numeric", month: "long", year: "numeric" }).replace(/\s*г\.$/, "");
}

export function clock(stampSeconds) {
  return new Date(stampSeconds * 1000).toLocaleTimeString("ru-RU", { hour: "2-digit", minute: "2-digit" });
}

/** Иконка файла по расширению — чтобы список загрузок читался глазами. */
export function fileIcon(path) {
  const ext = fileName(path).split(".").pop()?.toLowerCase() ?? "";
  if (["png", "jpg", "jpeg", "gif", "webp", "svg", "bmp", "avif", "heic"].includes(ext)) return "image";
  if (["mp3", "flac", "wav", "ogg", "m4a", "aac", "opus"].includes(ext)) return "music";
  if (["mp4", "mkv", "webm", "mov", "avi"].includes(ext)) return "video";
  if (["zip", "rar", "7z", "tar", "gz", "xz"].includes(ext)) return "archive";
  if (["exe", "msi", "msix", "appx"].includes(ext)) return "app";
  if (["pdf", "doc", "docx", "txt", "rtf", "odt", "xls", "xlsx", "csv", "ppt", "pptx", "md"].includes(ext)) return "doc-text";
  return "document";
}

export function debounce(fn, ms) {
  let timer = 0;
  return (...args) => {
    clearTimeout(timer);
    timer = setTimeout(() => fn(...args), ms);
  };
}

/** Элемент под курсором → прямоугольник в CSS-пикселях окна для попапа. */
export function anchorOf(node) {
  const rect = node.getBoundingClientRect();
  return { x: rect.left, y: rect.top, width: rect.width, height: rect.height };
}
