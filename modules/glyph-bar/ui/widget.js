import { invoke, listen } from '@unidtk/bridge';
import { saveConfig, showDebugMessage, config, editMode } from '../../unidtk-core.js';

const GB_SEGMENTS = 10;
const GB_GAP = 3;
const GB_SIDE_DEFAULT = 10;
const GB_SIDE_MIN = 6;
const GB_SIDE_MAX = 64;

let systemVolumeState = { level: 0, muted: false };
let systemVolumeReady = false;

/** Volume % for segment index 0..9 (bottom→top). Index 0 is mute, not a level. */
function glyphBarVolumeForIndex(index) {
  if (index <= 0) return 0;
  return Math.min(100, (index + 1) * 10);
}

/** How many LEDs light from the bottom for a given level (0..10). */
function glyphBarLitCount(level, muted) {
  if (muted || level <= 0) return 0;
  return Math.min(GB_SEGMENTS, Math.max(1, Math.round(level / 10)));
}

/** Total widget height so 10 segments stay perfect squares with fixed gap. */
function glyphBarHeightForSide(side) {
  const s = Math.max(GB_SIDE_MIN, Math.round(side));
  return s * GB_SEGMENTS + GB_GAP * (GB_SEGMENTS - 1);
}

function glyphBarSideFromCard(card) {
  const fromStyle = parseFloat(card.style.width);
  if (Number.isFinite(fromStyle) && fromStyle > 0) {
    return Math.max(GB_SIDE_MIN, Math.min(GB_SIDE_MAX, Math.round(fromStyle)));
  }
  return Math.max(
    GB_SIDE_MIN,
    Math.min(GB_SIDE_MAX, Math.round(card.clientWidth || GB_SIDE_DEFAULT)),
  );
}

/** Fit square LED size from card width; height always follows the formula. */
function layoutGlyphBarSquares(card) {
  const segs = card.querySelectorAll(".gb-seg");
  if (!segs.length) return null;
  const side = glyphBarSideFromCard(card);
  const needH = glyphBarHeightForSide(side);
  card.style.width = `${side}px`;
  card.style.height = `${needH}px`;
  segs.forEach((seg) => {
    seg.style.width = `${side}px`;
    seg.style.height = `${side}px`;
  });
  card.style.setProperty("--gb-side", `${side}px`);
  return { side, height: needH };
}

function renderGlyphBar(card, widgetId) {
  card._glyphBarGeometry = {
    heightForSide: glyphBarHeightForSide,
    sideForHeight: height => Math.round((height - GB_GAP * (GB_SEGMENTS - 1)) / GB_SEGMENTS),
    clampSide: side => Math.max(GB_SIDE_MIN, Math.min(GB_SIDE_MAX, side)),
    layout: () => layoutGlyphBarSquares(card),
  };
  card.classList.add("glyph-bar-widget");
  card.classList.add("no-header");
  const pos = config.positions[widgetId] || {};
  // Prefer saved width as square side. Cap old oversized saves (min-width:64 bug).
  let side = Math.round(pos.w || GB_SIDE_DEFAULT);
  if (side > GB_SIDE_MAX || side < GB_SIDE_MIN) side = GB_SIDE_DEFAULT;
  const h = glyphBarHeightForSide(side);
  if (!config.positions[widgetId]) config.positions[widgetId] = { ...pos };
  if (config.positions[widgetId].w !== side || config.positions[widgetId].h !== h) {
    config.positions[widgetId].w = side;
    config.positions[widgetId].h = h;
    saveConfig();
  }
  card.style.width = `${side}px`;
  card.style.height = `${h}px`;

  // column-reverse: first child is visually at the bottom (red mute segment)
  const segs = Array.from({ length: GB_SEGMENTS }, (_, i) => {
    const red = i === 0 ? " red" : "";
    const pct = i === 0 ? 0 : glyphBarVolumeForIndex(i);
    return `<button type="button" class="gb-seg${red}" data-gb-index="${i}" aria-label="Volume ${pct}%"></button>`;
  }).join("");

  card.innerHTML += `<div class="gb-container">${segs}<span class="gb-knob" aria-hidden="true"></span></div>`;

  const container = card.querySelector(".gb-container");
  let suppressClick = false;
  container.addEventListener("click", (e) => {
    if (editMode) return;
    const seg = e.target.closest(".gb-seg");
    if (!seg) return;
    e.stopPropagation();
    if (suppressClick) { suppressClick = false; return; }
    const index = Number(seg.dataset.gbIndex);
    handleGlyphBarClick(index);
  });

  // Drag the level: it snaps to the nearest LED mark. Window-level listeners
  // (not pointer capture): on the desktop, input arrives as forwarded
  // messages, which pointer capture does not follow.
  container.addEventListener("pointerdown", (e) => {
    if (editMode || e.button !== 0) return;
    const startIndex = glyphBarIndexAt(container, e.clientY);
    let lastIndex = startIndex;
    const move = (ev) => {
      const index = glyphBarIndexAt(container, ev.clientY);
      if (index === lastIndex) return;
      lastIndex = index;
      suppressClick = true;
      dragGlyphBarTo(index);
    };
    const up = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      window.removeEventListener("pointercancel", up);
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
    window.addEventListener("pointercancel", up);
  });

  // Native wheel when webview has focus; wallpaper-wheel covers desktop mode.
  container.addEventListener(
    "wheel",
    (e) => {
      if (editMode) return;
      e.preventDefault();
      e.stopPropagation();
      const dir = e.deltaY < 0 ? 1 : -1;
      nudgeSystemVolume(dir * 10);
    },
    { passive: false },
  );

  layoutGlyphBarSquares(card);
  applyGlyphBarVisual(card);
  if (!systemVolumeReady) refreshSystemVolume();
}

