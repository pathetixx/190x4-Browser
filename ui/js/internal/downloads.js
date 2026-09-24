/**
 * Страница 190x4://downloads — полный список загрузок, как chrome://downloads:
 * группы по дням, поиск, действия над каждым файлом.
 */

import { invoke } from "../bridge.js";
import { dayLabel, el, fileIcon, hostOf, icon, iconButton } from "../dom.js";
import {
  clearFinished,
  control,
  displayName,
  downloads,
  initDownloads,
  isActive,
  onDownloads,
  progressOf,
  statusText,
} from "../downloads-model.js";
import { hooks, navigate } from "../actions.js";

export function createDownloadsPage(root) {
  let query = "";
  let order = "";
  const cards = new Map();

  const page = el("div", "page page--single");
  const main = el("div", "page__main");
  page.append(main);

  const header = el("div", "page__header");
  header.append(el("h1", "page__title", "Загрузки"));

  const search = el("label", "search");
  search.append(icon("search-16", 16));
  const input = el("input", "field");
  input.type = "search";
  input.placeholder = "Поиск в загрузках";
  input.addEventListener("input", () => {
    query = input.value.trim().toLowerCase();
    order = "";
    render();
  });
  search.append(input);
  header.append(search);

  const folder = el("button", "btn btn--ghost");
  folder.append(icon("folder-open", 20), el("span", null, "Открыть папку"));
  folder.addEventListener("click", () => invoke("downloads_folder_open").catch((error) => hooks.toast(String(error))));
  const clear = el("button", "btn btn--ghost", "Очистить всё");
  clear.addEventListener("click", () => clearFinished().catch(() => {}));
  header.append(folder, clear);
  main.append(header);

  const list = el("div", "downloads");
  main.append(list);
  root.append(page);

  const render = () => {
    const items = downloads().filter(
      (item) =>
        !query ||
        displayName(item).toLowerCase().includes(query) ||
        item.url.toLowerCase().includes(query)
    );
    clear.disabled = !items.some((item) => !isActive(item));

    const key = items.map((item) => `${item.id}:${item.state}`).join(",");
    if (key === order) {
      // Состав тот же — обновляем цифры на месте, не пересоздавая карточки:
      // иначе прыгает выделение текста и hover.
      for (const item of items) updateCard(cards.get(item.id), item);
      return;
    }
    order = key;
    cards.clear();
    list.replaceChildren();

    if (!items.length) {
      const empty = el("div", "empty");
      empty.textContent = query ? "Ничего не нашлось" : "Здесь появятся файлы, которые вы скачаете";
      list.append(empty);
      return;
    }

    let day = "";
    for (const item of items) {
      const label = dayLabel(item.started_at);
      if (label !== day) {
        day = label;
        list.append(el("div", "day", label));
      }
      const card = buildCard(item);
      cards.set(item.id, card);
      list.append(card.node);
    }
  };

  // Прогресс приходит от каждой загрузки несколько раз в секунду — рисуем раз в кадр.
  let frame = 0;
  const unsubscribe = onDownloads(() => {
    frame ||= requestAnimationFrame(() => {
      frame = 0;
      render();
    });
  });
  initDownloads().then(render);
  render();

  return {
    destroy() {
      unsubscribe();
      cancelAnimationFrame(frame);
    },
  };
}

function buildCard(item) {
  const node = el("div", "download");
  node.dataset.state = item.state;

  const tile = el("div", "download__icon");
  tile.append(icon(fileIcon(item.path), 20));

  const body = el("div", "download__body");
  const name = el(item.state === "done" ? "button" : "span", "download__name", displayName(item));
  if (item.state === "done") {
    name.title = "Открыть файл";
    name.addEventListener("click", () => act(item.id, "open"));
  }
  const url = el("button", "download__url", item.url);
  url.title = "Открыть страницу загрузки";
  url.addEventListener("click", () => navigate(item.url, { newTab: true }));
  const status = el("div", "download__status", statusText(item));
  body.append(name, url, status);

  let meter = null;
  let fill = null;
  if (isActive(item)) {
    meter = el("div", "meter");
    fill = el("div", "meter__fill");
    meter.append(fill);
    body.append(meter);
  }

  const links = el("div", "download__links");
  for (const [action, label] of linksFor(item)) {
    const link = el("button", "link", label);
    link.addEventListener("click", () => act(item.id, action));
    links.append(link);
  }
  if (links.childElementCount) body.append(links);

  const side = el("div");
  if (!isActive(item)) {
    side.append(iconButton("dismiss-16", "Убрать из списка", () => act(item.id, "remove")));
  }

  node.append(tile, body, side);
  const card = { node, status, meter, fill };
  updateCard(card, item);
  return card;
}

function updateCard(card, item) {
  if (!card) return;
  card.status.textContent = statusText(item);
  if (card.meter) {
    const share = progressOf(item);
    card.meter.dataset.paused = String(item.state === "paused");
    card.meter.dataset.indeterminate = String(share == null);
    card.fill.style.width = `${Math.round((share ?? 0) * 100)}%`;
  }
}

function linksFor(item) {
  const media = item.kind === "media";
  switch (item.state) {
    case "running":
      return media ? [["cancel", "Отменить"]] : [["pause", "Приостановить"], ["cancel", "Отменить"]];
    case "paused":
      return [["resume", "Продолжить"], ["cancel", "Отменить"]];
    case "done":
      return [["show", "Показать в папке"]];
    case "failed":
      return media ? [] : [["resume", "Продолжить"], ["retry", "Повторить"]];
    case "cancelled":
      return media ? [] : [["retry", "Повторить"]];
    default:
      return [];
  }
}

async function act(id, action) {
  try {
    await control(id, action);
  } catch (error) {
    hooks.toast(String(error?.message ?? error));
  }
}

export { hostOf };
