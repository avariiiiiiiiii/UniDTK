/**
 * UniDTK — Unified Surface Manager (unicoremodmotion.md §2, §3)
 *
 * Enforces the 2-object model:
 *   1. Desktop Widgets (Widget) — permanent desktop cards
 *   2. Surfaces/Windows (Surface) — modals, popups, overlays, context menus
 *
 * Core Guarantees:
 *   - Unified LIFO stack (activeStack) for strictly ordered dismissal.
 *   - Global Escape capture: Esc closes ONLY the topmost surface.
 *   - GPU origin-based clip-path morphing (zero content zoom/blur).
 *   - Seamless animation reversal via Web Animations API (animation.reverse()).
 *   - Dynamic logarithmic physical timing (calculateDuration).
 *   - Full working-set safety and 0 DOM resets of widgets canvas.
 *   - Robust dead-node pruning & sub-frame rapid Escape handling.
 */

import { calculateDuration, getDuration, EASINGS, motionTimeline } from "./motion.js";

function parseInsetValues(clipPath) {
  const match = String(clipPath || "").match(/inset\(\s*([^)]*)\)/i);
  if (!match) return null;
  const insetPart = match[1].split(/\s+round\b/i)[0];
  const values = Array.from(insetPart.matchAll(/(-?\d+(?:\.\d+)?)px/gi), (item) => Number(item[1]));
  if (!values.length) return null;
  if (values.length === 1) return [values[0], values[0], values[0], values[0]];
  if (values.length === 2) return [values[0], values[1], values[0], values[1]];
  if (values.length === 3) return [values[0], values[1], values[2], values[1]];
  return values.slice(0, 4);
}

class SurfaceManager {
  constructor() {
    /** @type {Array<SurfaceRecord>} */
    this.activeStack = [];
    this._initialized = false;
    this._initGlobalListeners();
  }

  /**
   * Automatically prune any elements from activeStack that have been
   * unmounted or removed from the DOM.
   */
  _cleanupDeadNodes() {
    this.activeStack = this.activeStack.filter((rec) => {
      if (!rec.element || !rec.element.isConnected) {
        this._cancelRecordAnimations(rec);
        this._releaseShadowLayer(rec);
        if (rec.backdrop && rec.backdrop.isConnected && rec.options.removeBackdropOnClose) {
          rec.backdrop.remove();
        }
        // Morph stand-ins are portaled to <body>, so they outlive the card that
        // owns them. Unmounting the owner would strand one on screen forever.
        if (rec.element) {
          for (const clone of document.querySelectorAll(".clock-morph-clone")) {
            if (clone._clockMorphOwner === rec.element) clone.remove();
          }
        }
        return false;
      }
      return true;
    });
  }

  _cancelRecordAnimations(record) {
    const motionGroup = record.motionGroup;
    record.motionGroup = null;
    const animations = [
      record.boundaryAnim,
      ...(record.contentAnims || []),
      record.backdropAnim,
      record.shadowAnim,
    ].filter(Boolean);
    record.boundaryAnim = null;
    record.contentAnims = [];
    record.backdropAnim = null;
    record.shadowAnim = null;
    const pendingResolve = record.pendingTransitionResolve;
    record.pendingTransitionResolve = null;
    if (pendingResolve) pendingResolve();
    if (motionGroup) {
      motionGroup.cleanup();
      return;
    }
    for (const anim of animations) {
      try {
        // Replacement transitions intentionally cancel their predecessor.
        // Detach first so its stale cancel event cannot settle the new state.
        anim.onfinish = null;
        anim.oncancel = null;
        anim.cancel();
      } catch {}
    }
  }

  _collapsedShadowTransform(rect, originRect, axis = "both") {
    const sx = rect.width > 0 ? Math.max(0.001, originRect.width / rect.width) : 1;
    const sy = axis === "x" ? 1 : (rect.height > 0 ? Math.max(0.001, originRect.height / rect.height) : 1);
    const dx = originRect.left - rect.left;
    const dy = axis === "x" ? 0 : originRect.top - rect.top;
    return `translate(${dx}px, ${dy}px) scale(${sx}, ${sy})`;
  }

