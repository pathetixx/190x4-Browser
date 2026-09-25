/**
 * Расширения браузера — одно описание на все места, где они видны: меню
 * «Расширения» на панели инструментов, закреплённые значки и раздел настроек.
 * Ключи — те же, что читает Rust (`src-tauri/src/sponsorblock.rs`,
 * `autoscroll.rs`, `twitch.rs`), умолчания — в `prefs.js`.
 *
 * `settings` — настройки расширения: переключатели (`switch`), списки
 * (`select`) и строки (`text`). `service` — сервис 190x4, без которого расширение ничего не
 * сделает. `open` — у расширения есть своё окно, и так называется кнопка,
 * которая его открывает.
 */

import { LANGUAGES } from "./languages.js";

export const SPONSORBLOCK_CATEGORIES = [
  ["sponsor", "Спонсорская вставка", "Оплаченная реклама внутри видео"],
  ["selfpromo", "Реклама автора", "Свой мерч, курсы, донаты, другие каналы"],
  ["interaction", "Просьба подписаться", "«Подпишитесь, поставьте лайк, нажмите колокольчик»"],
  ["intro", "Заставка", "Вступление без содержания"],
  ["outro", "Концовка", "Титры, конечные заставки, прощание"],
  ["preview", "Анонс", "Нарезка того, что будет дальше в видео или в прошлых выпусках"],
  ["filler", "Отступление", "Шутки и сцены не по теме"],
  ["music_offtopic", "Не музыка в клипе", "Разговоры и сценки в музыкальном видео"],
];

export const SPONSORBLOCK_MODES = [
  ["skip", "Пропускать"],
  ["ask", "Спрашивать"],
  ["show", "Отмечать на полосе"],
  ["off", "Не трогать"],
];

export const EXTENSIONS = [
  {
    id: "translate",
    name: "Переводчик",
    icon: "translate",
    summary:
      "Переводит любой текст: вставьте его в окно расширения или выделите на странице и нажмите Ctrl+Shift+U.",
    enabled: "ext_translate_enabled",
    pinned: "ext_translate_pinned",
    pinHint: "Без значка переводчик открывается сочетанием Ctrl+Shift+U и из меню выделенного текста",
    service: "translate",
    open: "Открыть переводчик",
    settings: [
      {
        type: "select",
        key: "translate_lang",
        label: "Язык перевода",
        hint: "На этот язык переводчик переводит текст, пока в его окне не выбран другой",
        options: LANGUAGES.map((name) => [name, name]),
      },
    ],
  },
  {
    id: "sponsorblock",
    name: "SponsorBlock",
    icon: "skip",
    summary:
      "Пропускает в видео YouTube спонсорские вставки, просьбы подписаться и другие сегменты, которые разметили зрители.",
    note: "Разметка — сообщество SponsorBlock (sponsor.ajay.app, CC BY-NC-SA 4.0); на сервер уходит не номер видео, а начало его хеша.",
    enabled: "ext_sponsorblock_enabled",
    pinned: "ext_sponsorblock_pinned",
    pinHint: "Значок появляется на YouTube и открывает эти настройки",
    settings: SPONSORBLOCK_CATEGORIES.map(([category, label, hint]) => ({
      type: "select",
      key: `sponsorblock_${category}`,
      label,
      hint,
      options: SPONSORBLOCK_MODES,
    })),
  },
  {
    id: "autoscroll",
    name: "Автопролистывание",
    icon: "autoscroll",
    summary:
      "Доигравший ролик в YouTube Shorts, Reels в Instagram и TikTok сам сменяется следующим — лента листается так же, как стрелкой вниз. Пока вы пишете комментарий, лента стоит.",
    enabled: "ext_autoscroll_enabled",
    pinned: "ext_autoscroll_pinned",
    pinHint: "Кнопка появляется только в этих лентах и одним щелчком включает или выключает пролистывание",
    settings: [
      { type: "switch", key: "autoscroll_youtube", label: "YouTube Shorts", hint: "youtube.com/shorts" },
      { type: "switch", key: "autoscroll_instagram", label: "Reels в Instagram", hint: "instagram.com/reels" },
      { type: "switch", key: "autoscroll_tiktok", label: "TikTok", hint: "tiktok.com" },
    ],
  },
  {
    id: "twitch",
    name: "Twitch",
    icon: "stream",
    summary:
      "Лучшее качество трансляций там, где Twitch его режет (из России — выше 720p), бонусы баллов канала сами и смайлы BetterTTV и FrankerFaceZ в чате.",
    note: "Ради качества плейлист трансляции браузер берёт через сервер за пределами России (по умолчанию — прокси ReYohoho); само видео идёт с серверов Twitch напрямую.",
    enabled: "ext_twitch_enabled",
    pinned: "ext_twitch_pinned",
    pinHint: "Значок появляется на Twitch и открывает эти настройки",
    settings: [
      {
        type: "switch",
        key: "twitch_quality",
        label: "Лучшее качество трансляций",
        hint: "Сервер не ответил — трансляция играет в том качестве, что даёт Twitch",
      },
      {
        type: "switch",
        key: "twitch_points",
        label: "Собирать бонусы баллов канала",
        hint: "Кнопку «Получить бонус» браузер нажимает сам, как только она появилась",
      },
      {
        type: "switch",
        key: "twitch_bttv",
        label: "Смайлы BetterTTV",
        hint: "Коды смайлов в сообщениях чата становятся картинками — общие и смайлы канала",
      },
      {
        type: "switch",
        key: "twitch_ffz",
        label: "Смайлы FrankerFaceZ",
        hint: "То же для смайлов FrankerFaceZ",
      },
      {
        type: "switch",
        key: "twitch_proxy_token",
        label: "Передавать серверу вход в Twitch",
        hint: "Нужно для 1440p и чтобы вместо трансляции не показывалась рекламная заставка. Токен даёт серверу доступ к вашему аккаунту — включайте, только если доверяете ему",
      },
      {
        type: "text",
        key: "twitch_proxy",
        label: "Свой сервер плейлистов",
        hint: "Адрес, к которому браузер приписывает адрес плейлиста, — как у ReYohoho. Пусто — серверы ReYohoho",
        placeholder: "https://…/",
      },
    ],
  },
  {
    id: "media",
    name: "Загрузчик видео",
    icon: "video",
    summary: "Скачивает видео и звук с YouTube, VK, Rutube и других сайтов через сервер 190x4. Ctrl+Shift+D.",
    enabled: "ext_media_enabled",
    pinned: "ext_media_pinned",
    pinHint: "Без значка загрузчик открывается сочетанием Ctrl+Shift+D и из меню видео на странице",
    service: "media",
    open: "Открыть загрузчик",
    settings: [],
  },
];

export function extensionById(id) {
  return EXTENSIONS.find((extension) => extension.id === id) ?? null;
}

/** Сервис 190x4, без которого расширение не работает, не настроен. */
export function missingService(extension, services) {
  return Boolean(extension.service) && !services?.[extension.service];
}
