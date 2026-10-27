/**
 * UniDTK — Core Theme & Style Engine (theme.js)
 *
 * Single source of truth for runtime appearance tokens: the versioned theme
 * schema, the Nothing Classic + Sea Glass built-in presets, user-custom
 * presets, GPU-safe live preview throttling, and zero-FOUC persistence.
 *
 * Schema v4 — theme fields (v3 + material, palette, per-surface tints):
 *   accent           hex    accent color of the interface
 *   bgBase           rgb    glass panel base color ("r, g, b")
 *   fgRgb            rgb    text / icon / stroke channel
 *   bgElevRgb        rgb    elevated tint channel (fills, hairlines on glass)
 *   scrimRgb         rgb    dark overlay channel (scrims, shadows)
 *   surfaceRgb       rgb    solid panel surface color
 *   surfaceDeepRgb   rgb    deep solid surface color (gradients, wells)
 *   statusOk/Warn/Error/Info  hex   semantic status colors
 *   glassOpacity, blur, radiusCard, radiusSurface, motionScale, motionProfile
 *   fontDisplay      css    display typeface stack (clock digits, headers)
 *   fontBody         css    body typeface stack (readable text)
 *   fontMono         css    monospace stack (code, paths, debug values)
 *   sizes            map    default widget geometry {id: {w, h}};
 *                    "folder" key = default for all user folder widgets.
 *                    Sizes are DEFAULTS, not layout: a user-resized widget
 *                    always wins on preset switch (migrateSizesToPreset);
 *                    positions (x/y) are never touched by themes.
 *   material         enum   "flat" (Classic cards) | "jelly" (sea-glass material)
 *   palette          map    {slot: hex} pastel slots (PALETTE_SLOTS)
 *   tintMap          map    {target: slot} — which slot tints which surface
 *                    (TINT_TARGETS: widgets, panels, micrographics groups)
 *   glassGrain, glassDepth, inkDepth, shadowOffset, radiusStrip
 *                    jelly optics: grain alpha, bevel strength, ink lightness
 *                    (oklch L), shadow drop in px, strip radius in px
 *   mgLayout         enum   "panel" | "pills" (micrographics backdrop)
 *   avatarColor      bool   colored Telegram avatars (false = grayscale)
 *
 * Migration: a built-in preset persisted under an older schema takes the
 * current built-in style; only its widget sizes survive (flag `migrated`,
 * healed into storage by applyTheme).
 *
 * Invariants:
 *   - Every persisted theme (localStorage, redb, legacy Shell) is normalized
 *     onto the full schema with the built-in defaults as base — partial or
 *     legacy records can never inherit fields of a previously active theme.
 *   - theme.js writes ONLY primitive custom properties to :root. All derived
 *     values (glow, glass panels, borders) are computed aliases in styles.css.
 *   - The static :root fallbacks in styles.css MUST equal the Nothing Classic
 *     preset (FOUC invariant for the first paint before JS runs).
 *   - --dur-* / --ease-* ownership stays with motion.js; theme.js only calls
 *     setMotionScale / setMotionProfile.
 */

import { saveWidgetConfig } from "./widgets-api.js";
import { setMotionScale, setMotionProfile } from "./motion.js";

export const SCHEMA_VERSION = 4;

// Validated geometry defaults shared by the built-in presets (live-verified
// against the 2026-10-20 reference desktop). Sizes only — positions (x/y)
// are user data and never live in presets.
const PRESET_GEOMETRY = {
  clock: { w: 207, h: 187 },
  "mp3-player": { w: 250, h: 250 },
  telegram: { w: 55, h: 593 },
  "glyph-bar": { w: 14, h: 167 },
  folder: { w: 67, h: 67 },
};

const FONT_DISPLAY_OPTIONS = new Set([
  "'Ndot', monospace",
  "'Geist', sans-serif",
  "'Segoe UI', 'Geist', sans-serif",
  "system-ui, sans-serif",
]);

const FONT_BODY_OPTIONS = new Set([
  "'Geist', sans-serif",
  "'Segoe UI', 'Geist', sans-serif",
  "system-ui, sans-serif",
  "'Ndot', monospace",
]);

const FONT_MONO_OPTIONS = new Set([
  "'Consolas', 'Courier New', monospace",
  "'Cascadia Code', 'Consolas', monospace",
  "ui-monospace, monospace",
]);

export const PALETTE_SLOTS = ["neutral", "blue", "aqua", "mint", "lilac", "peach", "magenta", "coral"];

const MG_GROUPS = ["date", "time", "freshness", "temperature", "condition", "feels", "wind",
  "precipitation", "precipitation-amount", "sunrise", "sunset", "daylight",
  "geolocation", "location", "ip-location", "network", "volume"];

export const TINT_TARGETS = ["panel", "folder", "clock", "telegram", "mp3-player", "glyph-bar",
  ...MG_GROUPS.map((g) => `mg.${g}`)];

// Equal lightness/chroma pastels (sea glass pebbles); ink is derived in CSS.
const SEA_PALETTE = {
  neutral: "#f2f4f7",
  blue: "#a9c2f5",
  aqua: "#9ee3e0",
  mint: "#b5e8b0",
  lilac: "#c9b8f2",
  peach: "#ffd49a",
  magenta: "#f2a6d8",
  coral: "#f7a38c",
};

// Micrographics tinted by meaning: calendar/place blue, weather aqua, sky/sun mint.
const MG_TINTS = {
  date: "blue", time: "blue", location: "blue", "ip-location": "blue", geolocation: "blue", network: "blue", volume: "blue",
  temperature: "aqua", condition: "aqua", feels: "aqua", freshness: "aqua",
  wind: "mint", precipitation: "mint", "precipitation-amount": "mint", sunrise: "mint", sunset: "mint", daylight: "mint",
};

const SEA_TINTS = {
  panel: "lilac",
  folder: "mint",
  clock: "lilac",
  telegram: "peach",
  "mp3-player": "magenta",
  "glyph-bar": "aqua",
  ...Object.fromEntries(MG_GROUPS.map((g) => [`mg.${g}`, MG_TINTS[g]])),
};