  _createShadowLayer(record, collapsed = true) {
    const element = record?.element;
    if (!element || record.shadowLayer) return record?.shadowLayer || null;

    const computed = getComputedStyle(element);
    const boxShadow = computed.boxShadow;
    if (!boxShadow || boxShadow === "none") return null;

    const rect = element.getBoundingClientRect();
    const originRect = record.originRect || rect;
    const shadow = document.createElement("div");
    shadow.setAttribute("aria-hidden", "true");
    Object.assign(shadow.style, {
      position: "fixed",
      left: `${rect.left}px`,
      top: `${rect.top}px`,
      width: `${rect.width}px`,
      height: `${rect.height}px`,
      boxSizing: "border-box",
      borderRadius: computed.borderRadius,
      boxShadow,
      background: "transparent",
      border: "0",
      pointerEvents: "none",
      transformOrigin: "top left",
      transform: collapsed ? this._collapsedShadowTransform(rect, originRect, record.options?.axis) : "none",
      opacity: collapsed ? "0" : "1",
      zIndex: computed.zIndex,
      willChange: "transform, opacity",
    });

    const originalInline = {
      value: element.style.getPropertyValue("box-shadow"),
      priority: element.style.getPropertyPriority("box-shadow"),
    };
    element.style.setProperty("box-shadow", "none", "important");
    (element.parentElement || document.body).insertBefore(shadow, element);

    record.surfaceShadowInline = originalInline;
    record.shadowLayer = { element: shadow, boxShadow, rect, originRect };
    return record.shadowLayer;
  }

  _sampleShadow(record) {
    const layer = record?.shadowLayer?.element;
    if (!layer?.isConnected) return null;
    const style = getComputedStyle(layer);
    return { transform: style.transform, opacity: style.opacity };
  }

  _shadowTrack(record, open, sampled = null) {
    let layer = record.shadowLayer;
    if (!layer) layer = this._createShadowLayer(record, open);
    if (!layer) return;

    const current = sampled || getComputedStyle(layer.element);
    const from = {
      transform: current.transform,
      opacity: Number.parseFloat(current.opacity) || 0,
    };
    const toTransform = open ? "none" : this._collapsedShadowTransform(layer.rect, layer.originRect, record.options?.axis);
    return {
      element: layer.element,
      keyframes: [
        { transform: from.transform, opacity: from.opacity },
        { transform: toTransform, opacity: open ? 1 : 0 },
      ],
      options: { easing: open ? EASINGS.expoOut : EASINGS.surfaceExit },
    };
  }

  _releaseShadowLayer(record) {
    const element = record?.element;
    if (!record?.shadowLayer) return;
    record.shadowLayer.element.remove();
    record.shadowLayer = null;
    const previous = record.surfaceShadowInline;
    if (element && previous?.value) {
      element.style.setProperty("box-shadow", previous.value, previous.priority);
    } else if (element) {
      element.style.removeProperty("box-shadow");
    }
    record.surfaceShadowInline = null;
  }

  _sampleSurface(record) {
    const { element, insets } = record;
    const style = getComputedStyle(element);
    const fullClip = "inset(0px 0px 0px 0px round " + (insets?.targetR || "0px") + ")";
    const clipPath = style.clipPath && style.clipPath !== "none" ? style.clipPath : fullClip;
    const parseOpacity = (value, fallback = 1) => {
      const parsed = Number.parseFloat(value);
      return Number.isFinite(parsed) ? parsed : fallback;
    };
    return {
      clipPath,
      opacity: parseOpacity(style.opacity),
      contentOpacities: this._getContentTargets(record).map((target) =>
        parseOpacity(getComputedStyle(target).opacity)
      ),
      backdropOpacity: record.backdrop ? parseOpacity(getComputedStyle(record.backdrop).opacity) : null,
    };
  }

  _getContentTargets(record) {
    if (record.options?.suppressContentAnimation) return [];
    if (record.contentTargets?.length) return record.contentTargets;
    return Array.from(record.element.children || []);
  }

  _refreshInsets(record) {
    const { element, originRect } = record;
    const surfaceRect = element.getBoundingClientRect();
    const topInset = record.options?.axis === "x" ? 0 : (originRect ? Math.max(0, originRect.top - surfaceRect.top) : (record.insets?.topInset || 0));
    const leftInset = originRect ? Math.max(0, originRect.left - surfaceRect.left) : (record.insets?.leftInset || 0);
    const rightInset = originRect ? Math.max(0, surfaceRect.right - (originRect.left + originRect.width)) : (record.insets?.rightInset || 0);
    const bottomInset = record.options?.axis === "x" ? 0 : (originRect ? Math.max(0, surfaceRect.bottom - (originRect.top + originRect.height)) : (record.insets?.bottomInset || 0));
    const originR = record.insets?.originR || "16px";
    const computedRadius = getComputedStyle(element).borderRadius;
    const hasExplicitTargetRadius = record.options?.targetRadius != null;
    const targetR = hasExplicitTargetRadius
      ? (record.insets?.targetR || "16px")
      : ((computedRadius && computedRadius !== "0px") ? computedRadius : (record.insets?.targetR || "16px"));
    record.surfaceRect = surfaceRect;
    record.insets = { topInset, rightInset, bottomInset, leftInset, originR, targetR };
    return record.insets;
  }

