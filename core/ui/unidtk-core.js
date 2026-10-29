import { convertFileSrc, invoke, listen, openPath } from "@unidtk/bridge";
import { getWidgets, getWidgetShortcuts, getWidgetConfig, saveWidgetConfig } from "./widgets-api.js";
import { initMotionSystem, animate, spring, staggerEnter, fadeTransition, setMotionScale, getDuration, EASINGS, DURATIONS, motionTimeline } from "./motion.js";
import { initThemeSystem, applyTheme, getActiveTheme, getAllPresets, syncThemeIntoConfig, resolvePresetSize } from "./theme.js";
import { surfaceManager, syncSurfaceWash } from "./surface-manager.js";
import { initMenu, openMenu, closeMenu } from "./menu.js";
import { renderFolderBody } from "./folder-body.js";

initMotionSystem();
initThemeSystem();
initMenu();

function rlog(msg) { console.log(msg); invoke("frontend_log", { message: msg }).catch(()=>{}); try{ window.__wallpaper_log?.(msg);}catch{} }
rlog("[renderer] module loaded");

// Suppress default WebView2 browser context menu globally across the whole application
window.addEventListener("contextmenu", (e) => {
  e.preventDefault();
}, { capture: true });

// в”Ђв”Ђв”Ђ Icon system: name-based в†’ type-based fallback в”Ђв”Ђв”Ђ



const widgetsCanvas = document.getElementById("widgets");

// в”Ђв”Ђв”Ђ State в”Ђв”Ђв”Ђ
let editMode = false;
let config = { order: [], align: "left" };
let cachedWidgetTypes = [];
let lastBands = null;
let lastBandsAt = 0;

let cachedTelegramCard = null;
let cachedClockDigitals = [];
let cachedGothicDates = [];
let cachedClockStackeds = [];
let cachedClockRings = [];
let cachedGridSeconds = [];
let cachedNothingWatchPros = [];
let cachedClockAnalogues = [];



function refreshDomCaches() {
  cachedTelegramCard = document.querySelector(".telegram-widget")
    || document.querySelector('[data-widget="telegram"]')
    || document.querySelector('[data-special-type="telegram"]')
    || document.querySelector('.widget-card[data-id="telegram"]');
  cachedClockDigitals = document.querySelectorAll(".clock-digital");
  cachedGothicDates = document.querySelectorAll(".gothic-date");
  cachedClockStackeds = document.querySelectorAll(".clock-digital-stacked");
  cachedClockRings = document.querySelectorAll(".clock-rings");
  cachedGridSeconds = document.querySelectorAll(".grid-second");
  cachedNothingWatchPros = document.querySelectorAll(".nothing-watch-pro");
  cachedClockAnalogues = document.querySelectorAll(".clock-analogue");
}



function gothicWidgetFrame() {
  return "";
}

// Audio bands / arc-visualization helpers live in the mp3-player audio-arcs
// module. core.js only touches them through lazy delegators so top-level
// listeners can be wired synchronously without statically importing the module
// (circular import safety — audio-arcs.js imports { lastBands, lastBandsAt }
// back from core.js).
function normalizeBands(payload) {
  return import("../modules/mp3-player/audio-arcs.js").then((m) => m.normalizeBands(payload)).catch(() => ({
    bass: 0, mid: 0, treble: 0,
  }));
}
function wakeAudioCircleLoop() {
  import("../modules/mp3-player/audio-arcs.js").then((m) => m.wakeAudioCircleLoop()).catch(() => {});
}

listen("audio-bands", (event) => {
  normalizeBands(event.payload).then((bands) => {
    lastBands = bands;
    lastBandsAt = performance.now();
  });
  wakeAudioCircleLoop();
}).catch(() => {});
document.addEventListener("visibilitychange", () => {
  wakeAudioCircleLoop();
});

// Background playlist auto-sync (and manual sync) finishes → drop track cache so
// covers / playlist opens see newly linked downloads without a full reload.
listen("sync-tasks", (event) => {
  const tasks = event.payload;
  if (!Array.isArray(tasks)) return;
  const finished = tasks.some((t) => t?.status === "done" || t?.status === "error");
  if (!finished) return;
  musicTrackCache = null;
  musicTrackCachePromise = null;
  musicTrackCacheMissRetried = false;
  const expandedLibrary = document.querySelector(".music-library-popup.expanded");
  if (expandedLibrary) {
    const body = expandedLibrary.querySelector(".music-popup-body");
    if (body) {
      import("../modules/mp3-player/widget.js").then((m) => m.renderMusicDashboard(body)).catch(() => {});
    }
  }
}).catch(() => {});

// в”Ђв”Ђв”Ђ Load config from file в”Ђв”Ђв”Ђ



async function loadConfig() {
  config = await getWidgetConfig().catch(() => ({ order: [], align: "left" }));
  initThemeSystem(config);
  // Telegram dock always starts collapsed: `expanded` is runtime-only state and
  // must not be restored from disk (shell startup opened the full chat panel
  // whenever the app had quit with the widget open).
  for (const pos of Object.values(config.positions || {})) {
    if (pos && typeof pos === "object" && "expanded" in pos) pos.expanded = false;
  }
  // The Telegram dock used to be snapped to the right edge at draw time (x past
  // 1700 px or within 50 px of the edge, 12 px margin). Same place, now as data.
  const dock = config.positions?.telegram;
  const canvasW = window.innerWidth || window.screen.width || 1920;
  if (dock && !dock.anchor && canvasW >= 1200) {
    const maxX = canvasW - (Number(dock.w) || 56) - 12;
    if (Number(dock.x) >= 1700 || Number(dock.x) >= maxX - 50) {
      Object.assign(dock, { anchor: "tr", ax: 12, ay: Math.max(0, Number(dock.y) || 0) });
    }
  }
}

async function saveConfig() {
  // Mirror the live theme into the shared config first so widget-layout
  // writes can never serialize a stale theme payload (single redb writer
  // object for both theme.js and core saves).
  syncThemeIntoConfig();
  const ok = await saveWidgetConfig(config);
  if (!ok) showDebugMessage("Настройки не сохранены — база недоступна, изменения пропадут после перезапуска");
  return ok;
}



function defaultWidgetPosition(widgetType, fallbackY = 68) {
  const sizes = {
    clock: { w: 210, h: 180 },
    "mp3-player": { w: 210, h: 220 },
    // 10 square LEDs + 9 gaps: side 10 → h = 10*10 + 3*9 = 127
    "glyph-bar": { w: 10, h: 127 },
    telegram: { w: 56, h: 520 },
  };
  const size = sizes[widgetType] || { w: 340, h: 64 };
  const canvasW = widgetsCanvas.clientWidth || window.innerWidth || 800;
  return {
    x: Math.max(0, canvasW - size.w - 28),
    y: Math.max(28, fallbackY),
    ...size,
  };
}



const ANCHORS = ["tl", "tr", "bl", "br"];

/**
 * Where a saved position lands on the current canvas. A position pinned to a
 * corner (`anchor` ∈ tl/tr/bl/br, `ax`/`ay` — distance from that corner) keeps
 * its distance to the corner when the screen size changes; a record without
 * an anchor is a plain top-left x/y, exactly as before anchors existed. Pure:
 * transient canvas sizes (sleep/wake, display switches) never reach the data.
 */
function placeOnCanvas(pos, card = null) {
  const x = Number(pos?.x) || 0;
  const y = Number(pos?.y) || 0;
  const anchor = pos?.anchor;
  if (!ANCHORS.includes(anchor)) return { x, y };
  const canvasW = widgetsCanvas.clientWidth || window.innerWidth || 1920;
  const canvasH = widgetsCanvas.clientHeight || window.innerHeight || 1080;
  const w = Number(pos.w) || card?.offsetWidth || 0;
  const h = Number(pos.h) || card?.offsetHeight || 0;
  const ax = Number(pos.ax) || 0;
  const ay = Number(pos.ay) || 0;
  return {
    x: anchor[1] === "r" ? Math.max(0, canvasW - w - ax) : ax,
    y: anchor[0] === "b" ? Math.max(0, canvasH - h - ay) : ay,
  };
}

// Pin a laid-out card to its nearest screen corner. Only «Готово» calls this:
// the canvas is the real one then, never a size reported mid-sleep.
function pinToNearestCorner(pos, card) {
  const canvasW = widgetsCanvas.clientWidth || window.innerWidth;
  const canvasH = widgetsCanvas.clientHeight || window.innerHeight;
  const x = parseInt(card.style.left, 10) || 0;
  const y = parseInt(card.style.top, 10) || 0;
  const w = Number(pos.w) || card.offsetWidth;
  const h = Number(pos.h) || card.offsetHeight;
  const right = x + w / 2 > canvasW / 2;
  const bottom = y + h / 2 > canvasH / 2;
  pos.x = x;
  pos.y = y;
  pos.anchor = `${bottom ? "b" : "t"}${right ? "r" : "l"}`;
  pos.ax = right ? Math.round(canvasW - x - w) : x;
  pos.ay = bottom ? Math.round(canvasH - y - h) : y;
}



function closeWidgetAddPopover() {
  widgetsCanvas.querySelector(".widget-add-popover")?.classList.remove("open");
}

/**
 * Whether a widget of this type is already on the desktop — either as its base
 * card or as a placed instance. Used to refuse a second copy of a singleton.
 */
function alreadyPlaced(widgetType) {
  if (Array.isArray(config.hidden) && config.hidden.includes(widgetType)) return false;
  if (Array.isArray(config.instances) && config.instances.some((i) => i.widgetType === widgetType)) {
    return true;
  }
  return !!widgetsCanvas.querySelector(`.widget-card[data-widget-type="${widgetType}"]`);
}



async function renderWidgetAddPopover(popover) {
  let types = await getWidgets().catch(() => cachedWidgetTypes);
  if (!types || !types.length) types = cachedWidgetTypes;
  cachedWidgetTypes = types || [];

  popover.innerHTML = `
    <div class="widget-add-list">
      ${cachedWidgetTypes.map((w) => `
        <button type="button" class="widget-add-option" data-widget-type="${w.id}">
          <span class="widget-add-option-icon">${w.iconSvg || `
            <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
              <rect x="4" y="4" width="16" height="16" rx="4" />
              <path d="M12 8v8" /><path d="M8 12h8" />
            </svg>
          `}</span>
          <span class="widget-add-option-text">
            <span class="widget-add-option-name">${w.name || w.id}</span>
            <span class="widget-add-option-desc">${w.description || w.id}</span>
          </span>
        </button>
      `).join("")}
    </div>
  `;

  popover.querySelectorAll(".widget-add-option").forEach((btn) => {
    btn.addEventListener("click", async (e) => {
      e.stopPropagation();
      if (!editMode) return;
      closeWidgetAddPopover();
      await createWidgetInstance(btn.dataset.widgetType);
    });
  });

  // A singleton widget is one object on the desktop, not a thing you add twice:
  // a second instance would claim a derived rect of its own while the first one
  // kept painting the same free space, and neither would be able to tell which is
  // the real panel.
  popover.querySelectorAll(".widget-add-option").forEach((btn) => {
    const type = cachedWidgetTypes.find((w) => w.id === btn.dataset.widgetType);
    if (type?.singleton && alreadyPlaced(type.id)) btn.disabled = true;
  });
}

function ensureWidgetAddControls(widgetTypes) {
  cachedWidgetTypes = widgetTypes || cachedWidgetTypes;

  const controls = document.createElement("div");
  controls.className = "widget-add";
  controls.innerHTML = `
    <button type="button" class="widget-add-btn" aria-label="Add widget">
      <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.3" stroke-linecap="round" stroke-linejoin="round">
        <path d="M12 5v14" /><path d="M5 12h14" />
      </svg>
    </button>
    <div class="widget-add-popover"></div>
  `;

  const btn = controls.querySelector(".widget-add-btn");
  const popover = controls.querySelector(".widget-add-popover");
  btn.addEventListener("click", async (e) => {
    e.stopPropagation();
    if (!editMode) return;
    const willOpen = !popover.classList.contains("open");
    closeWidgetAddPopover();
    if (willOpen) {
      await renderWidgetAddPopover(popover);
      popover.classList.add("open");
    }
  });

  widgetsCanvas.appendChild(controls);
}



