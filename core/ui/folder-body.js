/**
 * UniDTK — an opened desktop folder (folder-body.js)
 *
 * Shortcuts are edited where they live: a «+» tile adds, right click renames
 * or removes, dragging reorders, Ctrl+V pastes copied files or a link. Right
 * click and dragging only act inside the opened folder; clicks elsewhere on
 * the desk are untouched. A shortcut whose file is gone says so and offers
 * «Найти…» / «Убрать».
 */

import { listen, invoke } from "@unidtk/bridge";
import { escapeHtml, editMode, getShortcutIcon, openExplorer, showDebugMessage, logFrontend } from "./unidtk-core.js";
import { openShortcut } from "./widgets-api.js";
import {
  listFolders, addItems, removeItem, renameItem, replaceItemTarget, moveItem, missingTargets,
  pickTarget, clipboardDrafts,
} from "./folders.js";
import { showToast } from "./toast.js";

const DRAG_START_PX = 6;

async function folderRecord(folderId) {
  return (await listFolders()).find((f) => f.id === folderId) || null;
}

function closeFolderMenu() {
  document.querySelectorAll(".folder-menu").forEach((m) => m.remove());
}

// A small action list at the pointer (shared by «+» and right click).
function showFolderMenu(x, y, actions) {
  closeFolderMenu();
  const menu = document.createElement("div");
  menu.className = "folder-menu";
  menu.setAttribute("role", "menu");
  menu.innerHTML = actions.map((a, i) => `
    <button type="button" role="menuitem" class="folder-menu-item ${a.danger ? "is-danger" : ""}" data-i="${i}">${a.label}</button>
  `).join("");
  document.body.appendChild(menu);
  const rect = menu.getBoundingClientRect();
  menu.style.left = `${Math.max(4, Math.min(x, window.innerWidth - rect.width - 4))}px`;
  menu.style.top = `${Math.max(4, Math.min(y, window.innerHeight - rect.height - 4))}px`;
  menu.addEventListener("click", (e) => {
    const btn = e.target.closest("[data-i]");
    if (!btn) return;
    e.stopPropagation();
    closeFolderMenu();
    Promise.resolve(actions[Number(btn.dataset.i)].run()).catch((err) => {
      showToast(`Не получилось: ${err?.message || err}`, { tone: "error" });
    });
  });
  menu.addEventListener("keydown", (e) => {
    if (e.key === "Escape") {
      e.stopPropagation();
      closeFolderMenu();
    }
  });
  menu.querySelector("button")?.focus();
  // Any press outside dismisses it; capture so the desk handler sees it after.
  setTimeout(() => document.addEventListener("pointerdown", (e) => {
    if (!menu.contains(e.target)) closeFolderMenu();
  }, { capture: true, once: true }));
}

