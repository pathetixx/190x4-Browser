/**
 * Подтверждение действия браузера: «Закрыть окно?», пока идут загрузки.
 *
 * Спрашивает всплывающее окно поверх страницы — HTML-слой интерфейса оказался
 * бы под нативной страницей. Ответ — `true`, если человек согласился; окно,
 * закрытое без ответа (Escape, щелчок мимо), — это «нет».
 */

import { listen } from "./bridge.js";
import { onPopupAction, openPopup } from "./popups.js";

const WIDTH = 400;
let pending = null;

onPopupAction("confirm", ({ action }) => pending?.(action === "yes"));
// Выбор приходит раньше закрытия, но порядок двух событий не гарантирован:
// «нет» по закрытию — с небольшой задержкой.
listen("popup-closed", () => setTimeout(() => pending?.(false), 150));

export function confirmAction({ title, text, confirm, cancel = "Отмена" }) {
  pending?.(false);
  return new Promise((resolve) => {
    const answer = (value) => {
      if (pending !== answer) return;
      pending = null;
      resolve(value);
    };
    pending = answer;
    const anchor = { x: Math.max(8, (innerWidth - WIDTH) / 2), y: 64, width: WIDTH, height: 0 };
    openPopup("confirm", anchor, { width: WIDTH, payload: { title, text, confirm, cancel } })
      .then((opened) => opened || answer(false))
      .catch(() => answer(false));
  });
}

/** «Идёт загрузка 3 файлов.» */
export function downloadsText(count) {
  const n = Number(count) || 1;
  if (n === 1) return "Идёт загрузка файла.";
  const tail = n % 10 === 1 && n % 100 !== 11 ? "а" : "ов";
  return `Идёт загрузка ${n} файл${tail}.`;
}