function applyGlyphBarVisual(card) {
  const segs = card.querySelectorAll(".gb-seg");
  if (!segs.length) return;
  const { level, muted } = systemVolumeState;
  const lit = glyphBarLitCount(level, muted);
  // Jelly draws the level as one continuous fill with a sliding knob (glyph-bar.css).
  card.style.setProperty("--gb-lit", String(muted ? 0 : lit));
  card.style.setProperty("--gb-gap", `${GB_GAP}px`);
  card.classList.toggle("gb-has-level", !muted && lit > 0);

  segs.forEach((seg) => {
    const i = Number(seg.dataset.gbIndex);
    // i < lit ⇒ clicking the Nth LED (1-based) lights N segments including itself.
    seg.classList.toggle("on", !muted && i < lit);
    seg.classList.toggle("muted-only", muted && i === 0);
  });
}

function applyGlyphBarVisualAll() {
  document.querySelectorAll(".glyph-bar-widget").forEach(applyGlyphBarVisual);
}

async function refreshSystemVolume() {
    try {
    const v = await invoke("get_system_volume");
    if (v && typeof v.level === "number") {
      systemVolumeState = { level: v.level, muted: !!v.muted };
      systemVolumeReady = true;
      applyGlyphBarVisualAll();
    }
  } catch (err) {
    console.warn("getSystemVolume failed", err);
  }
}

async function handleGlyphBarClick(index) {
  if (index === 0) {
    // Red segment: mute (toggle if already muted → unmute)
    try {
      const nextMuted = !systemVolumeState.muted;
      systemVolumeState = { ...systemVolumeState, muted: nextMuted };
      applyGlyphBarVisualAll();
      const v = await invoke("set_system_mute", { muted: nextMuted });
      if (v) systemVolumeState = { level: v.level, muted: !!v.muted };
      applyGlyphBarVisualAll();
    } catch (err) {
      showDebugMessage(`mute: ${err?.message || err}`);
    }
    return;
  }

  // 2nd from bottom → 20%, …, 9th → 90%, 10th (top) → 100%.
  const target = glyphBarVolumeForIndex(index);
  try {
    systemVolumeState = { level: target, muted: false };
    applyGlyphBarVisualAll();
    const v = await invoke("set_system_volume", { level: target });
    if (v) systemVolumeState = { level: v.level, muted: !!v.muted };
    applyGlyphBarVisualAll();
  } catch (err) {
    showDebugMessage(`volume: ${err?.message || err}`);
  }
}

/** LED index (0 = bottom) under a client Y, clamped to the bar. */
function glyphBarIndexAt(container, clientY) {
  const rect = container.getBoundingClientRect();
  const pitch = rect.height / GB_SEGMENTS;
  return Math.max(0, Math.min(GB_SEGMENTS - 1, Math.floor((rect.bottom - clientY) / pitch)));
}

// Drag levels: index i lights i+1 LEDs, i.e. (i+1)*10 %. The visual moves at
// once; the system call is chained so the last mark always wins.
let glyphDragChain = Promise.resolve();
function dragGlyphBarTo(index) {
  const level = (index + 1) * 10;
  systemVolumeState = { level, muted: false };
  applyGlyphBarVisualAll();
  glyphDragChain = glyphDragChain.then(async () => {
    if (systemVolumeState.level !== level) return;
    try {
      const v = await invoke("set_system_volume", { level });
      if (v && systemVolumeState.level === level) systemVolumeState = { level: v.level, muted: !!v.muted };
    } catch (err) {
      showDebugMessage(`volume: ${err?.message || err}`);
    }
  });
}

async function nudgeSystemVolume(delta) {
  let next = systemVolumeState.level;
  if (systemVolumeState.muted && delta > 0) {
    next = Math.min(100, Math.max(10, delta));
  } else if (systemVolumeState.muted && delta < 0) {
    return;
  } else {
    next = Math.max(0, Math.min(100, systemVolumeState.level + delta));
  }
  try {
    if (next === 0) {
      const v = await invoke("set_system_mute", { muted: true });
      if (v) systemVolumeState = { level: v.level, muted: !!v.muted };
    } else {
      const v = await invoke("set_system_volume", { level: next });
      if (v) systemVolumeState = { level: v.level, muted: !!v.muted };
    }
    applyGlyphBarVisualAll();
  } catch (err) {
    showDebugMessage(`volume: ${err?.message || err}`);
  }
}

listen("system-volume", (event) => {
  const v = event.payload;
  if (!v || typeof v.level !== "number") return;
  systemVolumeState = { level: v.level, muted: !!v.muted };
  systemVolumeReady = true;
  applyGlyphBarVisualAll();
}).catch(() => {});

export { renderGlyphBar, layoutGlyphBarSquares, glyphBarHeightForSide, nudgeSystemVolume, applyGlyphBarVisualAll };
