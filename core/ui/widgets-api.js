/**
 * widgets-api.js — widget discovery, shortcuts, and config management.
 */

import {
  invoke,
  appDataDir,
  resolveResource,
  BaseDirectory,
  mkdir,
  readDir,
  readTextFile,
  writeTextFile,
  openPath,
} from "@unidtk/bridge";

function flog(msg) {
  console.log(msg);
  invoke("frontend_log", { message: msg }).catch(() => {});
  try { window.__wallpaper_log?.(msg); } catch {}
}

async function ensureAppData() {
  await mkdir(await appDataDir(), { recursive: true });
}

async function readResourceText(path) {
  try {
    return await readTextFile(path, { baseDir: BaseDirectory.Resource });
  } catch {}
  try {
    return await readTextFile(await resolveResource(path));
  } catch {}
  return null;
}

async function readResourceDir(path) {
  try {
    return await readDir(path, { baseDir: BaseDirectory.Resource });
  } catch {}
  try {
    return await readDir(await resolveResource(path));
  } catch {}
  return null;
}

async function resourcePath(path) {
  try {
    return await resolveResource(path);
  } catch {
    return path.replaceAll("\\", "/");
  }
}

// Fallback for when the resource directory cannot be read at all. It is a list
// of ids, and every id here is also a folder under `modules/`, so an entry is
// cheap and a missing one costs the whole widget its presence in the catalogue.
const KNOWN_MODULES = [
  "clock", "glyph-bar", "telegram", "mp3-player", "vpn", "micrographics",
];

// ─── One-time widget id migration: `yt-music` → `mp3-player` ──────────────────
// The player module was renamed to `mp3-player` (id === `specialType` like every
// other special widget). Existing redb layouts still key it as `yt-music`, which
// would drop the widget back to its default position, so remap it in place.
// Returns true only when something actually changed, keeping this idempotent and
// making sure a save is emitted only when there is something to persist.
function migrateWidgetConfig(config) {
  if (!config || typeof config !== "object") return false;
  let changed = false;

  const positions = config.positions;
  if (positions && typeof positions === "object") {
    if (Object.prototype.hasOwnProperty.call(positions, "yt-music")) {
      if (!Object.prototype.hasOwnProperty.call(positions, "mp3-player")) {
        positions["mp3-player"] = positions["yt-music"];
      }
      delete positions["yt-music"];
      changed = true;
    }
    // Satellites were removed in an earlier session; drop their stale keys.
    for (const stale of ["yt-music-playlists", "yt-music-menu"]) {
      if (Object.prototype.hasOwnProperty.call(positions, stale)) {
        delete positions[stale];
        changed = true;
      }
    }
  }

  if (Array.isArray(config.hidden)) {
    const idx = config.hidden.indexOf("yt-music");
    if (idx !== -1) {
      config.hidden[idx] = "mp3-player";
      changed = true;
    }
  }

  return changed;
}

export async function getWidgets() {
  const moduleEntries = await readResourceDir("modules");
  const moduleIds = moduleEntries
    ? moduleEntries.filter((e) => e.isDirectory && e.name !== "explorer").map((e) => e.name)
    : KNOWN_MODULES;

  flog(`[bridge] moduleIds: ${JSON.stringify(moduleIds)}`);

  const modulePromises = moduleIds.map(async (id) => {
    const text = await readResourceText(`modules/${id}/metadata.json`);
    if (!text) return null;
    try {
      const meta = JSON.parse(text);
      if (!meta) return null;
      let iconSvg = "";
      try { iconSvg = await readResourceText(`modules/${id}/icon.svg`) || ""; } catch {}
      return { id, name: meta.name || id, description: meta.description || "", iconSvg, isFolder: false, ...meta };
    } catch { return null; }
  });

  // User-created folders exclusively from redb
  let userFolders = [];
  try {
    userFolders = await invoke("get_folders");
  } catch (e) {
    flog(`[bridge] get_folders error: ${e?.message || e}`);
  }

  const folderWidgets = (userFolders || []).map((f) => ({
    id: f.id,
    name: f.name,
    description: `${(f.items || []).length} ярлыков`,
    iconSvg: f.icon_svg || `<svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5"><path d="M22 19a2 2 0 01-2 2H4a2 2 0 01-2-2V5a2 2 0 012-2h5l2 3h9a2 2 0 012 2z"/></svg>`,
    isFolder: true,
    specialType: "",
    items: f.items || [],
  }));

  const modules = (await Promise.all(modulePromises)).filter(Boolean);
  const widgets = [...modules, ...folderWidgets];
  flog(`[bridge] loaded widgets: ${widgets.length} (modules: ${modules.length}, folders: ${folderWidgets.length})`);
  return widgets;
}

export async function getWidgetShortcuts(widgetId) {
  try {
    const folders = await invoke("get_folders");
    const found = (folders || []).find((f) => f.id === widgetId);
    if (found && Array.isArray(found.items)) {
      return found.items.map((item) => ({
        name: item.name,
        filename: item.name,
        path: item.target,
        type: item.kind || "file",
      }));
    }
  } catch (e) {
    console.warn("get_widget_shortcuts invoke get_folders failed:", e);
  }
  return [];
}

