/**
 * Подтверждение действия браузера: «Закрыть окно?», пока идут загрузки.
 *
 * Спрашивает всплывающее окно поверх страницы — HTML-слой интерфейса оказался
 * бы под нативной страницей. Ответ — `true`, если человек согласился; окно,
 * закрытое без ответа (Escape, щелчок мимо), — это «нет».
 */

import { onPopupAction, onPopupClosed, openPopup } from "./popups.js";

const WIDTH = 400;
let pending = null;
/** Номер показа окна подтверждения: закрытие прежнего меню — не ответ «нет». */
let pendingSeq = 0;

onPopupAction("confirm", ({ action }) => pending?.(action === "yes"));
// Выбор приходит раньше закрытия, но порядок двух событий не гарантирован:
// «нет» по закрытию — с небольшой задержкой.
onPopupClosed((seq) => {
  if (!seq || seq !== pendingSeq) return;
  const answer = pending;
  setTimeout(() => answer?.(false), 150);
});

export function confirmAction({ title, text, confirm, cancel = "Отмена" }) {
  pending?.(false);
  return new Promise((resolve) => {
    const answer = (value) => {
      if (pending !== answer) return;
      pending = null;
      resolve(value);
    };
    pending = answer;
    pendingSeq = 0;
    const anchor = { x: Math.max(8, (innerWidth - WIDTH) / 2), y: 64, width: WIDTH, height: 0 };
    openPopup("confirm", anchor, { width: WIDTH, payload: { title, text, confirm, cancel } })
      .then((opened) => {
        if (!opened) answer(false);
        else if (pending === answer && typeof opened === "number") pendingSeq = opened;
      })
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