/** Render the shortcuts of an opened folder card into `body`. */
export async function renderFolderBody(card, body) {
  const folderId = card._folderId;
  const shortcuts = card._shortcutItems || [];
  const missing = await missingTargets(shortcuts.map((s) => ({ target: s.path })));

  body.innerHTML = `
    <div class="shortcuts-grid" role="list">
      ${shortcuts.map((s, i) => `
        <button type="button" class="shortcut-item ${missing.has(s.path) ? "is-missing" : ""}" role="listitem" data-index="${i}"
          aria-label="${escapeHtml(s.name)}${missing.has(s.path) ? " — недоступен" : ""}">
          <div class="shortcut-icon ${s.type === "folder" ? "shortcut-folder" : ""}">${getShortcutIcon(s.name, s.type)}</div>
          <span class="shortcut-name">${escapeHtml(s.name)}</span>
        </button>
      `).join("")}
      <button type="button" class="shortcut-item shortcut-add" aria-label="Добавить ярлык">
        <div class="shortcut-icon">
          <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" aria-hidden="true"><path d="M12 5v14"/><path d="M5 12h14"/></svg>
        </div>
        <span class="shortcut-name">${shortcuts.length ? "Добавить" : "Добавить ярлык"}</span>
      </button>
    </div>
    ${shortcuts.length ? "" : `<p class="widget-empty">Пусто. Нажмите «+» или вставьте скопированный файл (Ctrl+V).</p>`}
  `;

  const withRecord = async (fn) => {
    const folder = await folderRecord(folderId);
    if (!folder) throw new Error("папка не найдена");
    return fn(folder);
  };
  const addDrafts = (drafts) => withRecord((folder) => addItems(folder, drafts));
  const itemAt = (i) => shortcuts[i];
  // Item ids are positional on the card; the record is the source of truth.
  const recordItem = (folder, i) => folder.items[i];

  const relink = (i) => withRecord(async (folder) => {
    const item = recordItem(folder, i);
    const draft = await pickTarget(item?.kind === "folder");
    if (draft) await replaceItemTarget(folder, item.id, draft);
  });
  const remove = (i) => withRecord((folder) => removeItem(folder, recordItem(folder, i).id));

  const startRename = (btn, i) => {
    const nameEl = btn.querySelector(".shortcut-name");
    const input = document.createElement("input");
    input.className = "shortcut-rename";
    input.value = itemAt(i).name;
    input.maxLength = 60;
    input.setAttribute("aria-label", "Название ярлыка");
    // Esc here cancels the rename, it must not close the folder.
    input.setAttribute("data-prevent-escape-surface-close", "");
    nameEl.replaceWith(input);
    input.focus();
    input.select();
    let done = false;
    const finish = (commit) => {
      if (done) return;
      done = true;
      input.replaceWith(nameEl);
      const name = input.value.trim();
      if (commit && name && name !== itemAt(i).name) {
        withRecord((folder) => renameItem(folder, recordItem(folder, i).id, name)).catch((err) => {
          showToast(`Не переименовано: ${err?.message || err}`, { tone: "error" });
        });
      }
    };
    input.addEventListener("keydown", (e) => {
      e.stopPropagation();
      if (e.key === "Enter") finish(true);
      if (e.key === "Escape") finish(false);
    });
    input.addEventListener("blur", () => finish(true));
    input.addEventListener("click", (e) => e.stopPropagation());
  };

  const addMenu = (x, y) => showFolderMenu(x, y, [
    { label: "Файл или программа…", run: async () => { const d = await pickTarget(false); if (d) await addDrafts([d]); } },
    { label: "Папка…", run: async () => { const d = await pickTarget(true); if (d) await addDrafts([d]); } },
    { label: "Вставить скопированное", run: () => pasteInto() },
    { label: "Из меню Пуск…", run: () => import("./menu.js").then((m) => m.openFoldersFor(folderId)) },
  ]);

  const pasteInto = async () => {
    const drafts = await clipboardDrafts();
    if (!drafts.length) {
      showToast("В буфере обмена нет ни файлов, ни ссылки");
      return;
    }
    await addDrafts(drafts);
  };
  card._pasteIntoFolder = pasteInto;

  body.querySelector(".shortcut-add").addEventListener("click", (e) => {
    e.stopPropagation();
    if (editMode) return;
    const r = e.currentTarget.getBoundingClientRect();
    addMenu(r.left, r.bottom + 4);
  });

  body.querySelectorAll(".shortcut-item[data-index]").forEach((btn) => {
    const i = Number(btn.dataset.index);
    let dragged = false;

    btn.addEventListener("click", (e) => {
      e.stopPropagation();
      e.preventDefault();
      if (editMode || dragged) return;
      const s = itemAt(i);
      if (btn.classList.contains("is-missing")) {
        const r = btn.getBoundingClientRect();
        showFolderMenu(r.left, r.bottom + 4, [
          { label: "Найти…", run: () => relink(i) },
          { label: "Убрать", danger: true, run: () => remove(i) },
        ]);
        return;
      }
      const type = (s.type || "").toLowerCase();
      if (s.name.toLowerCase().trim() === "explorer") {
        openExplorer(null, btn);
        return;
      }
      if (type === "folder") {
        openExplorer(s.path, btn);
        return;
      }
      openShortcut(s.path, { type, name: s.name, args: s.args }).catch((err) => {
        showDebugMessage(`open error: ${err}`);
        showToast(`Не открылось: ${err?.message || err}`, { tone: "error" });
      });
    });

    btn.addEventListener("contextmenu", (e) => {
      e.preventDefault();
      e.stopPropagation();
      if (editMode) return;
      const actions = [
        { label: "Переименовать", run: () => startRename(btn, i) },
        { label: "Убрать", danger: true, run: () => remove(i) },
      ];
      if (btn.classList.contains("is-missing")) actions.unshift({ label: "Найти…", run: () => relink(i) });
      showFolderMenu(e.clientX, e.clientY, actions);
    });

    // Reorder: pointer events with capture (input reaches this window through
    // PostMessage, without mouse capture HTML5 drag may never start).
    btn.addEventListener("pointerdown", (e) => {
      if (e.button !== 0 || editMode) return;
      dragged = false;
      const startX = e.clientX;
      const startY = e.clientY;
      const onMove = (ev) => {
        if (!dragged && Math.hypot(ev.clientX - startX, ev.clientY - startY) < DRAG_START_PX) return;
        if (!dragged) {
          dragged = true;
          btn.setPointerCapture(e.pointerId);
          btn.classList.add("is-dragging");
        }
        btn.style.transform = `translate(${ev.clientX - startX}px, ${ev.clientY - startY}px)`;
      };
      const onUp = (ev) => {
        btn.removeEventListener("pointermove", onMove);
        btn.removeEventListener("pointerup", onUp);
        btn.removeEventListener("pointercancel", onUp);
        if (!dragged) return;
        btn.style.transform = "";
        btn.classList.remove("is-dragging");
        const target = [...body.querySelectorAll(".shortcut-item[data-index]")].find((other) => {
          const r = other.getBoundingClientRect();
          return other !== btn && ev.clientX >= r.left && ev.clientX <= r.right && ev.clientY >= r.top && ev.clientY <= r.bottom;
        });
        if (target && ev.type === "pointerup") {
          withRecord((folder) => moveItem(folder, recordItem(folder, i).id, Number(target.dataset.index))).catch((err) => {
            showToast(`Порядок не сохранён: ${err?.message || err}`, { tone: "error" });
          });
        }
        // The click that follows a drag must not open the shortcut.
        setTimeout(() => { dragged = false; });
      };
      btn.addEventListener("pointermove", onMove);
      btn.addEventListener("pointerup", onUp);
      btn.addEventListener("pointercancel", onUp);
    });
  });
}

