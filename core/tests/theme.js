/** Run through `udtk gui js` in the real WebView; no external JS runtime. */
import {
  normalizeTheme, computeThemeVars, computeThemeAttrs, BUILTIN_PRESETS, SCHEMA_VERSION,
  rollColors, withColorOverrides, loadColorOverrides, inkFor, PALETTE_SLOTS,
  registerThemeContracts, tintSlot, getTintTargets, getLookOptions, readableOn, contrastRatio,
} from "../ui/theme.js";

// Nothing Classic primitives as shipped before schema v4: the Classic
// preset must keep producing exactly these values (zero pixel regression).
const CLASSIC_V3_VARS = {
  "--theme-accent": "#d71921", "--theme-accent-rgb": "215, 25, 33", "--theme-bg-base": "15, 18, 30",
  "--theme-glass-opacity": "0.78", "--theme-glass-rim": "0", "--theme-glass-sheen": "0", "--theme-glass-shadow": "0",
  "--theme-card-shadow": "none", "--theme-card-sheen-image": "none",
  "--theme-surface-shadow": "0 28px 90px rgba(0, 0, 0, 0.8), inset 0 1px 0 rgba(var(--bg-elev-rgb), 0.08)",
  "--fg-rgb": "255, 255, 255", "--bg-elev-rgb": "255, 255, 255", "--scrim-rgb": "0, 0, 0",
  "--bg-surface-rgb": "18, 18, 24", "--bg-surface-deep-rgb": "12, 12, 16",
  "--status-ok": "#34c759", "--status-ok-rgb": "52, 199, 89", "--status-warn": "#ffd60a", "--status-warn-rgb": "255, 214, 10",
  "--status-error": "#ff3b30", "--status-error-rgb": "255, 59, 48", "--status-info": "#0a84ff", "--status-info-rgb": "10, 132, 255",
  "--theme-blur-strength": "16px", "--theme-radius-card": "16px", "--theme-radius-surface": "24px",
  "--font-accent": "'Ndot', monospace", "--font-base": "'Geist', sans-serif", "--font-mono": "'Consolas', 'Courier New', monospace",
};

// A stand-in for what modules declare in metadata.json.
const tint = (id, role) => ({ id, label: id, default: "lilac", role });
const MODULE_CONTRACTS = [
  { tints: [tint("clock", "calm")] }, { tints: [tint("telegram", "social")] },
  { tints: [tint("mp3-player", "player"), tint("glyph-bar", "player")] },
  { tints: [tint("mg.date", "calendar"), tint("mg.ip-location", "calendar"), tint("mg.temperature", "weather"),
    tint("mg.feels", "weather"), tint("mg.wind", "sky"), tint("mg.sunset", "sky")],
  lookOptions: [{ field: "mgLayout", label: "Layout", options: [["panel", "Panel"]] }] },
];

