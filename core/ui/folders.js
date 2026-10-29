/**
 * UniDTK — desktop folders: one set of operations for the menu and the desk
 * (folders.js)
 *
 * A folder is a kernel record (`get_folders` / `save_folder` / `delete_folder`);
 * its shortcuts point at the real program, file, folder or URL. Every change
 * that loses something offers «Отменить» in a toast instead of asking first.
 */

import { invoke } from "@unidtk/bridge";
import { config, saveConfig, syncWidgets } from "./unidtk-core.js";
import { showToast, showUndo } from "./toast.js";
import { shortcutCount } from "./widgets-api.js";

export { shortcutCount };

export const FOLDER_ICONS = {
  folder: `<svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M22 19a2 2 0 01-2 2H4a2 2 0 01-2-2V5a2 2 0 012-2h5l2 3h9a2 2 0 012 2z"/></svg>`,
  terminal: `<svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><polyline points="4 17 10 11 4 5"/><line x1="12" y1="19" x2="20" y2="19"/></svg>`,
  gamepad: `<svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><line x1="6" y1="12" x2="10" y2="12"/><line x1="8" y1="10" x2="8" y2="14"/><line x1="15" y1="13" x2="15.01" y2="13"/><line x1="18" y1="11" x2="18.01" y2="11"/><rect x="2" y="6" width="20" height="12" rx="6"/></svg>`,
  code: `<svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><polyline points="16 18 22 12 16 6"/><polyline points="8 6 2 12 8 18"/></svg>`,
  globe: `<svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="10"/><line x1="2" y1="12" x2="22" y2="12"/><path d="M12 2a15.3 15.3 0 0 1 4 10 15.3 15.3 0 0 1-4 10 15.3 15.3 0 0 1-4-10 15.3 15.3 0 0 1 4-10z"/></svg>`,
  music: `<svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M9 18V5l12-2v13"/><circle cx="6" cy="18" r="3"/><circle cx="18" cy="16" r="3"/></svg>`,
  tools: `<svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M14.7 6.3a1 1 0 0 0 0 1.4l1.6 1.6a1 1 0 0 0 1.4 0l3.77-3.77a6 6 0 0 1-7.94 7.94l-6.91 6.91a2.12 2.12 0 0 1-3-3l6.91-6.91a6 6 0 0 1 7.94-7.94l-3.76 3.76z"/></svg>`,
  book: `<svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M4 19.5A2.5 2.5 0 0 1 6.5 17H20"/><path d="M6.5 2H20v20H6.5A2.5 2.5 0 0 1 4 19.5v-15A2.5 2.5 0 0 1 6.5 2z"/></svg>`,
  star: `<svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><polygon points="12 2 15.09 8.26 22 9.27 17 14.14 18.18 21.02 12 17.77 5.82 21.02 7 14.14 2 9.27 8.91 8.26 12 2"/></svg>`,
};

export async function listFolders() {
  return (await invoke("get_folders")) || [];
}

async function store(folder) {
  await invoke("save_folder", { folder });
  await syncWidgets();
}

function newItem(draft) {
  return {
    id: `item_${crypto.randomUUID()}`,
    name: draft.name,
    target: draft.target,
    args: draft.args || "",
    kind: draft.kind || "file",
  };
}

/** A new folder, shown on the desk at once. Returns the record. */
export async function createFolder(name, iconKey = "folder") {
  const folder = {
    id: `folder_${crypto.randomUUID()}`,
    name: name.trim() || "Новая папка",
    icon_svg: FOLDER_ICONS[iconKey] || FOLDER_ICONS.folder,
    items: [],
  };
  await store(folder);
  return folder;
}

export async function updateFolder(folder, changes) {
  const next = { ...folder, ...changes };
  await store(next);
  return next;
}

/**
 * Delete at once; «Отменить» brings back the record and everything the desk
 * kept for it — position, hidden flag, placed instances.
 */
