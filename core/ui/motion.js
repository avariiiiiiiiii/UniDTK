/**
 * Shell Motion Core — Unified Animation System (architecture.md §7)
 *
 * Canonical project easings and duration tokens. Every widget and core UI
 * component animates using these tokens and helper functions.
 */

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
  // Distinctive ease-outQuint entry curve for account/list item pop-ins.
  itemIn: "cubic-bezier(0.16, 1, 0.3, 1)",
});

export const DURATIONS = Object.freeze({
  hover: 120,
  transient: 220,
  motion: 300,
  layout: 340,
});

// Motion profiles — an independent axis that reshapes *which* animation
// classes move faster/slower, orthogonal to the single global speed scale.
// Per-token duration multipliers are applied inside getDuration (and to the
// window-morph formula), so a profile is a real axis, not a label.
export const MOTION_PROFILES = Object.freeze({
  balanced: { hover: 1, transient: 1, motion: 1, layout: 1 },
  responsive: { hover: 0.85, transient: 0.9, motion: 0.8, layout: 0.75 },
  fluid: { hover: 1, transient: 1.1, motion: 1.2, layout: 1.3 },
});

let currentProfile = "balanced";
let profileFactors = MOTION_PROFILES.balanced;

let currentScale = 1.0;
/// Last scale the user chose. `currentScale` mirrors it, except while the OS
/// asks for reduced motion — toggling the OS setting must not overwrite the
/// user's own preference (the old code stored the reduced-motion 0.01 INTO
/// currentScale, so leaving reduced-motion left animations stuck at ~0).
let userScale = 1.0;
let isReducedMotion = false;

/**
 * Get current global motion scale multiplier.
 */
export function getMotionScale() {
  return currentScale;
}

/**
 * Dynamic duration calculation via logarithmic physical formula (unicoremodmotion.md §2.1).
 * T(Δd) = [Tmin + (Tmax - Tmin) * ln(1 + Δd / d0) / ln(1 + dmax / d0)] * currentScale
 * Where Tmin = 240ms, Tmax = 440ms, d0 = 100px, dmax = 1600px.
 */