// Ctrl+V pastes into the opened folder. Ctrl+V chains reach this window the
// same way they reach the explorer; an input keeps its own paste.
document.addEventListener("keydown", (e) => {
  if (!e.ctrlKey || e.code !== "KeyV" || e.altKey || e.shiftKey) return;
  if (e.target.closest?.("input, textarea, [contenteditable]")) return;
  const card = document.querySelector(".widget-card.expanded");
  if (!card?._pasteIntoFolder) return;
  e.preventDefault();
  card._pasteIntoFolder().catch((err) => showToast(`Не вставлено: ${err?.message || err}`, { tone: "error" }));
});

// Files dropped from Explorer onto a folder (opened or not) become its
// shortcuts. Tauri reports the drop in physical pixels.
listen("tauri://drag-drop", ({ payload }) => {
  const paths = payload?.paths || [];
  const dpr = window.devicePixelRatio || 1;
  const x = (payload?.position?.x ?? -1) / dpr;
  const y = (payload?.position?.y ?? -1) / dpr;
  const card = document.elementFromPoint(x, y)?.closest(".widget-card");
  logFrontend(`folder drop: ${paths.length} path(s) at ${Math.round(x)},${Math.round(y)} on ${card?.dataset.id || "desk"}`);
  if (!card?._folderId || !paths.length || editMode) return;
  Promise.all(paths.map((target) => invoke("describe_shortcut_target", { target })))
    .then(async (drafts) => {
      const folder = await folderRecord(card._folderId);
      if (folder) await addItems(folder, drafts);
    })
    .catch((err) => showToast(`Не добавлено: ${err?.message || err}`, { tone: "error" }));
}).catch(() => {});
