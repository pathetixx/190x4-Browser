// Косметика 190x4: прячет рекламу, которую сайт отдаёт вместе с содержимым.
//
// Встраивается в документы своего хоста — страницы вкладки и фреймов со своими
// правилами (плеер чужого сайта) — до скриптов страницы; настройки подставляет
// браузер вместо __X4_CONFIG__ (crates/adblock/src/cosmetic.rs):
// * hide — селекторы скрытия из списков: в обычном режиме они прячут сразу, а
//   потом снимаются с блоков, которые оказались своим содержимым сайта, — как
//   у Brave (components/cosmetic_filters/resources/data/content_cosmetic.ts);
//   в агрессивном (aggressive) — просто прячут;
// * css — то, что прячется всегда: свои правила и :style() списков;
// * site — регистрируемый домен документа: свой ресурс или чужой;
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

  /* ── Окна фрейма чужого сайта ──────────────────────────────── */

  // Плеер на чужом домене открывает рекламу в новой вкладке скриптом по
  // щелчку: `window.open` на третий сайт или невидимая ссылка с `target`,
  // по которой «щёлкает» сам скрипт. Домены такой рекламы меняются каждый
  // день, и правил на них нет. Поэтому во фрейме чужого сайта (popups) окно
  // на чужой сайт скрипт не открывает: ему достаётся пустое окно-заглушка,
  // которое через секунду закрывается, — на отказ (`null`) реклама отвечает
  // новыми попытками на каждом щелчке. Ссылки, по которым щёлкнул человек,
  // работают как обычно, окна на свой сайт (вход через Google, VK, Telegram) —
  // тоже.
  if (config.popups && window !== window.top) {
    const own = (host) => host === config.site || host.endsWith("." + config.site);
    const foreign = (raw) => {
      try {
        const url = new URL(String(raw), location.href);
        return /^https?:$/.test(url.protocol) && !own(url.hostname);
      } catch (_) {
        return false;
      }
    };
    const decoy = () => {
      const frame = document.createElement("iframe");
      frame.style.display = "none";
      (document.body || document.documentElement).append(frame);
      const opened = frame.contentWindow;
      setTimeout(() => frame.remove(), 1000);
      return opened;
    };
    const open = window.open;
    const guarded = new Proxy(open, {
      apply(target, self, args) {
        if (args.length && foreign(args[0])) return decoy();
        return Reflect.apply(target, window, args);
      },
    });
    const install = (win) => {
      try {
        if (win && win.open !== guarded) {
          Object.defineProperty(win, "open", { value: guarded, writable: true, configurable: true });
        }
      } catch (_) {}
    };
    install(window);
    // Свежий пустой фрейм — обход: его `open` ещё родной.
    const frameWindow = Object.getOwnPropertyDescriptor(HTMLIFrameElement.prototype, "contentWindow");
    if (frameWindow && frameWindow.get) {
      Object.defineProperty(HTMLIFrameElement.prototype, "contentWindow", {
        ...frameWindow,
        get() {
          const win = frameWindow.get.call(this);
          install(win);
          return win;
        },
      });
    }
    // Ссылку на чужой сайт в новом окне «нажал» скрипт (`a.click()`,
    // `dispatchEvent`) — не человек.
    window.addEventListener(
      "click",
      (event) => {
        if (event.isTrusted) return;
        const link = event.target instanceof Element && event.target.closest("a[href], area[href]");
        if (link && link.target && link.target !== "_self" && foreign(link.href)) event.preventDefault();
      },
      true
    );
  }

  /* ── Стили ──────────────────────────────────────────────────── */

  // Листы стилей — сконструированные (`adoptedStyleSheets`), как у Brave
  // пользовательские: их нет ни в DOM, ни в `document.styleSheets`, и скрипт
  // сайта, который ищет и удаляет чужие <style>, их не видит. Сайт, который
  // сам присвоит `document.adoptedStyleSheets`, выкинет и наши — `attach`
  // возвращает их на каждом проходе. Где таких листов нет, — обычный <style>.
  const constructed = (() => {
    try {
      return "adoptedStyleSheets" in document && typeof new CSSStyleSheet().replaceSync === "function";
    } catch (_) {
      return false;
    }
  })();
  const sheets = [];
  const attach = () => {
    if (constructed) {
      try {
        const current = document.adoptedStyleSheets;
        const missing = sheets.filter((sheet) => !current.includes(sheet));
        if (missing.length) document.adoptedStyleSheets = [...current, ...missing];
        return true;
      } catch (_) {
        return false;
      }
    }
    const parent = document.head || document.documentElement;
    if (!parent) return false;
    for (const sheet of sheets) if (!sheet.isConnected) parent.append(sheet);
    return true;
  };
  const newSheet = (text) => {
    if (constructed) {
      const sheet = new CSSStyleSheet();
      // Невалидное правило выбрасывается одно, остальные остаются — как в <style>.
      sheet.replaceSync(text);
      return sheet;
    }
    const sheet = document.createElement("style");
    sheet.textContent = text;
    return sheet;
  };
  if (config.css) {
    sheets.push(newSheet(config.css));
    attach();
  }

  // Лист из многих правил, которые приходят пачками всю жизнь страницы
  // (общие правила по классам и id). Каждое правило помнится, чтобы его можно
  // было снять; новый лист на каждую пачку пересчитывал бы стили всего
  // документа, и за долгую сессию их набегали бы сотни.
  const ruleSheet = () => {
    const sheet = newSheet("");
    sheets.push(sheet);
    const rules = new Map();
    return {
      add(selector) {
        if (rules.has(selector)) return false;
        const text = `${selector}{display:none!important}`;
        if (constructed) {
          try {
            sheet.insertRule(text, sheet.cssRules.length);
            rules.set(selector, sheet.cssRules[sheet.cssRules.length - 1]);
          } catch (_) {
            // Селектор, которого браузер не знает, — пропустить только его.
            return false;
          }
        } else {
          const node = document.createTextNode(`${text}\n`);
          sheet.appendChild(node);
          rules.set(selector, node);
        }
        return true;
      },
      remove(selector) {
        const rule = rules.get(selector);
        if (!rule) return;
        rules.delete(selector);
        if (!constructed) {
          rule.remove();
          return;
        }
        const index = Array.prototype.indexOf.call(sheet.cssRules, rule);
        if (index >= 0) sheet.deleteRule(index);
      },
    };
  };
  const forced = ruleSheet();
  const listed = ruleSheet();
  const addForced = (selectors) => {
    for (const selector of selectors) forced.add(selector);
    attach();
  };

  /* ── Свой или чужой: правила списков в обычном режиме ──────────── */

  // Перенос логики Brave (content_cosmetic.ts, обычный режим Shields): правило
  // списка прячет сразу, а в простое страницы блоки, которые оно спрятало,
  // проверяются. Блок со ссылкой на свой сайт, без внешних ресурсов вообще или
  // с заметным текстом — содержимое сайта, и правило снимается насовсем;
  // остальное остаётся спрятанным. Три прохода — для того, что догрузилось.
  // Так блокировщик не трогает приманки и пустые места, по которым сайты
  // (Дзен) его замечают, а реклама с чужих адресов остаётся спрятанной.
  const maxTimeMSBeforeStart = 2500;
  const minAdTextChars = 30;
  const minAdTextWords = 5;
  const pumpIntervalMinMs = 40;
  const pumpIntervalMaxMs = 1000;
  const maxWorkSize = 60;

  const queues = [new Set(), new Set(), new Set()];
  const alreadyUnhiddenSelectors = new Set();
  const alreadyKnownFirstPartySubtrees = new WeakSet();
  let hasDelayOccurred = false;
  let startCheckingId;
  let queueIsSleeping = false;

  const site = String(config.site || location.hostname).toLowerCase();
  const isRelativeUrl = (url) => !url.startsWith("//") && !url.startsWith("http://") && !url.startsWith("https://");
  const isFirstPartyUrl = (url) => {
    if (isRelativeUrl(url)) return true;
    try {
      const host = new URL(url, location.href).hostname.toLowerCase();
      return host === site || host.endsWith(`.${site}`);
    } catch (_) {
      return false;
    }
  };

  const stripChildTagsFromText = (elm, tagName, text) => {
    let localText = text;
    for (const child of Array.from(elm.getElementsByTagName(tagName))) localText = localText.replaceAll(child.innerText, "");
    return localText;
  };
  const showsSignificantText = (elm) => {
    if (!("innerText" in elm)) return false;
    let currentText = elm.innerText;
    for (const tagName of ["script", "style"]) currentText = stripChildTagsFromText(elm, tagName, currentText);
    const trimmed = currentText.trim();
    if (trimmed.length < minAdTextChars) return false;
    let wordCount = 0;
    for (const word of trimmed.split(" ")) if (word.trim().length) wordCount += 1;
    return wordCount >= minAdTextWords;
  };

  // Обход в том же порядке, что у Brave (сам узел, его потомки, затем следующие
  // за ним соседи — у Brave это рекурсия по firstChild и nextSibling, и
  // верхний узел тоже идёт к соседям), но без рекурсии: длинный список соседей
  // не переполнит стек.
  const isSubTreeFirstParty = (elm) => {
    let foundThirdPartyResource = false;
    const stack = [elm];
    while (stack.length) {
      const node = stack.pop();
      if (node.getAttribute) {
        const id = node.getAttribute("id");
        if (id && (id.startsWith("google_ads_iframe_") || id.startsWith("div-gpt-ad") || id.startsWith("adfox_"))) return false;
        const src = node.getAttribute("src");
        if (src !== null) {
          if (isFirstPartyUrl(src)) return true;
          foundThirdPartyResource = true;
        }
        const style = node.getAttribute("style");
        if (style !== null && (style.includes("url(") || style.includes("//"))) foundThirdPartyResource = true;
        const srcdoc = node.getAttribute("srcdoc");
        if (srcdoc !== null && srcdoc.trim() === "") foundThirdPartyResource = true;
      }
      if (node.nextSibling) stack.push(node.nextSibling);
      if (node.firstChild) stack.push(node.firstChild);
    }
    return !foundThirdPartyResource;
  };

  const pumpCosmeticFilterQueues = () => {
    if (queueIsSleeping) return;
    let didPumpAnything = false;
    for (let queueIndex = 0; queueIndex < queues.length; queueIndex += 1) {
      const currentQueue = queues[queueIndex];
      const nextQueue = queues[queueIndex + 1];
      if (currentQueue.size === 0) continue;
      const currentWorkLoad = Array.from(currentQueue.values()).slice(0, maxWorkSize);
      let matchingElms = [];
      try {
        matchingElms = document.querySelectorAll(currentWorkLoad.join(","));
      } catch (_) {
        // Селектор в листе встал, а в querySelectorAll не разбирается — пачка
        // остаётся спрятанной, как и была.
      }
      const newlyIdentifiedFirstPartySelectors = new Set();
      for (const matchingElm of matchingElms) {
        if (alreadyKnownFirstPartySubtrees.has(matchingElm)) continue;
        if (!(isSubTreeFirstParty(matchingElm) || showsSignificantText(matchingElm))) continue;
        for (const selector of currentWorkLoad) {
          let matches = false;
          try {
            matches = matchingElm.matches(selector);
          } catch (_) {
            // Как выше: такой селектор не снимается.
          }
          if (!matches || alreadyUnhiddenSelectors.has(selector)) continue;
          newlyIdentifiedFirstPartySelectors.add(selector);
          alreadyUnhiddenSelectors.add(selector);
        }
        alreadyKnownFirstPartySubtrees.add(matchingElm);
      }
      for (const selector of newlyIdentifiedFirstPartySelectors) listed.remove(selector);
      for (const usedSelector of currentWorkLoad) {
        currentQueue.delete(usedSelector);
        if (nextQueue && !newlyIdentifiedFirstPartySelectors.has(usedSelector)) nextQueue.add(usedSelector);
      }
      didPumpAnything = true;
      break;
    }
    if (didPumpAnything) {
      queueIsSleeping = true;
      setTimeout(() => {
        queueIsSleeping = false;
        pumpOnIdle();
      }, pumpIntervalMinMs);
    }
  };
  const whenIdle = (fn, timeout) =>
    typeof requestIdleCallback === "function" ? requestIdleCallback(fn, { timeout }) : setTimeout(fn, 0);
  let pumpIdleId;
  const pumpOnIdle = () => {
    if (pumpIdleId !== undefined) return;
    pumpIdleId = whenIdle(() => {
      pumpIdleId = undefined;
      pumpCosmeticFilterQueues();
    }, pumpIntervalMaxMs);
  };
  const schedulePump = () => {
    if (hasDelayOccurred) {
      pumpOnIdle();
      return;
    }
    if (startCheckingId !== undefined) return;
    startCheckingId = whenIdle(() => {
      hasDelayOccurred = true;
      pumpOnIdle();
    }, maxTimeMSBeforeStart);
  };

  /** Селекторы из списков: спрятать, а в обычном режиме — поставить в очередь проверки. */
  const addListed = (selectors) => {
    let queued = false;
    for (const selector of selectors) {
      if (!listed.add(selector) || config.aggressive) continue;
      queues[0].add(selector);
      queued = true;
    }
    attach();
    if (queued) schedulePump();
  };
  addListed(Array.isArray(config.hide) ? config.hide.filter((selector) => typeof selector === "string") : []);

  if (!constructed && !document.documentElement) {
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
        if (!data || data.cmd !== "cosmetic_css" || data.origin !== location.origin) return;
        const strings = (list) => (Array.isArray(list) ? list.filter((selector) => typeof selector === "string") : []);
        addForced(strings(data.force));
        addListed(strings(data.selectors));
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
