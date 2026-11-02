# Unified Core Modules Motion (unicoremodmotion.md)

> **Document status:** Official architecture standard of the UniDTK core.  
> **Scope:** The animation engine (`motion.js`), the unified surface manager (`surface-manager.js`), all widgets, popups, modals, overlays and context menus.

In this document you will learn how one animation request in UniDTK becomes pixels on the screen. We follow the path in order. First come the timing tokens, which `motion.js` writes into CSS variables. Then come the global speed switch and reduced motion. Next is the duration formula. After that we follow a surface morph frame by frame, then closing and reversal, and finally the stack that decides which surface Escape closes. Each step names the file and the function that does the work, so you can check it against the code.

Every claim below was checked against the source in this tree. Where the older text of this document said something the code does not do, the section says so in a "Note" and the list in section 12 collects all of them.

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

- **Scaling content (`scale`) is forbidden:** window content (text, Ndot/Inter fonts, icons, covers) must **never be scaled with `transform: scale()`**. Scaling fonts blurs DirectWrite subpixel rendering in WebView2.
- **Morphing the border:** The window is mounted immediately at its final pixel size (one layout pass, no reflow during the animation). The container's border is expanded on the GPU through `clip-path: inset(top right bottom left round radius)`:
  - $t = 0$: the mask is compressed to the exact coordinates and corner radius of the trigger button or hitbox. For example, it matches the `zone-top` sector of the player with a radius of $R=75\text{px}$, following the vinyl circle exactly.
  - $t = 1$: the mask expands exponentially to `inset(0 0 0 0 round 24px)`.
- **Content fade-in:** The content fades in with `opacity: 0 -> 1`.

> **Note:** The older text also asked for a $\Delta y = 6\text{–}8\text{ px}$ move of the content and for the fade to start at 65% of the border movement. Neither is in the code. `surface-manager.js` animates only `opacity` on the content (section 7.3), and the string `translateY` does not occur in the file. The content fades over the same duration as the morph.

### Rule 2.3: Seamless Animation Reversal

- `setTimeout` must not be used to finish animations.
- If the user presses `Escape` or clicks outside the window while it is opening (for example, 60 ms into a 260 ms animation), the animation **does not freeze or jerk**. The window closes smoothly back into the hitbox from the same intermediate point.

> **Note:** The mechanism is not `animation.reverse()`. `surface-manager.js` reads the rendered clip-path and opacity first, then starts a new close timeline from those values (section 8). `MotionTimeline.reverse()` exists in `motion.js`, but no caller uses it.

### Rule 2.4: A Single LIFO Close Stack (Last In, First Out)

- Every open surface is registered in the core array `SurfaceManager.activeStack`.
- The `Escape` key and clicks outside a window are handled by one global core listener.
- Pressing `Escape` closes **exactly one surface, the topmost one**:
  $$\text{Context menu} \xrightarrow{\text{Esc}} \text{Modal} \xrightarrow{\text{Esc}} \text{Explorer} \xrightarrow{\text{Esc}} \text{Collapse widgets}$$
- Background windows and widgets must not be closed while a top-level modal is open.

The last arrow is a fallback, not a stack entry. It is explained in section 9.

### Rule 2.5: Protecting Widget Coordinates and Integrity

- No operation on surfaces or animations may, under any circumstances, reset `config.positions` or perform a destructive DOM reset (`innerHTML = ""`) on `widgetsCanvas`.

> **Note:** Section 10 shows one path in the code that clears `widgetsCanvas`. This rule is therefore not fully enforced yet.

---

## 3. From Tokens to CSS Variables

The motion system keeps two sources of truth for timing and easing. The JavaScript token tables in `motion.js` are the real source. The `:root` values in `styles.css` are only a fallback for the first paint, before JavaScript runs.

### 3.1 The Token Tables

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

Where the value comes from. `openSurface()` uses the token instead of the formula when the caller passes `opts.duration`. The code is `opts.duration != null ? getDuration(opts.duration) : calculateDuration(...)`. Otherwise `_openDuration()` in `surface-manager.js` (lines 232–243) calls `calculateDuration()` with the surface size and the origin size. The player popup passes `duration: "motion"` (section 12, item 1).