async function createWidgetInstance(widgetType) {
  if (!widgetType) return;
  if (!config.instances) config.instances = [];
  if (!config.positions) config.positions = {};
  if (!Array.isArray(config.hidden)) config.hidden = [];

  // Если базовый виджет этого типа был скрыт — возвращаем его, а не плодим инстанс.
  if (config.hidden.includes(widgetType)) {
    config.hidden = config.hidden.filter((id) => id !== widgetType);
    await saveConfig();
    await syncWidgets();
    return;
  }

  // Second line of the same guard the add-popover draws from: a singleton has no
  // meaningful second copy, and this path is also reachable without the popover.
  const meta = cachedWidgetTypes.find((w) => w.id === widgetType);
  if (meta?.singleton && alreadyPlaced(widgetType)) return;

  const instanceId = `${widgetType}-${Date.now()}`;
  config.instances = [{ instanceId, widgetType }, ...config.instances];
  config.positions[instanceId] = defaultWidgetPosition(widgetType);
  await saveConfig();
  await syncWidgets();
}

async function removeWidgetInstance(instanceId) {
  if (!instanceId || !config.instances) return;
  config.instances = config.instances.filter((item) => item.instanceId !== instanceId);
  if (config.positions) {
    delete config.positions[instanceId];
    delete config.positions[`${instanceId}-playlists`];
    delete config.positions[`${instanceId}-menu`];
  }
  await saveConfig();
  await syncWidgets();
}

async function hideBaseWidget(widgetId) {
  if (!widgetId) return;
  if (!Array.isArray(config.hidden)) config.hidden = [];
  if (!config.hidden.includes(widgetId)) config.hidden.push(widgetId);
  await saveConfig();
  await syncWidgets();
}

// в”Ђв”Ђв”Ђ Load widgets в”Ђв”Ђв”Ђ

// Widget list the desktop should show: modules + folders from the backend,
// with special types pinned for the built-ins. Null when nothing came back.
async function fetchRenderableWidgets() {
  let widgets = await getWidgets().catch((err) => {
    const msg = `getWidgets error: ${err?.message || err}`;
    showDebugMessage(msg);
    invoke("frontend_log", { message: msg }).catch(() => {});
    return [];
  });

  rlog(`[renderer] getWidgets returned: ${widgets?.length} widgets`);
  invoke("frontend_log", { message: `loadWidgets: loaded ${widgets?.length || 0} widgets` }).catch(() => {});

  if (!widgets || !widgets.length) {
    rlog("[renderer] no widgets, returning early");
    return null;
  }

  // Special widgets must never degrade into the generic shortcuts view if an
  // older packaged metadata reader omits `specialType`.
  const builtinSpecialTypes = {
    telegram: "telegram",
    "glyph-bar": "glyph-bar",
    "mp3-player": "mp3-player",
    micrographics: "micrographics",
  };
  widgets = widgets.map((widget) => ({
    ...widget,
    specialType: widget.specialType || builtinSpecialTypes[widget.id] || "",
  }));
  return widgets;
}

// Base widgets minus hidden types, plus the configured instances.
function desiredWidgets(widgets) {
  if (!Array.isArray(config.hidden)) config.hidden = [];
  if (!config.instances) config.instances = [];
  const widgetByType = new Map(widgets.map((w) => [w.id, w]));
  const hiddenSet = new Set(config.hidden);
  const renderWidgets = [
    ...widgets
      .filter((w) => !hiddenSet.has(w.id))
      .map((w) => ({ ...w, widgetType: w.id, isInstance: false })),
    ...config.instances
      .filter((instance) => !hiddenSet.has(instance.widgetType))
      .map((instance) => {
        const base = widgetByType.get(instance.widgetType);
        if (!base) return null;
        return {
          ...base,
          id: instance.instanceId,
          widgetType: instance.widgetType,
          isInstance: true,
        };
      })
      .filter(Boolean),
  ];
  return renderWidgets;
}

// One desktop card, fully wired; `layout.offsetY` stacks cards that have no
// stored position yet.
function buildWidgetCard(w, layout) {
    const card = document.createElement("div");
    card.className = "widget-card";
    card.dataset.id = w.id;
    card.dataset.widgetType = w.widgetType || w.id;

    const isMicrographics = w.specialType === "micrographics";
    if (isMicrographics) card.classList.add("micrographics-card");
    if (!config.positions[w.id]) {
      config.positions[w.id] = w.isInstance
        ? defaultWidgetPosition(w.widgetType || w.id, layout.offsetY)
        : { x: 28, y: layout.offsetY };
      // Preset geometry defaults win over the hardcoded fallback sizes
      // (user data never exists yet at this point, so nothing to preserve).
      const presetSize = resolvePresetSize(w.widgetType || w.id, Boolean(w.isFolder));
      if (presetSize) {
        config.positions[w.id].w = presetSize.w;
        config.positions[w.id].h = presetSize.h;
      }
      layout.offsetY += 88;
    }

    const pos = config.positions[w.id] || { x: 0, y: 0 };
    const at = placeOnCanvas(pos);
    card.style.left = at.x + "px";
    card.style.top = at.y + "px";
    card.style.transform = "";

    if (pos.w) {
      card.style.width = pos.w + "px";
    }
    if (pos.h) {
      card.style.height = pos.h + "px";
    }
    if (isMicrographics) {
      // The position is the user's anchor; the module derives only row footprints.
      card.style.width = "0px";
      card.style.height = "0px";
    }

    const isSpecial = !!w.specialType;

    card.innerHTML = `
      ${gothicWidgetFrame()}
      <div class="resize-handle left"></div>
      <div class="resize-handle right"></div>
      ${isSpecial ? `
        <div class="resize-handle top"></div>
        <div class="resize-handle bottom"></div>
        <div class="resize-handle corner"></div>
      ` : ""}
      <button type="button" class="widget-instance-remove" aria-label="Remove widget">
        <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round">
          <path d="M18 6L6 18" /><path d="M6 6l12 12" />
        </svg>
      </button>
      <div class="widget-header">
        <div class="widget-icon">${w.iconSvg || ""}</div>
        <div class="widget-info">
          <div class="widget-title">${w.name}</div>
          <div class="widget-desc">${w.description}</div>
        </div>
        <div class="widget-chevron">
          <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round">
            <polyline points="9 18 15 12 9 6" />
          </svg>
        </div>
      </div>
      <div class="widget-body"></div>
    `;

    const header = card.querySelector(".widget-header");
    const body = card.querySelector(".widget-body");

    if (Array.isArray(w.items)) {
      card._folderId = w.widgetType || w.id;
      card._shortcutItems = w.items.map((item) => ({
        name: item.name,
        filename: item.name,
        path: item.target,
        args: item.args || "",
        type: item.kind || "file",
      }));
    }

    if (w.specialType === "clock") {
      card._widgetModulePromise = import("../modules/clock/widget.js");
      card._widgetModulePromise.then((m) => {
        if (!card.isConnected) return;
        card._widgetModule = m;
        m.renderClock(card, w.id);
      }).catch((err) => {
        console.error(`clock render failed: ${w.id}`, err);
        showDebugMessage(`clock ${w.id} failed: ${err?.message || err}`);
      });
    } else if (w.specialType === "mp3-player") {
      card._widgetModulePromise = import("../modules/mp3-player/widget.js");
      card._widgetModulePromise.then((m) => {
        card._widgetModule = m;
        m.renderMusicPlayer(card, w.id);
      }).catch((err) => {
        console.error(`music render failed: ${w.id}`, err);
        showDebugMessage(`music ${w.id} failed: ${err?.message || err}`);
        invoke("frontend_log", { message: `MUSIC RENDER FAIL: ${w.id}: ${err?.stack || err?.message || err}` }).catch(() => {});
      });
    } else if (w.specialType === "glyph-bar") {
      import("../modules/glyph-bar/widget.js").then((m) => m.renderGlyphBar(card, w.id)).catch(() => {});
    } else if (w.specialType === "telegram") {
      card.classList.add("telegram-widget");
      import("../modules/telegram/widget.js").then((m) => {
        m.renderTelegramWidget(card, w.id);
        refreshDomCaches();
      }).catch((err) => {
        console.error(`telegram render failed: ${w.id}`, err);
        showDebugMessage(`telegram ${w.id} failed: ${err?.message || err}`);
      });
    } else if (w.specialType === "micrographics") {
      import("../modules/micrographics/widget.js")
        .then((m) => { if (card.isConnected) m.renderMicrographics(card, w.id); })
        .catch((err) => {
          console.error(`micrographics render failed: ${w.id}`, err);
          invoke("frontend_log", { message: `MICROGRAPHICS RENDER FAIL: ${w.id}: ${err?.stack || err?.message || err}` }).catch(() => {});
        });
    }

    const removeBtn = card.querySelector(".widget-instance-remove");
    if (removeBtn) {
      removeBtn.addEventListener("pointerdown", (e) => e.stopPropagation());
      removeBtn.addEventListener("click", async (e) => {
        e.stopPropagation();
        if (!editMode) return;
        if (w.isInstance) {
          await removeWidgetInstance(w.id);
        } else {
          await hideBaseWidget(w.id);
        }
      });
    }

    // Музыкальный виджет полностью управляется собственными touch-зонами,
    // поэтому общий click-handler ему не нужен (иначе клик мимо зон раскрывал бы плеер в дашборд).
    // Micrographics is ambient: it has nothing to open, and it covers the largest
    // area on the desktop, so a click target on it would swallow clicks meant for
    // the desktop behind it and expand it as if it were a folder.
    if (w.specialType !== "mp3-player" && w.specialType !== "glyph-bar" && w.specialType !== "telegram" && w.specialType !== "micrographics") {
      const clickTarget = w.specialType ? card : header;
      clickTarget.addEventListener("click", (e) => {
        if (editMode) return;
        if (e.target.closest(".version-switcher")) return;
        if (e.target.closest(".widget-instance-remove")) return;
        if (e.target.closest(".dash-vpn")) return;
        toggleWidget(card, body, w.id, w.specialType, w.widgetType || w.id);
      });
    }
  return card;
}

// Releases what a card holds outside its own subtree (morph state, geometry
// controller, player timers) so it can leave the desktop without a rebuild.
function disposeWidgetCard(card) {
  disposeClockMorph(card);
  card._geometryController?.dispose();
  const players = card.matches(".music-widget") ? [card] : [...card.querySelectorAll(".music-widget")];
  if (players.length) {
    import("../modules/mp3-player/widget.js")
      .then((m) => players.forEach((el) => m.stopMusicCardLoops(el)))
      .catch(() => {});
  }
  _dockGeometry.delete(card.dataset.id);
}

let _syncInFlight = null;
let _syncAgain = false;
// Reconcile the desktop with the in-memory config without resetting the DOM
// (architecture §4): cards that should go are disposed and removed, missing
// ones are built, folder cards pick up renamed titles and edited shortcuts in
// place. Everything else — player, Telegram, clocks — keeps running.
async function syncWidgets() {
  if (_isLoadingWidgets) return;
  if (_syncInFlight) {
    _syncAgain = true;
    return _syncInFlight;
  }
  _syncInFlight = (async () => {
    do {
      _syncAgain = false;
      await reconcileWidgets();
    } while (_syncAgain);
  })().finally(() => { _syncInFlight = null; });
  return _syncInFlight;
}

async function reconcileWidgets() {
  const widgets = await fetchRenderableWidgets();
  if (!widgets) return;
  if (!config.positions) config.positions = {};
  ensureWidgetAddControls(widgets);
  const desired = desiredWidgets(widgets);
  const wanted = new Set(desired.map((w) => w.id));
  const cards = new Map([...widgetsCanvas.querySelectorAll(":scope > .widget-card")].map((c) => [c.dataset.id, c]));

  cards.forEach((card, id) => {
    if (wanted.has(id)) return;
    if (card.classList.contains("expanded")) collapseActiveDesktopWidget();
    disposeWidgetCard(card);
    card.remove();
  });

  const layout = { offsetY: 28 };
  desired.forEach((w) => {
    const card = cards.get(w.id);
    if (!card) {
      try {
        widgetsCanvas.appendChild(buildWidgetCard(w, layout));
      } catch (error) {
        console.error(`widget render failed: ${w.id}`, error);
        logFrontend(`widget render failed: ${w.id}: ${error?.stack || error?.message || error}`);
      }
      return;
    }
    if (!Array.isArray(w.items)) return;
    // A folder: refresh what its expanded body and header are built from.
    card._shortcutItems = w.items.map((item) => ({
      name: item.name,
      filename: item.name,
      path: item.target,
      args: item.args || "",
      type: item.kind || "file",
    }));
    const header = card.querySelector(":scope > .widget-header");
    const title = header?.querySelector(".widget-title");
    if (title) title.textContent = w.name;
    const desc = header?.querySelector(".widget-desc");
    if (desc) desc.textContent = w.description;
    const icon = header?.querySelector(".widget-icon");
    if (icon) icon.innerHTML = w.iconSvg || "";
    // An opened folder is being edited in place: redraw it, keep it open.
    const openBody = card.classList.contains("expanded") && card.querySelector(":scope > .widget-body");
    if (openBody) renderFolderBody(card, openBody);
  });

  relayoutWidgets();
  observeDockGeometry();
  refreshDomCaches();
  requestAnimationFrame(() => notifyGeometryChanged("sync"));
}