export function calculateDuration(targetW = 0, targetH = 0, originW = 0, originH = 0) {
  if (isReducedMotion) return 1;
  if (currentScale <= 0) return 0;
  let oW = 0;
  let oH = 0;
  if (typeof originW === "object" && originW !== null) {
    oW = Number(originW.width) || 0;
    oH = Number(originW.height) || 0;
  } else {
    oW = Number(originW) || 0;
    oH = Number(originH) || 0;
  }
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

/**
 * Check if the user prefers reduced motion (OS accessibility setting).
 */
export function prefersReducedMotion() {
  return isReducedMotion;
}

/**
 * Get scaled duration in ms, respecting user settings and accessibility.
 */
export function getDuration(token = "transient") {
  if (isReducedMotion) return 1;
  const base = typeof token === "number" ? token : (DURATIONS[token] || DURATIONS.transient);
  const factor = typeof token === "string" ? (profileFactors[token] ?? 1) : 1;
  return Math.max(1, Math.round(base * factor * currentScale));
}

/**
 * Set global motion duration scale factor (e.g. 0.5 for fast, 1.0 for standard, 0 for off).
 * The value is remembered as the user's preference; reduced-motion overrides
 * the effective scale without destroying it.
 */
export function setMotionScale(scale = 1.0) {
  const requested = Number(scale);
  userScale = Number.isFinite(requested) ? Math.max(0, Math.min(3.0, requested)) : 1.0;
  applyEffectiveMotionScale();
}

export function getMotionProfile() {
  return currentProfile;
}

/**
 * Set the motion profile (balanced / responsive / fluid). An independent axis
 * from the global speed scale: it reshapes per-token durations via their
 * profile multipliers, leaving the single `currentScale` (and `--dur-scale`)
 * untouched. `bouncy` is kept as a legacy alias for `responsive`.
 */
export function setMotionProfile(profile = "balanced") {
  const resolved = profile === "bouncy" || profile === "snappy" ? "responsive" : profile;
  currentProfile = MOTION_PROFILES[resolved] ? resolved : "balanced";
  profileFactors = MOTION_PROFILES[currentProfile];
  applyEffectiveMotionScale();
}

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

// One owner per WAAPI effect group. This keeps a finished `fill: both` effect
// from silently winning over a later transition on the same element.
const activeMotionByElement = new WeakMap();

function toCssProperty(property) {
  if (property.startsWith("--")) return property;
  return property.replace(/[A-Z]/g, (letter) => `-${letter.toLowerCase()}`);
}

function applyFinalFrame(element, frames) {
  const frame = Array.isArray(frames) ? frames[frames.length - 1] : frames;
  if (!frame || typeof frame !== "object" || !element?.style) return;
  for (const [property, rawValue] of Object.entries(frame)) {
    if (["offset", "easing", "composite", "computedOffset"].includes(property) || rawValue == null) continue;
    const value = Array.isArray(rawValue) ? rawValue[rawValue.length - 1] : rawValue;
    try { element.style.setProperty(toCssProperty(property), String(value)); } catch {}
  }
}

/**
 * A small, WebView-safe WAAPI timeline. Tracks start together, may use an
 * offset (`at`/`delay`) and share lifecycle controls. SurfaceManager remains
 * responsible for semantic state; this class owns animation effects only.
 */
export class MotionTimeline {
  constructor(options = {}) {
    this.options = { fill: "both", easing: "standard", ...options };
    this.tracks = [];
    this.animations = [];
    this.state = "idle";
    this._listeners = [];
    this._resolveFinished = null;
    this.finished = new Promise((resolve) => { this._resolveFinished = resolve; });
  }

  add(element, keyframes, options = {}) {
    if (this.state !== "idle") throw new Error("MotionTimeline tracks must be added before play().");
    if (element && typeof element.animate === "function") {
      this.tracks.push({ element, keyframes, options });
    }
    return this;
  }

  getAnimation(element) {
    return this.animations.find((entry) => entry.element === element)?.animation || null;
  }

  play() {
    if (this.state !== "idle") return this;
    this.state = "running";
    const tracks = this.tracks;
    if (!tracks.length) {
      queueMicrotask(() => this._settle("finished"));
      return this;
    }

    // Cancel the previous owner as a whole, so sibling properties from a
    // superseded group cannot remain stuck in their filled state.
    if (this.options.replace !== false) {
      const previous = new Set();
      for (const { element } of tracks) {
        for (const owner of activeMotionByElement.get(element) || []) {
          if (owner !== this) previous.add(owner);
        }
      }
      for (const owner of previous) owner.cancel();
    }
    for (const { element } of tracks) {
      let owners = activeMotionByElement.get(element);
      if (!owners) activeMotionByElement.set(element, owners = new Set());
      owners.add(this);
    }

    // Reduced motion and a zero user scale have a deterministic synchronous
    // end state; no callback depends on a browser finish event at duration 0.
    if (isReducedMotion || currentScale <= 0) {
      for (const { element, keyframes } of tracks) applyFinalFrame(element, keyframes);
      queueMicrotask(() => this._settle("finished"));
      return this;
    }

    let settledTracks = 0;
    const onFinish = () => {
      settledTracks += 1;
      if (settledTracks === tracks.length) this._settle("finished");
    };
    const onCancel = () => {
      if (this.state === "running") this.cancel();
    };

    for (const track of tracks) {
      const settings = { ...this.options, ...track.options };
      const durationValue = settings.duration ?? "transient";
      const duration = settings.durationIsScaled
        ? Math.max(0, Number(durationValue) || 0)
        : getDuration(durationValue);
      const delay = Math.max(0, Number(settings.delay ?? settings.at) || 0) * currentScale;
      const easeValue = settings.easing ?? "standard";
      const easing = typeof easeValue === "string" && EASINGS[easeValue]
        ? EASINGS[easeValue]
        : (easeValue || EASINGS.standard);
      const animation = track.element.animate(track.keyframes, {
        duration,
        delay,
        easing,
        fill: settings.fill || "both",
      });
      const finishListener = () => onFinish();
      const cancelListener = () => onCancel();
      animation.addEventListener("finish", finishListener, { once: true });
      animation.addEventListener("cancel", cancelListener, { once: true });
      this._listeners.push({ animation, finishListener, cancelListener });
      this.animations.push({ element: track.element, animation });
    }
    return this;
  }

  reverse() {
    if (this.state !== "running") return this;
    for (const { animation } of this.animations) {
      try { animation.reverse(); } catch {}
    }
    return this;
  }

  finish() {
    if (this.state === "idle") this.play();
    if (this.state !== "running") return this;
    for (const { animation } of this.animations) {
      try { animation.finish(); } catch {}
    }
    if (!this.animations.length) this._settle("finished");
    return this;
  }

  cancel({ preserveCurrent = false } = {}) {
    if (this.state === "cancelled" || this.state === "disposed") return this;
    if (preserveCurrent) {
      for (const { animation } of this.animations) {
        try { animation.commitStyles?.(); } catch {}
      }
    }
    this._detachAndCancel();
    this._settle("cancelled");
    return this;
  }

  cleanup({ commitStyles = false } = {}) {
    if (commitStyles) {
      for (const { animation } of this.animations) {
        try { animation.commitStyles?.(); } catch {}
      }
    }
    const wasFinished = this.state === "finished";
    this._detachAndCancel();
    if (wasFinished) this.state = "disposed";
    else this._settle("cancelled");
    return this;
  }

  _detachAndCancel() {
    for (const { animation, finishListener, cancelListener } of this._listeners) {
      animation.removeEventListener("finish", finishListener);
      animation.removeEventListener("cancel", cancelListener);
      try { animation.cancel(); } catch {}
    }
    this._listeners = [];
    for (const { element } of this.tracks) {
      const owners = activeMotionByElement.get(element);
      owners?.delete(this);
      if (owners?.size === 0) activeMotionByElement.delete(element);
    }
  }

  _settle(status) {
    if (this.state === "finished" || this.state === "cancelled" || this.state === "disposed") return;
    this.state = status;
    try {
      if (status === "finished") this.options.onFinish?.(this);
      else this.options.onCancel?.(this);
    } catch (error) {
      console.error("Motion timeline callback failed:", error);
    }
    this._resolveFinished?.({ status, timeline: this });
    this._resolveFinished = null;
  }
}

export function motionTimeline(options = {}) {
  return new MotionTimeline(options);
}

/**
 * Standard element animation using Web Animations API and canonical easings.
 */
export function animate(element, keyframes, options = {}) {
  if (!element || !element.animate) return Promise.resolve(null);
  const timeline = motionTimeline().add(element, keyframes, options).play();
  return timeline.finished.then(({ status }) => {
    const animation = timeline.getAnimation(element);
    if (status === "finished") timeline.cleanup({ commitStyles: true });
    return animation;
  });
}

/**
 * Spring bounce animation for popups, toggles, and widget card expands.
 */
export function spring(element, keyframes, options = {}) {
  return animate(element, keyframes, {
    duration: options.duration ?? "motion",
    easing: "spring",
    fill: options.fill || "both",
    delay: options.delay || 0,
  });
}

/**
 * Staggered fade/slide enter for lists of elements (chats, search results, tracks).
 */
export function staggerEnter(elements, options = {}) {
  const list = Array.from(elements || []).filter(Boolean);
  if (!list.length) return Promise.resolve();
  const interval = options.staggerMs ?? 25;
  const fromY = options.fromY != null ? options.fromY : 8;

  const promises = list.map((el, i) => {
    return animate(
      el,
      [
        { opacity: 0, transform: `translateY(${fromY}px)` },
        { opacity: 1, transform: "translateY(0)" },
      ],
      {
        duration: options.duration ?? "transient",
        easing: "expo",
        delay: i * interval,
      }
    );
  });

  return Promise.all(promises);
}

/**
 * Fade transition between two views/elements.
 */
export async function fadeTransition(fromEl, toEl, options = {}) {
  const baseDuration = typeof options.duration === "number"
    ? options.duration
    : (DURATIONS[options.duration] || DURATIONS.transient);
  const halfDuration = baseDuration / 2;
  if (fromEl) {
    await animate(fromEl, [{ opacity: 1 }, { opacity: 0 }], {
      duration: halfDuration,
      easing: "standard",
    });
    fromEl.style.display = "none";
  }
  if (toEl) {
    toEl.style.display = options.toDisplay || "block";
    await animate(toEl, [{ opacity: 0 }, { opacity: 1 }], {
      duration: halfDuration,
      easing: "standard",
    });
  }
}

/**
 * Initialize motion system: listen to OS accessibility changes and bind CSS tokens.
 */
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
