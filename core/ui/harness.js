/**
 * Shell E2E Test & Live Inspection Harness
 *
 * Provides deterministic observability and control for AI agents and test runners:
 * - Error ring buffer (window.__SHELL_ERRORS__)
 * - Zero-Motion mode toggle (instant animations in tests)
 * - Compact Semantic Tree generator (AOM tree for text LLMs, 300-500 tokens)
 * - Interactive element resolver with occlusion detection
 */

import { setMotionScale } from "./motion.js";
import { invoke, sessionInfo as fetchSessionInfo } from "@unidtk/bridge";

// Global error ring buffer (last 50 errors)
window.__UNIDTK_ERRORS__ = window.__UNIDTK_ERRORS__ || window.__SHELL_ERRORS__ || [];
window.__SHELL_ERRORS__ = window.__UNIDTK_ERRORS__;

window.addEventListener("error", (event) => {
  const err = {
    ts: Date.now(),
    type: "error",
    message: event.message || String(event.error),
    filename: event.filename,
    lineno: event.lineno,
    colno: event.colno,
    stack: event.error?.stack || null,
  };
  window.__UNIDTK_ERRORS__.push(err);
  if (window.__UNIDTK_ERRORS__.length > 50) {
    window.__UNIDTK_ERRORS__.shift();
  }
});

window.addEventListener("unhandledrejection", (event) => {
  const err = {
    ts: Date.now(),
    type: "unhandledrejection",
    message: event.reason?.message || String(event.reason),
    stack: event.reason?.stack || null,
  };
  window.__UNIDTK_ERRORS__.push(err);
  if (window.__UNIDTK_ERRORS__.length > 50) {
    window.__UNIDTK_ERRORS__.shift();
  }
});

// Stable per-element reference. Unlike the positional `id` (which is just an index
// into the current tree), it survives re-renders and can target an element across
// steps of one batch.
let __udtkRefSeq = 0;
function stableRef(el) {
  let ref = el.getAttribute("data-udtk-ref");
  if (!ref) {
    ref = "r" + ++__udtkRefSeq;
    el.setAttribute("data-udtk-ref", ref);
  }
  return ref;
}

// What sits on top of the given point, when it is neither the element nor its
// descendant. Callers need to know WHAT blocked the click, not merely that it was
// blocked — otherwise a receipt can claim success while a foreign surface ate it.
function coveringAt(x, y, el) {
  let hit = null;
  try {
    hit = document.elementFromPoint(x, y);
  } catch (_) {
    return null;
  }
  if (!hit || el.contains(hit) || hit.contains(el)) return null;
  return {
    tag: hit.tagName.toLowerCase(),
    id: hit.id || null,
    testid: hit.getAttribute("data-testid") || null,
    ref: stableRef(hit),
  };
}

