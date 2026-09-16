/**
 * Встроенные страницы на месте нативной вкладки.
 *
 * Страница создаётся, когда её вкладка становится активной, и живёт, пока
 * активна. Переключение на обычную вкладку прячет контейнер: нативная
 * поверхность снова ложится поверх.
 */

import { activeTab, upsertTab } from "../state.js";
import { createDownloadsPage } from "./downloads.js";
import { createHistoryPage } from "./history.js";
import { createSettingsPage } from "./settings.js";

const container = document.getElementById("internal");
let current = null;

export function renderInternal() {
  const tab = activeTab();
  if (!tab?.internal) {
    container.hidden = true;
    return;
  }
  container.hidden = false;

  if (current?.tabId !== tab.id || current?.name !== tab.internal) {
    current?.page.destroy?.();
    container.replaceChildren();
    container.scrollTop = 0;
    const page =
      tab.internal === "settings"
        ? createSettingsPage(container, {
            section: tab.section,
            onSection: (section) =>
              upsertTab(tab.id, { section, url: `190x4://settings${section ? `/${section}` : ""}` }),
          })
        : tab.internal === "history"
          ? createHistoryPage(container)
          : createDownloadsPage(container);
    current = { tabId: tab.id, name: tab.internal, section: tab.section, page };
    return;
  }

  if (tab.section !== current.section) {
    current.section = tab.section;
    current.page.show?.(tab.section);
  }
}