  _openDuration(record) {
    const { durationSpec, surfaceRect, originRect } = record;
    if (durationSpec != null) {
      return getDuration(durationSpec);
    }
    return calculateDuration(
      surfaceRect.width,
      surfaceRect.height,
      originRect?.width || 0,
      originRect?.height || 0,
    );
  }

  _startTransition(record, open, sampled = null) {
    const from = sampled || this._sampleSurface(record);
    const shadowFrom = this._sampleShadow(record);
    const insets = this._refreshInsets(record);
    const fullClip = `inset(0px 0px 0px 0px round ${insets.targetR})`;
    const closedClip = `inset(${insets.topInset}px ${insets.rightInset}px ${insets.bottomInset}px ${insets.leftInset}px round ${insets.originR})`;
    const duration = this._openDuration(record);
    const generation = (record.animationGeneration || 0) + 1;
    record.animationGeneration = generation;
    this._cancelRecordAnimations(record);

    if (open) {
      record.state = "opening";
      record.element.classList.add("surface-active");
      record.element.classList.remove("surface-closing");
      record.element.style.pointerEvents = "";
      record.element.style.willChange = "clip-path, opacity";
      if (record.backdrop) record.backdrop.style.pointerEvents = "";
    } else {
      record.state = "closing";
      record.element.classList.add("surface-closing");
      record.element.style.pointerEvents = "none";
      record.element.style.willChange = "clip-path, opacity";
      if (record.backdrop) record.backdrop.style.pointerEvents = "none";
    }

    let transitionDuration = duration;
    if (!open) {
      const startInsets = parseInsetValues(from.clipPath);
      const endInsets = [insets.topInset, insets.rightInset, insets.bottomInset, insets.leftInset];
      const fullDistance = Math.hypot(...endInsets);
      const remainingDistance = startInsets ? Math.hypot(...startInsets.map((value, index) => endInsets[index] - value)) : fullDistance;
      const remainingRatio = fullDistance > 0 ? Math.max(0, Math.min(1, remainingDistance / fullDistance)) : 1;
      transitionDuration = Math.round(duration * 0.65 * Math.sqrt(remainingRatio));
    }

    if (typeof record.options?.onTransitionStart === "function") {
      try { record.options.onTransitionStart({ open, duration: transitionDuration, record }); }
      catch (error) { console.error("Surface transition callback failed:", error); }
    }

    let boundaryKeyframes;
    if (open) {
      boundaryKeyframes = [
        { clipPath: from.clipPath, opacity: from.opacity, offset: 0 },
        { clipPath: fullClip, opacity: 1, offset: 1 },
      ];
    } else {
      const startInsets = parseInsetValues(from.clipPath);
      const endInsets = [insets.topInset, insets.rightInset, insets.bottomInset, insets.leftInset];
      const middleInsets = startInsets
        ? startInsets.map((value, index) => value + (endInsets[index] - value) * 0.85)
        : endInsets.map((value) => value * 0.85);
      const middleClip = `inset(${middleInsets.join("px ")}px round ${insets.originR})`;
      boundaryKeyframes = [
        { clipPath: from.clipPath, opacity: from.opacity, offset: 0 },
        { clipPath: middleClip, opacity: from.opacity, offset: 0.8 },
        { clipPath: closedClip, opacity: record.options?.closeOpacity ?? 0, offset: 1 },
      ];
    }

    const contentTargets = this._getContentTargets(record);
    record.activeContentTargets = contentTargets;
    const contentDuration = open ? transitionDuration : Math.round(transitionDuration * 0.45);
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
    const tracks = [];
    const shadowTrack = this._shadowTrack(record, open, shadowFrom);
    if (shadowTrack) tracks.push(shadowTrack);
    tracks.push({ element: record.element, keyframes: boundaryKeyframes });

    for (const [index, target] of contentTargets.entries()) {
      const currentOpacity = Number.parseFloat(getComputedStyle(target).opacity);
      const opacity = from.contentOpacities?.[index] ?? (Number.isFinite(currentOpacity) ? currentOpacity : 1);
      tracks.push({
        element: target,
        keyframes: [{ opacity }, { opacity: open ? 1 : 0 }],
        options: { duration: contentDuration, easing: open ? EASINGS.content : EASINGS.smooth },
      });
    }

    if (record.backdrop) {
      const currentOpacity = Number.parseFloat(getComputedStyle(record.backdrop).opacity);
      const opacity = from.backdropOpacity ?? (Number.isFinite(currentOpacity) ? currentOpacity : (open ? 0 : 1));
      if (open) record.backdrop.style.display = record.backdropDisplay || "block";
      tracks.push({
        element: record.backdrop,
        keyframes: [{ opacity }, { opacity: open ? 1 : 0 }],
        options: { easing: EASINGS.standard },
      });
    }

    for (const track of tracks) {
      timeline.add(track.element, track.keyframes, track.options || {});
    }
    record.motionGroup = timeline;
    timeline.play();
    record.boundaryAnim = timeline.getAnimation(record.element);
    record.shadowAnim = record.shadowLayer ? timeline.getAnimation(record.shadowLayer.element) : null;
    record.contentAnims = contentTargets.map((target) => timeline.getAnimation(target)).filter(Boolean);
    record.backdropAnim = record.backdrop ? timeline.getAnimation(record.backdrop) : null;
  }