function freezePreset(p) {
  for (const key of ["sizes", "palette", "tintMap"]) Object.freeze(p[key]);
  return Object.freeze(p);
}

export const BUILTIN_PRESETS = Object.freeze([
  freezePreset({
    id: "nothing-dark",
    name: "Nothing Classic",
    isBuiltin: true,
    schemaVersion: SCHEMA_VERSION,
    accent: "#d71921",
    bgBase: "15, 18, 30",
    fgRgb: "255, 255, 255",
    bgElevRgb: "255, 255, 255",
    scrimRgb: "0, 0, 0",
    surfaceRgb: "18, 18, 24",
    surfaceDeepRgb: "12, 12, 16",
    statusOk: "#34c759",
    statusWarn: "#ffd60a",
    statusError: "#ff3b30",
    statusInfo: "#0a84ff",
    glassOpacity: 0.78,
    glassRim: 0,
    glassSheen: 0,
    glassShadow: 0,
    blur: 16,
    radiusCard: 16,
    radiusSurface: 24,
    motionScale: 1.0,
    motionProfile: "balanced",
    fontDisplay: "'Ndot', monospace",
    fontBody: "'Geist', sans-serif",
    fontMono: "'Consolas', 'Courier New', monospace",
    sizes: { ...PRESET_GEOMETRY },
    material: "flat",
    palette: { ...SEA_PALETTE },
    tintMap: { ...SEA_TINTS },
    glassGrain: 0,
    glassDepth: 0,
    inkDepth: 0.3,
    shadowOffset: 0,
    radiusStrip: 24,
    surfaceTint: 0.35,
    mgLayout: "panel",
    avatarColor: false,
  }),
  freezePreset({
    id: "sea-glass",
    name: "Sea Glass",
    isBuiltin: true,
    schemaVersion: SCHEMA_VERSION,
    accent: "#4a5fc1",
    bgBase: "242, 244, 247",
    fgRgb: "30, 34, 48",
    bgElevRgb: "255, 255, 255",
    scrimRgb: "40, 48, 70",
    surfaceRgb: "242, 244, 247",
    surfaceDeepRgb: "226, 230, 238",
    statusOk: "#1a7f37",
    statusWarn: "#9a6700",
    statusError: "#c4402a",
    statusInfo: "#0969da",
    glassOpacity: 0.9,
    glassRim: 0.85,
    glassSheen: 0.6,
    glassShadow: 0.6,
    blur: 0,
    radiusCard: 20,
    radiusSurface: 30,
    motionScale: 1.0,
    motionProfile: "balanced",
    fontDisplay: "'Ndot', monospace",
    fontBody: "'Segoe UI', 'Geist', sans-serif",
    fontMono: "'Consolas', 'Courier New', monospace",
    sizes: { ...PRESET_GEOMETRY },
    material: "jelly",
    palette: { ...SEA_PALETTE },
    tintMap: { ...SEA_TINTS },
    glassGrain: 0.22,
    glassDepth: 0.6,
    inkDepth: 0.3,
    shadowOffset: 10,
    radiusStrip: 999,
    surfaceTint: 0.35,
    mgLayout: "pills",
    avatarColor: true,
  }),
]);

const BUILTIN_BY_ID = new Map(BUILTIN_PRESETS.map((p) => [p.id, p]));

// Presets retired in favor of the single Nothing Classic baseline.
// Persisted themes referencing them silently fall back to the default preset.
const RETIRED_PRESET_IDS = new Set([
  "pure-oled",
  "cyber-amber",
  "frost-glass",
  "monochrome-slate",
]);

function isRetiredTheme(theme) {
  return Boolean(theme && RETIRED_PRESET_IDS.has(theme.id));
}

const STORAGE_KEY = "unidtk_theme_state";

// ── Schema validation ───────────────────────────────────────────────────────

const HEX_RE = /^#(?:[0-9a-fA-F]{3}|[0-9a-fA-F]{6})$/;
const MOTION_PROFILE_IDS = new Set(["responsive", "balanced", "fluid"]);
const ID_RE = /^[\w.-]{1,64}$/;

const HEX_FIELDS = ["accent", "statusOk", "statusWarn", "statusError", "statusInfo"];
const RGB_FIELDS = ["bgBase", "fgRgb", "bgElevRgb", "scrimRgb", "surfaceRgb", "surfaceDeepRgb"];
const NUM_FIELDS = {
  glassOpacity: [0, 1, false],
  glassRim: [0, 1, false],
  glassSheen: [0, 1, false],
  glassShadow: [0, 1, false],
  blur: [0, 48, true],
  radiusCard: [0, 32, true],
  radiusSurface: [0, 48, true],
  motionScale: [0, 3, false],
  glassGrain: [0, 1, false],
  glassDepth: [0, 1, false],
  // Ceiling keeps ink >= 7:1 on every pastel (0.45 would drop to ~4.8:1).
  inkDepth: [0.15, 0.38, false],
  surfaceTint: [0, 1, false],
  shadowOffset: [0, 24, true],
  radiusStrip: [0, 999, true],
};

const ENUM_FIELDS = {
  material: new Set(["flat", "jelly"]),
  mgLayout: new Set(["panel", "pills"]),
};

const FONT_OPTIONS = {
  fontDisplay: FONT_DISPLAY_OPTIONS,
  fontBody: FONT_BODY_OPTIONS,
  fontMono: FONT_MONO_OPTIONS,
};

/**
 * Validate a sizes map ({id: {w, h}}) — clamps to sane card bounds.
 * Returns a fresh object; unknown/malformed entries are dropped.
 */
function normalizeSizes(rawSizes, fallback) {
  const out = {};
  if (rawSizes && typeof rawSizes === "object") {
    for (const [id, s] of Object.entries(rawSizes)) {
      if (typeof id !== "string" || id.length === 0 || id.length > 64) continue;
      if (!s || typeof s !== "object") continue;
      const w = clampNumber(s.w, 8, 2000);
      const h = clampNumber(s.h, 8, 2000);
      if (w === null || h === null) continue;
      out[id] = { w: Math.round(w), h: Math.round(h) };
    }
  }
  return { ...fallback, ...out };
}

