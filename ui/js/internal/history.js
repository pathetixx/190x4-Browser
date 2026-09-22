/**
 * Страница 190x4://history — вся история посещений, как chrome://history:
 * группы по дням, поиск, удаление записи и очистка за период.
 *
 * Панель истории справа показывает последние адреса и годится, чтобы быстро
 * вернуться; здесь — полный список каждого посещения: один и тот же сайт
 * встречается столько раз, сколько вы на нём были.
 */

import { invoke } from "../bridge.js";
import { clock, dayLabel, el, hostOf, icon, iconButton, siteIcon } from "../dom.js";
import { hooks, navigate, openInNewTab } from "../actions.js";

/** Сколько записей тянем за раз: дальше — по мере прокрутки. */
const PAGE = 120;

const PERIODS = [
  ["hour", "за последний час"],
  ["day", "за сутки"],
  ["week", "за неделю"],
  ["all", "за всё время"],
];

export function createHistoryPage(root) {
  let query = "";
  let items = [];
  let done = false;
  let loading = false;
  let search = 0;

  const page = el("div", "page page--single");
  const main = el("div", "page__main");
  page.append(main);

  const header = el("div", "page__header");
  header.append(el("h1", "page__title", "История"));

  const searchBox = el("label", "search");
  searchBox.append(icon("search-16", 16));
  const input = el("input", "field");
  input.type = "search";
  input.placeholder = "Поиск в истории";
  input.addEventListener("input", () => {
    query = input.value.trim();
    // Печать быстрее базы: отвечает только последний запрос.
    const token = ++search;
    setTimeout(() => {
      if (token === search) reload();
    }, 180);
  });
  searchBox.append(input);
  header.append(searchBox);

  const clear = el("button", "btn btn--ghost");
  clear.append(icon("broom", 20), el("span", null, "Очистить историю"));
  clear.addEventListener("click", () => askClear());
  header.append(clear);
  main.append(header);

  const list = el("div", "history-list");
  main.append(list);
  root.append(page);

  const more = el("button", "btn btn--ghost", "Показать ещё");
  more.addEventListener("click", () => load());

  async function load() {
    if (loading || done) return;
    loading = true;
    const before = items.length ? items[items.length - 1].visited_at : null;
    const batch = await invoke("history_page", { query, before, limit: PAGE }).catch(() => []);
    loading = false;
    // Записи той же секунды могли попасть в прошлую страницу: дубли убираем
    // по номеру посещения.
    const known = new Set(items.map((item) => item.id));
    const fresh = batch.filter((item) => !known.has(item.id));
    if (!fresh.length) {
      done = true;
    } else {
      items = items.concat(fresh);
    }
    if (batch.length < PAGE) done = true;
    render();
  }

  async function reload() {
    items = [];
    done = false;
    await load();
  }

  function render() {
    list.replaceChildren();
    if (!items.length) {
      list.append(el("div", "empty", query ? "Ничего не нашлось" : "История пуста"));
      return;
    }

    let day = "";
    for (const item of items) {
      const label = dayLabel(item.visited_at);
      if (label !== day) {
        day = label;
        list.append(el("div", "day", label));
      }
      list.append(row(item));
    }
    if (!done) list.append(more);
  }

  function row(item) {
    const node = el("div", "history-row");
    node.append(el("span", "history-row__time", clock(item.visited_at)));
    node.append(siteIcon(item.url, "favicon"));

    const link = el("button", "history-row__link");
    link.append(el("span", "history-row__title", item.title || item.url));
    link.append(el("span", "history-row__host", hostOf(item.url) || item.host));
    link.title = item.url;
    link.addEventListener("click", (event) => {
      if (event.ctrlKey) openInNewTab(item.url);
      else navigate(item.url);
    });
    // Средняя кнопка — в фоновой вкладке, как у ссылок на любой странице.
    link.addEventListener("mousedown", (event) => {
      if (event.button === 1) event.preventDefault(); // без автопрокрутки
    });
    link.addEventListener("auxclick", (event) => {
      if (event.button === 1) openInNewTab(item.url);
    });
    node.append(link);

    node.append(
      iconButton("dismiss-16", "Удалить из истории", async () => {
        await invoke("history_forget_visit", { id: item.id }).catch(() => {});
        items = items.filter((other) => other.id !== item.id);
        render();
      })
    );
    return node;
  }

  async function askClear() {
    const { modal } = await import("./settings.js");
    const buttons = PERIODS.map(([id, label]) => [id, label[0].toUpperCase() + label.slice(1), "btn"]);
    const answer = await modal({
      title: "Очистить историю",
      text: "Файлы cookie и кэш остаются на месте — их убирает «Удалить данные о работе в браузере».",
      actions: [[null, "Отмена", "btn btn--ghost"], ...buttons],
    });
    if (!answer) return;
    await invoke("history_clear_period", { period: answer }).catch((error) => hooks.toast(String(error)));
    await reload();
  }

  reload();

  return {
    destroy() {
      search += 1;
    },
  };
}