let _isLoadingWidgets = false;
async function loadWidgets() {
  if (_isLoadingWidgets) {
    rlog("[renderer] loadWidgets already running, skipping concurrent call");
    return;
  }
  _isLoadingWidgets = true;
  try {
    rlog("[renderer] loadWidgets called");
    await loadConfig();
    rlog("[renderer] loadConfig done");

  // Clean up intervals before DOM rebuild to prevent leaks
  const musicModule = await import("../modules/mp3-player/widget.js").catch(() => null);
  if (musicModule?.resetMusicIntervals) musicModule.resetMusicIntervals();

  const widgets = await fetchRenderableWidgets();
  if (!widgets) return;

  // в”Ђв”Ђв”Ђ Arrange widgets в”Ђв”Ђв”Ђ
  disposeClockMorphs();
  widgetsCanvas.querySelectorAll('.micrographics-card').forEach((card) => card._geometryController?.dispose());
  _dockResizeObserver?.disconnect();
  _dockGeometry.clear();
  widgetsCanvas.innerHTML = "";
  if (!config.positions) config.positions = {};
  if (!config.instances) config.instances = [];
  if (!Array.isArray(config.hidden)) config.hidden = [];

  ensureWidgetAddControls(widgets);

  const renderWidgets = desiredWidgets(widgets);

  const layout = { offsetY: 28 };
  renderWidgets.forEach((w) => {
    try {
    widgetsCanvas.appendChild(buildWidgetCard(w, layout));
    } catch (error) {
      console.error(`widget render failed: ${w.id}`, error);
      showDebugMessage(`widget ${w.id} failed: ${error?.message || error}`);
      logFrontend(`widget render failed: ${w.id}: ${error?.stack || error?.message || error}`);
    }
  });

    setupDragReorder();
    observeDockGeometry();
    setupDesktopOutsideClickHandler();
    refreshDomCaches();
    // Every card has just been placed, so the free space is known for the first
    // time. Deferred by a frame: a derived widget measures the DOM, and the cards
    // appended above must be laid out before that measurement means anything.
    requestAnimationFrame(() => notifyGeometryChanged("load"));
    _startWidgetLoops().catch(() => {});
  } finally {
    _isLoadingWidgets = false;
    updateDeskHint();
  }
}

// Widget render registry — populated lazily so core never statically
// imports widget modules (avoids circular imports). Widget modules self-register
// on import (see widget-registry.js) OR loadWidgets resolves them dynamically.
const _widgetRenderers = new Map();

async function _renderWidget(specialType, card, w) {
  switch (specialType) {
    case "clock": {
      const m = await import("../modules/clock/widget.js");
      m.renderClock(card, w.id);
      return;
    }
    case "mp3-player": {
      const m = await import("../modules/mp3-player/widget.js");
      m.renderMusicPlayer(card, w.id);
      return;
    }
    case "glyph-bar": {
      const m = await import("../modules/glyph-bar/widget.js");
      m.renderGlyphBar(card, w.id);
      return;
    }
    case "telegram": {
      const m = await import("../modules/telegram/widget.js");
      m.renderTelegramWidget(card, w.id);
      return;
    }
    default:
      break;
  }
}

async function _startWidgetLoops() {
  const clock = await import("../modules/clock/widget.js");
  clock.startClockInterval();
  const ac = await import("../modules/mp3-player/audio-arcs.js");
  ac.startAudioCircleLoop();
}

async function _closeMusicSatellite(musicCard, sat) {
  const m = await import("../modules/mp3-player/widget.js");
  m.closeMusicSatellite(musicCard, sat);
}



// ─── Desktop Widget State Manager (Mutual Exclusion & Safe Lifecycles) ───
let activeExpandedWidgetId = null;
let expandSequence = 0;

function collapseSingleWidget(card) {
  if (!card) return;
  const widgetId = card.dataset.id;
  const pos = config.positions[widgetId] || (typeof defaultWidgetPosition === "function" ? defaultWidgetPosition(widgetId) : null) || { x: 28, y: 28 };

  if (card.classList.contains("telegram-widget")) {
    if (pos) pos.expanded = false;
    import("../modules/telegram/widget.js").then((m) => m.collapseTelegramWidget(card, widgetId || "telegram")).catch(() => {});
    return;
  }

  const surface = surfaceManager.getRecord(card);
  if (surface && surface.state !== "closed") {
    if (card._collapseTimer) {
      clearTimeout(card._collapseTimer);
      card._collapseTimer = null;
    }
    if (pos) pos.expanded = false;
    if (activeExpandedWidgetId === widgetId) activeExpandedWidgetId = null;
    surfaceManager.closeSurface(card);
    return;
  }

  card.classList.remove("expanded");
  const at = placeOnCanvas({ x: 28, y: 28, ...pos });
  card.style.left = at.x + "px";
  card.style.top = at.y + "px";
  card.style.width = (pos.w || 340) + "px";
  card.style.height = (pos.h || 64) + "px";

  // No surface record left to drive onClosed, so the morph handoff would never
  // run: finish it here or the stand-in stays on <body> and the dock clock keeps
  // its visibility: hidden override.
  finishClockMorph(card, true);

  const dur = getDuration("motion");
  if (card._collapseTimer) clearTimeout(card._collapseTimer);
  card._collapseTimer = setTimeout(() => {
    card._collapseTimer = null;
    card._dockGeometryFrozen = false;
    card.style.zIndex = "10";
    if (!card.classList.contains("expanded")) {
      const body = card.querySelector(".widget-body");
      if (body) {
        // Collapsed cards show their header only; the body starts empty, as
        // when the card was built.
        body.innerHTML = "";
      }
    }
  }, dur);
}

function collapseActiveDesktopWidget(excludeCard = null) {
  let any = false;
  document.querySelectorAll(".widget-card.expanded").forEach((other) => {
    if (excludeCard && other === excludeCard) return;
    collapseSingleWidget(other);
    any = true;
  });
  if (!excludeCard || activeExpandedWidgetId !== excludeCard?.dataset?.id) {
    activeExpandedWidgetId = null;
  }
  return any;
}

function rectSnapshot(rect) {
  return { left: rect.left, top: rect.top, width: rect.width, height: rect.height };
}

function clockMorphTransform(baseRect, destination) {
  const sx = baseRect.width > 0 ? destination.width / baseRect.width : 1;
  const sy = baseRect.height > 0 ? destination.height / baseRect.height : 1;
  return `translate(${destination.left - baseRect.left}px, ${destination.top - baseRect.top}px) scale(${sx}, ${sy})`;
}

function saveInlineProperty(element, property) {
  return {
    value: element.style.getPropertyValue(property),
    priority: element.style.getPropertyPriority(property),
  };
}

function restoreInlineProperty(element, property, saved) {
  if (saved?.value) element.style.setProperty(property, saved.value, saved.priority);
  else element.style.removeProperty(property);
}

function hideClockMorphNode(state, element, key) {
  if (!element) return;
  if (!state[`${key}Visibility`]) state[`${key}Visibility`] = saveInlineProperty(element, "visibility");
  element.style.setProperty("visibility", "hidden", "important");
}

function clockMorphFaces(container) {
  // Gooey's date and SVG have different dashboard sizes. They need separate
  // endpoints; grid-second likewise has two independently laid-out faces.
  const gooey = container?.querySelector(":scope > .gooey-clock-face");
  return Array.from((gooey || container)?.children || []);
}

function clockMorphLayoutNodes(face) {
  // SVG ink can change its tree (e.g. Gooey 0 has a circle, 1 does not).
  // Freeze layout boxes, never path data or styles indexed through that ink.
  return [face, ...face.querySelectorAll("*")].filter((node) =>
    !(node instanceof SVGElement) || node.localName === "svg");
}

function captureClockMorphFaces(source) {
  return clockMorphFaces(source).map((face) => ({
    rect: rectSnapshot(face.getBoundingClientRect()),
    styles: clockMorphLayoutNodes(face).map((node) => {
      const computed = getComputedStyle(node);
      return Array.from(computed)
        .filter((property) => !/^(animation|transition)(-|$)/.test(property)
          && !["transform", "visibility", "opacity", "d"].includes(property))
        .map((property) => [property, computed.getPropertyValue(property)]);
    }),
  }));
}

function createClockMorphClone(state, target, open) {
  const sourceFaces = clockMorphFaces(state.source);
  const targetRects = clockMorphFaces(target).map((face) => rectSnapshot(face.getBoundingClientRect()));
  if (sourceFaces.length !== state.faces.length || targetRects.length !== state.faces.length
    || [...state.faces.map((face) => face.rect), ...targetRects].some((rect) => rect.width <= 0 || rect.height <= 0)) return;

  const clone = document.createElement("div");
  clone.className = `${state.source.className} clock-morph-clone`;
  clone.setAttribute("aria-hidden", "true");
  clone.style.cssText = "position:fixed;inset:0;padding:0;min-height:0;pointer-events:none;z-index:2501;visibility:hidden;transition:none!important;opacity:1";
  const faceParent = state.source.querySelector(":scope > .gooey-clock-face")
    ? clone.appendChild(document.createElement("div")) : clone;
  if (faceParent !== clone) faceParent.className = "gooey-clock-face";
  state.planes = sourceFaces.map((face, index) => {
    // Freeze the dock's logical layout before portaling. Scaling the wrapper
    // alone stretched fixed-size rings and lost inherited/contextual styles.
    // Clone current content so a long-open dashboard never returns stale digits.
    const plane = face.cloneNode(true);
    const snapshot = state.faces[index];
    clockMorphLayoutNodes(plane).forEach((node, nodeIndex) => {
      for (const [property, value] of snapshot.styles[nodeIndex] || []) node.style.setProperty(property, value);
    });
    [plane, ...plane.querySelectorAll("*")].forEach((node) => {
      node.style.setProperty("animation", "none", "important");
      node.style.setProperty("transition", "none", "important");
    });
    Object.assign(plane.style, {
      position: "absolute", left: `${snapshot.rect.left}px`, top: `${snapshot.rect.top}px`,
      right: "auto", bottom: "auto", margin: "0", flex: "none",
      transform: "none", transformOrigin: "top left", visibility: "visible",
    });
    faceParent.appendChild(plane);
    return { element: plane, sourceRect: snapshot.rect, targetRect: targetRects[index] };
  });
  document.body.appendChild(clone);
  for (const plane of state.planes) plane.baseRect = rectSnapshot(plane.element.getBoundingClientRect());
  for (const plane of state.planes) {
    plane.element.style.transform = clockMorphTransform(plane.baseRect, open ? plane.sourceRect : plane.targetRect);
  }
  clone._clockMorphOwner = state.source.closest(".widget-card");
  clone.style.visibility = "visible";
  state.clone = clone;
}

function animateClockMorph(card, body, open, duration, seed = null) {
  const target = body?.querySelector(".dash-clock");
  let state = card._clockMorph;
  if (!state && open && seed?.source && seed.originRect) {
    state = {
      source: seed.source,
      originRect: seed.originRect,
      sourceOpacity: seed.sourceOpacity,
      faces: seed.faces,
      planes: [],
      target: null,
      clone: null,
      timeline: null,
      generation: 0,
    };
    card._clockMorph = state;
  }
  if (!state || !state.source || !target) return;
  state.target = target;

  if (!state.clone?.isConnected) createClockMorphClone(state, target, open);
  if (!state.clone?.isConnected) return;

  // Both ends of the handoff stay hidden for as long as the stand-in exists.
  // Re-entering with a live clone (a reopen inside the crossfade window) used
  // to skip this, painting the fresh .dash-clock next to the stand-in.
  hideClockMorphNode(state, state.source, "source");
  hideClockMorphNode(state, target, "target");

  const fromTransforms = state.planes.map((plane) => getComputedStyle(plane.element).transform);
  state.generation += 1;
  const generation = state.generation;
  if (state.timeline) {
    state.timeline.cleanup({ commitStyles: true });
    state.timeline = null;
  }
  if (open) {
    hideClockMorphNode(state, state.source, "source");
    state.source.style.setProperty("opacity", "0", "important");
  }
  const timeline = motionTimeline({
    duration,
    durationIsScaled: true,
    easing: open ? EASINGS.expoOut : EASINGS.surfaceExit,
    onFinish: (timeline) => {
      if (state.generation !== generation || state.timeline !== timeline) return;
      // The stand-in rests on the frame this animation commits: dropping the
      // filled last frame would snap it back to its static box (the expanded
      // dashboard) before finishClockMorph hands it over to the dock clock.
      timeline.cleanup({ commitStyles: true });
      state.timeline = null;
    },
  });
  state.planes.forEach((plane, index) => timeline.add(plane.element, [
    { transform: fromTransforms[index] },
    { transform: clockMorphTransform(plane.baseRect, open ? plane.targetRect : plane.sourceRect) },
  ]));
  state.timeline = timeline.play();
}