  _initGlobalListeners() {
    if (this._initialized || typeof window === "undefined") return;
    this._initialized = true;

    // Capture-phase Escape listener: strictly closes the topmost surface (unicoremodmotion.md §2.4)
    window.addEventListener(
      "keydown",
      (e) => {
        if (e.key === "Escape" || e.key === "Esc") {
          this._cleanupDeadNodes();
          if (this.activeStack.length === 0) return;

          // If user is inside an active inline text editor (e.g. explorer file rename)
          // or music sync URL input, allow the input's own Escape listener to cancel editing first.
          const activeEl = document.activeElement;
          if (activeEl && (activeEl.classList?.contains("explorer-inline-rename") || activeEl.classList?.contains("music-sync-url") || activeEl.hasAttribute("data-prevent-escape-surface-close"))) {
            return;
          }

          // If user is inside the explorer search input and it has a query,
          // clear search query and blur without immediately closing the entire explorer window.
          if (activeEl && activeEl.id === "searchInput" && activeEl.value) {
            activeEl.value = "";
            const searchRes = document.getElementById("searchResults");
            if (searchRes) searchRes.classList.add("hidden");
            activeEl.blur();
            e.preventDefault();
            e.stopPropagation();
            e.stopImmediatePropagation();
            return;
          }

          e.preventDefault();
          e.stopPropagation();
          e.stopImmediatePropagation();
          document.activeElement?.blur?.();

          const top = this.activeStack[this.activeStack.length - 1];
          if (!top || top.options.closeOnEscape === false) return;

          // If user pressed Escape again while topmost was already closing,
          // fast-finalize it immediately and dismiss the next surface in the LIFO stack.
          if (top.state === "closing") {
            this._finalizeClose(top);
            this._cleanupDeadNodes();
            if (this.activeStack.length > 0) {
              const nextTop = this.activeStack[this.activeStack.length - 1];
              if (nextTop && nextTop.options.closeOnEscape !== false) {
                this.closeSurface(nextTop.element);
              }
            }
          } else {
            this.closeSurface(top.element);
          }
        }
      },
      { capture: true }
    );

    // Capture-phase pointerdown listener: dismiss top surface on outside click
    document.addEventListener("pointerdown", (e) => {
      this._cleanupDeadNodes();
      if (this.activeStack.length === 0) return;
      let top = this.activeStack[this.activeStack.length - 1];
      if (!top || top.options.closeOnOutsideClick === false) return;

      // Inside surface element -> do not close
      if (top.element && top.element.contains(e.target)) return;

      // Inside origin trigger element -> do not close here (let button toggle handler run)
      if (top.options.origin instanceof HTMLElement && top.options.origin.contains(e.target)) {
        return;
      }

      // Inside explicitly ignored elements
      if (Array.isArray(top.options.ignoreOutsideElements)) {
        for (const el of top.options.ignoreOutsideElements) {
          if (el && el.contains(e.target)) return;
        }
      }

      // Fast-finalize closing surface if user clicks outside again
      if (top.state === "closing") {
        this._finalizeClose(top);
        this._cleanupDeadNodes();
        if (this.activeStack.length === 0) return;
        top = this.activeStack[this.activeStack.length - 1];
        if (!top || top.options.closeOnOutsideClick === false) return;
        if (top.element && top.element.contains(e.target)) return;
      }

      // Outside click on top surface
      this.closeSurface(top.element);
    }, true);
  }

  /**
   * Returns the canonical portal container for floating surfaces.
   * @returns {HTMLElement}
   */
  getPortal() {
    return document.getElementById("shell-portals") || document.body;
  }

  /**
   * Find entry in activeStack for the given element.
   * @param {HTMLElement} element
   * @returns {SurfaceRecord | null}
   */
  getRecord(element) {
    if (!element) return null;
    this._cleanupDeadNodes();
    return this.activeStack.find((r) => r.element === element) || null;
  }

