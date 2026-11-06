/**
 * Меню правой кнопки в полях ввода и в выделенном тексте интерфейса: своё
 * вместо меню движка. Окно браузера показывает его всплывающим окном, само
 * всплывающее окно — меню поверх себя.
 *
 * Буфер обмена читает и пишет браузер (`src-tauri/src/clipboard.rs`): чтение
 * из страницы спрашивало бы разрешение окном движка, а запись — требовала бы
 * фокуса, который на время меню у всплывающего окна.
 */

import { invoke } from "./bridge.js";

const TEXT_INPUTS = new Set(["", "text", "search", "url", "email", "tel", "password", "number"]);

/** Поле ввода под щелчком или `null`. */
export function editableAt(target) {
  const field = target?.closest?.("input, textarea, [contenteditable]");
  if (!field) return null;
  if (field instanceof HTMLInputElement) return TEXT_INPUTS.has(field.type) ? field : null;
  if (field instanceof HTMLTextAreaElement) return field;
  return field.isContentEditable ? field : null;
}

/**
 * Пункты меню для поля `field` (или для выделенного текста, если поля нет) и
 * исполнитель выбора. Выделение запоминается сейчас: пока открыто меню, фокус
 * у него, а не у поля.
 */
export function editMenu(field) {
  const plain = field instanceof HTMLInputElement || field instanceof HTMLTextAreaElement;
  const editable = Boolean(field) && (plain ? !field.readOnly && !field.disabled : field.isContentEditable);
  const start = plain ? field.selectionStart : null;
  const end = plain ? field.selectionEnd : null;
  const selection = getSelection();
  const range = !plain && selection?.rangeCount ? selection.getRangeAt(0).cloneRange() : null;
  const selected = plain ? field.value.slice(start ?? 0, end ?? 0) : String(selection ?? "");
  // Пароль из поля не копируется и не вырезается — как в любом браузере.
  const secret = plain && field.type === "password";

  const items = field
    ? [
        { id: "undo", label: "Отменить", keys: "Ctrl+Z", disabled: !editable },
        { separator: true },
        { id: "cut", label: "Вырезать", keys: "Ctrl+X", disabled: !editable || !selected || secret },
        { id: "copy", label: "Копировать", icon: "copy-16", keys: "Ctrl+C", disabled: !selected || secret },
        { id: "paste", label: "Вставить", icon: "clipboard", keys: "Ctrl+V", disabled: !editable },
        { id: "delete", label: "Удалить", disabled: !editable || !selected },
        { separator: true },
        { id: "select-all", label: "Выделить всё", keys: "Ctrl+A" },
      ]
    : [{ id: "copy", label: "Копировать", icon: "copy-16", keys: "Ctrl+C", disabled: !selected }];

  const restore = () => {
    if (!field) return;
    field.focus({ preventScroll: true });
    if (plain && start != null) field.setSelectionRange(start, end ?? start);
    else if (range) {
      const current = getSelection();
      current?.removeAllRanges();
      current?.addRange(range);
    }
  };

  const run = async (action) => {
    restore();
    switch (action) {
      case "undo":
        document.execCommand("undo");
        break;
      case "copy":
        await invoke("clipboard_write", { text: selected }).catch(() => {});
        break;
      case "cut":
        await invoke("clipboard_write", { text: selected })
          .then(() => {
            restore();
            document.execCommand("delete");
          })
          .catch(() => {});
        break;
      case "paste": {
        const text = await invoke("clipboard_read").catch(() => "");
        restore();
        // insertText сохраняет историю правки (Ctrl+Z) и шлёт полю `input`.
        if (typeof text === "string" && text) document.execCommand("insertText", false, text);
        break;
      }
      case "delete":
        document.execCommand("delete");
        break;
      case "select-all":
        if (plain) field.select();
        else if (field) {
          const all = document.createRange();
          all.selectNodeContents(field);
          getSelection()?.removeAllRanges();
          getSelection()?.addRange(all);
        }
        break;
    }
  };

  return { items, run };
}
