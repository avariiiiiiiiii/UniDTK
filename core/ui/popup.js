/**
 * UniDTK — menu settings sheet (popup.js)
 *
 * The settings of one widget or folder open over the menu itself, in the
 * panel's place, so the desk stays uncovered. It is a surface like every other
 * one: surfaceManager morphs it out of the row that opened it, stacks it for
 * Esc and outside clicks, and closes it the same way.
 */

import { escapeHtml } from "./unidtk-core.js";
import { surfaceManager } from "./surface-manager.js";

/**
 * Open a sheet inside `panel`, grown out of `origin`. `render(body)` fills it
 * before it opens, so it never appears empty, and runs again on `refresh()`.
 * `onClose` runs once after the sheet is gone.
 */
export async function openSheet(panel, origin, title, render, onClose = null) {
  const sheet = document.createElement("section");
  sheet.className = "menu-sheet";
  sheet.setAttribute("aria-label", title);
  sheet.innerHTML = `
    <header class="menu-sheet-head">
      <h3 class="menu-sheet-title">${escapeHtml(title)}</h3>
      <button type="button" class="menu-icon-btn" data-sheet-close aria-label="Закрыть">
        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" aria-hidden="true"><line x1="18" y1="6" x2="6" y2="18"></line><line x1="6" y1="6" x2="18" y2="18"></line></svg>
      </button>
    </header>
    <div class="menu-sheet-body"></div>
  `;
  const body = sheet.querySelector(".menu-sheet-body");
  const sheetApi = {
    isOpen: () => Boolean(surfaceManager.getRecord(sheet)),
    close: () => surfaceManager.closeSurface(sheet),
    refresh: () => render(body),
  };
  sheet.querySelector("[data-sheet-close]").addEventListener("click", sheetApi.close);
  panel.appendChild(sheet);
  await render(body);
  surfaceManager.openSurface(sheet, {
    origin,
    display: "flex",
    onOpened: () => sheet.querySelector("button, input")?.focus(),
    onClosed: () => {
      sheet.remove();
      onClose?.();
    },
  });
  return sheetApi;
}