function finishClockMorph(card, closed = false) {
  const state = card?._clockMorph;
  if (!state) return;
  state.generation += 1;
  if (state.timeline) {
    state.timeline.cleanup({ commitStyles: true });
    state.timeline = null;
  }
  if (state.target) restoreInlineProperty(state.target, "visibility", state.targetVisibility);
  if (closed) {
    if (state.source) {
      // One opaque face owns each paint. The clone already has the dock's ink
      // geometry, so exchanging it atomically avoids both the alpha dip of a
      // crossfade and brightening translucent ink by drawing two faces.
      const transition = saveInlineProperty(state.source, "transition");
      state.source.style.setProperty("transition", "none", "important");
      restoreInlineProperty(state.source, "opacity", state.sourceOpacity);
      restoreInlineProperty(state.source, "visibility", state.sourceVisibility);
      // Resolve the restored opacity while transitions are disabled, before
      // restoring CSS ownership; otherwise the dock starts a second fade.
      getComputedStyle(state.source).opacity;
      restoreInlineProperty(state.source, "transition", transition);
    }
    state.clone?.remove();
    card._clockMorph = null;
  } else {
    state.clone?.remove();
    state.clone = null;
    state.timeline = null;
  }
}

/**
 * Drop every live clock morph without animating it. A widget rebuild unmounts
 * the cards, and the stand-in lives on <body> rather than inside the card, so
 * without this it survives the rebuild as a permanent ghost over the desktop.
 */
function disposeClockMorph(card) {
  const state = card._clockMorph;
  if (!state) return;
  state.generation += 1;
  if (state.timeline) {
    state.timeline.cleanup();
    state.timeline = null;
  }
  if (state.target) restoreInlineProperty(state.target, "visibility", state.targetVisibility);
  if (state.source) {
    restoreInlineProperty(state.source, "visibility", state.sourceVisibility);
    restoreInlineProperty(state.source, "opacity", state.sourceOpacity);
  }
  state.clone?.remove();
  card._clockMorph = null;
}

function disposeClockMorphs() {
  document.querySelectorAll(".widget-card").forEach(disposeClockMorph);
  document.querySelectorAll(".clock-morph-clone").forEach((clone) => clone.remove());
}

async function toggleWidget(card, body, widgetId, specialType, widgetType = widgetId) {
  const surface = surfaceManager.getRecord(card);
  if (surface?.state === "closing") {
    if (card._collapseTimer) {
      clearTimeout(card._collapseTimer);
      card._collapseTimer = null;
    }
    activeExpandedWidgetId = widgetId;
    const position = config.positions[widgetId] || (config.positions[widgetId] = {});
    position.expanded = true;
    card.classList.add("expanded", "surface-washed");
    surfaceManager.openSurface(card);
    notifyGeometryChanged("expand");
    return;
  }

  const isExpanded = card.classList.contains("expanded");

  if (isExpanded || surface?.state === "open" || surface?.state === "opening") {
    if (activeExpandedWidgetId === widgetId) activeExpandedWidgetId = null;
    collapseSingleWidget(card);
    return;
  }

  // Cancel pending collapse timer
  if (card._collapseTimer) {
    clearTimeout(card._collapseTimer);
    card._collapseTimer = null;
  }

  // Enforce Widget Mutual Exclusion (unicoremodmotion.md §4.8)
  captureDockGeometry(card);
  card._dockGeometryFrozen = true;
  expandSequence++;
  const currentSeq = expandSequence;
  let clockMorphSeed = null;
  activeExpandedWidgetId = widgetId;
  collapseActiveDesktopWidget(card);

  // Close any popups in music widgets
  document.querySelectorAll(".music-widget").forEach((musicCard) => {
    import("../modules/mp3-player/widget.js").then((m) => m.closeAllMusicPopups(musicCard)).catch(() => {});
  });

  if (specialType === "clock") {
    const m = card._widgetModule || await card._widgetModulePromise || await import("../modules/clock/widget.js");
    if (currentSeq !== expandSequence) {
      if (!card.classList.contains("expanded") && !surfaceManager.getRecord(card)) card._dockGeometryFrozen = false;
      return;
    }
    const source = Array.from(card.children).find((child) => child.classList.contains("clock-container") && !child.classList.contains("dash-clock"));
    if (source) {
      clockMorphSeed = {
        source,
        originRect: rectSnapshot(source.getBoundingClientRect()),
        sourceOpacity: card._clockMorph?.source === source
          ? card._clockMorph.sourceOpacity
          : saveInlineProperty(source, "opacity"),
        faces: card._clockMorph?.faces || captureClockMorphFaces(source),
      };
    }
    m.renderClockDashboard(body, widgetId);
  } else if (specialType === "mp3-player") {
    const m = card._widgetModule || await card._widgetModulePromise || await import("../modules/mp3-player/widget.js");
    if (currentSeq !== expandSequence) {
      if (!card.classList.contains("expanded") && !surfaceManager.getRecord(card)) card._dockGeometryFrozen = false;
      return;
    }
    m.renderMusicDashboard(body, widgetId);
  } else {
    if (!Array.isArray(card._shortcutItems)) {
      card._shortcutItems = await getWidgetShortcuts(widgetType).catch(() => []);
      card._folderId = widgetType;
    }
    await renderFolderBody(card, body);
    if (currentSeq !== expandSequence) {
      if (!card.classList.contains("expanded") && !surfaceManager.getRecord(card)) card._dockGeometryFrozen = false;
      return;
    }
  }

    // Wait for content to render, then expand symmetrically from own center
    requestAnimationFrame(() => {
      if (currentSeq !== expandSequence) {
      if (!card.classList.contains("expanded") && !surfaceManager.getRecord(card)) card._dockGeometryFrozen = false;
      return;
    }
      const rect = card.getBoundingClientRect();
      const canvasRect = widgetsCanvas.getBoundingClientRect();
      const originRect = { top: rect.top, left: rect.left, width: rect.width, height: rect.height };
      const savedCardTransition = card.style.transition;
      const savedBodyTransition = body.style.transition;
      
      let targetW = 400;
      let targetH = 400; // Default fallback

      // The clock dashboard has a known compact layout. Avoid forcing a cold
      // synchronous body layout on the first click; other widgets still size
      // themselves from their rendered content.
      if (specialType === "clock") {
        targetH = 420;
      } else {
        body.style.visibility = "hidden";
        body.style.display = "block";
        body.style.position = "absolute";
        body.style.width = (targetW - 32) + "px";
        const contentHeight = body.scrollHeight;
        body.style.display = "";
        body.style.position = "";
        body.style.width = "";
        body.style.visibility = "";
        targetH = Math.min(500, contentHeight + 80);
      }

      let centerX = (rect.left - canvasRect.left) + rect.width / 2;
      let centerY = (rect.top - canvasRect.top) + rect.height / 2;
      
      // Prevent expanding off-screen
      const viewCenterX = rect.left + rect.width / 2;
      const viewCenterY = rect.top + rect.height / 2;
      const margin = 10;

      if (viewCenterX - targetW / 2 < margin) centerX += (margin - (viewCenterX - targetW / 2));
      else if (viewCenterX + targetW / 2 > window.innerWidth - margin) centerX -= ((viewCenterX + targetW / 2) - (window.innerWidth - margin));
      
      if (viewCenterY - targetH / 2 < margin) centerY += (margin - (viewCenterY - targetH / 2));
      else if (viewCenterY + targetH / 2 > window.innerHeight - margin) centerY -= ((viewCenterY + targetH / 2) - (window.innerHeight - margin));
      
      card.style.zIndex = "100";
      card.style.transition = "none";
      body.style.transition = "none";
      card.style.left = (centerX - targetW / 2) + "px";
      card.style.top = (centerY - targetH / 2) + "px";
      card.style.width = targetW + "px";
      card.style.height = targetH + "px";
      
      card.classList.add("expanded");
      activeExpandedWidgetId = widgetId;
      // An expanded card is much larger than its stored size, and that size is
      // never written to `config.positions` — it lives only in inline styles. So a
      // widget that fills the leftover space can only learn about it from here.
      notifyGeometryChanged("expand");

      const restoreTransitions = () => {
        requestAnimationFrame(() => {
          if (!card.isConnected) return;
          card.style.transition = savedCardTransition;
          body.style.transition = savedBodyTransition;
        });
      };
      const restoreCollapsedLayout = () => {
        if (!card.isConnected) return;
        const pos = config.positions[widgetId] || { x: 28, y: 28, w: 340, h: 64 };
        pos.expanded = false;
        if (activeExpandedWidgetId === widgetId) activeExpandedWidgetId = null;
        card.style.transition = "none";
        body.style.transition = "none";
        card.classList.remove("expanded");
        const at = placeOnCanvas({ x: 28, y: 28, ...pos });
        card.style.left = at.x + "px";
        card.style.top = at.y + "px";
        card.style.width = (pos.w || 340) + "px";
        card.style.height = (pos.h || 64) + "px";
        card.style.zIndex = "10";
        card._dockGeometryFrozen = false;
        notifyGeometryChanged("collapse");
        body._dashboardRenderGeneration = (body._dashboardRenderGeneration || 0) + 1;
        body.innerHTML = "";
        restoreTransitions();
      };

      surfaceManager.openSurface(card, {
        origin: originRect,
        closeOnOutsideClick: true,
        closeOnEscape: true,
        suppressContentAnimation: true,
        // The clock face morphs on its own clone; the card body must stay solid
        // through the last frame, or it fades out and pops back as the dock.
        ...(specialType === "clock" ? { openOpacity: 1, closeOpacity: 1 } : {}),
        onTransitionStart: ({ open, duration }) => {
          if (specialType === "clock") {
            syncSurfaceWash(card, open, duration);
            animateClockMorph(card, body, open, duration, clockMorphSeed);
          }
        },
        onOpened: () => {
          if (specialType === "clock") finishClockMorph(card);
          restoreTransitions();
        },
        onClosed: () => {
          restoreCollapsedLayout();
          if (specialType === "clock") finishClockMorph(card, true);
        },
      });
    });
}

// в”Ђв”Ђв”Ђ Clock Dashboard Content в”Ђв”Ђв”Ђ



