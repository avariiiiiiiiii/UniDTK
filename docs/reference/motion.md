# Motion: Tokens, Speed, and Duration

> **Scope:** The animation engine (`motion.js`), the unified surface manager (`surface-manager.js`), all widgets, popups, modals, overlays and context menus.

In this document you will learn how one animation request in UniDTK becomes pixels on the screen. We follow the path in order. First come the timing tokens, which `motion.js` writes into CSS variables. Then come the global speed switch and reduced motion. Next is the duration formula. After that we follow a surface morph frame by frame, then closing and reversal, and finally the stack that decides which surface Escape closes. Each step names the file and the function that does the work, so you can check it against the code.

Every claim below was checked against the source in this tree. Where the older text of this document said something the code does not do, the section says so in a "Note" and the list of known gaps at the end of [Surfaces](surfaces.md) collects all of them.

---

## 1. Two Kinds of Objects

The whole UniDTK graphics subsystem is split into **exactly two types of objects**. Hybrids and ad-hoc popup blocks are not allowed.

```
                     ┌────────────────────────────────────────┐
                     │             UniDTK Engine              │
                     └───────────────────┬────────────────────┘
                                         │
                 ┌───────────────────────┴───────────────────────┐
                 ▼                                               ▼
    ┌─────────────────────────┐                     ┌─────────────────────────┐
    │    1. WIDGETS           │                     │    2. SURFACES          │
    │     (WIDGETS)           │                     │     (SURFACES)          │
    ├─────────────────────────┤                     ├─────────────────────────┤
    │ • Always on the desktop │                     │ • Open on click         │
    │ • Store coordinates     │                     │ • Morph from hitbox     │
    │ • Drag & Drop / Resize  │                     │ • LIFO close stack      │
    │ (Player, Clock, Folders)│                     │ (Explorer, popups, TG)  │
    └─────────────────────────┘                     └─────────────────────────┘
```

### Type 1. Desktop Widget (`Widget`)

- **Definition:** A persistent card that is always present on the `widgetsCanvas` surface.
- **Examples:** The round audio player (`mp3-player`), the clock (`clock`), the Telegram dock (`telegram`), the glyph bar (`glyph-bar`). The names used in early versions of the docs (`yt-music`, `games`/`notes`/`launcher`/`network`/`tools`) are outdated. The current modules are `clock`, `explorer`, `glyph-bar`, `mp3-player`, `telegram`, and `vpn`.
- **Properties:** A widget keeps its coordinates in `config.positions`. It supports dragging (`setupDragReorder`) and recalculation when screens change (`relayoutWidgets`). It is never reset or removed from the DOM when other windows close.

### Type 2. Surface / Window (`Surface`)

- **Definition:** Any window, panel, modal, dialog, or menu that opens in response to a user action.
- **Examples:**
  - The Nothing Explorer file manager window (`#explorerOverlay`);
  - The audio player popups: the library (`.music-library-popup`) and the settings (`.music-settings-popup`);
  - The VPN server panel in the clock (`.vpn-server-menu`);
  - Telegram modals (forwarding messages, sending files, the photo lightbox);
  - Right-click context menus (in Explorer and in chats).
- **Properties:** Surfaces are controlled **only** through the single core contract `SurfaceManager`. They are mounted into the single coordinate-safe portal `#shell-portals`, use the same hardware border morph, and are registered on a LIFO stack.

The rest of this document is about surfaces. A widget only has a position. A surface is the only object that animates open and closed, so it is the only one that needs the morph machinery.

---

## 2. Hard Motion Guardrails

These rules are the contract. Sections 3 to 9 explain the mechanism behind each one.

### Rule 2.1: Full Ban on Hardcoded Milliseconds

r[motion.no-hardcoded-ms]

- **Writing fixed animation timings in CSS and JS is strictly forbidden** (`140ms`, `220ms`, `340ms`, `400ms`, `setTimeout(..., 220)`).
- The animation duration $T$ is **computed at runtime** by the function `calculateDuration(targetW, targetH, originW, originH)` in `motion.js`. It is based on the physical size, or on the expansion delta of the border ($\Delta d = \sqrt{\Delta W^2 + \Delta H^2}$):