/**
 * Validate a {key: value} map against allowed keys; invalid entries keep
 * the fallback's value. Returns a fresh object (never the fallback itself).
 */
function normalizeMap(raw, fallback, keys, valueOk) {
  const out = { ...fallback };
  if (raw && typeof raw === "object") {
    for (const key of keys) {
      if (valueOk(raw[key])) out[key] = raw[key];
    }
  }
  return out;
}

/** Deep copy of a theme record (nested maps included). */
function cloneTheme(t) {
  return { ...t, sizes: { ...(t.sizes || {}) }, palette: { ...(t.palette || {}) }, tintMap: { ...(t.tintMap || {}) } };
}

/**
 * Convert hex color (#rgb or #rrggbb) to "r, g, b" string.
 */
export function hexToRgb(hex) {
  if (!hex || typeof hex !== "string") return "255, 255, 255";
  let clean = hex.replace("#", "").trim();
  if (clean.length === 3) clean = clean.split("").map((c) => c + c).join("");
  if (clean.length >= 6) {
    const r = parseInt(clean.substring(0, 2), 16) || 0;
    const g = parseInt(clean.substring(2, 4), 16) || 0;
    const b = parseInt(clean.substring(4, 6), 16) || 0;
    return `${r}, ${g}, ${b}`;
  }
  return "255, 255, 255";
}

// ── Ink derivation (OKLCH) ──────────────────────────────────────────────────
// Ink = the tint's own hue at lightness `inkDepth`, chroma boosted so text
// reads as deep indigo on blue / plum on magenta rather than grey. Computed
// here (not via CSS relative color) so it can feed the "r, g, b" --fg-rgb
// channel every widget already paints with.

function srgbToLinear(v) {
  v /= 255;
  return v <= 0.04045 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4;
}

function linearToSrgb(v) {
  const s = v <= 0.0031308 ? v * 12.92 : 1.055 * v ** (1 / 2.4) - 0.055;
  return Math.round(Math.max(0, Math.min(1, s)) * 255);
}

function hexToOklch(hex) {
  const [r, g, b] = hexToRgb(hex).split(",").map((x) => srgbToLinear(Number(x)));
  const l = Math.cbrt(0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b);
  const m = Math.cbrt(0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b);
  const s = Math.cbrt(0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b);
  const L = 0.2104542553 * l + 0.793617785 * m - 0.0040720468 * s;
  const A = 1.9779984951 * l - 2.428592205 * m + 0.4505937099 * s;
  const B = 0.0259040371 * l + 0.7827717662 * m - 0.808675766 * s;
  return [L, Math.hypot(A, B), Math.atan2(B, A)];
}

function oklchToLinear(L, C, H) {
  const A = C * Math.cos(H), B = C * Math.sin(H);
  const l = (L + 0.3963377774 * A + 0.2158037573 * B) ** 3;
  const m = (L - 0.1055613458 * A - 0.0638541728 * B) ** 3;
  const s = (L - 0.0894841775 * A - 1.291485548 * B) ** 3;
  return [
    4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s,
    -1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s,
    -0.0041960863 * l - 0.7034186147 * m + 1.707614701 * s,
  ];
}

/** Deep same-hue ink for a pastel, as "r, g, b" (chroma reduced into sRGB gamut). */
export function inkFor(hex, depth) {
  const [, c, h] = hexToOklch(hex);
  let chroma = Math.min(0.16, c * 2 + 0.05);
  let rgb = oklchToLinear(depth, chroma, h);
  while (rgb.some((v) => v < 0 || v > 1) && chroma > 0.005) {
    chroma *= 0.92;
    rgb = oklchToLinear(depth, chroma, h);
  }
  return rgb.map(linearToSrgb).join(", ");
}

function linearToHex(rgb) {
  return `#${rgb.map((v) => linearToSrgb(v).toString(16).padStart(2, "0")).join("")}`;
}

const inGamut = (rgb) => rgb.every((v) => v >= -1e-4 && v <= 1 + 1e-4);

// ── Per-user colors ─────────────────────────────────────────────────────────
// Every user gets their own Sea Glass coloring: seven pastels of equal OKLCH
// lightness at random, well separated hues (olive band excluded — it reads as
// dirty at pastel lightness), dealt as a permutation over the seven colored
// roles. Coral keeps its hue: it doubles as the warm accent (mute, pins). Chroma is capped to the sRGB gamut per hue so no hue shifts on
// clipping. rollColors is pure; the random seed comes from newColorSeed().

const ROLL_L = 0.86;
const ROLL_C = 0.08;
const ROLL_MIN_GAP = 35;
const OLIVE_BAND = [90, 125];
const CHROMA_SLOTS = PALETTE_SLOTS.filter((s) => s !== "neutral");

// Seven colored roles; micrographics rows share their row's role.
const ROLL_ROLES = {
  folder: ["folder"],
  clock: ["clock"],
  telegram: ["telegram"],
  player: ["mp3-player"],
  calendar: ["mg.date", "mg.time", "mg.location", "mg.ip-location", "mg.geolocation", "mg.network", "mg.volume"],
  weather: ["mg.temperature", "mg.condition", "mg.feels", "mg.freshness"],
  sky: ["mg.wind", "mg.precipitation", "mg.precipitation-amount", "mg.sunrise", "mg.sunset", "mg.daylight"],
};