function getShortcutIcon(name, type) {
  const s = 'width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"';
  const key = name.toLowerCase().trim();

  // в”Ђв”Ђ Name-based icons в”Ђв”Ђ

  const named = {
    // AI Assistants
    "chatgpt": `<svg ${s}>
      <circle cx="12" cy="12" r="9"/>
      <path d="M12 3v4"/><path d="M12 17v4"/>
      <path d="M3 12h4"/><path d="M17 12h4"/>
      <circle cx="12" cy="12" r="3.5"/>
      <circle cx="12" cy="12" r="1" fill="currentColor" stroke="none"/>
    </svg>`,

    "claude": `<svg ${s}>
      <path d="M12 3c-5 0-9 4-9 9s4 9 9 9 9-4 9-9"/>
      <path d="M21 3l-9 9"/>
      <circle cx="12" cy="12" r="2.5"/>
      <circle cx="12" cy="12" r="0.8" fill="currentColor" stroke="none"/>
    </svg>`,

    "codex": `<svg ${s}>
      <polyline points="16 18 22 12 16 6"/>
      <polyline points="8 6 2 12 8 18"/>
      <line x1="14" y1="4" x2="10" y2="20"/>
    </svg>`,

    "google gemini": `<svg ${s}>
      <path d="M12 2l-10 10 10 10 10-10z"/>
      <path d="M12 7v10"/><path d="M7 12h10"/>
      <circle cx="12" cy="12" r="1.5" fill="currentColor" stroke="none"/>
    </svg>`,

    "kimi": `<svg ${s}>
      <circle cx="12" cy="12" r="9"/>
      <path d="M8 9l4 3-4 3"/>
      <circle cx="17" cy="12" r="1" fill="currentColor" stroke="none"/>
    </svg>`,

    "le chat": `<svg ${s}>
      <rect x="3" y="6" width="18" height="12" rx="3"/>
      <circle cx="9" cy="12" r="1" fill="currentColor" stroke="none"/>
      <circle cx="15" cy="12" r="1" fill="currentColor" stroke="none"/>
      <path d="M9 15c1.5 1.2 4.5 1.2 6 0"/>
      <path d="M6 6L4 3"/><path d="M18 6l2-3"/>
    </svg>`,

    "manus": `<svg ${s}>
      <path d="M18 11V6a2 2 0 00-2-2H8a2 2 0 00-2 2v5"/>
      <path d="M6 11c-2 0-3 1.5-3 3s1 3 3 3h12c2 0 3-1.5 3-3s-1-3-3-3"/>
      <circle cx="12" cy="14" r="1.5" fill="currentColor" stroke="none"/>
    </svg>`,

    "microsoft 365 copilot": `<svg ${s}>
      <path d="M4 12a8 8 0 0116 0"/>
      <path d="M8 12a4 4 0 018 0"/>
      <circle cx="12" cy="12" r="1.5" fill="currentColor" stroke="none"/>
      <path d="M12 14v5"/>
      <path d="M9 17h6"/>
    </svg>`,

    // Tools
    "explorer": `<svg ${s}>
      <path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"/>
      <line x1="9" y1="14" x2="15" y2="14"/>
    </svg>`,

    "blender 5.2": `<svg ${s}>
      <ellipse cx="12" cy="12" rx="9" ry="6"/>
      <ellipse cx="12" cy="12" rx="5" ry="3.2"/>
      <circle cx="12" cy="12" r="1.3" fill="currentColor" stroke="none"/>
    </svg>`,

    "antigravity": `<svg ${s}>
      <path d="M12 2v4"/>
      <path d="M8 6l4-4 4 4"/>
      <path d="M4 14c0-4.4 3.6-8 8-8s8 3.6 8 8"/>
      <path d="M8 14c0 2.2 1.8 4 4 4s4-1.8 4-4"/>
      <circle cx="12" cy="14" r="1" fill="currentColor" stroke="none"/>
    </svg>`,

    "google chrome": `<svg ${s}>
      <circle cx="12" cy="12" r="9"/>
      <circle cx="12" cy="12" r="3.8"/>
      <path d="M12 8.2a3.8 3.8 0 013.3 1.9l5.2-3A9 9 0 003.5 7.5l5.2 3A3.8 3.8 0 0112 8.2z"/>
      <path d="M15.3 10.1a3.8 3.8 0 011.6 3.2c0 .5-.1 1-.3 1.4l5.2 3A9 9 0 0112 21a9 9 0 01-7.8-4.5l5.2-3c-.2.5-.3.9-.3 1.4a3.8 3.8 0 016.2-4.8z"/>
      <circle cx="12" cy="13.8" r="1.2" fill="currentColor" stroke="none"/>
    </svg>`,

    "happ": `<svg ${s}>
      <rect x="4" y="4" width="16" height="16" rx="4"/>
      <path d="M8 12h8"/>
      <path d="M12 8v8"/>
      <circle cx="12" cy="12" r="1" fill="currentColor" stroke="none"/>
    </svg>`,

    "visual studio code": `<svg ${s}>
      <path d="M16 3l4 2v14l-4 2-11-9.5L16 3z"/>
      <path d="M5 5.5l6 6.5-6 6.5"/>
      <path d="M16 3v18"/>
    </svg>`,

    // Media & Devices
    "imazing": `<svg ${s}>
      <rect x="6" y="3" width="12" height="18" rx="2"/>
      <line x1="11" y1="18" x2="13" y2="18"/>
      <path d="M12 6v3"/>
      <path d="M10.5 7.5l1.5-1.5 1.5 1.5"/>
    </svg>`,

    "incspace": `<svg ${s}>
      <path d="M6 18a4 4 0 010-8 5 5 0 019.6-1.5A4 4 0 0118 18H6z"/>
      <path d="M12 10v5"/>
      <path d="M10 12l2-2 2 2"/>
    </svg>`,

    "rawtherapee": `<svg ${s}>
      <rect x="3" y="5" width="18" height="14" rx="2"/>
      <circle cx="12" cy="12" r="3.5"/>
      <circle cx="12" cy="12" r="1.2" fill="currentColor" stroke="none"/>
      <line x1="3" y1="16" x2="7" y2="16"/>
      <line x1="17" y1="16" x2="21" y2="16"/>
      <circle cx="5" cy="16" r="0.8" fill="currentColor" stroke="none"/>
      <circle cx="19" cy="16" r="0.8" fill="currentColor" stroke="none"/>
    </svg>`,

    // Network вЂ” servers
    "server tools": `<svg ${s}>
      <rect x="3" y="3" width="18" height="7" rx="2"/>
      <rect x="3" y="14" width="18" height="7" rx="2"/>
      <circle cx="7" cy="6.5" r="1" fill="currentColor" stroke="none"/>
      <circle cx="7" cy="17.5" r="1" fill="currentColor" stroke="none"/>
      <line x1="11" y1="6.5" x2="17" y2="6.5"/>
      <line x1="11" y1="17.5" x2="17" y2="17.5"/>
    </svg>`,

    "сервер - rdp": `<svg ${s}>
      <rect x="2" y="4" width="20" height="14" rx="2"/>
      <line x1="2" y1="20" x2="22" y2="20"/>
      <path d="M8 10h3v4"/>
      <circle cx="15" cy="11" r="0.7" fill="currentColor" stroke="none"/>
    </svg>`,

    "сервер - всё": `<svg ${s}>
      <circle cx="12" cy="12" r="9"/>
      <polyline points="8 12 11 15 16 9"/>
      <circle cx="12" cy="12" r="0.8" fill="currentColor" stroke="none"/>
    </svg>`,

    "сервер - пароль": `<svg ${s}>
      <rect x="5" y="11" width="14" height="10" rx="2"/>
      <path d="M12 3a4 4 0 00-4 4v4h8V7a4 4 0 00-4-4z"/>
      <circle cx="12" cy="16" r="1.5" fill="currentColor" stroke="none"/>
    </svg>`,

    "сервер - статус": `<svg ${s}>
      <path d="M22 12h-4l-3 7-4-14-3 7H2"/>
      <circle cx="12" cy="12" r="1" fill="currentColor" stroke="none"/>
    </svg>`,

    "сервер - шара": `<svg ${s}>
      <path d="M4 20h16a2 2 0 002-2V8a2 2 0 00-2-2h-7l-2-2H4a2 2 0 00-2 2v12a2 2 0 002 2z"/>
      <path d="M12 10v6"/>
      <path d="M9 13h6"/>
    </svg>`,

    // в”Ђв”Ђ Games в”Ђв”Ђ

    "steam": `<svg ${s}>
      <circle cx="12" cy="12" r="8.5"/>
      <circle cx="15.5" cy="9" r="2.3"/>
      <circle cx="8.5" cy="15" r="1.7" fill="currentColor" stroke="none"/>
      <path d="M10 14.2l4-3.5"/>
    </svg>`,

    "roblox player": `<svg ${s}>
      <rect x="5.5" y="5.5" width="13" height="13" rx="1.5" transform="rotate(10 12 12)"/>
      <rect x="10" y="10" width="4" height="4" rx="0.6" fill="currentColor" stroke="none" transform="rotate(10 12 12)"/>
    </svg>`,

    "roblox studio": `<svg ${s}>
      <rect x="5.5" y="5.5" width="13" height="13" rx="1.5" transform="rotate(10 12 12)"/>
      <rect x="10" y="10" width="4" height="4" rx="0.6" fill="currentColor" stroke="none" transform="rotate(10 12 12)"/>
    </svg>`,

    "epic games launcher": `<svg ${s}>
      <path d="M6 4.5h12v11l-6 4-6-4z"/>
      <path d="M9.5 8.5h5M9.5 12h3.5M9.5 15.5h5"/>
    </svg>`,

    "fortnite": `<svg ${s}>
      <path d="M8.5 5h7M8.5 5v14M8.5 12h5.5"/>
    </svg>`,

    "tlauncher": `<svg ${s}>
      <path d="M12 3.2l8 4.4v8.8L12 20.8 4 16.4V7.6z"/>
      <path d="M4 7.6l8 4.4 8-4.4M12 12v8.8"/>
    </svg>`,

    "r.e.p.o.": `<svg ${s}>
      <rect x="3.5" y="6" width="17" height="12" rx="3"/>
      <path d="M8 12h2.2M9.1 10.9v2.2"/>
      <circle cx="15.5" cy="11.5" r="0.9" fill="currentColor" stroke="none"/>
      <circle cx="17" cy="13.5" r="0.9" fill="currentColor" stroke="none"/>
    </svg>`,

    "arknights endfield": `<svg ${s}>
      <path d="M12 3l7 9-7 9-7-9 7-9z"/>
      <path d="M12 8l3.5 4-3.5 4-3.5-4 3.5-4z"/>
    </svg>`,

    "fall guys": `<svg ${s}>
      <path d="M12 3c-3.9 0-6 3-6 7.5S8 21 12 21s6-3.5 6-10.5S15.9 3 12 3z"/>
      <circle cx="9.5" cy="10" r="1" fill="currentColor" stroke="none"/>
      <circle cx="14.5" cy="10" r="1" fill="currentColor" stroke="none"/>
    </svg>`,

    "gryphlink": `<svg ${s}>
      <path d="M10 14a4 4 0 005.7.3l3-3a4 4 0 10-5.7-5.7l-1.5 1.5"/>
      <path d="M14 10a4 4 0 00-5.7-.3l-3 3a4 4 0 105.7 5.7l1.5-1.5"/>
    </svg>`,

    "peak": `<svg ${s}>
      <path d="M4 19L12 5l8 14H4z"/>
      <path d="M9.5 11.5L12 9l2.5 2.5"/>
    </svg>`,

    "thunderstore mod manager": `<svg ${s}>
      <rect x="4" y="4" width="16" height="16" rx="4"/>
      <path d="M13.5 7l-4.5 6H12l-1.5 4 4.5-6H12l1.5-4z"/>
    </svg>`,

    "how to fish": `<svg ${s}>
      <path d="M12 5c-3 0-6 3-6 7s3 7 6 7 6-3 6-7-3-7-6-7z"/>
      <circle cx="10" cy="11" r="0.8" fill="currentColor" stroke="none"/>
      <path d="M6 19c0 0 2-2 6-2s6 2 6 2"/>
      <path d="M19 5c-1 0-2 1-2 2.5S18 10 19 10"/>
      <line x1="19" y1="3" x2="19" y2="10"/>
      <path d="M17 3l2-1v2"/>
    </svg>`,
  };

  if (named[key]) return named[key];

  // в”Ђв”Ђ Type-based fallback в”Ђв”Ђ

  const byType = {
    folder: `<svg ${s}>
      <path d="M4 20h16a2 2 0 002-2V8a2 2 0 00-2-2h-7l-2-2H4a2 2 0 00-2 2v12a2 2 0 002 2z"/>
      <circle cx="12" cy="13" r="1" fill="currentColor" stroke="none"/>
    </svg>`,
    lnk: `<svg ${s}>
      <rect x="4" y="4" width="16" height="16" rx="4"/>
      <circle cx="12" cy="12" r="2" fill="currentColor" stroke="none"/>
    </svg>`,
    exe: `<svg ${s}>
      <rect x="4" y="4" width="16" height="16" rx="4"/>
      <polygon points="10,8 16,12 10,16" fill="currentColor" stroke="none"/>
    </svg>`,
    url: `<svg ${s}>
      <circle cx="12" cy="12" r="9"/>
      <path d="M2 12h20"/>
      <path d="M12 3a15 15 0 014 9 15 15 0 01-4 9 15 15 0 01-4-9 15 15 0 014-9z"/>
    </svg>`,
    cmd: `<svg ${s}>
      <rect x="3" y="3" width="18" height="18" rx="3"/>
      <polyline points="7 10 10 13 7 16"/>
      <line x1="13" y1="16" x2="17" y2="16"/>
    </svg>`,
    bat: `<svg ${s}>
      <rect x="3" y="3" width="18" height="18" rx="3"/>
      <polyline points="7 10 10 13 7 16"/>
      <line x1="13" y1="16" x2="17" y2="16"/>
    </svg>`,
    ps1: `<svg ${s}>
      <rect x="3" y="3" width="18" height="18" rx="3"/>
      <polyline points="7 10 10 13 7 16"/>
      <line x1="13" y1="16" x2="17" y2="16"/>
    </svg>`,
    rdp: `<svg ${s}>
      <rect x="2" y="4" width="20" height="14" rx="2"/>
      <line x1="2" y1="20" x2="22" y2="20"/>
    </svg>`,
    json: `<svg ${s}>
      <path d="M8 3H6a2 2 0 00-2 2v3c0 1.1-.9 2-2 2 1.1 0 2 .9 2 2v3a2 2 0 002 2h2"/>
      <path d="M16 3h2a2 2 0 012 2v3c0 1.1.9 2 2 2-1.1 0-2 .9-2 2v3a2 2 0 01-2 2h-2"/>
    </svg>`,
  };

  return byType[type] || `<svg ${s}>
    <path d="M14 2H6a2 2 0 00-2 2v16a2 2 0 002 2h12a2 2 0 002-2V8z"/>
    <polyline points="14 2 14 8 20 8"/>
    <circle cx="12" cy="14" r="1" fill="currentColor" stroke="none"/>
  </svg>`;
}