function buildDefaultWidgetConfig() {
  return {
    order: ["clock", "mp3-player", "telegram", "glyph-bar"],
    align: "left",
    positions: {
      "clock": { x: 28, y: 28, w: 340, h: 64 },
      "mp3-player": { x: 28, y: 116, w: 340, h: 64 },
      "telegram": { x: 28, y: 204, w: 340, h: 64 },
      "glyph-bar": { x: 28, y: 292, w: 340, h: 64 },
    },
    hidden: [],
  };
}

export async function getWidgetConfig() {
  let backendReachable = false;
  try {
    const config = await invoke("get_widget_config");
    backendReachable = true;
    if (config && typeof config === "object" && config.positions && Object.keys(config.positions).length > 0) {
      window.__shellConfigSource = "redb";
      if (migrateWidgetConfig(config)) {
        await saveWidgetConfig(config).catch(() => {});
      }
      return config;
    }
    // Layout blob without positions: start from defaults but PRESERVE the
    // theme state (theme + customPresets + userColors) — an empty layout must never wipe
    // the user's saved appearance from redb.
    if (config && typeof config === "object" && (config.theme || config.customPresets || config.userColors)) {
      const defaultConfig = buildDefaultWidgetConfig();
      if (config.theme && typeof config.theme === "object") defaultConfig.theme = config.theme;
      if (Array.isArray(config.customPresets)) defaultConfig.customPresets = config.customPresets;
      if (config.userColors && typeof config.userColors === "object") defaultConfig.userColors = config.userColors;
      window.__shellConfigSource = "redb";
      await saveWidgetConfig(defaultConfig).catch(() => {});
      return defaultConfig;
    }
  } catch (e) {
    console.warn("get_widget_config invoke failed:", e);
  }

  const defaultConfig = buildDefaultWidgetConfig();

  if (backendReachable) {
    window.__shellConfigSource = "redb";
    await saveWidgetConfig(defaultConfig).catch(() => {});
    return defaultConfig;
  }

  window.__shellConfigSource = "defaults";
  return defaultConfig;
}

export async function saveWidgetConfig(config) {
  if (window.__shellConfigSource && window.__shellConfigSource !== "redb") {
    console.warn(`save_widget_config skipped (config source: ${window.__shellConfigSource})`);
    return;
  }
  const hasPositions = config && typeof config === "object" && config.positions
    && Object.keys(config.positions).length > 0;
  const hasSettings = config && typeof config === "object" && config.settings
    && Object.keys(config.settings).length > 0;
  const hasTheme = config && typeof config === "object" && Boolean(config.theme || config.customPresets || config.userColors);
  if (!hasPositions && !hasSettings && !hasTheme) {
    return;
  }
  try {
    await invoke("save_widget_config", { config });
  } catch (e) {
    console.error("save_widget_config invoke failed:", e);
  }
}

// ─── Disabled modules (backend off) ──────────────────────────────────────────
// Stored as its own key in redb, independent of the widget layout blob. A
// disabled module still keeps its (light) managed shell so Tauri commands do
// not panic, but it skips the heavy background when it starts.

export async function getDisabledModules() {
  try {
    const list = await invoke("get_disabled_modules");
    return Array.isArray(list) ? list : [];
  } catch (e) {
    console.warn("get_disabled_modules failed:", e);
    return [];
  }
}

export async function setDisabledModules(ids) {
  try {
    await invoke("set_disabled_modules", { ids });
    return true;
  } catch (e) {
    console.error("set_disabled_modules failed:", e);
    return false;
  }
}

// ─── Shortcut opening ────────────────────────────────────────────────────────

export async function openShortcut(shortcutPath, meta = {}) {
  const p = String(shortcutPath || "");
  const base = p.replace(/\\/g, "/").split("/").pop() || "";
  const name = base.replace(/\.[^.]+$/, "").toLowerCase();
  const type = String(meta.type || "").toLowerCase();

  const openInExplorer = (folder = "") =>
    invoke("open_explorer", { path: folder });

  if (name === "explorer") {
    await openInExplorer("");
    return;
  }

  if (type === "folder") {
    await openInExplorer(p);
    return;
  }

  const recycleNames = new Set(["recycle bin", "корзина", "$recycle.bin"]);
  if (recycleNames.has(name) || /\$recycle\.bin$/i.test(p.replace(/\\/g, "/"))) {
    await openInExplorer(p);
    return;
  }

  if (type === "url" || p.startsWith("http://") || p.startsWith("https://")) {
    return openPath(p);
  }

  try {
    if (await invoke("path_is_directory", { path: p })) {
      await openInExplorer(p);
      return;
    }
  } catch {
    try {
      if ((await invoke("get_file_info", { path: p }))?.is_dir) {
        await openInExplorer(p);
        return;
      }
    } catch {}
  }

  return openPath(p);
}
