# Surfaces: The Morph, Frame by Frame

This chapter follows one surface (a popup, a modal, an overlay) from `openSurface()` to `_finalizeClose()`. It uses the tokens and the duration formula from [Motion](motion.md).

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
4. It collects the content targets. The root element is never a content target, because it is already animated through its own clip. The shadow layer (section 7.5) is made later, inside `_startTransition()`.
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

**Reads before writes.** Every geometry and style read of a transition happens at its top, before the first write: the sampled clip and opacity, the shadow sample, the insets, the shadow measure (`_measureShadow`), and the opacity of each content target and the backdrop when the caller did not sample them. Then come the writes: the state classes, `pointerEvents`, `willChange`, the `onTransitionStart` callback, the shadow layer, and the timeline. A read after a write makes the browser lay the page out again inside the same task, and before session 136 a transition start paid for up to three such layouts. The `onTransitionStart` callback still runs after the class writes, so a caller that reads geometry there pays one layout; that is the contract callers rely on.

### 7.4 Why `clip-path` and Not `transform`

The content is never scaled. The boundary track changes only the mask. The surface box is already at its final size, so the text, icons and covers keep their real pixel size, and DirectWrite keeps its subpixel rendering. This is Rule 2.2 in code.

### 7.5 The Shadow Layer

A clip-path also cuts away anything outside its box, including the element's own `box-shadow`. To keep the shadow visible, the layer is made in two halves:

1. `_measureShadow()` reads the element's computed `box-shadow`, radius and `zIndex`, and takes the box from the rect the insets just measured. It runs with the other reads of the transition.
2. `_createShadowLayer()` creates a sibling `div`, `position: fixed`, with that box, radius and shadow, and inserts it before the element. It reads nothing.
3. It sets `box-shadow: none !important` on the element. The shadow is now drawn by the sibling.

A layer made by the current transition has no animation yet, so its track starts from the layer's inline transform and opacity instead of a computed-style read.

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
// ... onClose, and focus leaves the surface ...
this._startTransition(record, false, sampled);
```

Pointer events stop and `surface-closing` is added by `_startTransition()` itself, in the same task, after its reads.

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
    const top = this.activeStack[this.activeStack.length - 1];
    if (!top || top.options.closeOnEscape === false) return;
    e.preventDefault();
    e.stopPropagation();
    e.stopImmediatePropagation();
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

**Why the desktop does not also react.** When the top surface closes on Escape, `stopImmediatePropagation()` stops the event before it reaches the desktop. When no surface is open, or the top one was opened with `closeOnEscape: false` (the Telegram card), the handler returns before touching the event, and it continues. The desktop then runs its own capture listener in `core/ui/unidtk-core.js`, in this order:

1. A focused text field is blurred, and nothing else happens.
2. An open settings menu takes the key: `handleMenuEscape()` in `core/ui/menu.js` steps back from VPN to "Desk", or closes the menu. It works wherever focus is, so a click on the desk while the menu is open does not strand it.
3. Otherwise `collapseAllShellWidgets()` runs. It folds the Telegram card through `collapseTelegramWidget` (its exit morph and dock restore) and then the other widgets. This is the "Collapse widgets" step in Rule 2.4. It is a fallback, not an entry on `activeStack`.

Before session 136 the manager consumed the key even for a `closeOnEscape: false` surface, so with Telegram expanded Escape closed neither Telegram nor the menu.

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

## 12. Known Gaps

These are the places where the code and this standard differ. They are listed here, not hidden, so they can be closed.

1. **Content motion (Rule 2.2).** No `translateY` offset, and the fade does not start at 65%. Content only fades its opacity.
2. **Reversal (Rule 2.3).** Closing re-samples and starts a new timeline. `animation.reverse()` is not used by the surface manager.
3. **Coordinates (Rule 2.5).** `relayoutWidgets()` can reach `syncWidgets()`, which clears `widgetsCanvas`.
4. **Player popup duration.** The code uses `"motion"`, not `"layout"`.
5. **Escape chain.** The settings menu and "Collapse widgets" are desktop steps, not stack entries (section 9). A menu opened over a closable surface is not on the stack, so Escape closes that surface first.
6. **Not re-checked for this rewrite.** The Telegram modals and context menus named in [section 1 of Motion](motion.md#1-two-kinds-of-objects), the numeric values in [the motion backlog](../history/rfcs/motion-plan.md), and the performance claims elsewhere in the older text.

---