---

## 7. The Morph, Frame by Frame

This section follows one open, from the call to the last frame.

### 7.1 Who Runs Each Frame

The JavaScript does **not** run once per frame. `surface-manager.js` contains no `requestAnimationFrame` loop. The flow is:

1. JavaScript measures the geometry, builds keyframes, and starts Web Animations (WAAPI) effects, once.
2. The browser's animation engine evaluates those keyframes on every frame. It interpolates the `clip-path` and `opacity` values, using the easing curve, and paints them.
3. JavaScript runs again only when an animation fires its `finish` event. Then `_finalizeOpen()` or `_finalizeClose()` runs once and removes the temporary styles.

This is why the morph is cheap: during the animation the main thread is idle, and the browser composites each frame.

### 7.2 `openSurface()` Prepares the Record

`openSurface(element, options)` does the setup. Its main steps are these:

1. It measures the surface (`surfaceRect`) and the origin (`originRect`, the hitbox).
2. It computes four insets, the distance from each edge of the surface to the matching edge of the origin:

```js
// Filename: core/ui/surface-manager.js (openSurface, abridged)
const topInset = opts.axis === "x" ? 0 : Math.max(0, Math.round(originRect.top - surfaceRect.top));
const leftInset = Math.max(0, Math.round(originRect.left - surfaceRect.left));
const rightInset = Math.max(0, Math.round(surfaceRect.right - (originRect.left + originRect.width)));
const bottomInset = opts.axis === "x" ? 0 : Math.max(0, Math.round(surfaceRect.bottom - (originRect.top + originRect.height)));
```

3. It creates a `record` with the state `closed`, pushes it onto `activeStack`, and sets `zIndex` when `elevation` is given.
4. It creates the shadow layer (section 7.5) and collects the content targets. The root element is never a content target, because it is already animated through its own clip.
5. It sets the first frame with `initialClip = inset(... round originR)` and calls `_startTransition(record, true, ...)`.

The element stays `visibility: hidden` until the first frame is ready, so the browser never paints the surface at full size with no mask.

### 7.3 `_startTransition()` Builds the Tracks

`_startTransition(record, open, sampled)` is the one place where keyframes are built, for both open and close. For an open it builds these tracks:

```js
// Filename: core/ui/surface-manager.js (_startTransition, open branch, abridged)
boundaryKeyframes = [
  { clipPath: from.clipPath, opacity: from.opacity, offset: 0 },
  { clipPath: fullClip, opacity: 1, offset: 1 },
];
// ...
const timeline = motionTimeline({
  duration: transitionDuration,
  durationIsScaled: true,
  easing: open ? EASINGS.expoOut : EASINGS.surfaceExit,
  onFinish: () => {
    if (record.animationGeneration !== generation || record.motionGroup !== timeline) return;
    if (record.state === "opening") this._finalizeOpen(record);
    else if (record.state === "closing") this._finalizeClose(record);
  },
});
```

The tracks are:

- **Boundary.** The surface element, from its current clip-path to `inset(0 0 0 0 round targetR)`, with opacity going to 1. The clip grows from the origin to the full box.
- **Shadow.** The sibling layer described in 7.5.
- **Content.** Each content target fades from its current opacity to `1`. On open its duration is the full transition duration and its easing is `content`. On close it is 45% of the duration (section 8).
- **Backdrop.** If the surface has one, it fades with the `standard` easing.

All tracks go into one `MotionTimeline`, so they start together. The timeline's `onFinish` runs only when the last track finishes, and the generation check makes sure a stale callback does nothing.

### 7.4 Why `clip-path` and Not `transform`

The content is never scaled. The boundary track changes only the mask. The surface box is already at its final size, so the text, icons and covers keep their real pixel size, and DirectWrite keeps its subpixel rendering. This is Rule 2.2 in code.

### 7.5 The Shadow Layer