export function runThemeDiagnostics() {
  let checks = 0;
  const assert = (ok, message) => { checks++; if (!ok) throw new Error(message); };
  registerThemeContracts(MODULE_CONTRACTS);
  const classic = BUILTIN_PRESETS.find((p) => p.id === "nothing-dark");
  const sea = BUILTIN_PRESETS.find((p) => p.id === "sea-glass");
  assert(SCHEMA_VERSION === 4, "schema is v4");

  // Classic: every v3 primitive unchanged, material stays flat.
  const classicVars = computeThemeVars(normalizeTheme(classic));
  for (const [name, value] of Object.entries(CLASSIC_V3_VARS)) {
    assert(classicVars[name] === value, `classic ${name} drifted: ${classicVars[name]} != ${value}`);
  }
  assert(computeThemeAttrs(normalizeTheme(classic)).material === "flat", "classic material flat");

  // v3 Sea Glass snapshot (cache/redb before v4): style comes from the new
  // built-in, only the user's widget sizes survive.
  const v3Sea = { id: "sea-glass", name: "Sea Glass", schemaVersion: 3, accent: "#007780", glassOpacity: 0.66,
    blur: 24, bgBase: "110, 205, 195", sizes: { clock: { w: 300, h: 280 } } };
  const migrated = normalizeTheme(v3Sea);
  assert(migrated.glassOpacity === sea.glassOpacity, "v3 sea-glass takes new opacity");
  assert(migrated.blur === sea.blur, "v3 sea-glass takes new blur");
  assert(migrated.material === "jelly", "v3 sea-glass becomes jelly");
  assert(migrated.sizes.clock.w === 300 && migrated.sizes.clock.h === 280, "user-resized size survives migration");
  assert(migrated.schemaVersion === 4 && migrated.migrated === true, "migration is flagged for healing");

  // v4 record keeps the user's own tweaks.
  const tweaked = normalizeTheme({ ...sea, glassOpacity: 0.5, palette: { ...sea.palette, blue: "#123456" } });
  assert(tweaked.glassOpacity === 0.5, "v4 tweak kept");
  assert(tweaked.palette.blue === "#123456", "v4 palette tweak kept");
  assert(!tweaked.migrated, "v4 record is not re-migrated");

  // v3 custom preset: keeps its fields, new fields default to Classic (flat).
  const custom = normalizeTheme({ id: "custom-1", name: "Мой", schemaVersion: 3, accent: "#00ff00", glassOpacity: 0.7 });
  assert(custom.accent === "#00ff00" && custom.glassOpacity === 0.7, "v3 custom keeps its values");
  assert(custom.material === "flat", "v3 custom stays flat");

  // Malformed nested maps fall back per key; unknown keys are dropped.
  const broken = normalizeTheme({ ...sea, palette: { blue: "nope", bogus: "#ffffff" },
    tintMap: { clock: "nope", "mg.date": 5, bogus: "blue", telegram: "coral" } });
  assert(broken.palette.blue === sea.palette.blue, "bad palette hex falls back");
  assert(!("bogus" in broken.palette), "unknown palette slot dropped");
  assert(broken.tintMap.clock === sea.tintMap.clock, "bad tint slot falls back");
  assert(!("mg.date" in broken.tintMap), "non-string tint dropped");
  assert(normalizeTheme({ ...sea, tintMap: { "bad key!": "blue" } }).tintMap["bad key!"] === undefined, "malformed tint key dropped");
  assert(broken.tintMap.telegram === "coral", "valid tint override kept");
  const enums = normalizeTheme({ ...sea, material: "lava", mgLayout: "grid", avatarColor: "yes" });
  assert(enums.material === "jelly" && enums.mgLayout === sea.mgLayout && enums.avatarColor === sea.avatarColor, "bad enums fall back");

  // Nested objects are never shared with the frozen built-ins.
  const copy = normalizeTheme(sea);
  assert(copy.palette !== sea.palette && copy.tintMap !== sea.tintMap && copy.sizes !== sea.sizes, "nested maps are copies");
  copy.palette.blue = "#000000";
  assert(sea.palette.blue !== "#000000", "built-in palette is not mutated through a normalized copy");
  assert(Object.isFrozen(sea.palette) && Object.isFrozen(sea.tintMap), "built-in nested maps frozen");

  // Every tint target resolves to a palette color var.
  const seaVars = computeThemeVars(normalizeTheme(sea));
  for (const [target, slot] of Object.entries(sea.tintMap)) {
    const name = `--tint-${target.replace(".", "-")}`;
    assert(seaVars[name] === sea.palette[slot], `${name} resolves to ${slot}`);
  }
  assert(seaVars["--theme-blur-strength"] === `${sea.blur}px`, "sea blur var");

  // Ink: deep same-hue text for every slot, AAA-readable on its pastel.
  const lum = (rgb) => {
    const [r, g, b] = rgb.map((v) => { v /= 255; return v <= 0.04045 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4; });
    return 0.2126 * r + 0.7152 * g + 0.0722 * b;
  };
  const parse = (s) => s.split(",").map(Number);
  const hexTriple = (h) => [1, 3, 5].map((i) => parseInt(h.slice(i, i + 2), 16));
  for (const slot of Object.keys(sea.palette)) {
    const ink = seaVars[`--ink-${slot}`];
    assert(/^\d{1,3}, \d{1,3}, \d{1,3}$/.test(ink || ""), `ink for ${slot} is an rgb triple`);
    const contrast = (lum(hexTriple(sea.palette[slot])) + 0.05) / (lum(parse(ink)) + 0.05);
    assert(contrast >= 7, `ink on ${slot} contrast ${contrast.toFixed(2)} >= 7`);
  }
  assert(seaVars["--ink-tint-clock"] === seaVars[`--ink-${tintSlot(sea, "clock")}`], "per-target ink follows its slot");
  assert(seaVars["--tint-mg-date"] === sea.palette.lilac, "module default tint applies");

  // The contract: modules declare tints and options, a late target finds its role's color.
  assert(getTintTargets().some((t) => t.id === "mg.date") && getTintTargets().some((t) => t.id === "panel"), "registry holds core and modules");
  assert(getLookOptions()[0].field === "mgLayout", "look options come from modules");
  const rolled = normalizeTheme({ ...sea, ...rollColors(3) });
  registerThemeContracts([{ tints: [tint("late-module", "weather")] }]);
  assert(tintSlot(rolled, "late-module") === rolled.tintMap["role:weather"], "late target takes its role color");

  // Contrast: a theme's choice stands while readable, otherwise it is replaced.
  assert(readableOn("#d71921", "255, 255, 255") === "255, 255, 255", "declared on-accent kept");
  assert(contrastRatio(readableOn("#ffee00", "255, 255, 255"), "255, 238, 0") >= 4.5, "unreadable on-accent replaced");
  for (const preset of BUILTIN_PRESETS) {
    const vars = computeThemeVars(normalizeTheme(preset));
    assert(contrastRatio(vars["--on-accent-rgb"], vars["--theme-accent-rgb"]) >= 4.5, `${preset.id} accent text is readable`);
  }
  const deeper = computeThemeVars(normalizeTheme({ ...sea, inkDepth: 0.2 }));
  assert(lum(parse(deeper["--ink-blue"])) < lum(parse(seaVars["--ink-blue"])), "inkDepth darkens ink");
  // The ink slider cannot leave AAA: 0.38 is its ceiling.
  assert(normalizeTheme({ ...sea, inkDepth: 0.6 }).inkDepth === 0.38, "inkDepth capped at 0.38");
  assert(seaVars["--theme-surface-tint"] === String(sea.surfaceTint), "surfaceTint var");

  // Per-user colors: rollColors is pure and deterministic, normalize never rolls.
  const a = rollColors(1), b = rollColors(1), c = rollColors(2);
  assert(JSON.stringify(a) === JSON.stringify(b), "rollColors deterministic");
  assert(JSON.stringify(a.palette) !== JSON.stringify(c.palette), "different seeds differ");
  assert(a.palette.coral === c.palette.coral, "coral accent keeps its hue across shuffles");
  assert(JSON.stringify(normalizeTheme(sea).palette) === JSON.stringify(normalizeTheme(sea).palette), "normalize is pure");
  const chroma = PALETTE_SLOTS.filter((s) => s !== "neutral");
  const roles = ["folder", "clock", "telegram", "mp3-player", "mg.date", "mg.temperature", "mg.wind"];
  const hueOf = (hex) => {
    const [r, g, bl] = hexTriple(hex).map((v) => { v /= 255; return v <= 0.04045 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4; });
    const l = Math.cbrt(0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * bl);
    const m = Math.cbrt(0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * bl);
    const s = Math.cbrt(0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * bl);
    const A = 1.9779984951 * l - 2.428592205 * m + 0.4505937099 * s;
    const B = 0.0259040371 * l + 0.7827717662 * m - 0.808675766 * s;
    return { L: 0.2104542553 * l + 0.793617785 * m - 0.0040720468 * s, h: (Math.atan2(B, A) * 180 / Math.PI + 360) % 360 };
  };
  const firstHues = new Set();
  for (let seed = 1; seed <= 1000; seed++) {
    const { palette, tintMap } = rollColors(seed);
    const hues = chroma.map((s) => hueOf(palette[s]));
    firstHues.add(Math.round(hues[0].h / 10));
    for (const { L, h } of hues) {
      assert(Math.abs(L - 0.86) < 0.02, `seed ${seed}: pastel lightness ${L.toFixed(3)}`);
      assert(!(h > 92 && h < 123), `seed ${seed}: olive hue ${h.toFixed(0)}`);
    }
    for (let i = 0; i < hues.length; i++) for (let j = i + 1; j < hues.length; j++) {
      const d = Math.abs(hues[i].h - hues[j].h);
      assert(Math.min(d, 360 - d) >= 30, `seed ${seed}: hues too close`);
    }
    for (const s of chroma) {
      const contrast = (lum(hexTriple(palette[s])) + 0.05) / (lum(parse(inkFor(palette[s], 0.3))) + 0.05);
      assert(contrast >= 7, `seed ${seed}: ink contrast ${contrast.toFixed(2)} on ${s}`);
    }
    const used = roles.map((r) => tintMap[r]);
    assert(new Set(used).size === 7 && used.every((s) => chroma.includes(s)), `seed ${seed}: roles are a permutation`);
    assert(chroma.includes(tintMap.panel), "panels get a palette color (shown as a light wash)");
    assert(tintMap["mg.feels"] === tintMap["mg.temperature"] && tintMap["mg.sunset"] === tintMap["mg.wind"]
      && tintMap["mg.ip-location"] === tintMap["mg.date"], "micrographics rows share a slot");
    assert(chroma.includes(tintMap["glyph-bar"]), "glyph-bar gets a palette color");
  }
  assert(firstHues.size > 20, `palettes really vary (${firstHues.size} hue buckets)`);

  // Overrides belong to one theme and cover every color field.
  const mine = rollColors(7);
  const composed = withColorOverrides(sea, { [sea.id]: { ...mine, accent: "#112233", fgRgb: "1, 2, 3" } });
  assert(composed.palette.blue === mine.palette.blue && composed.tintMap.clock === mine.tintMap.clock, "theme wears its palette");
  assert(composed.accent === "#112233" && composed.fgRgb === "1, 2, 3", "theme wears its other colors");
  assert(sea.palette.blue !== mine.palette.blue || sea.palette.aqua !== mine.palette.aqua, "built-in untouched");
  assert(withColorOverrides(classic, { [sea.id]: mine }).palette.blue === classic.palette.blue, "other theme unaffected");
  assert(withColorOverrides(sea, null).palette.blue === sea.palette.blue, "no overrides -> preset colors");

  // Old records: `userColors` becomes the Sea Glass entry; junk is dropped.
  const legacy = loadColorOverrides({ userColors: { palette: { blue: "#123456" }, tintMap: { clock: "coral" } } });
  assert(legacy[sea.id].palette.blue === "#123456" && legacy[sea.id].tintMap.clock === "coral", "userColors migrates");
  assert(legacy[sea.id].accent === sea.accent, "migrated entry keeps preset accent");
  const dirty = loadColorOverrides({ colorOverrides: { "bad id!": {}, [classic.id]: { accent: "nope", statusOk: "#010203" } } });
  assert(!("bad id!" in dirty) && dirty[classic.id].accent === classic.accent && dirty[classic.id].statusOk === "#010203", "overrides validated");
  assert(Object.keys(loadColorOverrides(null)).length === 0, "nothing stored -> no overrides");
  const attrs = computeThemeAttrs(normalizeTheme(sea));
  assert(attrs.material === "jelly" && attrs.mgLayout === "pills" && attrs.avatarColor === "true", "sea attrs");
  return { checks };
}

