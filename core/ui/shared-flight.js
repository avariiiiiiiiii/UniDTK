/**
 * UniDTK — shared-element flight (r[motion.shared-flight], docs/reference/surfaces.md)
 *
 * A surface that grows out of an object (a desk tile, a clock, a button) keeps the
 * object's identity: the named parts that exist on both sides of the morph (an
 * icon, the clock face) travel from where they were to where they end up, while
 * the rest of the surface is revealed around them by the clip-path.
 *
 * The part is not moved in the page. A stand-in copy of it, frozen to the look it
 * had before the surface was laid out, lives in a layer inside the surface, and a
 * transform animation on the copy carries it. The real nodes stay where layout
 * puts them, hidden for the length of the flight, and are shown again in the same
 * task that removes the copy.
 *
 * Callers do not animate anything: they name the parts when the origin is captured
 * (`surfaceManager.captureOrigin`) and the surface manager runs the flight inside
 * the transition's own timeline, so it shares its duration, easing, reversal and
 * reduced-motion path.
 */

import { getDuration } from "./motion.js";

export const FLIGHT_HIDDEN_ATTR = "data-flight-hidden";

export function rectSnapshot(rect) {
  return { left: rect.left, top: rect.top, width: rect.width, height: rect.height };
}

// SVG ink can change its tree (a gooey digit gains or loses a circle), so layout
// boxes are frozen, never path data or styles indexed through that ink.
function layoutNodes(root) {
  return [root, ...root.querySelectorAll("*")].filter((node) =>
    !(node instanceof SVGElement) || node.localName === "svg");
}

// What a part needs to keep its look and its box in a different place in the page.
// Reading every computed property costs about 2 ms per node; a glyph or a badge needs
// only these, and the rest follows from its own classes and inherited variables.
const VISUAL_PROPERTIES = [
  "display", "box-sizing", "width", "height", "min-width", "min-height", "max-width", "max-height",
  "margin-top", "margin-right", "margin-bottom", "margin-left",
  "padding-top", "padding-right", "padding-bottom", "padding-left",
  "border-top-width", "border-right-width", "border-bottom-width", "border-left-width",
  "border-top-style", "border-right-style", "border-bottom-style", "border-left-style",
  "border-top-color", "border-right-color", "border-bottom-color", "border-left-color",
  "border-top-left-radius", "border-top-right-radius", "border-bottom-right-radius", "border-bottom-left-radius",
  "background-color", "background-image", "background-size", "background-position", "background-repeat",
  "box-shadow", "color", "fill", "stroke", "stroke-width", "filter",
  "font-family", "font-size", "font-weight", "font-style", "line-height", "letter-spacing",
  "text-align", "text-transform", "white-space", "overflow-x", "overflow-y",
  "flex-grow", "flex-shrink", "flex-basis", "align-items", "justify-items", "justify-content", "place-items",
  "grid-template-columns", "grid-template-rows", "row-gap", "column-gap",
];

function freezeStyles(root, mode) {
  return layoutNodes(root).map((node) => {
    const computed = getComputedStyle(node);
    const properties = mode === "visual"
      ? VISUAL_PROPERTIES
      : Array.from(computed).filter((property) => !/^(animation|transition)(-|$)/.test(property)
        && !["transform", "visibility", "opacity", "d"].includes(property));
    return properties.map((property) => [property, computed.getPropertyValue(property)]);
  });
}

/**
 * Freeze the parts of an origin that will fly. Reads only, so the caller runs it
 * before it resizes or restyles anything.
 *
 * @param {Array<{source: Element, target?: Element|(() => Element|null), scale?: "uniform"|"free", hide?: Array<Element|(() => Element|null)>, fadeIn?: Array<Element|(() => Element|null)>, strip?: string, context?: Element}>} specs
 *   `source` is the part as it looks now; `target` is where it lands (a node, or a
 *   function that finds it once the surface is laid out; the source itself when
 *   the same node exists on both sides). `scale: "uniform"` keeps the aspect
 *   ratio (icons); `"free"` stretches each axis (a clock face, which is one object
 *   drawn at two sizes). `hide` lists extra nodes (or functions that find them) to hide for the flight, and
 *   `context` is an ancestor of `source` whose class chain the copy must sit in
 *   so descendant selectors keep matching. `strip` is a selector for descendants the
 *   copy leaves out (a badge that must not travel or scale with the avatar); `fadeIn`
 *   lists nodes (or lookups) that are hidden for the flight and fade in when it ends.
 *   `freeze: "visual"` freezes only the box and paint properties (enough for a glyph;
 *   the default freezes every computed property, which a face with nested layout needs).
 *   The copy is made from the live `source` when it is still in the page, and from a
 *   frozen prototype taken at capture when the owner has replaced it meanwhile.
 */