A clip-path also cuts away anything outside its box, including the element's own `box-shadow`. To keep the shadow visible, `_createShadowLayer()` does three things:

1. It reads the element's computed `box-shadow`.
2. It creates a sibling `div`, `position: fixed`, with the same box, radius, and shadow, and inserts it before the element.
3. It sets `box-shadow: none !important` on the element. The shadow is now drawn by the sibling.

The sibling starts collapsed. Its transform is `translate(dx, dy) scale(sx, sy)`, which maps it onto the origin. Its track animates that transform back to `none` on open, with the `expoOut` easing. On close it goes back to the collapsed transform with `surfaceExit`.

### 7.6 `_finalizeOpen()` Removes the Temporary Styles

When the timeline finishes, `_finalizeOpen()` runs:

- It sets `opacity = 1` and clears the inline `clipPath` and `willChange`, so the element is no longer masked.
- It adds the class `surface-open` and removes `surface-closing`. The class `surface-active` was already added when the transition started.
- It calls the caller's `onOpened` callback. The player uses it to release a deferred render.
- It releases the shadow layer and gives the box-shadow back to the element.

---

## 8. Closing and Reversal

### 8.1 `closeSurface()` Starts From the Rendered Values

```js
// Filename: core/ui/surface-manager.js (closeSurface, abridged)
const sampled = this._sampleSurface(record);
record.state = "closing";
// ... focus, pointer-events and class changes ...
this._startTransition(record, false, sampled);
```

The comment in the code gives the reason. The animation timeline's `currentTime` is not visual progress when the easing is not linear. So the close reads the computed clip-path, opacity and shadow transform first, and then animates from exactly that point.

### 8.2 The Close Keyframes

On close, `_startTransition()` uses three clip-path keyframes instead of two:

- **Start:** the sampled clip.
- **Middle, at offset `0.8`:** 85% of the way from the sampled inset values to the closed inset values. The middle clip keeps the same opacity.
- **End:** the closed inset (the origin box) with `closeOpacity`, which defaults to `0`.

The duration is shorter when the surface is already close to the origin:

```js
transitionDuration = Math.round(duration * 0.65 * Math.sqrt(remainingRatio));
```

Here `remainingRatio` is the distance still to travel divided by the full distance, clamped to `0`–`1`. The content runs for `Math.round(transitionDuration * 0.45)`. The easing is `surfaceExit`.

### 8.3 Reversal Is a Re-Sample

Reversal in the code means "sample, then start a new timeline from the sampled values". The result looks like a reversal, because the new timeline begins at the same point the opening was at. It is not the WAAPI `reverse()` call. Section 2 explained the difference.

### 8.4 A Second Escape or Click Finishes at Once

If the user closes a surface that is already closing, the listeners call `_finalizeClose()` immediately, then start the close of the next surface in the stack (section 9).

### 8.5 `_finalizeClose()` Hides and Restores

`_finalizeClose()` does the following:

1. It removes the record from `activeStack`.
2. It sets the element to `opacity: 0 !important`. Cancelling an animation drops its last filled frame, so this keeps the element hidden until the owner restores its state.
3. It clears the classes, `clipPath`, `pointerEvents`, `willChange` and `zIndex`, and restores `display` from the value saved at open.
4. It calls the caller's `onClosed` callback.
5. It restores the element's original inline `opacity` and releases the shadow layer.

When the surface was opened with an explicit `display` option, the saved inline value is restored with `record.prevDisplay || ""` (line 812). Otherwise the function restores the saved inline value or leaves it empty (lines 813–818).

---

## 9. The LIFO Stack and the Global Listeners

`_initGlobalListeners()` installs two listeners once. The first handles Escape and runs in the capture phase on `window`:

```js
// Filename: core/ui/surface-manager.js (abridged)
window.addEventListener("keydown", (e) => {
  if (e.key === "Escape" || e.key === "Esc") {
    this._cleanupDeadNodes();
    if (this.activeStack.length === 0) return;
    // ... inline editors and the search field keep their own Escape ...
    e.preventDefault();
    e.stopPropagation();
    e.stopImmediatePropagation();
    const top = this.activeStack[this.activeStack.length - 1];
    if (!top || top.options.closeOnEscape === false) return;
    if (top.state === "closing") {
      this._finalizeClose(top);
      // ... then close the next surface in the stack ...
    } else {
      this.closeSurface(top.element);
    }
  }
}, { capture: true });
```

The second handles clicks outside the top surface, using a `pointerdown` listener in the capture phase. A click inside the surface, on its origin, or on an element listed in `ignoreOutsideElements` does nothing.

**Why the desktop does not also react.** When a surface is open, `stopImmediatePropagation()` stops the event before it reaches the desktop. When no surface is open, the handler returns early and the event continues. The desktop then runs its own capture listener in `core/ui/unidtk-core.js` (lines 2403–2415). That listener blurs a focused text field, or calls `collapseAllShellWidgets()` when nothing is focused. This is the "Collapse widgets" step in Rule 2.4. It is a fallback, not an entry on `activeStack`.

---

## 10. Widget Coordinates and the DOM

Widgets are never closed by a surface. Their coordinates live in `config.positions`, and `relayoutWidgets()` in `core/ui/unidtk-core.js` applies them after a resize:

```js
// Filename: core/ui/unidtk-core.js (relayoutWidgets, abridged)
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
    // ... sets left and top from `at` ...
  });
}
```

It moves existing cards and keeps their instances, so the player state and network connections survive a resize. That is Rule 2.5 in intention.

**The gap.** `syncWidgets()` clears the canvas with `widgetsCanvas.innerHTML = ""` (line 769) and then rebuilds every card. `relayoutWidgets()` calls `syncWidgets()` when the canvas has no cards (line 2524). So a resize that finds an empty canvas takes the full rebuild path. Other callers of `syncWidgets()` also rebuild. The rule forbids this during geometry changes, and the code does not yet enforce it in every path.

---

## 11. Recorded Backlog (History, With Corrections)

The old section was headed "Recorded Backlog and Polish Items (Backlog, 100% Complete)" and recorded eight polish items as resolved. The identifiers below exist in the tree. The numeric values were not re-measured for this document.

1. **Player popup speed.** `openMusicPopup` (`modules/mp3-player/ui/widget.js`, line 197) calls `openSurface` with `duration: "motion"`. The older text said `"layout"`. The content offset is `0.45` in `surface-manager.js` (line 308). It applies to the **close** only. On open the content uses the full transition duration.
2. **Closing popups by the header.** `closePopupElement` and the `.music-popup-header` style both exist in `modules/mp3-player/ui/`.
3. **VPN server list layout.** `_finalizeClose` restores `record.prevDisplay || ""` (line 812). The `.vpn-server-menu` rule exists in `core/ui/styles.css` and `modules/vpn/ui/vpn.css`.
4. **Telegram widget opening.** The rule of one geometry pass with a WAAPI `clipPath` morph is the same mechanism as section 7. The old text's `400ms` removal is not separately re-checked here.
5. **Telegram account dropdown.** `.tg-account-dropdown` and `tg-account-dropdown` exist in `modules/telegram/ui/`.
6. **Internal transitions.** Explorer's `scale(0.98)` effects and the Explorer marquee selection were described. They were not re-verified.
7. **Desktop click to close folders.** The handler is `setupDesktopOutsideClickHandler()` in `core/ui/unidtk-core.js`. The older text named `shell-core.js`, which does not exist in this tree.
8. **Widget mutual exclusion.** `collapseSingleWidget`, `expandSequence` and `_collapseTimer` exist in `core/ui/unidtk-core.js` and the Telegram widget. The old text recorded the timer as `320ms`; that value was not re-checked.

---

## 12. Known Gaps

These are the places where the code and this standard differ. They are listed here, not hidden, so they can be closed.