  /**
   * Check if an element is currently open or opening.
   * @param {HTMLElement} element
   * @returns {boolean}
   */
  isOpen(element) {
    const rec = this.getRecord(element);
    return !!(rec && (rec.state === "open" || rec.state === "opening"));
  }

  /**
   * Open a surface element using GPU clip-path morphing from trigger hitbox.
   *
   * @param {HTMLElement} element - The surface container element
   * @param {Object} options
   * @param {HTMLElement|DOMRect|{left:number,top:number,width:number,height:number}|MouseEvent} [options.origin]
   * @param {string|number} [options.originRadius] - Border radius of hitbox (auto-resolved from origin if omitted)
   * @param {string|number} [options.targetRadius] - Border radius of open surface (defaults to computed CSS radius)
   * @param {string|HTMLElement|Array<HTMLElement>} [options.content] - Selector or elements to delay-fade
   * @param {HTMLElement} [options.backdrop] - Background overlay element to fade in
   * @param {boolean} [options.closeOnOutsideClick=true]
   * @param {boolean} [options.closeOnEscape=true]
   * @param {string|number} [options.elevation] - e.g. "var(--z-surface-modal)"
   * @param {string} [options.display] - Display mode to apply when shown
   * @param {"both"|"x"} [options.axis="both"] - Constrain the reveal to the horizontal axis when the dock and surface share a vertical lane
   * @param {number} [options.openOpacity=0] - Initial surface opacity; use 1 for an opaque clipped reveal
   * @param {number} [options.closeOpacity=0] - Keep the dock visible through the final frame when set to 1
   * @param {Function} [options.onOpen] - Called when open starts
   * @param {Function} [options.onTransitionStart] - Called with `{ open, duration, record }` for each transition
   * @param {Function} [options.onOpened] - Called when open animation completes
   * @param {Function} [options.onClose] - Called when close starts
   * @param {Function} [options.onClosed] - Called when close completes
   * @returns {Promise<void>}
   */
  async openSurface(element, options = {}) {
    if (!element) return;
    this._cleanupDeadNodes();

    // If already in stack:
    const existing = this.getRecord(element);
    if (existing) {
      if (existing.state === "closing") {
        // Promote to top of activeStack to preserve strictly ordered LIFO dismissal.
        const idx = this.activeStack.indexOf(existing);
        if (idx !== -1 && idx !== this.activeStack.length - 1) {
          this.activeStack.push(this.activeStack.splice(idx, 1)[0]);
        }
        // Retarget from the currently rendered clip to the full surface. A
        // close may itself have started partway through an opening, so reversing
        // that animation would only return to the intermediate shape.
        this._startTransition(existing, true);
        return;
      }
      if (existing.state === "open" || existing.state === "opening") {
        return; // already open/opening
      }
    }

    const opts = {
      closeOnOutsideClick: true,
      closeOnEscape: true,
      ...options,
    };

    // Ensure surface is ready in layout to measure bounding box
    const prevVisibility = element.style.visibility;
    const prevDisplay = element.style.display;
    const prevOpacity = {
      value: element.style.getPropertyValue("opacity"),
      priority: element.style.getPropertyPriority("opacity"),
    };
    const computedDisplay = getComputedStyle(element).display;
    if (computedDisplay === "none" || !element.style.display) {
      element.style.display = opts.display || (computedDisplay === "none" ? "block" : computedDisplay);
    }
    element.style.visibility = "hidden";
    element.classList.add("surface-active");

    const surfaceRect = element.getBoundingClientRect();
    const surfaceRadius = getComputedStyle(element).borderRadius;

    // Resolve origin hitbox coordinates and radius
    let originRect = null;
    let originR = opts.originRadius;

    if (opts.origin instanceof HTMLElement) {
      originRect = opts.origin.getBoundingClientRect();
      if (!originR) {
        originR = getComputedStyle(opts.origin).borderRadius || "16px";
      }
    } else if (opts.origin && typeof opts.origin.left === "number" && typeof opts.origin.top === "number") {
      originRect = {
        left: opts.origin.left,
        top: opts.origin.top,
        right: opts.origin.left + (opts.origin.width || 0),
        bottom: opts.origin.top + (opts.origin.height || 0),
        width: opts.origin.width || 0,
        height: opts.origin.height || 0,
      };
    } else if (opts.origin && typeof opts.origin.clientX === "number") {
      originRect = {
        left: opts.origin.clientX - 6,
        top: opts.origin.clientY - 6,
        right: opts.origin.clientX + 6,
        bottom: opts.origin.clientY + 6,
        width: 12,
        height: 12,
      };
    } else {
      // Default to center of surface
      const cx = surfaceRect.left + surfaceRect.width / 2;
      const cy = surfaceRect.top + surfaceRect.height / 2;
      originRect = {
        left: cx - 16,
        top: cy - 16,
        right: cx + 16,
        bottom: cy + 16,
        width: 32,
        height: 32,
      };
    }

    if (!originR) {
      const originElement = opts.origin instanceof HTMLElement ? opts.origin : element;
      const computedOriginRadius = getComputedStyle(originElement).borderRadius;
      originR = computedOriginRadius && computedOriginRadius !== "0px" ? computedOriginRadius : "24px";
    }
    let targetR = opts.targetRadius;
    if (!targetR) {
      const themeR = getComputedStyle(document.documentElement).getPropertyValue("--theme-radius-surface").trim();
      targetR = (surfaceRadius && surfaceRadius !== "0px") ? surfaceRadius : (themeR || "24px");
    } else if (typeof targetR === "number") {
      targetR = `${targetR}px`;
    }

    // Compute relative insets from surface boundaries into origin hitbox
    const topInset = opts.axis === "x" ? 0 : Math.max(0, Math.round(originRect.top - surfaceRect.top));
    const leftInset = Math.max(0, Math.round(originRect.left - surfaceRect.left));
    const rightInset = Math.max(0, Math.round(surfaceRect.right - (originRect.left + originRect.width)));
    const bottomInset = opts.axis === "x" ? 0 : Math.max(0, Math.round(surfaceRect.bottom - (originRect.top + originRect.height)));

    // Dynamic duration calculation via logarithmic physical formula (unicoremodmotion.md §2.1)
    // Respects explicit opts.duration token or millisecond override if specified
    const duration = opts.duration != null
      ? getDuration(opts.duration)
      : calculateDuration(
          surfaceRect.width,
          surfaceRect.height,
          originRect.width,
          originRect.height
        );

    let backdropDisplay = opts.backdropDisplay || null;
    if (opts.backdrop) {
      if (opts.backdrop.style.display === "none") {
        opts.backdrop.style.display = "";
      }
      opts.backdrop.classList.add("surface-active");
      if (!backdropDisplay) {
        const computedBackdrop = getComputedStyle(opts.backdrop).display;
        backdropDisplay = (computedBackdrop && computedBackdrop !== "none") ? computedBackdrop : "block";
      }
      opts.backdrop.style.display = backdropDisplay;
    }

    const record = {
      id: opts.id || `surface-${Date.now()}-${Math.random().toString(36).slice(2, 6)}`,
      element,
      backdrop: opts.backdrop || null,
      backdropDisplay,
      options: opts,
      state: "closed",
      animationGeneration: 0,
      motionGroup: null,
      boundaryAnim: null,
      contentAnims: [],
      contentTargets: [],
      activeContentTargets: [],
      backdropAnim: null,
      originRect,
      surfaceRect,
      prevDisplay,
      prevOpacity,
      insets: { topInset, rightInset, bottomInset, leftInset, originR, targetR },
      duration,
      durationSpec: opts.duration,
    };

    this.activeStack.push(record);
    if (typeof opts.onOpen === "function") opts.onOpen();

    // Elevation
    if (opts.elevation) {
      element.style.zIndex = String(opts.elevation);
    }

    // Keep the outside shadow on a sibling layer. The root's animated
    // clip-path masks its box-shadow until the final frame.
    this._createShadowLayer(record, true);
    let contentTargets = [];
    if (typeof opts.content === "string") {
      contentTargets = Array.from(element.querySelectorAll(opts.content));
    } else if (opts.content instanceof HTMLElement) {
      contentTargets = [opts.content];
    } else if (opts.content && (Array.isArray(opts.content) || typeof opts.content[Symbol.iterator] === "function" || (typeof NodeList !== "undefined" && opts.content instanceof NodeList))) {
      contentTargets = Array.from(opts.content).filter(Boolean);
    } else if (opts.contentSelector) {
      contentTargets = Array.from(element.querySelectorAll(opts.contentSelector));
    }

    // Never include root surface element itself in content targets:
    // element is already animated via boundaryAnim (clipPath + opacity). Duplicate animation causes conflicts/glitches.
    contentTargets = contentTargets.filter((t) => t && t !== element);
    record.contentTargets = contentTargets;

    // Optional backdrop animation
    if (opts.backdrop) {
      opts.backdrop.style.display = record.backdropDisplay || "block";
      opts.backdrop.classList.add("surface-active");
    }

    // Both first-open and later reversals now use the same grouped runner.
    // Its reduced-motion path applies the final frame synchronously, then
    // settles in a microtask so record handles are assigned before callbacks.
    const initialClip = `inset(${topInset}px ${rightInset}px ${bottomInset}px ${leftInset}px round ${originR})`;
    this._startTransition(record, true, {
      clipPath: initialClip,
      opacity: opts.openOpacity ?? 0,
      contentOpacities: contentTargets.map(() => 0),
      backdropOpacity: 0,
    });
    // Keep the newly displayed element hidden through measurement, callbacks,
    // and WAAPI setup. The first visible paint must already have its clip.
    element.style.visibility = prevVisibility;

    return new Promise((resolve) => {
      record.pendingTransitionResolve = resolve;
      record.motionGroup?.finished.then(() => {
        if (record.pendingTransitionResolve === resolve) {
          record.pendingTransitionResolve = null;
          resolve();
        }
      });
    });
  }

