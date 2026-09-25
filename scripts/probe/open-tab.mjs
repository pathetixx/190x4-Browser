// Открыть вкладку в окне пробы: node scripts/probe/open-tab.mjs <адрес>
import { chromeWindows, connect } from "./cdp.mjs";

const [win] = await chromeWindows();
const chrome = await connect(win);
console.log(await chrome.evaluate(`import('./js/tabs.js').then((tabs) => tabs.open(${JSON.stringify(process.argv[2])}))`));
chrome.close();