const CP1251_REVERSE = (() => {
  const bytes = new Uint8Array(256);
  for (let i = 0; i < 256; i += 1) bytes[i] = i;
  const decoded = new TextDecoder("windows-1251").decode(bytes);
  const map = new Map();
  for (let i = 0; i < decoded.length; i += 1) map.set(decoded[i], i);
  return map;
})();

function repairMojibake(str) {
  const text = String(str);
  if (!/[РС][\u0400-\u04ff]/.test(text)) return text;

  const bytes = [];
  for (const ch of text) {
    const byte = CP1251_REVERSE.get(ch);
    if (byte === undefined) return text;
    bytes.push(byte);
  }

  try {
    const repaired = new TextDecoder("utf-8", { fatal: true }).decode(new Uint8Array(bytes));
    return repaired.includes("\uFFFD") ? text : repaired;
  } catch {
    return text;
  }
}

function displayText(str) {
  return repairMojibake(str);
}

function escapeHtml(str) {
  return String(str ?? "").replace(/[&<>"']/g, (c) => ({
    "&": "&amp;",
    "<": "&lt;",
    ">": "&gt;",
    '"': "&quot;",
    "'": "&#39;",
  }[c]));
}

// в”Ђв”Ђв”Ђ Special Widget: Music Player в”Ђв”Ђв”Ђ



function logFrontend(message) {
  invoke("frontend_log", { message }).catch(() => {});
}



const debugSignal = document.getElementById("debugSignal");
const debugMessage = document.getElementById("debugMessage");
let debugTimeout;

function showDebugMessage(text) {
  if (!debugMessage) return;
  debugMessage.textContent = text;
  debugMessage.classList.remove("hidden");
  clearTimeout(debugTimeout);
  debugTimeout = setTimeout(() => debugMessage.classList.add("hidden"), 4000);
}



// The wallpaper window (WorkerW child) never receives real wheel input, and a
// posted WM_MOUSEWHEEL is ignored by Chromium — so the backend raw-input hook
// forwards wheel notches as this event and we scroll the DOM ourselves.
// Touchpads send many small wheel deltas; stepped controls (volume) move one
// step per whole notch (WHEEL_DELTA = 120) of accumulated travel.
const wheelCarry = new Map();
function wheelNotches(key, delta) {
  const travel = (wheelCarry.get(key) || 0) + delta;
  const notches = Math.trunc(travel / 120);
  wheelCarry.set(key, travel - notches * 120);
  return notches;
}

listen("wallpaper-wheel", (event) => {
  const { x, y, delta } = event.payload || {};
  if (typeof x !== "number" || typeof y !== "number" || !delta) return;
  const scale = window.devicePixelRatio || 1;
  const clientX = x / scale;
  const clientY = y / scale;
  let el = document.elementFromPoint(clientX, clientY);

  // 1. Music Popup takes absolute precedence over widget volume
  const musicPopup = el?.closest?.(".music-popup");
  if (musicPopup) {
    const graphContainer = el?.closest?.(".ai-dj-graph-container, .ai-dj-graph-canvas");
    if (graphContainer) {
      const canvas = graphContainer.classList.contains("ai-dj-graph-canvas")
        ? graphContainer
        : graphContainer.querySelector(".ai-dj-graph-canvas");
      if (canvas && typeof canvas._handleWheelZoom === "function") {
        // One zoom step per whole notch: touchpad travel arrives as many
        // small deltas. Delta > 0 is zoom in; clientX/Y in CSS viewport pixels
        const notches = wheelNotches("ai-dj-zoom", delta);
        for (let i = 0; i < Math.abs(notches); i++) {
          canvas._handleWheelZoom(Math.sign(notches), clientX, clientY);
        }
      }
      return; // Fully consume inside graph
    }

    // Inside popup, non-graph area: standard scroll loop bounded within popup
    let scrollEl = el;
    while (scrollEl && scrollEl !== musicPopup) {
      if (scrollEl.scrollHeight > scrollEl.clientHeight + 1) {
        const style = getComputedStyle(scrollEl);
        if (/(auto|scroll)/.test(style.overflowY)) {
          scrollEl.scrollTop -= delta;
          return;
        }
      }
      scrollEl = scrollEl.parentElement;
    }
    return; // Block leakage to desktop
  }

  // 2. Glyph Bar: wheel adjusts system volume (not DOM scroll).
  const glyphBar = el?.closest?.(".glyph-bar-widget, .gb-container, .gb-seg");
  if (glyphBar && !editMode) {
    const notches = wheelNotches("glyph-bar", delta);
    if (notches) nudgeSystemVolume(notches * 10);
    return;
  }

  // 3. Music Player: wheel adjusts player volume
  const musicWidget = el?.closest?.(".music-widget, .music-player-container");
  if (musicWidget && !editMode) {
    const notches = wheelNotches("music", delta);
    if (!notches) return;
    import("../modules/mp3-player/widget.js").then((m) => {
      m.nudgeMusicVolume(notches * 5, musicWidget);
    }).catch(() => {});
    return;
  }

  // 4. Default desktop scroll
  while (el && el !== document.documentElement) {
    if (el.scrollHeight > el.clientHeight + 1) {
      const style = getComputedStyle(el);
      if (/(auto|scroll)/.test(style.overflowY)) {
        el.scrollTop -= delta;
        return;
      }
    }
    el = el.parentElement;
  }
});

window.addEventListener("error", (e) => showDebugMessage(`Error: ${e.message}`));
window.addEventListener("unhandledrejection", (e) => showDebugMessage(`Promise: ${e.reason}`));

// в”Ђв”Ђв”Ђ System buttons в”Ђв”Ђв”Ђ

document.getElementById("openMenu")?.addEventListener("click", (e) => {
  openMenu(e.currentTarget);
});

// в”Ђв”Ђв”Ђ Visibility-driven refresh в”Ђв”Ђв”Ђ
document.addEventListener("visibilitychange", () => {
  if (!document.hidden) {
    try { updateClockTime(); } catch {}
  }
});

// в”Ђв”Ђв”Ђ Init в”Ђв”Ђв”Ђ

// ─── Shell Explorer (fullscreen overlay; not a desktop widget) ───



const SNAP_GRID = 16;
const SNAP_MAGNET = 10;
const SNAP_GAP = 12;

// Where a dragged card settles: the nearest screen edge or neighbour edge within
// the magnet distance, otherwise the grid; never off the screen.
function snapMove(card, rawX, rawY) {
  const canvasW = widgetsCanvas.clientWidth || window.innerWidth;
  const canvasH = widgetsCanvas.clientHeight || window.innerHeight;
  const w = card.offsetWidth;
  const h = card.offsetHeight;
  const xs = [SNAP_GAP, canvasW - w - SNAP_GAP];
  const ys = [SNAP_GAP, canvasH - h - SNAP_GAP];
  widgetsCanvas.querySelectorAll(":scope > .widget-card").forEach((other) => {
    if (other === card || other.classList.contains("expanded")) return;
    const ow = other.offsetWidth;
    const oh = other.offsetHeight;
    if (!ow || !oh) return;
    const ox = other.offsetLeft;
    const oy = other.offsetTop;
    xs.push(ox, ox + ow - w, ox + ow + SNAP_GAP, ox - SNAP_GAP - w);
    ys.push(oy, oy + oh - h, oy + oh + SNAP_GAP, oy - SNAP_GAP - h);
  });
  const settle = (raw, lines) => {
    let best = Math.round(raw / SNAP_GRID) * SNAP_GRID;
    let bestDist = SNAP_MAGNET;
    for (const line of lines) {
      const dist = Math.abs(line - raw);
      if (dist < bestDist) {
        best = line;
        bestDist = dist;
      }
    }
    return Math.round(best);
  };
  const clamp = (v, max) => (max > 0 ? Math.min(Math.max(0, v), max) : 0);
  return {
    x: clamp(settle(rawX, xs), canvasW - w),
    y: clamp(settle(rawY, ys), canvasH - h),
  };
}