  /**
   * Close a surface element. If called while opening, reverses seamlessly.
   *
   * @param {HTMLElement} element
   * @param {Object} [closeOptions]
   * @returns {Promise<void>}
   */
  async closeSurface(element, closeOptions = {}) {
    if (!element) return;
    this._cleanupDeadNodes();
    const record = this.getRecord(element);
    if (!record) return;

    if (record.state === "closing" || record.state === "closed") {
      return;
    }

    // Read the rendered values before changing state or cancelling WAAPI. The
    // animation timeline's currentTime is not visual progress when easing is
    // nonlinear, so the close must continue from computed styles instead.
    const sampled = this._sampleSurface(record);
    record.state = "closing";
    if (typeof record.options.onClose === "function") {
      record.options.onClose();
    }

    // Immediately drop active keyboard focus from within closing surface
    if (element.contains(document.activeElement)) {
      document.activeElement?.blur?.();
    }

    // Immediately stop receiving pointer events while closing
    record.element.classList.add("surface-closing");
    record.element.style.pointerEvents = "none";
    record.element.style.willChange = "clip-path, opacity";
    if (record.backdrop) record.backdrop.style.pointerEvents = "none";

    this._startTransition(record, false, sampled);
  }

  _finalizeOpen(record) {
    if (record.state === "open") return;
    record.state = "open";
    record.animationGeneration = (record.animationGeneration || 0) + 1;
    const animatedContentTargets = record.activeContentTargets || [];

    record.element.style.opacity = "1";
    this._cancelRecordAnimations(record);
    record.element.style.clipPath = "";
    record.element.style.willChange = "";
    record.element.classList.add("surface-open");
    record.element.classList.remove("surface-closing");

    for (const target of animatedContentTargets) target.style.opacity = "1";
    record.activeContentTargets = [];
    if (record.backdrop) record.backdrop.style.opacity = "1";

    try {
      record.options?.onOpened?.();
    } catch (error) {
      console.error("Surface open callback failed:", error);
    } finally {
      this._releaseShadowLayer(record);
    }
  }