export const UniDtkHarness = {
  /**
   * Toggle Zero-Motion mode: sets all animation and transition durations to 0ms
   * for deterministic, instantaneous UI state transitions in tests.
   */
  setZeroMotion(enabled = true) {
    if (enabled) {
      setMotionScale(0);
      document.documentElement.classList.add("zero-motion");
    } else {
      setMotionScale(1.0);
      document.documentElement.classList.remove("zero-motion");
    }
    return { zeroMotion: enabled };
  },

  /**
   * Return buffered errors and optionally clear the buffer.
   */
  getErrors(clear = false) {
    const list = [...window.__SHELL_ERRORS__];
    if (clear) {
      window.__SHELL_ERRORS__.length = 0;
    }
    return list;
  },

  /**
   * Clear the error buffer.
   */
  clearErrors() {
    window.__SHELL_ERRORS__.length = 0;
    return true;
  },

  /**
   * Build a compact semantic tree (AOM) of visible, interactive elements.
   * Returns an array of node descriptors designed for low token count.
   */
  getSemanticTree() {
    const nodes = [];
    let nextId = 1;

    // Standard interactive query
    const selector = [
      "[data-testid]",
      "button",
      "a[href]",
      "input",
      "select",
      "textarea",
      "[role='button']",
      "[role='tab']",
      "[role='link']",
      "[role='switch']",
      "[role='checkbox']",
      "[role='menuitem']",
      "[role='slider']",
      ".action-btn",
      ".nav-btn",
      ".tab-item",
      ".track-row",
      ".chat-item",
    ].join(", ");

    const candidates = Array.from(document.querySelectorAll(selector));

    for (const el of candidates) {
      // 1. Visibility check (skip hidden / display:none / opacity:0)
      if (typeof el.checkVisibility === "function") {
        if (!el.checkVisibility({ checkOpacity: true, checkVisibilityCSS: true })) {
          continue;
        }
      } else {
        const style = window.getComputedStyle(el);
        if (style.display === "none" || style.visibility === "hidden" || style.opacity === "0") {
          continue;
        }
      }

      // Whatever a running simulation rendered is offscreen scaffolding, not UI: it
      // must never appear as a target for the agent.
      if (el.closest(".simulation-sandbox")) {
        continue;
      }

      const rect = el.getBoundingClientRect();
      if (rect.width <= 0 || rect.height <= 0) {
        continue;
      }

      // Check if within viewport bounds
      if (rect.bottom < 0 || rect.right < 0 || rect.top > window.innerHeight || rect.left > window.innerWidth) {
        continue;
      }

      // 2. Center point in CSS viewport pixels
      const cx = Math.round(rect.left + rect.width / 2);
      const cy = Math.round(rect.top + rect.height / 2);

      // 3. Occlusion test: what (if anything) covers the center point
      const covering = coveringAt(cx, cy, el);
      const occluded = covering !== null;

      // 4. Extract accessible name / label
      const testid = el.getAttribute("data-testid") || null;
      const role = el.getAttribute("role") || el.tagName.toLowerCase();
      let name =
        el.getAttribute("aria-label") ||
        el.getAttribute("title") ||
        el.getAttribute("placeholder") ||
        el.innerText?.trim()?.split("\n")[0] ||
        "";

      if (name.length > 40) {
        name = name.slice(0, 37) + "...";
      }

      // 5. Extract state
      const state = {};
      if (el.disabled || el.getAttribute("aria-disabled") === "true") state.disabled = true;
      if (el.getAttribute("aria-pressed") === "true") state.pressed = true;
      if (el.getAttribute("aria-selected") === "true") state.selected = true;
      if (el.getAttribute("aria-expanded") === "true") state.expanded = true;
      if (el.checked !== undefined && el.type === "checkbox") state.checked = el.checked;
      if (el.value !== undefined && el.tagName === "INPUT" && el.type !== "password") {
        state.value = el.value.slice(0, 30);
      }

      // Locator hint. Deliberately no CSS classes: a class-based locator breaks
      // silently on a restyle, so classes stay internal to candidate discovery.
      let selHint = "";
      if (testid) {
        selHint = `[data-testid="${testid}"]`;
      } else if (el.id) {
        selHint = `#${el.id}`;
      }

      nodes.push({
        id: nextId++,
        ref: stableRef(el),
        role,
        name: name || null,
        testid,
        selector: selHint || null,
        fragile: !testid && !el.id,
        point: { x: cx, y: cy },
        bounds: {
          w: Math.round(rect.width),
          h: Math.round(rect.height),
        },
        state: Object.keys(state).length > 0 ? state : undefined,
        occluded: occluded ? true : undefined,
        covering: covering || undefined,
      });
    }

    return nodes;
  },

  /**
   * Find element target by semantic name, label, text, testid, selector, or numeric id.
   */
  resolveTarget(target) {
    if (target == null) return null;
    const tree = this.getSemanticTree();

    // 1. Numeric ID from getSemanticTree (e.g. 8 or "8")
    if (typeof target === "number" || (typeof target === "string" && /^\d+$/.test(target.trim()))) {
      const num = parseInt(String(target).trim(), 10);
      const match = tree.find((n) => n.id === num);
      if (match) return match;
    }

    const strTarget = String(target).trim();

    // 2. Stable reference (r12) — survives re-renders, unlike the positional id
    if (/^r\d+$/.test(strTarget)) {
      const el = document.querySelector(`[data-udtk-ref="${CSS.escape(strTarget)}"]`);
      if (el) return this._nodeFromElement(el);
    }

    // 3. Shorthand for testid: @play-btn -> [data-testid='play-btn']
    if (strTarget.startsWith("@")) {
      const testid = strTarget.slice(1);
      const match = tree.find((n) => n.testid === testid);
      if (match) return match;
      const el = document.querySelector(`[data-testid="${CSS.escape(testid)}"]`);
      if (el) return this._nodeFromElement(el);
    }

    // 3. Match by element accessible name/label/text in Semantic Tree (case-insensitive)
    const lower = strTarget.toLowerCase();
    const exactNameMatch = tree.find((n) => n.name && n.name.toLowerCase() === lower);
    if (exactNameMatch) return exactNameMatch;

    const prefixNameMatch = tree.find((n) => n.name && n.name.toLowerCase().startsWith(lower));
    if (prefixNameMatch) return prefixNameMatch;

    const partialNameMatch = tree.find((n) => n.name && n.name.toLowerCase().includes(lower));
    if (partialNameMatch) return partialNameMatch;

    // 4. Try CSS selector safely without throwing DOMException
    let el = null;
    try {
      el = document.querySelector(strTarget);
    } catch (_) {
      el = null;
    }

    if (el) {
      return this._nodeFromElement(el);
    }

    return null;
  },

  _nodeFromElement(el) {
    const rect = el.getBoundingClientRect();
    const cx = Math.round(rect.left + rect.width / 2);
    const cy = Math.round(rect.top + rect.height / 2);

    const covering = coveringAt(cx, cy, el);

    return {
      tagName: el.tagName.toLowerCase(),
      testid: el.getAttribute("data-testid") || null,
      ref: stableRef(el),
      point: { x: cx, y: cy },
      bounds: { w: Math.round(rect.width), h: Math.round(rect.height) },
      occluded: covering !== null,
      covering: covering || undefined,
    };
  },

  /// Bootstrap for the page's own door: port + the per-run token. Delegates to the
  /// bridge so the token is fetched in exactly one place and never stored.
  async sessionInfo() {
    return await fetchSessionInfo();
  },

  invoke(cmd, args) {
    return invoke(cmd, args);
  },
};

window.__UNIDTK_HARNESS__ = UniDtkHarness;