export function captureSharedItems(specs) {
  const items = [];
  for (const spec of specs || []) {
    const source = spec?.source;
    if (!(source instanceof Element) || !source.isConnected) continue;
    const rect = rectSnapshot(source.getBoundingClientRect());
    if (rect.width <= 0 || rect.height <= 0) continue;
    items.push({
      source,
      target: spec.target ?? source,
      scale: spec.scale === "free" ? "free" : "uniform",
      hide: Array.isArray(spec.hide) ? spec.hide.filter(Boolean) : [],
      fadeIn: Array.isArray(spec.fadeIn) ? spec.fadeIn.filter(Boolean) : [],
      strip: typeof spec.strip === "string" ? spec.strip : "",
      context: spec.context instanceof Element ? spec.context : null,
      rect,
      styles: freezeStyles(source, spec.freeze === "visual" ? "visual" : "full"),
      proto: source.cloneNode(true),
    });
  }
  return items;
}

function resolveTarget(item) {
  const node = typeof item.target === "function" ? item.target() : item.target;
  return node instanceof Element && node.isConnected ? node : null;
}

/**
 * The transform that carries a copy laid out at `base` onto `destination`. Both are
 * viewport rectangles; the transform is in the host's own coordinates, so a host
 * that is itself scaled (`k`, viewport pixels per local pixel) is accounted for.
 */
function pose(base, destination, mode, k) {
  if (mode === "free") {
    const sx = base.width > 0 ? destination.width / base.width : 1;
    const sy = base.height > 0 ? destination.height / base.height : 1;
    return `translate(${(destination.left - base.left) / k.x}px, ${(destination.top - base.top) / k.y}px) scale(${sx}, ${sy})`;
  }
  const s = Math.min(
    base.width > 0 ? destination.width / base.width : 1,
    base.height > 0 ? destination.height / base.height : 1,
  );
  const dx = destination.left - base.left + (destination.width - base.width * s) / 2;
  const dy = destination.top - base.top + (destination.height - base.height * s) / 2;
  return `translate(${dx / k.x}px, ${dy / k.y}px) scale(${s})`;
}

const CONTEXT_ROOT_CSS = "position:absolute;inset:0;padding:0;min-height:0;margin:0;pointer-events:none;"
  + "transition:none!important;opacity:1;visibility:visible;overflow:visible;background:none;border:0;box-shadow:none";

const LAYER_CSS = "position:absolute;inset:0;pointer-events:none;overflow:visible;z-index:10";

export class SharedFlight {
  /**
   * @param {HTMLElement} host - the surface element; the copies live inside it, so they
   *   are clipped by its morph and leave with it if it is unmounted
   * @param {ReturnType<typeof captureSharedItems>} items
   */
  constructor(host, items) {
    this.host = host;
    this.items = items;
    this.layer = null;
    this.planes = [];
    this.hidden = new Set();
    this.stands = new Map();
    this.fading = new Set();
  }

  /** Reads for one transition. Runs with the other reads, before the first write. */
  measure() {
    const hostRect = this.host.getBoundingClientRect();
    return {
      hostRect,
      clientLeft: this.host.clientLeft,
      clientTop: this.host.clientTop,
      // Viewport pixels per layout pixel: 1 unless the host sits under a CSS scale.
      k: {
        x: this.host.offsetWidth > 0 && hostRect.width > 0 ? hostRect.width / this.host.offsetWidth : 1,
        y: this.host.offsetHeight > 0 && hostRect.height > 0 ? hostRect.height / this.host.offsetHeight : 1,
      },
      items: this.items.map((item, index) => {
        const node = resolveTarget(item);
        const plane = this.planes[index];
        return {
          target: node ? rectSnapshot(node.getBoundingClientRect()) : null,
          pose: plane?.element.isConnected ? getComputedStyle(plane.element).transform : null,
        };
      }),
    };
  }

  /**
   * Writes for one transition: creates the copies that do not exist yet, hides the
   * real nodes, and returns the tracks to add to the transition's timeline.
   *
   * @param {boolean} open
   * @param {ReturnType<SharedFlight["measure"]>} measured
   * @param {{x: number, y: number}} shift - how far the origin moved since it was captured
   */
  start(open, measured, shift) {
    const fresh = [];
    this.items.forEach((item, index) => {
      const m = measured.items[index];
      const targetNode = resolveTarget(item);
      if (!m.target || !targetNode || m.target.width <= 0 || m.target.height <= 0) return;
      if (this.planes[index]) return;
      const plane = this._createPlane(item, measured);
      if (plane) {
        this.planes[index] = plane;
        fresh.push(plane);
      }
    });
    // A copy straight in the layer sits exactly where it was placed. Only a copy inside
    // a stand-in chain needs the one read after a write in a transition: where it
    // landed, which depends on the positioned ancestors of the chain.
    for (const plane of fresh) {
      plane.base = plane.item.context
        ? rectSnapshot(plane.element.getBoundingClientRect())
        : { left: plane.item.rect.left, top: plane.item.rect.top, width: plane.item.rect.width, height: plane.item.rect.height };
    }

    const tracks = [];
    this.items.forEach((item, index) => {
      const plane = this.planes[index];
      const m = measured.items[index];
      if (!plane || !m.target) return;
      const sourceRect = {
        left: item.rect.left + shift.x,
        top: item.rect.top + shift.y,
        width: item.rect.width,
        height: item.rect.height,
      };
      const sourcePose = pose(plane.base, sourceRect, item.scale, measured.k);
      const targetPose = pose(plane.base, m.target, item.scale, measured.k);
      const rest = open ? sourcePose : targetPose;
      const from = fresh.includes(plane) ? rest : (m.pose && m.pose !== "none" ? m.pose : rest);
      if (fresh.includes(plane)) plane.element.style.transform = rest;
      this._hide(item.source);
      this._hide(resolveTarget(item));
      for (const node of item.hide) this._hide(typeof node === "function" ? node() : node);
      for (const entry of item.fadeIn) {
        const node = typeof entry === "function" ? entry() : entry;
        this._hide(node);
        if (node instanceof Element) this.fading.add(node);
      }
      tracks.push({
        element: plane.element,
        keyframes: [{ transform: from }, { transform: open ? targetPose : sourcePose }],
      });
    });
    return tracks;
  }