  _finalizeClose(record) {
    if (record.state === "closed") return;
    record.state = "closed";
    record.animationGeneration = (record.animationGeneration || 0) + 1;

    const idx = this.activeStack.indexOf(record);
    if (idx !== -1) {
      this.activeStack.splice(idx, 1);
    }

    const { element, backdrop, options } = record;
    // Cancellation removes WAAPI's filled last frame. Hold the surface hidden
    // until its owner has restored dock geometry/content in onClosed.
    const previousOpacity = record.prevOpacity || { value: "", priority: "" };
    element.style.setProperty("opacity", "0", "important");
    this._cancelRecordAnimations(record);
    record.activeContentTargets = [];

    element.classList.remove("surface-active", "surface-open", "surface-closing");
    element.style.clipPath = "";
    element.style.pointerEvents = "";
    element.style.willChange = "";
    if (backdrop) backdrop.style.pointerEvents = "";
    if (options.elevation) {
      element.style.zIndex = "";
    }

    // Restore display state cleanly
    if (options.display) {
      element.style.display = record.prevDisplay || "";
    } else if (record.prevDisplay && record.prevDisplay !== "none") {
      element.style.display = record.prevDisplay;
    } else if (record.prevDisplay === "none") {
      element.style.display = "none";
    } else {
      element.style.display = "";
    }

    if (backdrop) {
      backdrop.classList.remove("surface-active", "surface-open", "expanded");
      backdrop.style.opacity = "";
      backdrop.style.display = "none";
    }

    try {
      if (typeof options.onClosed === "function") options.onClosed();
    } catch (error) {
      console.error("Surface close callback failed:", error);
    } finally {
      if (previousOpacity.value) {
        element.style.setProperty("opacity", previousOpacity.value, previousOpacity.priority);
      } else {
        element.style.removeProperty("opacity");
      }
      this._releaseShadowLayer(record);
    }
  }