function mulberry32(seed) {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = Math.imul(a ^ (a >>> 15), 1 | a);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

function pastelHex(hueDeg) {
  const h = (hueDeg * Math.PI) / 180;
  let lo = 0, hi = ROLL_C;
  if (!inGamut(oklchToLinear(ROLL_L, hi, h))) {
    for (let i = 0; i < 20; i++) {
      const mid = (lo + hi) / 2;
      if (inGamut(oklchToLinear(ROLL_L, mid, h))) lo = mid; else hi = mid;
    }
    hi = lo;
  }
  return linearToHex(oklchToLinear(ROLL_L, hi, h));
}

function circularGap(a, b) {
  const d = Math.abs(a - b) % 360;
  return Math.min(d, 360 - d);
}

/** Pure: seed -> {palette, tintMap} (fresh objects). */
export function rollColors(seed) {
  const rand = mulberry32(seed);
  const span = 360 - (OLIVE_BAND[1] - OLIVE_BAND[0]);
  const coralHue = (hexToOklch(SEA_PALETTE.coral)[2] * 180) / Math.PI;
  const hues = [];
  while (hues.length < CHROMA_SLOTS.length - 1) {
    let h = rand() * span;
    if (h >= OLIVE_BAND[0]) h += OLIVE_BAND[1] - OLIVE_BAND[0];
    if ([coralHue, ...hues].every((x) => circularGap(x, h) >= ROLL_MIN_GAP)) hues.push(h);
    else if (hues.length && rand() < 0.02) hues.length = 0; // dead end: restart
  }
  const palette = { neutral: SEA_PALETTE.neutral, coral: pastelHex(coralHue) };
  CHROMA_SLOTS.filter((s) => s !== "coral").forEach((slot, i) => { palette[slot] = pastelHex(hues[i]); });
  // Fisher-Yates deal of the colored slots over the roles.
  const deck = [...CHROMA_SLOTS];
  for (let i = deck.length - 1; i > 0; i--) {
    const j = Math.floor(rand() * (i + 1));
    [deck[i], deck[j]] = [deck[j], deck[i]];
  }
  const tintMap = {};
  Object.values(ROLL_ROLES).forEach((targets, i) => {
    for (const target of targets) tintMap[target] = deck[i];
  });
  // The volume bar sits beside Telegram: take a color that is not Telegram's.
  tintMap["glyph-bar"] = deck[Object.keys(ROLL_ROLES).indexOf("player")];
  // Panels (menu, explorer) wear a light wash (surfaceTint) of the clock's color.
  tintMap.panel = deck[Object.keys(ROLL_ROLES).indexOf("clock")];
  return { palette, tintMap };
}

export function newColorSeed() {
  return crypto.getRandomValues(new Uint32Array(1))[0];
}

// Sea Glass is the preset that wears the user's personal colors.
const USER_COLOR_PRESET = "sea-glass";

/** The preset record with the user's colors applied (pure, never mutates). */
export function withUserColors(preset, colors) {
  const theme = cloneTheme(preset);
  if (preset.id !== USER_COLOR_PRESET || !colors) return theme;
  theme.palette = { ...theme.palette, ...colors.palette };
  theme.tintMap = { ...theme.tintMap, ...colors.tintMap };
  return theme;
}

function normalizeUserColors(raw) {
  if (!raw || typeof raw !== "object") return null;
  const base = BUILTIN_BY_ID.get(USER_COLOR_PRESET);
  const palette = normalizeMap(raw.palette, base.palette, PALETTE_SLOTS, (v) => normalizeHex(v) !== null);
  for (const slot of PALETTE_SLOTS) palette[slot] = normalizeHex(palette[slot]);
  const tintMap = normalizeMap(raw.tintMap, base.tintMap, TINT_TARGETS, (v) => PALETTE_SLOTS.includes(v));
  return { palette, tintMap };
}

function normalizeHex(value) {
  if (typeof value !== "string") return null;
  const trimmed = value.trim();
  if (!HEX_RE.test(trimmed)) return null;
  let h = trimmed.slice(1);
  if (h.length === 3) h = h.split("").map((c) => c + c).join("");
  return `#${h.toLowerCase()}`;
}

function isRgbString(value) {
  if (typeof value !== "string") return false;
  const m = value.match(/^(\d{1,3})\s*,\s*(\d{1,3})\s*,\s*(\d{1,3})$/);
  return Boolean(m) && Number(m[1]) <= 255 && Number(m[2]) <= 255 && Number(m[3]) <= 255;
}

function clampNumber(value, min, max) {
  const n = Number(value);
  if (!Number.isFinite(n)) return null;
  return Math.max(min, Math.min(max, n));
}

/**
 * Normalize any theme record onto the full schema v4. The base is the
 * built-in preset with the same id (else Nothing Classic). A built-in
 * persisted under an older schema takes the current built-in style and
 * keeps only its sizes (`migrated: true`). Unknown / malformed fields are
 * dropped — the result is always a complete, valid theme.
 */
export function normalizeTheme(raw) {
  const isObject = Boolean(raw) && typeof raw === "object";
  const builtin = isObject ? BUILTIN_BY_ID.get(raw.id) : null;
  const base = builtin || BUILTIN_PRESETS[0];
  const theme = cloneTheme(base);
  const legacyBuiltin = Boolean(builtin) && raw !== builtin && !(Number(raw.schemaVersion) >= SCHEMA_VERSION);
  if (isObject && !legacyBuiltin) {
    for (const field of HEX_FIELDS) {
      const hex = normalizeHex(raw[field]);
      if (hex) theme[field] = hex;
    }
    for (const field of RGB_FIELDS) {
      if (isRgbString(raw[field])) {
        theme[field] = raw[field].split(",").map((part) => part.trim()).join(", ");
      }
    }
    for (const [field, [min, max, asInt]] of Object.entries(NUM_FIELDS)) {
      const n = clampNumber(raw[field], min, max);
      if (n !== null) theme[field] = asInt ? Math.round(n) : n;
    }
    for (const [field, allow] of Object.entries(FONT_OPTIONS)) {
      if (typeof raw[field] === "string" && allow.has(raw[field])) {
        theme[field] = raw[field];
      }
    }
    for (const [field, allow] of Object.entries(ENUM_FIELDS)) {
      if (allow.has(raw[field])) theme[field] = raw[field];
    }
    if (typeof raw.avatarColor === "boolean") theme.avatarColor = raw.avatarColor;
    theme.palette = normalizeMap(raw.palette, base.palette, PALETTE_SLOTS, (v) => normalizeHex(v) !== null);
    for (const slot of PALETTE_SLOTS) theme.palette[slot] = normalizeHex(theme.palette[slot]);
    theme.tintMap = normalizeMap(raw.tintMap, base.tintMap, TINT_TARGETS, (v) => PALETTE_SLOTS.includes(v));
    if (MOTION_PROFILE_IDS.has(raw.motionProfile)) theme.motionProfile = raw.motionProfile;
    if (typeof raw.id === "string" && ID_RE.test(raw.id)) theme.id = raw.id;
    if (typeof raw.name === "string" && raw.name.trim()) {
      theme.name = raw.name.trim().slice(0, 32);
    }
  }
  if (isObject) theme.sizes = normalizeSizes(raw.sizes, base.sizes);
  theme.accentRgb = hexToRgb(theme.accent);
  theme.isBuiltin = BUILTIN_BY_ID.has(theme.id);
  theme.schemaVersion = SCHEMA_VERSION;
  if (legacyBuiltin) theme.migrated = true;
  else delete theme.migrated;
  return theme;
}

// ── Declarative token map ───────────────────────────────────────────────────
/**
 * The single authority for how theme fields become CSS custom properties.
 * Used by BOTH write paths — applyTheme (commit) and the rAF live-preview
 * flush — so primitives can never drift apart. Derived values (glow, glass
 * panels, borders) are NOT written here: they are computed aliases in
 * styles.css that reference these primitives.
 */
const TOKEN_DEFS = [
  {
    fields: ["accent"],
    vars: (t) => ({
      "--theme-accent": t.accent,
      "--theme-accent-rgb": t.accentRgb,
    }),
  },
  {
    fields: ["bgBase"],
    vars: (t) => ({ "--theme-bg-base": t.bgBase }),
  },
  {
    fields: ["glassOpacity"],
    vars: (t) => ({ "--theme-glass-opacity": String(t.glassOpacity) }),
  },
  {
    fields: ["glassRim", "glassSheen", "glassShadow"],
    vars: (t) => ({
      "--theme-glass-rim": String(t.glassRim ?? 0),
      "--theme-glass-sheen": String(t.glassSheen ?? 0),
      "--theme-glass-shadow": String(t.glassShadow ?? 0),
      "--theme-card-shadow": (Number(t.glassShadow) > 0 || Number(t.glassRim) > 0)
        ? "var(--theme-glass-computed-shadow)"
        : "none",
      "--theme-card-sheen-image": (Number(t.glassSheen) > 0)
        ? "var(--theme-glass-computed-gradient)"
        : "none",
      "--theme-surface-shadow": (Number(t.glassShadow) > 0)
        ? "var(--theme-glass-computed-surface-shadow)"
        : "0 28px 90px rgba(0, 0, 0, 0.8), inset 0 1px 0 rgba(var(--bg-elev-rgb), 0.08)",
    }),
  },
  {
    fields: ["fgRgb"],
    vars: (t) => ({ "--fg-rgb": t.fgRgb }),
  },
  {
    fields: ["bgElevRgb"],
    vars: (t) => ({ "--bg-elev-rgb": t.bgElevRgb }),
  },
  {
    fields: ["scrimRgb"],
    vars: (t) => ({ "--scrim-rgb": t.scrimRgb }),
  },
  {
    fields: ["surfaceRgb"],
    vars: (t) => ({ "--bg-surface-rgb": t.surfaceRgb }),
  },
  {
    fields: ["surfaceDeepRgb"],
    vars: (t) => ({ "--bg-surface-deep-rgb": t.surfaceDeepRgb }),
  },
  {
    fields: ["statusOk"],
    vars: (t) => ({ "--status-ok": t.statusOk, "--status-ok-rgb": hexToRgb(t.statusOk) }),
  },
  {
    fields: ["statusWarn"],
    vars: (t) => ({ "--status-warn": t.statusWarn, "--status-warn-rgb": hexToRgb(t.statusWarn) }),
  },
  {
    fields: ["statusError"],
    vars: (t) => ({ "--status-error": t.statusError, "--status-error-rgb": hexToRgb(t.statusError) }),
  },
  {
    fields: ["statusInfo"],
    vars: (t) => ({ "--status-info": t.statusInfo, "--status-info-rgb": hexToRgb(t.statusInfo) }),
  },
  {
    fields: ["blur"],
    vars: (t) => ({
      "--theme-blur-strength": `${t.blur}px`,
      // No blur = no backdrop pass at all (a 0px filter still costs a surface).
      "--theme-backdrop": t.blur > 0 ? `blur(${t.blur}px)` : "none",
    }),
  },
  {
    fields: ["palette", "tintMap", "inkDepth"],
    vars: (t) => {
      const out = {};
      const ink = {};
      for (const slot of PALETTE_SLOTS) {
        ink[slot] = inkFor(t.palette[slot], t.inkDepth);
        out[`--palette-${slot}`] = t.palette[slot];
        out[`--ink-${slot}`] = ink[slot];
      }
      for (const target of TINT_TARGETS) {
        const key = target.replace(".", "-");
        out[`--tint-${key}`] = t.palette[t.tintMap[target]];
        out[`--ink-tint-${key}`] = ink[t.tintMap[target]];
      }
      return out;
    },
  },
  {
    fields: ["glassGrain", "glassDepth", "shadowOffset", "radiusStrip", "surfaceTint"],
    vars: (t) => ({
      "--theme-surface-tint": String(t.surfaceTint),
      "--theme-glass-grain": String(t.glassGrain),
      "--theme-glass-depth": String(t.glassDepth),
      "--theme-shadow-offset": `${t.shadowOffset}px`,
      "--theme-radius-strip": `${t.radiusStrip}px`,
    }),
  },
  {
    fields: ["radiusCard"],
    vars: (t) => ({ "--theme-radius-card": `${t.radiusCard}px` }),
  },
  {
    fields: ["radiusSurface"],
    vars: (t) => ({ "--theme-radius-surface": `${t.radiusSurface}px` }),
  },
  // Motion tokens are owned by motion.js — theme fields only drive it.
  { fields: ["motionScale"], motion: (t) => setMotionScale(t.motionScale) },
  { fields: ["motionProfile"], motion: (t) => setMotionProfile(t.motionProfile) },
  {
    fields: ["fontDisplay"],
    vars: (t) => ({ "--font-accent": t.fontDisplay }),
  },
  {
    fields: ["fontBody"],
    vars: (t) => ({ "--font-base": t.fontBody }),
  },
  {
    fields: ["fontMono"],
    vars: (t) => ({ "--font-mono": t.fontMono }),
  },
];

export function computeThemeVars(theme) {
  const vars = {};
  for (const def of TOKEN_DEFS) {
    if (def.vars) Object.assign(vars, def.vars(theme));
  }
  return vars;
}

// Fields CSS branches on as <html data-*> attributes (structure, not values).
const ATTR_FIELDS = ["material", "mgLayout", "avatarColor"];

export function computeThemeAttrs(theme) {
  return {
    theme: theme.id,
    material: theme.material,
    mgLayout: theme.mgLayout,
    avatarColor: String(Boolean(theme.avatarColor)),
  };
}

// ── State ───────────────────────────────────────────────────────────────────

let currentTheme = normalizeTheme(BUILTIN_PRESETS[0]);
let customPresets = [];
let saveTimer = null;
let saveChain = Promise.resolve();
let rafId = null;
let pendingFields = new Set();
let configRef = null;
let lastApplied = {};
let lastAttrs = {};
let lastStorageJson = "";
// The user's personal Sea Glass colors ({palette, tintMap}) and the colors
// they replaced last (one-step undo of "shuffle").
let userColors = null;
let previousColors = null;

/**
 * Get the currently active theme configuration object (normalized, full schema).
 */
export function getActiveTheme() {
  return cloneTheme(currentTheme);
}

/**
 * Get all available presets (the built-ins + user created), all normalized.
 */
export function getAllPresets() {
  return [...BUILTIN_PRESETS, ...customPresets].map(cloneTheme);
}

/**
 * Computed theme tokens (CSS custom property map) for canvas-drawing widgets.
 * Read once at mount / on the `unidtk-theme-changed` event — never per frame.
 */
export function getThemeTokens() {
  return { ...computeThemeVars(currentTheme) };
}

function writeVars(vars) {
  const style = document.documentElement.style;
  let changed = false;
  for (const [name, value] of Object.entries(vars)) {
    if (lastApplied[name] !== value) {
      style.setProperty(name, value);
      lastApplied[name] = value;
      changed = true;
    }
  }
  return changed;
}

function writeAttrs(attrs) {
  const ds = document.documentElement.dataset;
  let changed = false;
  for (const [name, value] of Object.entries(attrs)) {
    if (lastAttrs[name] !== value) {
      ds[name] = value;
      lastAttrs[name] = value;
      changed = true;
    }
  }
  return changed;
}

function writeStorage() {
  try {
    // `paint` is applied verbatim by the pre-paint script in index.html
    // before first paint — the token map itself, never a hand mirror of it.
    const json = JSON.stringify({
      schemaVersion: SCHEMA_VERSION,
      activeTheme: currentTheme,
      customPresets,
      userColors,
      paint: { vars: computeThemeVars(currentTheme), attrs: computeThemeAttrs(currentTheme) },
    });
    if (json === lastStorageJson) return;
    localStorage.setItem(STORAGE_KEY, json);
    lastStorageJson = json;
  } catch {}
}

/**
 * Apply a theme (replace semantics: the record is normalized onto the
 * built-in defaults, NOT merged onto the previously active theme).
 * @param {Object} theme - theme record (full or partial/legacy)
 * @param {Object} opts - { persist: boolean, skipStorage: boolean }
 */
export function applyTheme(theme, opts = {}) {
  // Cancel any pending live-preview flush so a stale drag frame can never
  // overwrite committed values (race: queued rAF vs commit).
  if (rafId) {
    cancelAnimationFrame(rafId);
    rafId = null;
  }
  pendingFields.clear();

  currentTheme = normalizeTheme(theme);
  // An older-schema built-in was just upgraded: heal storage once.
  const healed = Boolean(currentTheme.migrated);
  delete currentTheme.migrated;

  let changed = writeVars(computeThemeVars(currentTheme));
  // Structural switches (material, micrographics layout) are attributes:
  // CSS branches on them, so Classic rules are never touched.
  try {
    changed = writeAttrs(computeThemeAttrs(currentTheme)) || changed;
  } catch {}
  setMotionScale(currentTheme.motionScale);
  setMotionProfile(currentTheme.motionProfile);

  // Update the localStorage cache for instant zero-FOUC cold starts.
  if (!opts.skipStorage) {
    writeStorage();
  }

  // Notify drawing widgets (canvas visualizer, inline-colored widgets) — only
  // when tokens actually changed, so boot re-applies stay silent.
  if (changed) {
    try {
      window.dispatchEvent(new CustomEvent("unidtk-theme-changed", {
        detail: cloneTheme(currentTheme),
      }));
    } catch {}
  }

  if (opts.persist || healed) {
    if (healed && opts.skipStorage) writeStorage();
    schedulePersistence();
  }
}

/**
 * Set a single theme token dynamically during live slider drag (rAF batched).
 * Only primitives are written; derived CSS aliases update automatically.
 * Event dispatch and persistence are commit-only (see commitLiveChange).
 */
export function setLiveToken(field, value) {
  if (!(field in currentTheme)) return;
  currentTheme[field] = value;
  if (field === "accent") currentTheme.accentRgb = hexToRgb(value);
  pendingFields.add(field);

  if (!rafId) {
    rafId = requestAnimationFrame(() => {
      rafId = null;
      const fields = pendingFields;
      pendingFields = new Set();
      const vars = {};
      for (const def of TOKEN_DEFS) {
        if (def.fields.some((f) => fields.has(f))) {
          if (def.vars) Object.assign(vars, def.vars(currentTheme));
          if (def.motion) def.motion(currentTheme);
        }
      }
      writeVars(vars);
      if (ATTR_FIELDS.some((f) => fields.has(f))) writeAttrs(computeThemeAttrs(currentTheme));
    });
  }
}

/**
 * Called when a slider drag completes (change / pointerup) to commit and
 * persist. Re-normalizes the live-mutated state (validation happens here).
 */
export function commitLiveChange() {
  applyTheme(currentTheme, { persist: true });
  if (currentTheme.id === USER_COLOR_PRESET) {
    userColors = { palette: { ...currentTheme.palette }, tintMap: { ...currentTheme.tintMap } };
    writeStorage();
  }
}

/** Replace the active theme's colors and persist at once (no debounce). */
function applyColors(colors) {
  const next = { ...cloneTheme(currentTheme), palette: { ...colors.palette }, tintMap: { ...colors.tintMap } };
  if (currentTheme.id === USER_COLOR_PRESET) userColors = { palette: { ...colors.palette }, tintMap: { ...colors.tintMap } };
  applyTheme(next);
  flushPersistence();
}

/** "Перемешать цвета": a new random coloring; the old one stays restorable. */
export function shuffleColors() {
  previousColors = { palette: { ...currentTheme.palette }, tintMap: { ...currentTheme.tintMap } };
  applyColors(rollColors(newColorSeed()));
}

/** Undo the last shuffle. Returns false when there is nothing to restore. */
export function restorePreviousColors() {
  if (!previousColors) return false;
  const back = previousColors;
  previousColors = { palette: { ...currentTheme.palette }, tintMap: { ...currentTheme.tintMap } };
  applyColors(back);
  return true;
}

export function hasPreviousColors() {
  return Boolean(previousColors);
}

/**
 * Select a preset by ID and apply it immediately (replace semantics).
 * Widget SIZES follow only when they still equal the previously active
 * preset's defaults (i.e. the user never resized them); positions (x/y)
 * are never touched by themes. See migrateSizesToPreset.
 */
export function selectPreset(presetId) {
  const all = getAllPresets();
  const found = withUserColors(all.find((p) => p.id === presetId) || BUILTIN_PRESETS[0], userColors);
  const oldTheme = cloneTheme(currentTheme);
  applyTheme(found, { persist: true });
  migrateSizesToPreset(found, oldTheme);
  return found;
}

// Widget ids whose geometry presets must never manage:
// - micrographics positions itself (card forced 0x0 by the core);
// - vpn is hidden by default; orphans below are fossils, not widgets.
const SIZE_SKIP_IDS = new Set(["micrographics", "vpn", "glyph", "audio-circle"]);

function isSimulationId(id) {
  return typeof id === "string" && id.startsWith("simulation");
}

/**
 * Resolve the active preset's default size for a widget.
 * Folder widgets share the "folder" default. Returns null when the
 * preset has no opinion — then the core fallback applies.
 */
export function resolvePresetSize(widgetId, isFolder = false) {
  const sizes = (currentTheme && currentTheme.sizes) || {};
  if (widgetId && sizes[widgetId]) return { ...sizes[widgetId] };
  if (isFolder && sizes.folder) return { ...sizes.folder };
  return null;
}

/**
 * Push the configured sizes of the given preset into live cards
 * (no reload): mirrors the core's placement pass + glyph re-layout.
 */
function applySizesLive() {
  try {
    const positions = (configRef && configRef.positions) || {};
    document.querySelectorAll(".widget-card").forEach((card) => {
      const id = card.dataset.id || card.dataset.widgetType;
      if (!id) return;
      const pos = positions[id];
      if (!pos) return;
      if (pos.w) card.style.width = `${pos.w}px`;
      if (pos.h) card.style.height = `${pos.h}px`;
      if (card._glyphBarGeometry && typeof card._glyphBarGeometry.layout === "function") {
        try {
          card._glyphBarGeometry.layout();
        } catch {}
      }
    });
  } catch {}
}

/**
 * Bring widget sizes in configRef.positions onto newPreset's defaults —
 * but ONLY where they still equal oldPreset's defaults (or are missing).
 * Anything the user resized (differs from the old default) is preserved.
 * Positions (x/y) are never modified here.
 */
function migrateSizesToPreset(newPreset, oldTheme) {
  if (!configRef || !configRef.positions) return;
  const newSizes = (newPreset && newPreset.sizes) || {};
  const oldSizes = (oldTheme && oldTheme.sizes) || {};
  if (!Object.keys(newSizes).length) return;
  const positions = configRef.positions;
  let changed = false;
  const applyId = (posId, sizeKey) => {
    const next = newSizes[sizeKey];
    if (!next) return;
    const cur = positions[posId];
    if (!cur) return;
    if (!cur.w || !cur.h) {
      positions[posId] = { ...cur, w: next.w, h: next.h };
      changed = true;
      return;
    }
    const old = oldSizes[sizeKey];
    if (old && cur.w === old.w && cur.h === old.h && (cur.w !== next.w || cur.h !== next.h)) {
      positions[posId] = { ...cur, w: next.w, h: next.h };
      changed = true;
    }
  };
  for (const key of Object.keys(newSizes)) {
    if (key === "folder" || SIZE_SKIP_IDS.has(key) || isSimulationId(key)) continue;
    if (positions[key]) applyId(key, key);
  }
  // Folder widgets: every id that is not a known size key or skipped.
  if (newSizes.folder) {
    for (const id of Object.keys(positions)) {
      if (newSizes[id] || SIZE_SKIP_IDS.has(id) || isSimulationId(id)) continue;
      applyId(id, "folder");
    }
  }
  if (changed) {
    applySizesLive();
    schedulePersistence();
  }
}

/**
 * Reset all known widget sizes to the ACTIVE preset's defaults
 * (positions x/y are preserved). Returns the number of widgets reset.
 * Menu door: "Сбросить размеры по пресету".
 */
export function resetSizesToPreset() {
  if (!configRef || !configRef.positions) return 0;
  const sizes = (currentTheme && currentTheme.sizes) || {};
  if (!Object.keys(sizes).length) return 0;
  const positions = configRef.positions;
  let n = 0;
  const setId = (posId, sizeKey) => {
    const next = sizes[sizeKey];
    if (!next || !positions[posId]) return;
    positions[posId] = { ...positions[posId], w: next.w, h: next.h };
    n++;
  };
  for (const key of Object.keys(sizes)) {
    if (key === "folder" || SIZE_SKIP_IDS.has(key) || isSimulationId(key)) continue;
    setId(key, key);
  }
  if (sizes.folder) {
    for (const id of Object.keys(positions)) {
      if (sizes[id] || SIZE_SKIP_IDS.has(id) || isSimulationId(id)) continue;
      setId(id, "folder");
    }
  }
  if (n) {
    applySizesLive();
    schedulePersistence();
  }
  return n;
}

/**
 * Save current settings as a new custom user preset.
 */
export function saveCustomPreset(name, overrides = {}) {
  const safeName = (name || "").trim() || `Стиль ${customPresets.length + 1}`;
  const newPreset = normalizeTheme({
    ...currentTheme,
    ...overrides,
    id: `custom-${Date.now()}`,
    name: safeName,
  });
  customPresets.push(newPreset);
  applyTheme(newPreset, { persist: true });
  return newPreset;
}

/**
 * Delete a user-created preset.
 */
export function deleteCustomPreset(presetId) {
  const idx = customPresets.findIndex((p) => p.id === presetId);
  if (idx === -1) return false;

  customPresets.splice(idx, 1);

  // If the deleted preset was active, fall back to Nothing Classic.
  if (currentTheme.id === presetId) {
    applyTheme(BUILTIN_PRESETS[0], { persist: true });
  } else {
    schedulePersistence();
  }
  return true;
}

/**
 * Synchronously mirror the active theme into the shared config object.
 * Called by the core's saveConfig() so widget-layout writes can never
 * serialize a stale theme payload — both redb write paths share one object.
 */
export function syncThemeIntoConfig() {
  if (!configRef) return;
  configRef.theme = cloneTheme(currentTheme);
  configRef.customPresets = customPresets.map(cloneTheme);
  configRef.userColors = userColors ? { palette: { ...userColors.palette }, tintMap: { ...userColors.tintMap } } : null;
}

/** Immediate (non-debounced) persistence for state that must never be lost. */
function flushPersistence() {
  if (saveTimer) clearTimeout(saveTimer);
  saveTimer = null;
  writeStorage();
  syncThemeIntoConfig();
  if (!configRef) return;
  saveChain = saveChain
    .then(() => saveWidgetConfig(configRef))
    .catch((err) => {
      console.error("[theme] failed to persist theme to redb:", err);
    });
}

/**
 * Debounced, serialized persistence into redb via saveWidgetConfig.
 */
function schedulePersistence() {
  if (saveTimer) clearTimeout(saveTimer);
  saveTimer = setTimeout(() => {
    saveTimer = null;
    syncThemeIntoConfig();
    if (!configRef) return;
    saveChain = saveChain
      .then(() => saveWidgetConfig(configRef))
      .catch((err) => {
        console.error("[theme] failed to persist theme to redb:", err);
      });
  }, 600);
}

/**
 * Initialize theme engine:
 * 1. Synchronously reads the cached theme from localStorage to eliminate FOUC
 *    before DOM paint (the cache is never rewritten on this path — it must
 *    survive a crash between boot and the redb reconcile).
 * 2. Wires the reference to the global config and reconciles with redb:
 *    persisted custom presets and legacy partial themes are normalized to the
 *    full schema; retired preset records are healed in place (rewritten once).
 */
export function initThemeSystem(globalConfig = null) {
  if (globalConfig) {
    configRef = globalConfig;
  }

  // 1. Fast path: restore from localStorage without rewriting it.
  let restored = false;
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (raw) {
      lastStorageJson = raw;
      const parsed = JSON.parse(raw);
      userColors = normalizeUserColors(parsed.userColors);
      if (Array.isArray(parsed.customPresets)) {
        customPresets = parsed.customPresets
          .map((p) => normalizeTheme(p))
          .filter((p) => !isRetiredTheme(p));
      }
      if (parsed.activeTheme && typeof parsed.activeTheme === "object" && !isRetiredTheme(parsed.activeTheme)) {
        applyTheme(parsed.activeTheme, { skipStorage: true });
        restored = true;
      }
    }
  } catch (err) {
    console.warn("[theme] error reading localStorage theme cache:", err);
    lastStorageJson = "";
  }

  // 2. Slow path: reconcile with the redb config if provided.
  if (globalConfig) {
    // redb is authoritative for the user's colors; the cache only bridges boot.
    userColors = normalizeUserColors(globalConfig.userColors) || userColors;
    if (Array.isArray(globalConfig.customPresets)) {
      customPresets = globalConfig.customPresets
        .map((p) => normalizeTheme(p))
        .filter((p) => !isRetiredTheme(p));
    }
    if (globalConfig.theme && typeof globalConfig.theme === "object") {
      if (isRetiredTheme(globalConfig.theme)) {
        // Retired preset in redb: heal the record once with the effective theme.
        applyTheme(currentTheme, { persist: true });
      } else {
        applyTheme(globalConfig.theme, { persist: false });
      }
    } else if (!restored) {
      // First boot: apply default Nothing Classic.
      applyTheme(currentTheme, { persist: false });
    }
  } else if (!restored) {
    // Boot call without config: apply defaults without destroying the cache.
    applyTheme(currentTheme, { skipStorage: true });
  }

  // 3. First run for this user (after the redb reconcile, so a stored coloring
  // always wins): deal their personal Sea Glass colors once and persist now.
  if (globalConfig && !userColors) {
    const rolled = rollColors(newColorSeed());
    if (currentTheme.id === USER_COLOR_PRESET) {
      applyColors(rolled);
    } else {
      userColors = rolled;
      flushPersistence();
    }
  }
}