function setupDragReorder() {
  if (widgetsCanvas._dragReorderReady) return;
  widgetsCanvas._dragReorderReady = true;

  widgetsCanvas.addEventListener("pointerdown", (e) => {
    if (!editMode || e.button !== 0) return;

    const card = e.target.closest(".widget-card");
    if (!card) return;
    if (!card.classList.contains("expanded") && !surfaceManager.isOpen(card)) card._dockGeometryFrozen = false;
    if (card._geometryController) {
      card._geometryController.begin(e);
      return;
    }

    const isResizeLeft = e.target.classList.contains("left");
    const isResizeRight = e.target.classList.contains("right");
    const isResizeTop = e.target.classList.contains("top");
    const isResizeBottom = e.target.classList.contains("bottom");
    const isResizeCorner = e.target.classList.contains("corner");
    const isResize = isResizeLeft || isResizeRight || isResizeTop || isResizeBottom || isResizeCorner;

    const header = card.querySelector(".widget-header");
    const isClockContainer = e.target.closest(".clock-container");
    const isMusicContainer = e.target.closest(".music-player-container");
    const isGlyphBarContainer = e.target.closest(".gb-container");
    const isTelegramContainer = e.target.closest(".telegram-widget");

    if (!isResize && !header.contains(e.target) && !isClockContainer && !isMusicContainer && !isGlyphBarContainer && !isTelegramContainer) return;

    const widgetId = card.dataset.id;
    const startX = e.clientX;
    const startY = e.clientY;
    const pointerId = e.pointerId;
    const startLeft = parseInt(card.style.left) || 0;
    const startTop = parseInt(card.style.top) || 0;
    const startWidth = card.offsetWidth;
    const startHeight = card.offsetHeight;

    card.classList.add("dragging");
    card.style.transition = "none"; // Temporarily disable transitions

    let queuedEvent = null;
    let moveRaf = 0;
    let finished = false;

    const applyMove = (ev) => {
      const deltaX = ev.clientX - startX;
      const deltaY = ev.clientY - startY;
      // Alt: симметричное масштабирование — растягиваем во ВСЕ стороны равномерно
      // (как в Photoshop / Figma). Центр виджета остаётся на месте.
      const isAlt = ev.altKey;

      if (isResize) {
        const isGlyphBar = card.classList.contains("glyph-bar-widget");
        const minW = isGlyphBar ? 8 : 64;
        const minH = isGlyphBar ? card._glyphBarGeometry.heightForSide(8) : 64;
        if (isGlyphBar) {
          // Side length in px (1:1 LEDs). No 10px grid — that made small bars jump.
          let side;
          if (isResizeBottom || isResizeTop) {
            let rawDelta = isResizeBottom ? deltaY : -deltaY;
            if (isAlt) rawDelta *= 2;
            side = card._glyphBarGeometry.sideForHeight(startHeight + rawDelta);
          } else if (isResizeCorner) {
            const dx = isAlt ? deltaX * 2 : deltaX;
            const dy = isAlt ? deltaY * 2 : deltaY;
            const sideFromW = Math.round(startWidth + dx);
            const sideFromH = card._glyphBarGeometry.sideForHeight(startHeight + dy);
            side = Math.abs(dx) >= Math.abs(dy) ? sideFromW : sideFromH;
          } else {
            let rawDelta = isResizeRight ? deltaX : -deltaX;
            if (isAlt) rawDelta *= 2;
            side = Math.round(startWidth + rawDelta);
          }
          side = card._glyphBarGeometry.clampSide(side);
          const newWidth = side;
          const newHeight = card._glyphBarGeometry.heightForSide(side);
          const widthDiff = newWidth - startWidth;
          const heightDiff = newHeight - startHeight;
          if (isAlt) {
            card.style.left = (startLeft - widthDiff / 2) + "px";
            card.style.top = (startTop - heightDiff / 2) + "px";
          } else {
            if (isResizeLeft) card.style.left = (startLeft - widthDiff) + "px";
            if (isResizeTop) card.style.top = (startTop - heightDiff) + "px";
          }
          card.style.width = newWidth + "px";
          card.style.height = newHeight + "px";
          card._glyphBarGeometry.layout();
        } else if (isResizeCorner) {
            // Угол: при alt — растягиваем равномерно во все 4 стороны
            const dx = isAlt ? deltaX * 2 : deltaX;
            const dy = isAlt ? deltaY * 2 : deltaY;
            let newWidth = Math.round(startWidth + dx);
            let newHeight = Math.round(startHeight + dy);
            newWidth = Math.max(minW, Math.min(newWidth, 800));
            newHeight = Math.max(minH, Math.min(newHeight, 800));
            if (isAlt) {
              card.style.left = (startLeft - (newWidth - startWidth) / 2) + "px";
              card.style.top = (startTop - (newHeight - startHeight) / 2) + "px";
            }
            card.style.width = newWidth + "px";
            card.style.height = newHeight + "px";
        } else if (isResizeRight || isResizeLeft) {
            let rawDelta = isResizeRight ? deltaX : -deltaX;
            if (isAlt) rawDelta *= 2; // растягиваем сразу в обе стороны
            let newWidth = Math.round(startWidth + rawDelta);
            newWidth = Math.max(minW, Math.min(newWidth, 800));
            const widthDiff = newWidth - startWidth;
            if (isAlt) {
              card.style.left = (startLeft - widthDiff / 2) + "px";
            } else if (isResizeLeft) {
              card.style.left = (startLeft - widthDiff) + "px";
            }
            card.style.width = newWidth + "px";
        } else if (isResizeBottom || isResizeTop) {
            let rawDelta = isResizeBottom ? deltaY : -deltaY;
            if (isAlt) rawDelta *= 2;
            let newHeight = Math.round(startHeight + rawDelta);
            newHeight = Math.max(minH, Math.min(newHeight, 800));
            const heightDiff = newHeight - startHeight;
            if (isAlt) {
              card.style.top = (startTop - heightDiff / 2) + "px";
            } else if (isResizeTop) {
              card.style.top = (startTop - heightDiff) + "px";
            }
            card.style.height = newHeight + "px";
        }
      } else {
        const at = snapMove(card, startLeft + deltaX, startTop + deltaY);
        card.style.left = at.x + "px";
        card.style.top = at.y + "px";
      }

      if (isResize && card.classList.contains("glyph-bar-widget")) {
        card._glyphBarGeometry.layout();
      }
    };

    const onMove = (ev) => {
      if (ev.pointerId !== pointerId) return;
      queuedEvent = ev;
      if (moveRaf) return;
      moveRaf = requestAnimationFrame(() => {
        moveRaf = 0;
        const latest = queuedEvent;
        queuedEvent = null;
        if (latest && !finished) applyMove(latest);
      });
    };

    const finishDrag = (ev, cancelled = false) => {
      if (finished || ev.pointerId !== pointerId) return;
      finished = true;
      if (moveRaf) cancelAnimationFrame(moveRaf);
      moveRaf = 0;
      if (queuedEvent) applyMove(queuedEvent);
      queuedEvent = null;
      if (!cancelled) applyMove(ev);
      document.removeEventListener("pointermove", onMove);
      document.removeEventListener("pointerup", onUp);
      document.removeEventListener("pointercancel", onCancel);
      
      card.classList.remove("dragging");
      
      // Сохраняем ТОЛЬКО геометрию, не теряя пользовательских настроек
      // (authUser, autoShuffle, version и т.п.)
      let saveW = card.offsetWidth;
      let saveH = card.offsetHeight;
      // Glyph bar: store square side + derived height (offsetWidth was stuck at
      // min-width:64 and blew the LEDs up on next load).
      if (card.classList.contains("glyph-bar-widget")) {
        const laid = card._glyphBarGeometry.layout();
        if (laid) {
          saveW = laid.side;
          saveH = laid.height;
        }
      }
      // In memory only: «Готово» saves the whole arrangement, «Отменить» drops it.
      config.positions[widgetId] = {
        ...config.positions[widgetId],
        x: parseInt(card.style.left),
        y: parseInt(card.style.top),
        w: saveW,
        h: saveH,
      };
      // The free space just changed shape: anything that fills what is left over
      // has to recompute now, not on the next tick.
      notifyGeometryChanged("drag");

      requestAnimationFrame(() => {
        card.style.transition = "";
      });
    };

    const onUp = (ev) => finishDrag(ev, false);
    const onCancel = (ev) => finishDrag(ev, true);

    document.addEventListener("pointermove", onMove);
    document.addEventListener("pointerup", onUp);
    document.addEventListener("pointercancel", onCancel);
  });
}

let _desktopOutsideClickReady = false;
function setupDesktopOutsideClickHandler() {
  if (_desktopOutsideClickReady) return;
  _desktopOutsideClickReady = true;

  let startX = 0;
  let startY = 0;

  document.addEventListener("pointerdown", (e) => {
    if (e.button !== 0 || editMode) return;
    startX = e.clientX;
    startY = e.clientY;
  }, true);

  document.addEventListener("pointerup", (e) => {
    if (e.button !== 0 || editMode) return;
    // Dead-zone check for dragging (< 6px)
    const dist = Math.hypot(e.clientX - startX, e.clientY - startY);
    if (dist >= 6) return;

    // LIFO priority: If surfaceManager has active surfaces (popups, explorer, menus),
    // let surfaceManager handle dismissal first — do not touch desktop widgets.
    // (The Telegram widget card is a surface too, but a desktop empty-click
    // must not dismiss it — only Escape / its own toggle folds it.)
    const nonTelegramSurface = (surfaceManager?.activeStack || []).some(
      (rec) => !rec.element?.classList?.contains("telegram-widget"),
    );
    if (nonTelegramSurface) return;

    // Filter out clicks inside widget cards, topbar buttons, add popovers, portals, etc.
    if (e.target.closest(".widget-card, .topbar-actions, .widget-add, #shell-portals, .explorer-overlay, .menu-container, .arrange-bar, .toast-host, .debug-message")) {
      return;
    }

    if (document.querySelector(".widget-card.expanded")) {
      collapseActiveDesktopWidget();
    }
  });
}

// в”Ђв”Ђв”Ђ Edit mode toggle в”Ђв”Ђв”Ђ



// One arrangement session: entering snapshots the layout, «Готово» pins every
// card to its nearest corner and saves once, «Отменить» / Esc restores it all.
const arrangeBar = document.getElementById("arrangeBar");
let arrangeSnapshot = null;
let arrangeOnExit = null;

function enterArrangeMode(onExit = null) {
  if (editMode) return;
  arrangeSnapshot = {
    ...structuredClone({
      positions: config.positions || {},
      hidden: config.hidden || [],
      instances: config.instances || [],
    }),
    createdFolders: [],
  };
  arrangeOnExit = onExit;
  editMode = true;
  document.body.classList.add("edit-mode");
  document.querySelectorAll(".widget-card.expanded").forEach((c) => {
    if (c.classList.contains("telegram-widget")) {
      import("../modules/telegram/widget.js").then((m) => m.collapseTelegramWidget(c, c.dataset.id)).catch(() => {});
      return;
    }
    document.querySelectorAll(".music-widget").forEach((musicCard) => {
      import("../modules/mp3-player/widget.js").then((m) => m.closeAllMusicPopups(musicCard)).catch(() => {});
    });
    collapseSingleWidget(c);
  });
  if (arrangeBar) {
    arrangeBar.hidden = false;
    arrangeBar.querySelector('[data-arrange="done"]')?.focus();
  }
}

async function leaveArrangeMode(commit) {
  if (!editMode) return;
  editMode = false;
  document.body.classList.remove("edit-mode");
  closeWidgetAddPopover();
  if (arrangeBar) {
    arrangeBar.hidden = true;
    arrangeBar.querySelector(".arrange-folder").hidden = true;
  }
  const snapshot = arrangeSnapshot;
  arrangeSnapshot = null;
  if (commit) {
    widgetsCanvas.querySelectorAll(":scope > .widget-card").forEach((card) => {
      const pos = config.positions?.[card.dataset.id];
      if (!pos || card._geometryController || card.classList.contains("expanded")) return;
      if (!card.offsetWidth && !pos.w) return;
      pinToNearestCorner(pos, card);
    });
    await saveConfig();
  } else if (snapshot) {
    const { createdFolders, ...layout } = snapshot;
    for (const id of createdFolders) await invoke("delete_folder", { id }).catch(() => {});
    Object.assign(config, layout);
    await saveConfig();
    await syncWidgets();
    widgetsCanvas.querySelectorAll(":scope > .widget-card").forEach((card) => {
      const pos = config.positions[card.dataset.id];
      if (!pos || card._geometryController) return;
      if (pos.w) card.style.width = pos.w + "px";
      if (pos.h) card.style.height = pos.h + "px";
      card._glyphBarGeometry?.layout();
    });
    relayoutWidgets();
  }
  notifyGeometryChanged("arrange");
  const onExit = arrangeOnExit;
  arrangeOnExit = null;
  onExit?.(commit);
}

arrangeBar?.addEventListener("click", (e) => {
  const action = e.target.closest("[data-arrange]")?.dataset.arrange;
  if (action === "done" || action === "cancel") leaveArrangeMode(action === "done");
  if (action === "folder") {
    const form = arrangeBar.querySelector(".arrange-folder");
    form.hidden = !form.hidden;
    if (!form.hidden) form.querySelector("input").focus();
  }
  const icon = e.target.closest("[data-folder-icon]");
  if (icon) {
    arrangeBar.querySelectorAll("[data-folder-icon]").forEach((b) => b.setAttribute("aria-checked", String(b === icon)));
  }
});

// «+ Папка» while arranging: the folder lands on the desk at once, ready to drag.
arrangeBar?.querySelector(".arrange-folder")?.addEventListener("submit", async (e) => {
  e.preventDefault();
  const form = e.currentTarget;
  const input = form.querySelector("input");
  const iconKey = form.querySelector('[data-folder-icon][aria-checked="true"]')?.dataset.folderIcon || "folder";
  const { createFolder } = await import("./folders.js");
  const folder = await createFolder(input.value, iconKey);
  // The new card counts as part of this arrangement: «Отменить» removes it.
  arrangeSnapshot?.createdFolders.push(folder.id);
  input.value = "";
  form.hidden = true;
});

// Esc has one meaning while arranging: cancel. Open surfaces (popups) are
// closed first by surface-manager, which stops the event before it gets here.
window.addEventListener("keydown", (e) => {
  if (!editMode || e.key !== "Escape") return;
  e.preventDefault();
  leaveArrangeMode(false);
});

// With every widget hidden the desktop is blank; point at the way back.
const deskHint = document.getElementById("deskHint");
function updateDeskHint() {
  if (!deskHint) return;
  deskHint.hidden = _isLoadingWidgets || !!widgetsCanvas.querySelector(":scope > .widget-card");
}

document.addEventListener("click", (e) => {
  if (!e.target.closest(".widget-add")) closeWidgetAddPopover();
});

// в”Ђв”Ђв”Ђ Debug в”Ђв”Ђв”Ђ



const explorerOverlay = document.getElementById("explorerOverlay");
const explorerContainer = document.getElementById("explorerContainer");

// Load Explorer from the renderer that is already known to be running.  A
// separate module script failed silently in WebView2, leaving only the static
// HTML shell visible.  Keeping the import here also lets us persist the actual
// module error in shell.log if startup ever regresses.
import("../modules/explorer/explorer.js").catch((error) => {
  const message = `explorer module failed: ${error?.stack || error?.message || error}`;
  console.error(message);
  invoke("frontend_log", { message }).catch(() => {});
});