  /**
   * Close all active surfaces in reverse LIFO order.
   */
  closeAll() {
    this._cleanupDeadNodes();
    const copy = [...this.activeStack];
    for (let i = copy.length - 1; i >= 0; i--) {
      this.closeSurface(copy[i].element);
    }
  }
}

/**
 * Internal View Transition as an Overlaid Surface (user specification).
 * Morphs an incoming view as a surface overlay from originElement's hitbox
 * over the viewport container, then cleanly commits, frees compositor layers,
 * and seamlessly takes over without flashing or text blurring.
 *
 * @param {HTMLElement} viewport - The parent view container (e.g. .tg-thread or .file-area)
 * @param {HTMLElement|DOMRect} origin - The clicked element or its client rect
 * @param {Function} renderFn - Async function (stagingEl) => void to build new view
 * @param {Object} [options]
 * @returns {Promise<void>}
 */
export async function morphLayerReveal(viewport, origin, renderFn, options = {}) {
  if (!viewport || typeof renderFn !== "function") {
    if (typeof renderFn === "function") await renderFn();
    return;
  }

  const originRect = origin instanceof HTMLElement ? origin.getBoundingClientRect() : origin;
  const viewportRect = viewport.getBoundingClientRect();

  if (!originRect || viewportRect.width === 0 || viewportRect.height === 0) {
    await renderFn(viewport);
    return;
  }

  // Calculate relative insets of the origin hitbox inside the viewport
  const topInset = Math.max(0, Math.round(originRect.top - viewportRect.top));
  const leftInset = Math.max(0, Math.round(originRect.left - viewportRect.left));
  const rightInset = Math.max(0, Math.round(viewportRect.right - (originRect.left + (originRect.width || 0))));
  const bottomInset = Math.max(0, Math.round(viewportRect.bottom - (originRect.top + (originRect.height || 0))));
  const originR = options.originRadius || "14px";

  // Staging layer placed absolutely over the current view
  const staging = document.createElement("div");
  staging.className = "surface-morph-staging";
  staging.style.position = "absolute";
  staging.style.inset = "0";
  staging.style.zIndex = "10";
  staging.style.width = "100%";
  staging.style.height = "100%";
  staging.style.overflow = "hidden";
  staging.style.pointerEvents = "none";
  staging.style.clipPath = `inset(${topInset}px ${rightInset}px ${bottomInset}px ${leftInset}px round ${originR})`;

  // Ensure viewport has positioning context
  const prevPosition = viewport.style.position;
  if (getComputedStyle(viewport).position === "static") {
    viewport.style.position = "relative";
  }

  // Lock interaction on existing viewport during morph
  viewport.style.pointerEvents = "none";

  // Render incoming content into staging slot
  await renderFn(staging);
  viewport.appendChild(staging);

  const duration = calculateDuration(
    viewportRect.width,
    viewportRect.height,
    originRect.width || 40,
    originRect.height || 40
  );

  let finalized = false;
  const finalize = () => {
    if (finalized) return;
    finalized = true;
    timeline.cleanup();
    // Promote staging children into viewport and remove staging
    while (viewport.firstChild && viewport.firstChild !== staging) {
      viewport.removeChild(viewport.firstChild);
    }
    while (staging.firstChild) {
      viewport.insertBefore(staging.firstChild, staging);
    }
    staging.remove();
    viewport.style.position = prevPosition;
    viewport.style.pointerEvents = "";
  };
  const timeline = motionTimeline({
    duration,
    durationIsScaled: true,
    easing: EASINGS.expoOut,
    onFinish: finalize,
    onCancel: finalize,
  });
  timeline.add(staging, [
    { clipPath: `inset(${topInset}px ${rightInset}px ${bottomInset}px ${leftInset}px round ${originR})`, opacity: 0.92 },
    { clipPath: "inset(0px 0px 0px 0px round 0px)", opacity: 1 }
  ]).play();

  await timeline.finished;
}

/**
 * Drives a card's colour wash (jelly ::after layer) in step with an open/close morph.
 * Call from onTransitionStart: the fade gets the morph's own duration and the same
 * easing the clip-path timeline uses (expoOut on open, surfaceExit on close).
 */
export function syncSurfaceWash(element, open, duration) {
  element.style.setProperty("--wash-dur", `${duration}ms`);
  element.style.setProperty("--wash-ease", open ? "var(--ease-expo-out)" : "var(--ease-surface-exit)");
  element.classList.toggle("surface-washed", open);
}

export const surfaceManager = new SurfaceManager();
if (typeof window !== "undefined") {
  window.surfaceManager = surfaceManager;
  window.morphLayerReveal = morphLayerReveal;
}
export default surfaceManager;
