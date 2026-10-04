// Косметика 190x4: прячет рекламу, которую сайт отдаёт вместе с содержимым.
//
// Встраивается в каждый документ вкладки (и во фреймы её сайта) до скриптов
// страницы; настройки подставляет браузер вместо __X4_CONFIG__
// (crates/adblock/src/cosmetic.rs):
// * css — готовые правила скрытия сайта и общие сложные селекторы;
// * procedural — правила, которые CSS не выразить: «элемент с таким текстом»,
//   «подняться на два уровня», «убрать атрибут» (:has-text, :upward, :remove…);
// * generic — искать общие правила по классам и id: страница сообщает браузеру
//   свои классы и id, браузер отвечает селекторами (cosmetic_ids → cosmetic_css).
(() => {
  "use strict";

  const config = __X4_CONFIG__;
  if (location.hostname !== config.host) return;
  const MARK = Symbol.for("x4cosmetic");
  if (window[MARK]) return;
  try {
    Object.defineProperty(window, MARK, { value: true, enumerable: false });
  } catch (_) {
    return;
  }

  /* ── Стили ──────────────────────────────────────────────────── */

  const sheets = [];
  const attach = () => {
    const parent = document.head || document.documentElement;
    if (!parent) return false;
    for (const sheet of sheets) if (!sheet.isConnected) parent.append(sheet);
    return true;
  };
  const addCss = (text) => {
    if (!text) return;
    const sheet = document.createElement("style");
    sheet.textContent = text;
    sheets.push(sheet);
    attach();
  };
  addCss(config.css);

  // Ответы на классы и id страницы (общие правила) приходят пачками всю жизнь
  // страницы. Все — в один лист: каждый новый <style> — это ещё один лист, и
  // за долгую сессию их набегали бы сотни, а каждый пересчитывает стили всего
  // документа.
  let genericSheet = null;
  const addGeneric = (text) => {
    if (!text) return;
    if (!genericSheet) {
      genericSheet = document.createElement("style");
      sheets.push(genericSheet);
    }
    genericSheet.appendChild(document.createTextNode(`${text}\n`));
    attach();
  };
  if (!document.documentElement) {
    new MutationObserver((_, observer) => {
      if (attach()) observer.disconnect();
    }).observe(document, { childList: true });
  }

  const bridge = () => (window.chrome && window.chrome.webview) || null;
  const post = (message) => {
    const hook = bridge();
    if (!hook) return false;
    try {
      hook.postMessage(message);
      return true;
    } catch (_) {
      return false;
    }
  };

  /* ── Процедурные правила ────────────────────────────────────── */

  /** Строка или /регулярка/флаги → проверка текста. `exact` — литерал целиком. */
  const matcher = (arg, exact) => {
    const text = String(arg).trim();
    const regex = /^\/(.+)\/([dgimsuy]*)$/s.exec(text);
    if (regex) {
      try {
        const re = new RegExp(regex[1], regex[2].replace("g", ""));
        return (value) => re.test(value);
      } catch (_) {
        return null;
      }
    }
    const literal = text.replace(/^"(.*)"$/s, "$1");
    return exact ? (value) => value === literal : (value) => value.includes(literal);
  };

  /** `имя="значение"` или `имя: значение` → две проверки. */
  const pair = (arg, separator) => {
    let quoted = false;
    let slashed = false;
    for (let i = 0; i < arg.length; i += 1) {
      const c = arg[i];
      if (c === '"' && arg[i - 1] !== "\\") quoted = !quoted;
      else if (c === "/" && !quoted && arg[i - 1] !== "\\") slashed = !slashed;
      else if (c === separator && !quoted && !slashed) {
        return [arg.slice(0, i).trim(), arg.slice(i + 1).trim()];
      }
    }
    return [arg.trim(), ""];
  };

  /** Сделать шаг выборки над списком элементов: `null` — шаг не понят. */
  const step = (op, first) => {
    const arg = String(op.arg ?? "");
    switch (op.type) {
      case "css-selector": {
        if (first) return () => Array.from(document.querySelectorAll(arg));
        const rest = arg.trimStart();
        const combinator = rest[0];
        if (combinator === ">") return (nodes) => nodes.flatMap((node) => Array.from(node.querySelectorAll(`:scope ${rest}`)));
        if (combinator === "+" || combinator === "~") {
          const selector = rest.slice(1).trim();
          return (nodes) =>
            nodes.flatMap((node) => {
              const out = [];
              for (let next = node.nextElementSibling; next; next = next.nextElementSibling) {
                if (next.matches(selector)) out.push(next);
                if (combinator === "+") break;
              }
              return out;
            });
        }
        if (rest !== arg) return (nodes) => nodes.flatMap((node) => Array.from(node.querySelectorAll(`:scope ${rest}`)));
        return (nodes) => nodes.filter((node) => node.matches(arg));
      }
      case "has-text": {
        const test = matcher(arg, false);
        return test && ((nodes) => nodes.filter((node) => test(node.textContent || "")));
      }
      case "min-text-length": {
        const length = Number(arg);
        return Number.isFinite(length) && ((nodes) => nodes.filter((node) => (node.textContent || "").length >= length));
      }
      case "matches-attr": {
        const [name, value] = pair(arg, "=");
        const testName = matcher(name, true);
        const testValue = value ? matcher(value, true) : () => true;
        if (!testName || !testValue) return null;
        return (nodes) =>
          nodes.filter((node) => Array.from(node.attributes).some((attr) => testName(attr.name) && testValue(attr.value)));
      }
      case "matches-css":
      case "matches-css-before":
      case "matches-css-after": {
        const pseudo = op.type === "matches-css" ? null : op.type.endsWith("before") ? "::before" : "::after";
        const [property, value] = pair(arg, ":");
        const test = matcher(value, true);
        return test && ((nodes) => nodes.filter((node) => test(getComputedStyle(node, pseudo).getPropertyValue(property))));
      }
      case "matches-path": {
        // Проверка адреса страницы. Первым шагом она открывает весь документ:
        // следующий селектор ищет по нему (`compile`).
        const test = matcher(arg, false);
        const start = first ? () => [document.documentElement] : (nodes) => nodes;
        return test && ((nodes) => (test(location.pathname + location.search) ? start(nodes) : []));
      }
      case "upward": {
        const count = Number(arg);
        if (Number.isInteger(count) && count > 0) {
          return (nodes) =>
            nodes
              .map((node) => {
                let up = node;
                for (let i = 0; i < count && up; i += 1) up = up.parentElement;
                return up;
              })
              .filter(Boolean);
        }
        return (nodes) => nodes.map((node) => node.parentElement && node.parentElement.closest(arg)).filter(Boolean);
      }
      case "xpath":
        return (nodes) =>
          (first ? [document] : nodes).flatMap((context) => {
            const found = [];
            try {
              const result = document.evaluate(arg, context, null, XPathResult.ORDERED_NODE_SNAPSHOT_TYPE, null);
              for (let i = 0; i < result.snapshotLength; i += 1) {
                const node = result.snapshotItem(i);
                if (node.nodeType === 1) found.push(node);
              }
            } catch (_) {
              // Выражение не разобралось — правило ничего не находит.
            }
            return found;
          });
      default:
        return null;
    }
  };

  const compile = (filter) => {
    const ops = Array.isArray(filter && filter.selector) ? filter.selector : [];
    if (!ops.length) return null;
    // Правило начинается не с селектора: поиск по всему документу понятен
    // только для :xpath и :matches-path.
    if (!["css-selector", "xpath", "matches-path"].includes(ops[0].type)) return null;
    const steps = [];
    for (let i = 0; i < ops.length; i += 1) {
      // После проверки адреса селектор ищет по всему документу.
      const first = i === 0 || (i === 1 && ops[0].type === "matches-path");
      const fn = step(ops[i], first);
      if (!fn) return null;
      steps.push(fn);
    }
    return { steps, action: filter.action || null };
  };

  const procedural = (Array.isArray(config.procedural) ? config.procedural : []).map(compile).filter(Boolean);

  /** Объявления `a: b; c: d` → пары. */
  const declarations = (css) =>
    String(css)
      .split(";")
      .map((part) => pair(part, ":"))
      .filter(([property, value]) => property && value);

  const apply = (node, action) => {
    const type = action ? action.type : "hide";
    if (type === "hide") {
      if (node.style.getPropertyValue("display") !== "none" || node.style.getPropertyPriority("display") !== "important") {
        node.style.setProperty("display", "none", "important");
      }
    } else if (type === "remove") {
      node.remove();
    } else if (type === "style") {
      for (const [property, value] of declarations(action.arg)) {
        node.style.setProperty(property, value.replace(/\s*!important$/, ""), "important");
      }
    } else if (type === "remove-attr") {
      if (node.hasAttribute(action.arg)) node.removeAttribute(action.arg);
    } else if (type === "remove-class") {
      if (node.classList.contains(action.arg)) node.classList.remove(action.arg);
    }
  };

  const runProcedural = () => {
    for (const filter of procedural) {
      let nodes = [];
      try {
        for (const fn of filter.steps) {
          nodes = fn(nodes);
          if (!nodes.length) break;
        }
      } catch (_) {
        continue;
      }
      for (const node of new Set(nodes)) apply(node, filter.action);
    }
  };

  /* ── Общие правила по классам и id ──────────────────────────── */

  const seenClasses = new Set();
  const seenIds = new Set();
  /** Добавленные узлы — смотрим их целиком; узлы со сменившимся классом — только их самих. */
  const added = new Set();
  const touched = new Set();
  let full = true;

  const survey = () => {
    const classes = [];
    const ids = [];
    const note = (node) => {
      const id = node.id;
      if (id && !seenIds.has(id)) {
        seenIds.add(id);
        ids.push(id);
      }
      for (const name of node.classList) {
        if (!seenClasses.has(name)) {
          seenClasses.add(name);
          classes.push(name);
        }
      }
    };
    const roots = full ? [document.documentElement] : topmost(added);
    for (const node of full ? [] : touched) if (node.nodeType === 1) note(node);
    full = false;
    added.clear();
    touched.clear();
    // Потолок на один проход: страница, пересобирающая тысячи узлов, не
    // должна занимать поток каждую четверть секунды.
    let budget = 20000;
    for (const root of roots) {
      if (!root || root.nodeType !== 1 || !root.isConnected) continue;
      note(root);
      for (const node of root.querySelectorAll("[class],[id]")) {
        if (--budget <= 0) break;
        note(node);
      }
      if (budget <= 0) break;
    }
    send(classes, ids);
  };

  /**
   * Добавленные узлы без тех, что лежат внутри других добавленных: поддерево
   * обходится один раз, а не по разу на каждого предка из списка.
   */
  const topmost = (nodes) =>
    Array.from(nodes).filter((node) => {
      for (let up = node.parentElement; up; up = up.parentElement) if (nodes.has(up)) return false;
      return true;
    });

  /** Классы и id — браузеру, пачками до 40 КБ: длинные сообщения он не читает. */
  const send = (classes, ids) => {
    let batch = { classes: [], ids: [] };
    let size = 0;
    const flush = () => {
      if (batch.classes.length || batch.ids.length) post({ evt: "cosmetic_ids", ...batch });
      batch = { classes: [], ids: [] };
      size = 0;
    };
    for (const [list, key] of [
      [classes, "classes"],
      [ids, "ids"],
    ]) {
      for (const name of list) {
        if (name.length > 256) continue;
        if (size + name.length > 40000) flush();
        batch[key].push(name);
        size += name.length + 3;
      }
    }
    flush();
  };

  if (config.generic) {
    const hook = () => {
      const bridgeNow = bridge();
      if (!bridgeNow) return false;
      bridgeNow.addEventListener("message", (event) => {
        const data = event.data;
        if (!data || data.cmd !== "cosmetic_css" || data.origin !== location.origin || !Array.isArray(data.selectors)) return;
        addGeneric(data.selectors.map((selector) => `${selector}{display:none!important}`).join("\n"));
      });
      return true;
    };
    if (!hook()) document.addEventListener("DOMContentLoaded", hook, { once: true });
  }

  /* ── Когда проверять ────────────────────────────────────────── */

  // Документ меняется пачками; классы и id сверяются не чаще четырёх раз в
  // секунду, процедурные правила — двух: каждое из них ищет по всему документу.
  // Процедурные правила — только когда в документ что-то добавили, как в
  // uBlock Origin: смена класса или текста (таймер плеера, бегущий счётчик)
  // гоняла бы их по всему документу всё время, пока страница открыта. В
  // фоновой вкладке не делается ничего — всё, что накопилось, проверяется при
  // показе. Сами правила выполняются в простое страницы, а не посреди кадра.
  let timer = 0;
  let idle = 0;
  let proceduralAt = -Infinity;
  let structural = true;
  const runSoon = () => {
    if (idle) return;
    const run = () => {
      idle = 0;
      if (document.hidden) {
        structural = true;
        return;
      }
      proceduralAt = performance.now();
      structural = false;
      runProcedural();
    };
    // Первый проход — сразу: реклама не должна мелькнуть при загрузке.
    if (proceduralAt === -Infinity) run();
    else idle = typeof requestIdleCallback === "function" ? requestIdleCallback(run, { timeout: 200 }) : setTimeout(run, 0);
  };
  const pass = () => {
    timer = 0;
    if (document.hidden) return;
    attach();
    if (config.generic) survey();
    if (!procedural.length || !structural) return;
    const wait = proceduralAt + 500 - performance.now();
    if (wait > 0) {
      schedule(wait);
      return;
    }
    runSoon();
  };
  const schedule = (delay) => {
    if (!timer) timer = setTimeout(pass, delay);
  };

  if (procedural.length || config.generic) {
    new MutationObserver((records) => {
      // Пока документ разбирается, каждый его элемент приходит отдельной
      // записью. Копить их незачем: проход и так смотрит документ целиком.
      if (config.generic && !full && document.readyState === "loading") full = true;
      for (const record of records) {
        if (record.type === "childList" && record.addedNodes.length) {
          structural = true;
          break;
        }
      }
      if (config.generic && !full) {
        for (const record of records) {
          if (record.type === "attributes") touched.add(record.target);
          else for (const node of record.addedNodes) if (node.nodeType === 1) added.add(node);
        }
        if (added.size + touched.size > 5000) {
          added.clear();
          touched.clear();
          full = true;
        }
      }
      schedule(250);
    }).observe(document, config.generic ? { childList: true, subtree: true, attributes: true, attributeFilter: ["class", "id"] } : { childList: true, subtree: true });
    document.addEventListener("visibilitychange", () => {
      if (!document.hidden) schedule(0);
    });
    if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", () => schedule(0), { once: true });
    else schedule(0);
  }
})();
