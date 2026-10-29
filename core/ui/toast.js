/**
 * UniDTK — one toast for the whole UI (toast.js)
 *
 * Replaces alert/confirm: a short status line at the bottom of the screen with
 * an optional action. Irreversible actions run at once and offer «Отменить»
 * here for a few seconds instead of asking first.
 */

import { animate } from "./motion.js";

const UNDO_MS = 5000;

let host = null;
let current = null;

function ensureHost() {
  if (host?.isConnected) return host;
  host = document.createElement("div");
  host.className = "toast-host";
  host.setAttribute("role", "status");
  host.setAttribute("aria-live", "polite");
  document.body.appendChild(host);
  return host;
}

function dismiss(entry) {
  if (!entry || entry.closed) return;
  entry.closed = true;
  clearTimeout(entry.timer);
  if (current === entry) current = null;
  animate(entry.el, [
    { opacity: 1, transform: "translateY(0)" },
    { opacity: 0, transform: "translateY(8px)" },
  ], { duration: "transient", easing: "surfaceExit" }).finally(() => entry.el.remove());
}

/**
 * Show a toast; a newer one replaces the older.
 * @param {string} message
 * @param {{ tone?: "info"|"error", action?: string, onAction?: () => unknown, timeout?: number }} [options]
 * @returns {{ dismiss: () => void }}
 */
export function showToast(message, { tone = "info", action = "", onAction = null, timeout = UNDO_MS } = {}) {
  dismiss(current);
  const el = document.createElement("div");
  el.className = `toast toast-${tone}`;
  const text = document.createElement("span");
  text.className = "toast-text";
  text.textContent = message;
  el.appendChild(text);

  const entry = { el, closed: false, timer: 0 };
  if (action && onAction) {
    const btn = document.createElement("button");
    btn.type = "button";
    btn.className = "toast-action";
    btn.textContent = action;
    btn.addEventListener("click", async () => {
      if (entry.closed) return;
      btn.disabled = true;
      dismiss(entry);
      try {
        await onAction();
      } catch (err) {
        showToast(`Не получилось: ${err?.message || err}`, { tone: "error" });
      }
    });
    el.appendChild(btn);
  }

  ensureHost().appendChild(el);
  current = entry;
  animate(el, [
    { opacity: 0, transform: "translateY(8px)" },
    { opacity: 1, transform: "translateY(0)" },
  ], { duration: "transient", easing: "expoOut" });
  // Errors stay a little longer: they are read, not glanced at.
  entry.timer = setTimeout(() => dismiss(entry), tone === "error" ? Math.max(timeout, 7000) : timeout);
  return { dismiss: () => dismiss(entry) };
}

/** An action already done, with five seconds to take it back. */
export function showUndo(message, undo) {
  return showToast(message, { action: "Отменить", onAction: undo });
}