function openExplorer(path, triggerOrigin = null) {
  const targetPath =
    path != null && String(path).trim() !== "" ? String(path).trim() : null;

  if (explorerOverlay && explorerContainer) {
    const explorerEl = document.getElementById("explorer");
    surfaceManager.openSurface(explorerContainer, {
      backdrop: explorerOverlay,
      origin: triggerOrigin || null,
      originRadius: "16px",
      content: explorerEl && explorerEl !== explorerContainer ? [explorerEl] : [],
      elevation: "var(--z-surface-overlay, 2000)",
      onOpen: () => {
        explorerOverlay.classList.add("visible");
      },
      onClosed: () => {
        explorerOverlay.classList.remove("visible");
      },
    });
  } else {
    explorerOverlay?.classList.add("visible");
  }

  const openTab = () => {
    const tm = window.nothingExplorer?.tabManager;
    if (!targetPath || !tm) return;
    // Don't duplicate an already-open tab for the same path (e.g. the path
    // the app was launched with is also restored from redb).
    const exists = tm.tabs.some((t) => t.path === targetPath);
    if (exists) {
      tm.switchTab(tm.tabs.find((t) => t.path === targetPath).id);
      return;
    }
    tm.addTab(targetPath).catch((err) => {
      console.error("explorer addTab failed:", err);
    });
  };

  if (window.nothingExplorer?.tabManager) {
    openTab();
    return;
  }
  window.addEventListener("explorer-ready", openTab, { once: true });
}

function closeExplorer() {
  if (explorerContainer && surfaceManager.isOpen(explorerContainer)) {
    surfaceManager.closeSurface(explorerContainer);
  } else {
    explorerOverlay?.classList.remove("visible");
  }
}

function collapseAllShellWidgets() {
  let anyCollapsed = false;

  // LIFO: If any surface is open in SurfaceManager, close strictly the topmost one!
  // The Telegram widget card itself is a surface now, but desktop Escape must
  // still fold it — so skip the telegram card here and let step 3 handle it
  // via collapseTelegramWidget (which runs the exit morph + dock restore).
  const top = surfaceManager.activeStack.length > 0
    ? surfaceManager.activeStack[surfaceManager.activeStack.length - 1]
    : null;
  if (top && !top.element?.classList?.contains("telegram-widget")) {
    surfaceManager.closeSurface(top.element);
    return true;
  }
  if (top) {
    import("../modules/telegram/widget.js").then((m) => m.collapseTelegramWidget(top.element, top.element.dataset.id)).catch(() => {});
    return true;
  }

  // 1. Context menus, lightboxes, modals
  import("../modules/telegram/widget.js").then((m) => m.closeTelegramContextMenu()).catch(() => {});
  document.querySelectorAll(".tg-lightbox-overlay, .tg-forward-modal-overlay").forEach((el) => {
    el.remove();
    anyCollapsed = true;
  });
  if (window.nothingExplorer?.contextMenu) {
    window.nothingExplorer.contextMenu.hide();
  }

  // 2. Shell Explorer overlay
  if (explorerOverlay?.classList.contains("visible")) {
    closeExplorer();
    anyCollapsed = true;
  }

  // 3. Desktop widgets (Telegram, folders, dashboards, clock)
  if (collapseActiveDesktopWidget()) {
    anyCollapsed = true;
  }

  // 4. Close any open popups in music widgets
  document.querySelectorAll(".music-widget").forEach((musicCard) => {
    import("../modules/mp3-player/widget.js").then((m) => m.closeAllMusicPopups(musicCard)).catch(() => {});
    anyCollapsed = true;
  });

  return anyCollapsed;
}

explorerOverlay?.addEventListener("click", (e) => {
  if (e.target === explorerOverlay) closeExplorer();
});

// ─── Smart Desktop Text Focus & Two-Tier Escape Management ───
// A DOM input can stay active after Alt+Tab; only an explicit editable interaction rearms it.
let desktopFocusGeneration = 0;
const editableTarget = (node) => {
  const element = node?.closest?.("input, textarea, [contenteditable]");
  return element && !element.disabled && !element.readOnly &&
    (element.matches("input, textarea") || element.isContentEditable);
};
const publishDesktopFocus = (focused) => {
  desktopFocusGeneration++;
  invoke("set_desktop_input_focus", { focused }).catch(() => {});
};
window.addEventListener("blur", () => publishDesktopFocus(false));
document.addEventListener("visibilitychange", () => {
  if (document.hidden) publishDesktopFocus(false);
});
document.addEventListener("pointerdown", (e) => {
  if (editableTarget(e.target)) publishDesktopFocus(true);
}, true);
document.addEventListener("focusin", (e) => {
  publishDesktopFocus(!!editableTarget(e.target));
});
document.addEventListener("focusout", () => {
  const generation = desktopFocusGeneration;
  setTimeout(() => {
    if (generation === desktopFocusGeneration && !editableTarget(document.activeElement)) {
      publishDesktopFocus(false);
    }
  }, 50);
});

document.addEventListener("keydown", (e) => {
  if (e.key === "Escape") {
    const active = document.activeElement;
    if (active && (active.matches("input, textarea, [contenteditable]") || active.closest?.("input, textarea, [contenteditable]"))) {
      active.blur();
      e.stopPropagation();
      e.preventDefault();
      invoke("set_desktop_input_focus", { focused: false }).catch(() => {});
      return;
    }
    collapseAllShellWidgets();
  }
}, true);

// Win+E / open_explorer command / tools shortcut → shell explorer overlay.
// (Do NOT use window.__TAURI__ — withGlobalTauri is off; use the ES module API.)
listen("explorer-open", (event) => {
  openExplorer(event.payload);
}).catch((err) => {
  console.error("explorer-open listen failed:", err);
});

window.addEventListener("shell-open-explorer", (e) => {
  openExplorer(e.detail?.path);
});

window.addEventListener("unidtk-open-explorer", (e) => {
  openExplorer(e.detail?.path);
});

// Listen for telegram-send-file events from explorer "Send to Telegram"
window.addEventListener("telegram-send-file", async (e) => {
  const filePath = e.detail?.path;
  if (!filePath) return;
  closeExplorer();
  const card = document.querySelector(`.widget-card.telegram-widget, .widget-card[data-id="telegram"], .widget-card[data-widget-type="telegram"]`);
  if (!card) return;
  const widgetId = card.dataset.id || "telegram";
  if (!card.classList.contains("expanded")) {
    import("../modules/telegram/widget.js").then((m) => m.toggleTelegramWidget(card, widgetId)).catch(() => {});
  }
  if (!telegramRuntime.chatsLoaded && !telegramRuntime.chats?.length) {
    if (!telegramRuntime.loadingChats) {
      const tg = await import("../modules/telegram/widget.js");
      await tg.refreshTelegramChats(card, widgetId);
    } else {
      for (let i = 0; i < 30; i++) {
        if (telegramRuntime.chatsLoaded || (telegramRuntime.chats && telegramRuntime.chats.length)) break;
        await new Promise((r) => setTimeout(r, 100));
      }
    }
  }
  import("../modules/telegram/widget.js").then((m) => m.openTelegramSendFilePicker(card, widgetId, filePath)).catch(() => {});
});

// Automatically re-align widgets whenever display configuration or window size changes.
// Uses soft relayout without destroying the DOM canvas or wiping innerHTML to prevent flickering.
// ─── Geometry announcements ───────────────────────────────────────────────────
// Permanent desktop footprints are independent of temporary expanded surfaces.
// Offset dimensions ignore the transform used by reveal/morph animations.
const _dockGeometry = new Map();
let _dockResizeObserver = null;
let _dockGeometryRaf = 0;

function captureDockGeometry(card) {
  if (!card?.isConnected || card.classList.contains("micrographics-card")) return false;
  const id = card.dataset.id;
  const prev = _dockGeometry.get(id);
  const widgetType = card.dataset.widgetType || id;
  const version = config.positions[id]?.version || "digital";
  const capabilities = widgetType === "clock" ? ["time", ...(["gooey", "gothic"].includes(version) ? ["date"] : [])] :
    widgetType === "glyph-bar" ? ["system-volume"] : [];
  const surface = surfaceManager.getRecord(card);
  const frozen = card.classList.contains("expanded") || card._dockGeometryFrozen || card._collapseTimer ||
    (surface && surface.state !== "closed");
  if (frozen && !prev) return false;
  const next = frozen ? { ...prev, widgetType, capabilities } :
    { id, widgetType, capabilities, x: parseFloat(card.style.left) || 0, y: parseFloat(card.style.top) || 0,
      w: card.offsetWidth, h: card.offsetHeight };
  if (!id || !next.w || !next.h) return false;
  if (prev && JSON.stringify(prev) === JSON.stringify(next)) return false;
  _dockGeometry.set(id, next);
  return true;
}

function getDockGeometry() {
  return [..._dockGeometry.values()].map((rect) => ({ ...rect, capabilities: [...rect.capabilities] }));
}

function observeDockGeometry() {
  _dockResizeObserver?.disconnect();
  if (_dockGeometryRaf) cancelAnimationFrame(_dockGeometryRaf);
  _dockGeometryRaf = 0;
  const cards = [...widgetsCanvas.querySelectorAll(".widget-card:not(.micrographics-card)")];
  cards.forEach(captureDockGeometry);
  _dockResizeObserver = new ResizeObserver(() => {
    if (_dockGeometryRaf) return;
    _dockGeometryRaf = requestAnimationFrame(() => {
      _dockGeometryRaf = 0;
      let changed = false;
      cards.forEach((card) => { if (captureDockGeometry(card)) changed = true; });
      if (changed) notifyGeometryChanged("dock-size");
    });
  });
  cards.forEach((card) => _dockResizeObserver.observe(card));
}

let _geometryVersion = 0;
function notifyGeometryChanged(reason) {
  if (["load", "drag", "relayout", "dock-content"].includes(reason)) {
    widgetsCanvas.querySelectorAll(".widget-card").forEach(captureDockGeometry);
  }
  _geometryVersion += 1;
  window.dispatchEvent(new CustomEvent("unidtk-geometry-changed", {
    detail: { version: _geometryVersion, reason: reason || "unknown" },
  }));
}

function relayoutWidgets() {
  const cards = widgetsCanvas.querySelectorAll(".widget-card");
  if (!cards.length) {
    syncWidgets();
    return;
  }
  cards.forEach((card) => {
    if (card._geometryController) { card._geometryController.relayout(); return; }
    const id = card.dataset.id;
    if (!id || !config.positions || !config.positions[id]) return;
    if (card.classList.contains("expanded")) return;
    const at = placeOnCanvas(config.positions[id], card);
    card.style.left = at.x + "px";
    card.style.top = at.y + "px";
  });
  updateDeskHint();
  refreshDomCaches();
  notifyGeometryChanged("relayout");
}

let displayChangeTimer = null;
function handleDisplayOrResizeChange() {
  if (displayChangeTimer) clearTimeout(displayChangeTimer);
  displayChangeTimer = setTimeout(() => {
    logFrontend("display or resize changed -> relayouting widgets");
    relayoutWidgets();
  }, 150);
}

listen("display-changed", handleDisplayOrResizeChange).catch(() => {});
window.addEventListener("resize", handleDisplayOrResizeChange);
window.visualViewport?.addEventListener("resize", handleDisplayOrResizeChange);
window.screen?.addEventListener?.("change", handleDisplayOrResizeChange);

loadWidgets();




if (typeof window !== "undefined") {
  window.surfaceManager = surfaceManager;
  window.unidtk = window.unidtk || {};
  window.unidtk.surfaceManager = surfaceManager;
  window.nothingShell = window.nothingShell || {};
  window.nothingShell.surfaceManager = surfaceManager;
}

export {
  config, editMode, widgetsCanvas, placeOnCanvas, enterArrangeMode, leaveArrangeMode, rlog, logFrontend, escapeHtml, repairMojibake,
  displayText, showDebugMessage, refreshDomCaches, defaultWidgetPosition,
  loadConfig, saveConfig, closeWidgetAddPopover,
  notifyGeometryChanged,
  getDockGeometry,
  renderWidgetAddPopover, ensureWidgetAddControls, createWidgetInstance,
  removeWidgetInstance, hideBaseWidget, loadWidgets, syncWidgets, relayoutWidgets, toggleWidget,
  collapseActiveDesktopWidget,
  setupDragReorder, openExplorer, closeExplorer, collapseAllShellWidgets,
  handleDisplayOrResizeChange, getShortcutIcon, _renderWidget, _startWidgetLoops,
  _closeMusicSatellite, cachedTelegramCard, lastBands, lastBandsAt,
  animate, spring, staggerEnter, fadeTransition, setMotionScale, EASINGS, DURATIONS,
  surfaceManager, openMenu, closeMenu,
  initThemeSystem, applyTheme, getActiveTheme, getAllPresets,
};
