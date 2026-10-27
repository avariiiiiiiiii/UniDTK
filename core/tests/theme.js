/** Run through `udtk gui js` in the real WebView; no external JS runtime. */
import {
  normalizeTheme, computeThemeVars, computeThemeAttrs, BUILTIN_PRESETS, SCHEMA_VERSION,
  rollColors, withUserColors, inkFor, PALETTE_SLOTS,
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

export function runThemeDiagnostics() {
  let checks = 0;
  const assert = (ok, message) => { checks++; if (!ok) throw new Error(message); };
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
  assert(broken.tintMap["mg.date"] === sea.tintMap["mg.date"], "non-string tint falls back");
  assert(!("bogus" in broken.tintMap), "unknown tint target dropped");
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
  assert(seaVars["--ink-tint-clock"] === seaVars[`--ink-${sea.tintMap.clock}`], "per-target ink follows its slot");
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

  // Sea Glass takes the user's colors; other presets keep their own.
  const mine = rollColors(7);
  const composed = withUserColors(sea, mine);
  assert(composed.palette.blue === mine.palette.blue && composed.tintMap.clock === mine.tintMap.clock, "sea glass wears user colors");
  assert(sea.palette.blue !== mine.palette.blue || sea.palette.aqua !== mine.palette.aqua, "built-in untouched");
  assert(withUserColors(classic, mine).palette.blue === classic.palette.blue, "classic ignores user colors");
  assert(withUserColors(sea, null).palette.blue === sea.palette.blue, "no user colors -> preset colors");
  const attrs = computeThemeAttrs(normalizeTheme(sea));
  assert(attrs.material === "jelly" && attrs.mgLayout === "pills" && attrs.avatarColor === "true", "sea attrs");
  return { checks };
}