/**
 * The commit path against a recording `saved` stub of saveWidgetConfig: each
 * write must be complete on its own, because any of them can be the last.
 */
export function runThemePersistenceDiagnostics(theme, saved) {
  let checks = 0;
  const assert = (ok, message) => { checks++; if (!ok) throw new Error(message); };
  const last = () => JSON.parse(JSON.stringify(saved[saved.length - 1]));
  const config = { positions: { clock: { x: 0, y: 0 } }, userColors: { palette: { blue: "#000001" }, tintMap: {} } };
  theme.initThemeSystem(config);
  theme.selectPreset("sea-glass");
  assert(theme.getActiveTheme().palette.blue === "#000001", "migrated userColors reach the theme");
  assert(!("userColors" in last()), "old key leaves the record");

  const coral = (value) => theme.setLiveToken("palette", { ...theme.getActiveTheme().palette, coral: value });
  coral("#123456");
  theme.syncThemeIntoConfig();
  assert(config.theme.palette.coral !== "#123456", "drag value stays out of the config");
  theme.commitLiveChange();
  assert(last().theme.palette.coral === "#123456", "commit saves the theme");
  assert(last().colorOverrides["sea-glass"].palette.coral === "#123456", "same write saves the override");

  theme.setLiveToken("accent", "#00ff01");
  theme.commitLiveChange();
  theme.selectPreset("nothing-dark");
  assert(theme.getActiveTheme().accent !== "#00ff01", "other theme keeps its accent");
  theme.setLiveToken("fgRgb", "9, 8, 7");
  theme.commitLiveChange();
  theme.selectPreset("sea-glass");
  assert(theme.getActiveTheme().accent === "#00ff01" && theme.getActiveTheme().palette.coral === "#123456", "colors survive a theme switch");
  theme.selectPreset("nothing-dark");
  assert(theme.getActiveTheme().fgRgb === "9, 8, 7", "classic keeps its colors too");
  return { checks };
}