  _createPlane(item, measured) {
    if (!this.layer) {
      const layer = document.createElement("div");
      layer.className = "surface-flight-layer";
      layer.setAttribute("aria-hidden", "true");
      layer.style.cssText = LAYER_CSS;
      this.host.appendChild(layer);
      this.layer = layer;
    }

    const element = (item.source.isConnected ? item.source : item.proto).cloneNode(true);
    element.removeAttribute(FLIGHT_HIDDEN_ATTR);
    element.removeAttribute("id");
    element.querySelectorAll("[id]").forEach((node) => node.removeAttribute("id"));
    const nodes = layoutNodes(element);
    if (nodes.length !== item.styles.length) return null;
    nodes.forEach((node, index) => {
      for (const [property, value] of item.styles[index]) node.style.setProperty(property, value);
    });
    if (item.strip) element.querySelectorAll(item.strip).forEach((node) => node.remove());
    for (const node of [element, ...element.querySelectorAll("*")]) {
      node.style.setProperty("animation", "none", "important");
      node.style.setProperty("transition", "none", "important");
    }
    Object.assign(element.style, {
      position: "absolute",
      left: `${(item.rect.left - measured.hostRect.left) / measured.k.x - measured.clientLeft}px`,
      top: `${(item.rect.top - measured.hostRect.top) / measured.k.y - measured.clientTop}px`,
      right: "auto", bottom: "auto", margin: "0", flex: "none",
      transform: "none", transformOrigin: "top left",
      visibility: "visible", opacity: "1", pointerEvents: "none", willChange: "transform",
    });

    this._contextParent(item).appendChild(element);
    return { element, item, base: null };
  }

  // The copy sits in empty stand-ins of its ancestors, down from `context`, so the
  // selectors that style it by ancestry (a gooey face inside its container) still match.
  _contextParent(item) {
    const context = item.context;
    if (!context || context === item.source || !item.source.isConnected || !context.contains(item.source)) return this.layer;
    // Parts that shared a parent share the stand-in chain too, so a module that
    // updates "its face" in the document (the gooey clock does) finds them together.
    const parentNode = item.source.parentElement;
    if (this.stands.has(parentNode)) return this.stands.get(parentNode);
    const chain = [];
    for (let node = item.source.parentElement; node; node = node.parentElement) {
      chain.unshift(node);
      if (node === context) break;
    }
    let parent = this.layer;
    chain.forEach((node, depth) => {
      const stand = document.createElement("div");
      stand.className = node.className;
      stand.setAttribute("aria-hidden", "true");
      if (depth === 0) stand.style.cssText = CONTEXT_ROOT_CSS;
      parent.appendChild(stand);
      parent = stand;
    });
    this.stands.set(parentNode, parent);
    return parent;
  }

  _hide(node) {
    if (!(node instanceof Element) || node.hasAttribute(FLIGHT_HIDDEN_ATTR)) return;
    node.setAttribute(FLIGHT_HIDDEN_ATTR, "");
    this.hidden.add(node);
  }

  /**
   * Remove the copies and show the real nodes again, in one task. The nodes come
   * back with their transitions switched off for that moment: a node whose own
   * state changed while it was hidden (a dock clock that the collapsing card
   * fades back in) must appear in its final state, not start a second fade.
   */
  release() {
    this.layer?.remove();
    this.layer = null;
    this.planes = [];
    this.stands.clear();
    const nodes = [...this.hidden];
    this.hidden.clear();
    const saved = nodes.map((node) => ({
      value: node.style.getPropertyValue("transition"),
      priority: node.style.getPropertyPriority("transition"),
    }));
    for (const node of nodes) {
      node.style.setProperty("transition", "none", "important");
      node.removeAttribute(FLIGHT_HIDDEN_ATTR);
    }
    for (const node of nodes) void getComputedStyle(node).opacity;
    nodes.forEach((node, index) => {
      if (saved[index].value) node.style.setProperty("transition", saved[index].value, saved[index].priority);
      else node.style.removeProperty("transition");
    });
    for (const node of this.fading) {
      if (node.isConnected) node.animate?.({ opacity: [0, 1] }, { duration: getDuration("hover"), easing: "ease-out" });
    }
    this.fading.clear();
  }
}
