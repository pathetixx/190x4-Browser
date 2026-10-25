/** Встроенные адреса проверяются по origin и пути, а не по тексту URL. */
export function isNewTabUrl(value) {
  if (!value || value === "about:blank" || value === "about:newtab") return true;
  try {
    const url = new URL(value);
    return url.origin === "http://190x4-pages.invalid" && url.pathname === "/newtab.html";
  } catch {
    return false;
  }
}
