# Chapter 8. Themes and Motion

This chapter explains how UniDTK decides how it looks and how it moves. After it, you can name the file that owns each value, follow a theme from a preset record to a CSS variable on the page, explain why the page is painted in the right colors before the first frame, and say why a duration is a token and not a number typed into a stylesheet. Appearance and time are two separate systems. `core/ui/theme.js` owns appearance: colors, fonts, radii, and the material of a surface. `core/ui/motion.js` owns time: durations and easing curves. The rules for both are in [The Desk, the Menu, and Themes](../reference/desk.md) and [Motion](../reference/motion.md).

## Values come from tokens

A token is a named value. Where a token exists, the page uses it instead of a literal color or a literal timing. CSS reads a custom property, for example `var(--dur-motion)`, and JavaScript reads the same name from the same place. The rule is [theme.tokens-only](../reference/desk.md#r-theme.tokens-only). The reason is practical: a user can change the accent color or the speed of the interface while the program runs, and a value written into twenty files would have to be changed in twenty places.

Each token has one owner. `theme.js` writes the appearance tokens, such as `--theme-accent` and `--font-base`. `motion.js` writes the timing tokens, `--dur-*` and `--ease-*`. The stylesheet `core/ui/styles.css` declares a fallback for these in its `:root` block, so the page has correct values before any script runs.

Filename: core/ui/styles.css (abridged, the top of `:root`)

```css
:root {
  --ease-standard: cubic-bezier(0.4, 0, 0.2, 1);
  --dur-scale: 1.0;
  --dur-hover: 120ms;
  --dur-transient: 220ms;
  --dur-motion: 300ms;
  --dur-layout: 340ms;

  /* Theme System Tokens (theme.js) — primitives.
     The static values below MUST equal the Nothing Classic preset in
     theme.js: they are the first-paint fallbacks before JS applies the
     persisted theme (FOUC invariant). */
  --theme-accent: #d71921;
  --theme-accent-rgb: 215, 25, 33;
  --on-accent-rgb: 255, 255, 255;
  --fg-rgb: 255, 255, 255;
  ...
}
```

The fallback is the reason the comment calls these values a FOUC invariant. FOUC means a flash of unstyled content: a frame painted with the wrong colors before the saved theme is applied. The fallback values must match the default preset, so that a page without any script still looks like the default theme.

## Presets

A preset is a complete theme record. It has an `id`, a human `name`, and every field of the current schema. `theme.js` ships two built-in presets, and it keeps a list of retired ids so that an old saved record falls back to the default.

Filename: core/ui/theme.js (abridged)

```js
export const SCHEMA_VERSION = 4;

export const BUILTIN_PRESETS = Object.freeze([
  freezePreset({
    id: "nothing-dark",
    name: "Nothing Classic",
    isBuiltin: true,
    schemaVersion: SCHEMA_VERSION,
    accent: "#d71921",
    bgBase: "15, 18, 30",
    fgRgb: "255, 255, 255",
    ...
  }),
  freezePreset({
    id: "sea-glass",
    name: "Sea Glass",
    ...
  }),
]);
```

The first preset, Nothing Classic, is the default. The second, Sea Glass, uses a pastel palette and the `jelly` material. The header of `theme.js` lists the schema fields: colors as hex values or as `"r, g, b"` channels, the glass opacity and blur, the radii, the fonts, the palette slots, the `tintMap`, and the widget `sizes`.

A preset changes appearance only. The header states the rule: positions are user data and never belong to a theme, and sizes are only defaults. When the preset changes, `migrateSizesToPreset()` moves a widget to the new default size only where the widget still has the old preset's size. A size the user changed is kept, and positions are never modified.

Note: the schema version is stored with every record. A preset saved under an older schema is upgraded when it is read. Its `migrated` flag tells `applyTheme()` to write the healed record back once.

## From a record to the page: applyTheme()

Every change of theme goes through one function, `applyTheme(theme, opts)`. Its steps are fixed, and the order matters.

1. It normalizes the record onto the full schema. The built-in defaults are the base, so a partial record never inherits fields of the previously active theme. The invariant is stated in the header of `theme.js`.
2. It computes the token map with `computeThemeVars()`, and writes only the properties whose value changed since the last call, using the `lastApplied` cache.
3. It writes the structural attributes, `data-theme`, `data-material`, `data-mg-layout`, and `data-avatar-color`, with `computeThemeAttrs()`. CSS branches on these attributes, so the Classic rules are not touched when the Sea Glass rules are added.
4. It calls `setMotionScale()` and `setMotionProfile()` from `motion.js`. Timing is owned by motion, so the theme only passes the user's values on.
5. It writes the cache to `localStorage` unless `skipStorage` is set.
6. If any token changed, it dispatches `unidtk-theme-changed`.
7. If `persist` is set, it saves the record to the database through `persistTheme()`.

Warning: step 2 is why a theme change is cheap, and it is also a trap. Only changed properties are written. If code sets a token directly on `document.documentElement.style`, `lastApplied` does not learn about it. A later `applyTheme()` with the same value then skips the write, and the page keeps the value that was set by hand.

## Live changes while a slider moves

A slider in the settings sheet changes one token many times a second. Writing the database on each move would be slow, so `theme.js` splits the work in two. `setLiveToken(field, value)` updates the in-memory theme and queues the field in `pendingFields`. It schedules one `requestAnimationFrame` callback, and that callback writes the tokens for every queued field in one batch. When the drag ends, `commitLiveChange()` stores the colors, calls `applyTheme(currentTheme, { persist: true })`, and so writes once.

The consequence is that drag frames never reach storage. The comment on `committedTheme` states it: the last applied theme is kept apart from the uncommitted drag values, "and those must never reach storage through an unrelated config save".

## The pre-paint cache

The theme must be correct before the first paint, and the database is read by JavaScript after the page starts. The solution is a small cache in `localStorage`. The key is `unidtk_theme_state`, and it holds the schema version, the active theme, the custom presets, the colors per theme, and a `paint` object with the computed token map and attributes.

`index.html` reads that `paint` object in a classic script that runs before the stylesheets are applied.

Filename: core/ui/index.html (abridged, the first script)

```html
<script>
  (function () {
    try {
      var paint = JSON.parse(localStorage.getItem("unidtk_theme_state") || "{}").paint;
      if (!paint || typeof paint !== "object") return;
      var root = document.documentElement;
      var vars = paint.vars || {}, attrs = paint.attrs || {};
      for (var k in vars) if (k.indexOf("--") === 0) root.style.setProperty(k, String(vars[k]));
      for (var a in attrs) root.dataset[a] = String(attrs[a]);
    } catch (e) { /* cache corrupt: fall back to :root defaults */ }
  })();
</script>
```

Two details matter. The script is a classic script, not a module, because a module runs after the document is parsed and the first paint could happen before it. And the script never writes storage. The comment in `initThemeSystem()` says why: the cache "must survive a crash between boot and the redb reconcile". The cache is written by `writeStorage()` as the theme changes, and the next start reads it back.

The `paint` object is produced by the same functions that paint the page: `computeThemeVars()` and `computeThemeAttrs()`. The script contains no list of tokens of its own, so a new token is added in one place.

## Telling the widgets: unidtk-theme-changed

Most of the page reads colors from CSS, and CSS updates by itself. A few widgets draw on a canvas, and a canvas does not read CSS variables. They need a signal. `applyTheme()` sends it as a DOM event on `window`.

```js
window.dispatchEvent(new CustomEvent("unidtk-theme-changed", {
  detail: cloneTheme(currentTheme),
}));
```

The event is sent only when a token or an attribute really changed, so a re-apply at boot is silent. The `detail` is a copy of the theme, so a listener cannot change the state of `theme.js` by accident.

## Modules declare their share of the theme

`theme.js` names no module. A module that wants a tintable surface declares it in its `metadata.json`, in a `tints` list. Each entry has an `id`, a `label`, a `default` palette slot, and a `role`. The `role` groups targets that share one color when the colors are shuffled. A module can also declare `lookOptions`, which are settings that belong to the theme.

`registerThemeContracts(modules)` reads those declarations. It checks each id against `TINT_KEY_RE` and each default against `PALETTE_SLOTS`, adds the target to the registry, and, when a new target appears, rewrites the token map and the cache, so the new surface is painted at once. The rule is [theme.module-contracts](../reference/desk.md#r-theme.module-contracts).

The color of one target is found by `tintSlot(theme, target)`. The lookup order is: the target's own entry in `tintMap`, then the entry for its role (stored under the key `role:<role>`), then the default slot from the declaration, and finally `neutral`. An entry for the target itself therefore takes precedence over its role, and a role entry applies to every surface that shares the role.

## Text on color: readableOn()

A user can pick any accent color, and some colors make the text on them unreadable. A theme declares the text color it wants on the accent, `onAccentRgb`. The function `readableOn(surfaceHex, preferredRgb)` keeps that color while it is readable, and otherwise chooses white or near-black, whichever has more contrast.

Filename: core/ui/theme.js (abridged)

```js
export function readableOn(surfaceHex, preferredRgb) {
  const surface = hexToRgb(surfaceHex);
  if (contrastRatio(surface, preferredRgb) >= MIN_TEXT_CONTRAST) return preferredRgb;
  return ["255, 255, 255", "16, 16, 20"].sort((a, b) => contrastRatio(surface, b) - contrastRatio(surface, a))[0];
}
```

`MIN_TEXT_CONTRAST` is 4.5, the WCAG AA threshold for normal text. The `contrastRatio()` function uses the relative luminance of the two colors. Two examples, computed with the same formula:

- The default accent, `#d71921`, gives 5.18:1 against white. White is kept.
- The pastel coral `#f7a38c` gives 1.99:1 against white and 9.56:1 against near-black. The function returns near-black.

The result is written as `--on-accent-rgb`, and the rule is [theme.contrast](../reference/desk.md#r-theme.contrast).

## Motion: the token tables

`core/ui/motion.js` begins with two frozen tables. `EASINGS` names the curves, and `DURATIONS` names the base durations. Widgets and surfaces are meant to use these names, and the rule is stated in [Motion](../reference/motion.md).

Filename: core/ui/motion.js (abridged)

```js
export const EASINGS = Object.freeze({
  standard: "cubic-bezier(0.4, 0, 0.2, 1)",
  spring: "cubic-bezier(0.34, 1.56, 0.64, 1)",
  expo: "cubic-bezier(0.30, 0.95, 0.35, 1.00)",
  ...
});

export const DURATIONS = Object.freeze({
  hover: 120,
  transient: 220,
  motion: 300,
  layout: 340,
});
```

An easing is the curve that maps time to progress. `standard` starts and ends gently, and `spring` overshoots and settles, which suits a popup that bounces into place. The names are the vocabulary of the project: a developer asks for `expo`, not for four numbers.

A duration is a base in milliseconds and a multiplier. `getDuration(token)` returns the base times the profile factor times the global scale, rounded, and never less than 1 ms. The three profiles, `balanced`, `responsive`, and `fluid`, each give a factor for every token. A `fluid` profile makes the `layout` token 30 percent slower, and a `responsive` profile makes it 25 percent faster. The profile is an axis of its own. It does not change the global scale, which is a different setting.

Warning: a number typed into CSS or JavaScript, such as `setTimeout(fn, 220)` or `transition: all 220ms`, skips the profile and the scale, so the user's speed setting does not apply to it. The rule is [motion.no-hardcoded-ms](../reference/motion.md#r-motion.no-hardcoded-ms).

## Duration from distance

A large movement should take longer than a small one, but not without limit. `calculateDuration(targetW, targetH, originW, originH)` takes the distance between the origin and the target, and maps it on a logarithmic scale between 240 ms and 440 ms.

The formula, from the code:

```text
d  = sqrt(dW^2 + dH^2)           the distance in pixels
r  = ln(1 + d / 100) / ln(1 + 1600 / 100)     clamped to 0..1
t  = 240 + (440 - 240) * r
ms = round(t * layout factor * scale), minimum 1
```

The comment in `motion.js` calls it a logarithmic physical formula. The curve rises quickly over the first few hundred pixels and more slowly after that, so the time is capped at 440 ms. A distance of 400 px gives `r` of about 0.568, so the time is about 354 ms in the balanced profile at full scale.

## Scale, profile, and reduced motion

The global scale has two values. `userScale` is what the user chose, between 0 and 3. `currentScale` is what is in effect. Normally they are equal. When the operating system asks for reduced motion, `currentScale` becomes 0.01, and `userScale` is not touched.

The separation is deliberate. An early version stored the reduced-motion value in the user's setting, so a user who turned the system setting off found the animations stuck near zero. The comment in `motion.js` records that failure. The reduced-motion rule is stated in [Motion](../reference/motion.md).

`applyEffectiveMotionScale()` is the one function that writes the results. It sets `--dur-scale` and each `--dur-*` token on `document.documentElement`, and it copies every entry of `EASINGS` into a `--ease-*` variable with a name in kebab case. So `expoOut` becomes `--ease-expo-out`. Any CSS that reads these variables picks up a change without a reload.

## Animating an element: MotionTimeline

Some animations need several properties moving together, and need to be reversed in the middle. `MotionTimeline` wraps the Web Animations API. A timeline holds a list of tracks; each track is an element, its keyframes, and its options. `play()` starts all tracks at once. When a previous timeline is still running on the same element, `play()` cancels it, so a finished effect cannot hold a property against a newer one.

`reverse()` calls `animation.reverse()` on each track, so a surface that is closed while it opens goes back along the same path. The surface manager relies on this. The rule is [motion.reversal](../reference/motion.md#r-motion.reversal), and the frame-by-frame account is in [Surfaces](../reference/surfaces.md).

Note: with reduced motion, or with a scale of zero, `play()` does not start an animation at all. It writes the final keyframe to the element's style and resolves at once, so the code that waits on `finished` still runs.

## Where to read more

The complete tables and the rules are in [Motion](../reference/motion.md). The morph of a surface, from its first frame to its last, and the rule that it is never scaled with `transform`, are in [Surfaces](../reference/surfaces.md) and in the rule [motion.clip-morph](../reference/motion.md#r-motion.clip-morph). The placement of widgets, which theme changes do not touch, is in [The Desk, the Menu, and Themes](../reference/desk.md).

## Summary

A value in UniDTK has one owner, one name, and one fallback. The theme is a preset record that is normalized, applied through `applyTheme()`, and cached in a form that a classic script paints before the first frame. A module adds its surfaces through its metadata, and a text color is chosen by a contrast check. A duration is a token from a fixed table, scaled by the user's profile and speed, and set to near zero when the system asks for reduced motion.

The next chapter follows the same system into the cases where something goes wrong. Continue with [Chapter 9. When Things Fail](ch09-when-things-fail.md).