$$T(\Delta d) = \left[ T_{\min} + (T_{\max} - T_{\min}) \cdot \frac{\ln\left(1 + \frac{\Delta d}{d_0}\right)}{\ln\left(1 + \frac{d_{\max}}{d_0}\right)} \right] \times \text{currentScale}$$

Where:
- $T_{\min} = 240\text{ ms}$ (micro menus, context lists);
- $T_{\max} = 440\text{ ms}$ (full-screen Explorer);
- $d_0 = 100\text{ px}$, $d_{\max} = 1600\text{ px}$;
- $\text{currentScale}$ is the global speed multiplier set by `setMotionScale()`.

The code also multiplies by the `layout` factor of the active motion profile (section 6). The formula above is the profile-neutral core.

Because the term is a logarithm, the duration rises quickly for small changes and flattens out for large ones.

### Rule 2.2: Hardware Border Morph from the Hitbox (Origin Clip-Morphing)

r[motion.clip-morph]

- **Scaling content (`scale`) is forbidden:** window content (text, Ndot/Inter fonts, icons, covers) must **never be scaled with `transform: scale()`**. Scaling fonts blurs DirectWrite subpixel rendering in WebView2. The one exception is a stand-in that travels between an object and its surface ([`motion.shared-flight`](surfaces.md#r-motion.shared-flight)): a copy of an icon glyph scaled uniformly, or of a clock face scaled per axis. The real content in the surface is never scaled, and body text is never a flight.
- **Morphing the border:** The window is mounted immediately at its final pixel size (one layout pass, no reflow during the animation). The container's border is expanded on the GPU through `clip-path: inset(top right bottom left round radius)`:
  - $t = 0$: the mask is compressed to the exact coordinates and corner radius of the trigger button or hitbox. For example, it matches the `zone-top` sector of the player with a radius of $R=75\text{px}$, following the vinyl circle exactly.
  - $t = 1$: the mask expands exponentially to `inset(0 0 0 0 round 24px)`.
- **Content fade-in:** The content fades in with `opacity: 0 -> 1`.

> **Note:** The older text also asked for a $\Delta y = 6\text{–}8\text{ px}$ move of the content and for the fade to start at 65% of the border movement. Neither is in the code. `surface-manager.js` animates only `opacity` on the content ([section 7 of Surfaces](surfaces.md#7-the-morph-frame-by-frame).3), and the string `translateY` does not occur in the file. The content fades over the same duration as the morph.

### Rule 2.3: Seamless Animation Reversal

r[motion.reversal]

- `setTimeout` must not be used to finish animations.
- If the user presses `Escape` or clicks outside the window while it is opening (for example, 60 ms into a 260 ms animation), the animation **does not freeze or jerk**. The window closes smoothly back into the hitbox from the same intermediate point.

> **Note:** The mechanism is not `animation.reverse()`. `surface-manager.js` reads the rendered clip-path and opacity first, then starts a new close timeline from those values ([section 8 of Surfaces](surfaces.md#8-closing-and-reversal)). `MotionTimeline.reverse()` exists in `motion.js`, but no caller uses it.

### Rule 2.4: A Single LIFO Close Stack (Last In, First Out)

r[motion.lifo-stack]

- Every open surface is registered in the core array `SurfaceManager.activeStack`.
- The `Escape` key and clicks outside a window are handled by one global core listener.
- Pressing `Escape` closes **exactly one surface, the topmost one**:
  $$\text{Context menu} \xrightarrow{\text{Esc}} \text{Modal} \xrightarrow{\text{Esc}} \text{Explorer} \xrightarrow{\text{Esc}} \text{Settings menu} \xrightarrow{\text{Esc}} \text{Collapse widgets}$$
- Background windows and widgets must not be closed while a top-level modal is open.

The last two steps are desktop steps, not stack entries, and a surface that Escape does not close (`closeOnEscape: false`) passes the key on to them. They are explained in [section 9 of Surfaces](surfaces.md#9-the-lifo-stack-and-the-global-listeners).

### Rule 2.5: Protecting Widget Coordinates and Integrity

r[motion.widget-integrity]

- No operation on surfaces or animations may, under any circumstances, reset `config.positions` or perform a destructive DOM reset (`innerHTML = ""`) on `widgetsCanvas`.

> **Note:** Section 10 shows one path in the code that clears `widgetsCanvas`. This rule is therefore not fully enforced yet.

---

## 3. From Tokens to CSS Variables

The motion system keeps two sources of truth for timing and easing. The JavaScript token tables in `motion.js` are the real source. The `:root` values in `styles.css` are only a fallback for the first paint, before JavaScript runs.

### 3.1 The Token Tables

r[motion.canonical-tokens]

Only the easings and durations in the tables below exist. `motion.js` (`EASINGS`, `DURATIONS`) is their source of truth; the `:root` values in `styles.css` are a fallback, and an arbitrary `cubic-bezier` or timing in CSS is not allowed.

The easing curves and duration names live in `motion.js`:

```js
// Filename: core/ui/motion.js
export const EASINGS = Object.freeze({
  standard: "cubic-bezier(0.4, 0, 0.2, 1)",
  spring: "cubic-bezier(0.34, 1.56, 0.64, 1)",
  expo: "cubic-bezier(0.30, 0.95, 0.35, 1.00)",
  expoOut: "cubic-bezier(0.30, 0.95, 0.35, 1.00)",
  expoOutRev: "cubic-bezier(0.38, 0.05, 0.45, 1.00)",
  smooth: "cubic-bezier(0.25, 0.1, 0.25, 1)",
  snap: "cubic-bezier(0.18, 1.04, 0.22, 1)",
  content: "cubic-bezier(0.25, 1, 0.5, 1)",
  surfaceExit: "cubic-bezier(0.38, 0.05, 0.45, 1.00)",
  itemIn: "cubic-bezier(0.16, 1, 0.3, 1)",
});

export const DURATIONS = Object.freeze({
  hover: 120,
  transient: 220,
  motion: 300,
  layout: 340,
});
```

The camel-case names are the JavaScript keys. The CSS names are derived from them. `expoOut` becomes `--ease-expo-out` and `itemIn` becomes `--ease-item-in`.

### 3.2 Writing the Variables

`initMotionSystem()` runs when the core starts. It is called at the top level of `core/ui/unidtk-core.js`, line 11, right before `initThemeSystem()`. The function that actually writes variables is `applyEffectiveMotionScale()`:

```js
// Filename: core/ui/motion.js (abridged)
function applyEffectiveMotionScale() {
  currentScale = isReducedMotion ? 0.01 : userScale;
  document.documentElement.style.setProperty("--dur-scale", String(currentScale));
  document.documentElement.style.setProperty("--dur-hover", `${getDuration("hover")}ms`);
  document.documentElement.style.setProperty("--dur-transient", `${getDuration("transient")}ms`);
  document.documentElement.style.setProperty("--dur-motion", `${getDuration("motion")}ms`);
  document.documentElement.style.setProperty("--dur-layout", `${getDuration("layout")}ms`);

  // Dynamically synchronize all canon easings to :root
  for (const [key, val] of Object.entries(EASINGS)) {
    const cssKey = `--ease-${key.replace(/([A-Z])/g, "-$1").toLowerCase()}`;
    if (document.documentElement.style.getPropertyValue(cssKey) !== val) {
      document.documentElement.style.setProperty(cssKey, val);
    }
  }
}
```

Step by step, the function does four things:

1. It picks the effective scale. Reduced motion forces `0.01`. Otherwise the scale is the user's own value.
2. It converts each duration token into a millisecond string with `getDuration()`, which applies the scale and the motion profile, and writes it as `--dur-*` on `<html>`.
3. It loops over `EASINGS`, converts each key to a kebab-case variable name, and writes it only when the value changed.
4. Because the write is on `document.documentElement.style`, these are inline custom properties. They beat the `:root` defaults in the stylesheet.

### 3.3 The Fallback in the Stylesheet

`core/ui/styles.css` declares the same names on `:root`, so the first paint has the correct values before any script runs:

```css
/* Filename: core/ui/styles.css (abridged) */
:root {
  --ease-standard: cubic-bezier(0.4, 0, 0.2, 1);
  --ease-expo-out: cubic-bezier(0.30, 0.95, 0.35, 1.00);
  --ease-surface-exit: cubic-bezier(0.38, 0.05, 0.45, 1.00);
  --z-surface-base: 100;
  --z-surface-panel: 250;
  --z-surface-overlay: 2000;
  --z-surface-modal: 3000;
  --z-surface-menu: 4000;
  --z-surface-toast: 5000;
  --dur-scale: 1.0;
  --dur-hover: 120ms;
  --dur-transient: 220ms;
  --dur-motion: 300ms;
  --dur-layout: 340ms;
}
```

Rules that need a timing use `var(--dur-*)` and `var(--ease-*)`. A rule that hard-codes a value, such as `ease` without a variable, is outside the contract. Some older rules in `styles.css` still do this, for example `transition: background var(--dur-hover) ease`.

### 3.4 Motion Profiles

Profiles are a second, separate axis. They change the duration of each token by a factor, without touching the global scale:

```js
// Filename: core/ui/motion.js
export const MOTION_PROFILES = Object.freeze({
  balanced: { hover: 1, transient: 1, motion: 1, layout: 1 },
  responsive: { hover: 0.85, transient: 0.9, motion: 0.8, layout: 0.75 },
  fluid: { hover: 1, transient: 1.1, motion: 1.2, layout: 1.3 },
});
```

`setMotionProfile()` stores the chosen factors in `profileFactors` and then calls the same `applyEffectiveMotionScale()`. The key point is that `--dur-scale` stays the same when the profile changes, because `currentScale` is not touched.

---

## 4. The Global Speed Switch

`setMotionScale(scale)` is the user's speed setting. Here is the function:

```js
// Filename: core/ui/motion.js
export function setMotionScale(scale = 1.0) {
  const requested = Number(scale);
  userScale = Number.isFinite(requested) ? Math.max(0, Math.min(3.0, requested)) : 1.0;
  applyEffectiveMotionScale();
}
```

It does three things. It converts the input to a number, and any value that is not a finite number falls back to `1.0`. It clamps the result to the range `0` to `3`. It stores the value in `userScale`, not in `currentScale`. Then it reapplies the variables.

The callers are:

- `core/ui/theme.js`, when a theme field `motionScale` changes (line 772), and at theme application (line 918, next to `setMotionProfile`).
- `core/ui/harness.js`, in `UniDtkHarness.setZeroMotion()`. Passing `true` calls `setMotionScale(0)` and adds the class `zero-motion` to `<html>`. Passing `false` restores `1.0`. Tests use this to get instant state changes.

A scale of `0` is a real value. In `MotionTimeline.play()` a `currentScale <= 0` check takes the same synchronous path as reduced motion (section 5). Nothing waits for a browser event.

---

## 5. Reduced Motion

r[motion.reduced-motion]

`prefers-reduced-motion` is always honoured, without overwriting the user's own speed: `userScale` (the choice) and `currentScale` (the effective value, `0.01` under reduced motion) are kept apart.

Reduced motion has two layers, and they work independently.

**Layer 1: the stylesheet.** `styles.css` has a media query for the operating-system setting. It shortens every CSS animation and transition to `0.01ms`:

```css
/* Filename: core/ui/styles.css (abridged) */
@media (prefers-reduced-motion: reduce) {
  *,
  *::before,
  *::after {
    animation-duration: 0.01ms !important;
    animation-iteration-count: 1 !important;
    transition-duration: 0.01ms !important;
    scroll-behavior: auto !important;
  }
}
```

This layer needs no JavaScript. It catches the CSS transitions in the widgets.

**Layer 2: the engine.** `initMotionSystem()` reads the same media query and keeps the answer in `isReducedMotion`:

```js
// Filename: core/ui/motion.js (abridged)
export function initMotionSystem() {
  const mediaQuery = window.matchMedia("(prefers-reduced-motion: reduce)");
  isReducedMotion = mediaQuery.matches;

  const update = () => {
    isReducedMotion = mediaQuery.matches;
    applyEffectiveMotionScale();
  };
  try {
    mediaQuery.addEventListener("change", update);
  } catch {
    mediaQuery.addListener?.(update);
  }

  applyEffectiveMotionScale();
}
```

When the flag is set, three things change:

- `applyEffectiveMotionScale()` sets `currentScale = 0.01`.
- `calculateDuration()` and `getDuration()` return `1` ms immediately.
- `MotionTimeline.play()` does not start any WAAPI animation. It writes the last keyframe of every track straight into the element's inline style, then settles the timeline in a microtask. The comment in the code says so: no callback depends on a browser finish event at duration 0.

The user's own value lives in `userScale`, which the reduced-motion flag never overwrites. The code comment explains why this matters. The old code stored the reduced-motion value into the user's scale, so leaving reduced motion left animations stuck at about zero. Now turning the OS setting off restores the user's value.

---

## 6. The Duration Formula

A surface's opening time is not a constant. It depends on how far the border has to move. `calculateDuration()` computes it:

```js
// Filename: core/ui/motion.js (abridged)
export function calculateDuration(targetW = 0, targetH = 0, originW = 0, originH = 0) {
  if (isReducedMotion) return 1;
  if (currentScale <= 0) return 0;
  // ... originW may also be a DOMRect; it is read as { width, height } ...
  const deltaW = Math.abs((Number(targetW) || 0) - oW);
  const deltaH = Math.abs((Number(targetH) || 0) - oH);
  const deltaD = Math.sqrt(deltaW * deltaW + deltaH * deltaH);
  const tMin = 240;
  const tMax = 440;
  const d0 = 100;
  const dMax = 1600;

  const ratio = Math.max(0, Math.min(1, Math.log(1 + deltaD / d0) / Math.log(1 + dMax / d0)));
  const t = tMin + (tMax - tMin) * ratio;
  const res = Math.max(1, Math.round(t * profileFactors.layout * currentScale));
  return Number.isFinite(res) ? res : 1;
}
```

Read it as five steps:

1. **Measure the expansion.** `deltaW` and `deltaH` are the absolute differences between the surface size and the origin (hitbox) size.
2. **Combine them.** `deltaD` is the diagonal of those differences, so one number describes how far the border travels.
3. **Take a logarithm.** `ln(1 + deltaD / d0)` grows quickly for small distances and slowly for large ones. Dividing by the same logarithm at `dMax` puts the result in the range `0` to `1`, and the clamp keeps it there.
4. **Interpolate.** The ratio moves the time between `tMin` (240 ms) and `tMax` (440 ms).
5. **Apply the scales.** The result is multiplied by the `layout` profile factor and by `currentScale`, then rounded and kept at least `1`.

**Worked example (arithmetic on the formula, scale 1, balanced profile).** A surface that grows by 400 px in width and nothing in height has `deltaD = 400`. Then `ratio = ln(5) / ln(17) = 1.6094 / 2.8332 = 0.568`, so `t = 240 + 200 × 0.568 = 353.6`, which rounds to **354 ms**.

Where the value comes from. `openSurface()` uses the token instead of the formula when the caller passes `opts.duration`. The code is `opts.duration != null ? getDuration(opts.duration) : calculateDuration(...)`. Otherwise `_openDuration()` in `surface-manager.js` (lines 232–243) calls `calculateDuration()` with the surface size and the origin size. The player popup passes `duration: "motion"` ([section 12 of Surfaces](surfaces.md#12-known-gaps), item 1).

The surfaces that these tokens animate are explained in [Surfaces: the morph, frame by frame](surfaces.md).