1. **Content motion (Rule 2.2).** No `translateY` offset, and the fade does not start at 65%. Content only fades its opacity.
2. **Reversal (Rule 2.3).** Closing re-samples and starts a new timeline. `animation.reverse()` is not used by the surface manager.
3. **Coordinates (Rule 2.5).** `relayoutWidgets()` can reach `syncWidgets()`, which clears `widgetsCanvas`.
4. **Player popup duration.** The code uses `"motion"`, not `"layout"`.
5. **Escape chain.** "Collapse widgets" is a desktop fallback, not a stack entry.
6. **Not re-checked for this rewrite.** The Telegram modals and context menus named in section 1, the numeric values in section 11, and the performance claims elsewhere in the older text.

---

## 13. The Engineering Plan (Historical)

This plan was the order in which the work was done. It is kept for reference, and each stage is already reflected in the code above.

### Stage 1. Motion Core and Dynamic Timing Math (`motion.js`)

1. Implemented in `core/ui/motion.js`:
   - The function `calculateDuration(width, height, originRect)` (the code takes four numbers, or an origin object), based on the logarithmic formula above.
   - The full set of tokens (EASINGS in `core/ui/motion.js`): `--ease-standard` `(0.4,0,0.2,1)`, `--ease-spring` `(0.34,1.56,0.64,1)`, `--ease-expo`/`--ease-expo-out` `(0.30,0.95,0.35,1.00)`, `--ease-expo-out-rev`/`--ease-surface-exit` `(0.38,0.05,0.45,1.00)`, `--ease-snap` `(0.18,1.04,0.22,1)`, `--ease-content` `(0.25,1,0.5,1)`, `--ease-smooth` `(0.25,0.1,0.25,1)`, `--ease-item-in` `(0.16,1,0.3,1)`.
   - Support for seamless `reverse()` in the Web Animations API. The timeline class provides `reverse()`, but the surface manager does not use it (section 8.3).

### Stage 2. The Core Surface Dispatcher (`surface-manager.js`)

1. Created in `core/ui/surface-manager.js`:
   - The LIFO stack of open windows (`activeStack`);
   - A single `Escape` handler in the capture phase, which guarantees that exactly one top window closes;
   - A single `pointerdown` handler for closing on a click outside;
   - The function `openSurface(element, options)`: calculates the hitbox coordinates, starts the hardware `clip-path` border morph, and fades in the content with a delay;
   - The function `closeSurface(element)`: smoothly reverses back into the hitbox;
   - The canonical layer scale `--z-surface-*`.

### Stage 3. Moving the Player Popups (Library and Settings)

1. Connect the player popups (`.music-library-popup` and `.music-settings-popup`) to `surface-manager.js`:
   - Clicking `zone-top` expands the border from the top sector of the circle ($R=75\text{px} \to 24\text{px}$);
   - Clicking `zone-bottom` expands it from the bottom touch zone;
   - The playlist content fades in smoothly, without distorting covers or text;
   - Pressing `Escape` in the middle of the opening animation reverses it instantly back into the hitbox.

### Stage 4. Connecting Explorer, Telegram Modals, and Context Menus

1. Move the full-screen Explorer overlay (`#explorerOverlay`) to registration in `surface-manager.js`:
   - Replace the old `scale(0.92) + translateY(12px)` with hardware border expansion using dynamic timing;
   - Explorer and Telegram context menus are registered as top-layer surfaces and are closed first by `Escape`.
2. Remove the duplicate click handlers in `vpn-panel.js` and fix the memory leaks.

### Stage 5. End-to-End Verification and Measurements

1. Start the application and run an end-to-end measurement through the CLI bridge (`udtk`): `udtk doctor` (including `-simulate`), `udtk logs --level ERROR`, `udtk shot`.
2. Start UniDTK through Task Scheduler `UniDTK` (`schtasks /run /tn UniDTK`) or `udtk run`, and measure:
   - `udtk doctor --quick`: checks the integrity of DLLs, IPC, and tasks;
   - `udtk logs --level ERROR`: checks that the error count is zero;
   - `udtk shot`: a required visual audit in the collapsed, half-open, and fully open states.
3. Record the session results in `docs/architecture.md`.