export async function deleteFolder(folder, onRestored = null) {
  const saved = {
    position: structuredClone(config.positions?.[folder.id]),
    hidden: (config.hidden || []).includes(folder.id),
    instances: structuredClone((config.instances || []).filter((it) => it.widgetType === folder.id)),
  };
  await invoke("delete_folder", { id: folder.id });
  delete config.positions?.[folder.id];
  config.hidden = (config.hidden || []).filter((h) => h !== folder.id);
  config.instances = (config.instances || []).filter((it) => it.widgetType !== folder.id);
  await saveConfig();
  await syncWidgets();
  showUndo(`Папка «${folder.name}» удалена`, async () => {
    await invoke("save_folder", { folder });
    if (saved.position) config.positions[folder.id] = saved.position;
    if (saved.hidden) config.hidden = [...new Set([...(config.hidden || []), folder.id])];
    config.instances = [...(config.instances || []), ...saved.instances];
    await saveConfig();
    await syncWidgets();
    onRestored?.();
  });
}

/** Append shortcuts (drafts from the kernel: name, target, args, kind). */
export async function addItems(folder, drafts) {
  const fresh = drafts.filter((d) => d && d.target);
  if (!fresh.length) return folder;
  const next = await updateFolder(folder, { items: [...(folder.items || []), ...fresh.map(newItem)] });
  showToast(fresh.length === 1
    ? `«${fresh[0].name}» добавлен в «${folder.name}»`
    : `${shortcutCount(fresh.length)} добавлено в «${folder.name}»`);
  return next;
}

export async function removeItem(folder, itemId, onRestored = null) {
  const index = (folder.items || []).findIndex((it) => it.id === itemId);
  if (index < 0) return folder;
  const removed = folder.items[index];
  const next = await updateFolder(folder, { items: folder.items.filter((it) => it.id !== itemId) });
  showUndo(`Ярлык «${removed.name}» убран`, async () => {
    const current = (await listFolders()).find((f) => f.id === folder.id);
    if (!current) return;
    const items = [...current.items];
    items.splice(Math.min(index, items.length), 0, removed);
    await updateFolder(current, { items });
    onRestored?.();
  });
  return next;
}

export async function renameItem(folder, itemId, name) {
  const clean = name.trim();
  if (!clean) return folder;
  return updateFolder(folder, {
    items: folder.items.map((it) => (it.id === itemId ? { ...it, name: clean } : it)),
  });
}

export async function replaceItemTarget(folder, itemId, draft) {
  return updateFolder(folder, {
    items: folder.items.map((it) => (it.id === itemId ? { ...it, target: draft.target, args: draft.args || "", kind: draft.kind } : it)),
  });
}

export async function moveItem(folder, itemId, toIndex) {
  const items = [...folder.items];
  const from = items.findIndex((it) => it.id === itemId);
  if (from < 0 || from === toIndex) return folder;
  const [item] = items.splice(from, 1);
  items.splice(Math.max(0, Math.min(toIndex, items.length)), 0, item);
  return updateFolder(folder, { items });
}

/** Which of these targets no longer exist on disk (URLs never count as missing). */
export async function missingTargets(items) {
  const targets = (items || []).map((it) => it.target);
  if (!targets.length) return new Set();
  const flags = await invoke("missing_targets", { targets }).catch(() => targets.map(() => false));
  return new Set(targets.filter((_, i) => flags[i]));
}

/** System file/folder dialog → a draft, or null when cancelled. */
export function pickTarget(folder = false) {
  return invoke("pick_shortcut_target", { folder });
}

/**
 * What the clipboard holds as shortcuts: copied files first, otherwise a
 * link or a path typed as text. Names of links come from their domain —
 * there is no way to read the page title, the user renames when needed.
 */
export async function clipboardDrafts() {
  const clip = await invoke("clipboard_shortcut_targets");
  if (clip?.files?.length) {
    return Promise.all(clip.files.map((path) => invoke("describe_shortcut_target", { target: path })));
  }
  const text = String(clip?.text || "").trim();
  if (!text) return [];
  return [await invoke("describe_shortcut_target", { target: text })];
}
