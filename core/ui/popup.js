/**
 * UniDTK — menu popup (popup.js)
 *
 * The one modal the customization panel uses: packs and folders keep their
 * settings here so the panel stays a short list however many there are.
 * A native <dialog> gives Esc, the focus trap and the top layer.
 */

import { escapeHtml } from "./unidtk-core.js";

/**
 * Open a popup inside `parent`. `render(body)` fills it and runs again on
 * `refresh()`; `onClose` runs once after it is gone.
 */
export function openPopup(parent, title, render, onClose = null) {
  const dialog = document.createElement("dialog");
  dialog.className = "menu-popup";
  dialog.setAttribute("aria-label", title);
  dialog.innerHTML = `
    <header class="menu-popup-head">
      <h3 class="menu-popup-title">${escapeHtml(title)}</h3>
      <button type="button" class="menu-icon-btn" data-popup-close aria-label="Закрыть">
        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" aria-hidden="true"><line x1="18" y1="6" x2="6" y2="18"></line><line x1="6" y1="6" x2="18" y2="18"></line></svg>
      </button>
    </header>
    <div class="menu-popup-body"></div>
  `;
  const body = dialog.querySelector(".menu-popup-body");
  const popup = {
    isOpen: () => dialog.open,
    close: () => dialog.close(),
    refresh: () => render(body),
  };
  dialog.querySelector("[data-popup-close]").addEventListener("click", popup.close);
  // A click on the backdrop lands on the dialog element itself.
  dialog.addEventListener("click", (e) => {
    if (e.target === dialog) popup.close();
  });
  dialog.addEventListener("keydown", (e) => e.stopPropagation());
  dialog.addEventListener("close", () => {
    dialog.remove();
    onClose?.();
  });
  parent.appendChild(dialog);
  dialog.showModal();
  queueMicrotask(popup.refresh);
  return popup;
}
