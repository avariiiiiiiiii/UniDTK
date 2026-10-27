/** Individually routed micrographics. Permanent dock snapshots exclude popups. */
import { invoke, listen } from "@unidtk/bridge";
import { config, saveConfig, getDockGeometry } from "../../unidtk-core.js";
import { buildTokens, routeElements, STYLES, STYLE_NAMES, PILL_PAD } from "./composition.js";
export { buildTokens, routeElements } from "./composition.js";
const runtimes = new Map();
const clamp = (n, lo, hi) => Math.max(lo, Math.min(hi, n));
const GLYPHS = {
  feels: `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"><circle cx="12" cy="5" r="2.5"/><path d="M8 21v-7H5l3-5h8l3 5h-3v7M12 14v7"/></svg>`,
  sunrise: `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"><path d="M2 18h20M5 15a7 7 0 0114 0M12 2v8M9 5l3-3 3 3M3 9l2 2M21 9l-2 2"/></svg>`,
  sunset: `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"><path d="M2 18h20M5 15a7 7 0 0114 0M12 2v8M9 7l3 3 3-3M3 9l2 2M21 9l-2 2"/></svg>`,
  daylength: `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"><circle cx="12" cy="12" r="8"/><path d="M12 6v6l4 2M12 1v1M12 22v1M1 12h1M22 12h1"/></svg>`,
  clear: `<svg viewBox="0 0 24 24" fill="currentColor"><path d="M12 1l2.4 5.3 5.5-2.5-1 6L24 12l-5.1 2.2 1 6-5.5-2.5L12 23l-2.4-5.3-5.5 2.5 1-6L0 12l5.1-2.2-1-6 5.5 2.5Z"/></svg>`,
  moon: `<svg viewBox="0 0 24 24" fill="currentColor"><path d="M21 15.4A10 10 0 018.6 3a10 10 0 1012.4 12.4Z"/></svg>`,
  cloud: `<svg viewBox="0 0 24 24" fill="currentColor"><path d="M6 19a5.8 5.8 0 01-1.1-11.5A7 7 0 0118 7a6 6 0 011 12Z"/></svg>`,
  fog: `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3"><path d="M1 5l4-2 7 4 7-4 4 2M1 12l4-2 7 4 7-4 4 2M1 19l4-2 7 4 7-4 4 2"/></svg>`,
  drizzle: `<svg viewBox="0 0 24 24" fill="currentColor"><path d="M7 2s-5 6-5 9a5 5 0 0010 0C12 8 7 2 7 2ZM18 9s-4 5-4 8a4 4 0 008 0c0-3-4-8-4-8Z"/></svg>`,
  rain: `<svg viewBox="0 0 24 24" fill="currentColor"><path d="M7 1S1 9 1 13a6 6 0 0012 0C13 9 7 1 7 1ZM19 7s-5 7-5 10a5 5 0 0010 0c0-3-5-10-5-10Z"/></svg>`,
  sleet: `<svg viewBox="0 0 24 24" fill="currentColor"><circle cx="3" cy="6" r="2"/><circle cx="10" cy="6" r="2"/><circle cx="17" cy="6" r="2"/><circle cx="6" cy="13" r="2"/><circle cx="13" cy="13" r="2"/><circle cx="20" cy="13" r="2"/><circle cx="3" cy="20" r="2"/><circle cx="10" cy="20" r="2"/><circle cx="17" cy="20" r="2"/></svg>`,
  snow: `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4"><path d="M12 1v22M2.5 6.5l19 11M2.5 17.5l19-11M8 3l4 4 4-4M8 21l4-4 4 4M2 11l5-1-1-5M22 13l-5 1 1 5M6 19l1-5-5-1M18 5l-1 5 5 1"/></svg>`,
  thunder: `<svg viewBox="0 0 24 24" fill="currentColor"><path d="M11 1h10l-7 9h7L7 24l3-12H3Z"/></svg>`,
  thermometer: `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"><path d="M9 14.5V5a3 3 0 016 0v9.5a5 5 0 11-6 0Z"/><path d="M12 8v10"/><circle cx="12" cy="18" r="1.6" fill="currentColor" stroke="none"/></svg>`,
  signal: `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round"><path d="M4 19h1.6M9 19h1.6M14 19h1.6M19 19h1.6"/><path d="M4 14h4M11 14h4M18 14h2M4 9h6M13 9h6" opacity=".45"/></svg>`,
  offline: `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round"><path d="M4 19h16M7 14h10M4 9h16" opacity=".9"/><path d="M4 4l16 16"/></svg>`,
  volume: `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><path d="M5 9.5h3.5L13 5.5v13L8.5 14.5H5z"/><path d="M16.5 9.5a4 4 0 010 5"/></svg>`,
  mute: `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><path d="M5 9.5h3.5L13 5.5v13L8.5 14.5H5z"/><path d="M16.5 9.5l4 5M20.5 9.5l-4 5"/></svg>`,
  wind: `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><path d="M1 7h14a3 3 0 10-3-3M1 12h19a3 3 0 11-3 3M1 17h9a3 3 0 11-3 3"/></svg>`,
  partly: `<svg viewBox="0 0 24 24" fill="currentColor"><path d="M8 0l2 3 4-1-1 4 3 2-3 2 1 4-4-1-2 3-2-3-4 1 1-4-3-2 3-2-1-4 4 1Z"/><path d="M7 21a5 5 0 010-10 6 6 0 0111 0 5 5 0 010 10Z"/></svg>`,
  "partly-night": `<svg viewBox="0 0 24 24" fill="currentColor"><path d="M13 10A7 7 0 017 0a7 7 0 106 10Z"/><path d="M7 22a5 5 0 010-10 6 6 0 0111 0 5 5 0 010 10Z"/></svg>`,
  pin: `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><path d="M12 21s6.5-6.2 6.5-10.5a6.5 6.5 0 10-13 0C5.5 14.8 12 21 12 21z"/><circle cx="12" cy="10.5" r="2.2"/></svg>`,
};

// ─── Cell construction ────────────────────────────────────────────────────────
// Rows are built once and then *updated in place*: `textContent` and a class are
// the only things that change, so the panel never re-parses markup and never
// holds state that a rebuild would take with it.

function makeCell(kind) {
  const el = document.createElement("span");
  el.className = `mg-cell mg-${kind}`;
  return el;
}

/** Applies a cell description to an existing element, reusing the node. */
function paintCell(el, spec) {
  if (el.dataset.kind !== spec.k) {
    el.className = `mg-cell mg-${spec.k}`;
    el.dataset.kind = spec.k;
    delete el.dataset.glyph;
    el.textContent = "";
  }

  if (spec.k === "ic") {
    // A glyph is markup, and there are a fixed, tiny set of them.
    if (el.dataset.glyph !== spec.g) {
      el.innerHTML = GLYPHS[spec.g] || GLYPHS.cloud;
      el.dataset.glyph = spec.g;
    }
    el.classList.toggle("mg-lit", Boolean(spec.lit));
    return;
  }

  if (spec.k === "daytrack") {
    el.style.setProperty("--mg-day-progress", `${Math.max(0,Math.min(100,Number(spec.v)||0))}%`);
    return;
  }

  if (spec.k === "chance") {
    el.style.setProperty("--mg-chance", `${Math.max(0,Math.min(100,parseFloat(spec.v)||0))}%`);
  }

  if (spec.k === "dots") {
    // Intensity as a *count* of dots. Built from children rather than text, and
    // reconciled one at a time, so changing the intensity never re-parses markup.
    const want = Math.max(0, Math.min(3, Number(spec.v) || 0));
    while (el.childElementCount > want) el.removeChild(el.lastElementChild);
    while (el.childElementCount < want) {
      const dot = document.createElement("i");
      el.appendChild(dot);
    }
    el.dataset.n = String(want);
    return;
  }

  if (el.textContent !== spec.v) el.textContent = spec.v;
}


function settingsFor(id, preview) {
  if (preview) return { x: 8, y: 8, mgScale: 1, mgStyle: "mechanical", mgLayout: 2 };
  const pos = config.positions[id] || (config.positions[id] = {}), before = JSON.stringify(pos);
  pos.x = Number.isFinite(pos.x) ? pos.x : 28;
  pos.y = Number.isFinite(pos.y) ? pos.y : 28;
  pos.mgScale = clamp(Number(pos.mgScale) || 1, 0.55, 3);
  pos.mgStyle = STYLES.includes(pos.mgStyle) ? pos.mgStyle : "mechanical";
  pos.mgLayout = 2;
  delete pos.mgWidth; delete pos.w; delete pos.h;
  if (before !== JSON.stringify(pos)) saveConfig();
  return pos;
}

// Jelly material with mgLayout "pills": every group run gets its own tinted
// pill. Read from <html> attributes written by the theme engine.
const pillsEnabled = () => {
  const ds = document.documentElement.dataset;
  return ds.material === "jelly" && ds.mgLayout === "pills";
};

/** One pill per contiguous run of a group within a row (a group that wraps
 *  to the next row gets a second pill instead of covering its neighbours). */
function drawPills(layer, placed, ox, oy, scale) {
  const runs = [];
  for (const e of placed) {
    const run = runs.at(-1);
    if (run && run.group === e.group && e.y < run.bottom && e.y + e.h > run.top && e.x >= run.right) {
      run.right = e.x + e.w; run.top = Math.min(run.top, e.y); run.bottom = Math.max(run.bottom, e.y + e.h);
    } else {
      runs.push({ group: e.group, left: e.x, right: e.x + e.w, top: e.y, bottom: e.y + e.h });
    }
  }
  const px = PILL_PAD.x * scale, py = PILL_PAD.y * scale;
  layer.replaceChildren(...runs.map(run => {
    const pill = document.createElement("div");
    pill.className = "mg-pill jelly";
    pill.dataset.group = run.group;
    pill.style.setProperty("--card-tint", `var(--tint-mg-${run.group})`);
    Object.assign(pill.style, { left: `${run.left - px - ox}px`, top: `${run.top - py - oy}px`,
      width: `${run.right - run.left + 2 * px}px`, height: `${run.bottom - run.top + 2 * py}px` });
    return pill;
  }));
}

export function renderMicrographics(card, id) {
  if (!card.isConnected) return;
  runtimes.get(card)?.dispose();
  const preview = !card.classList.contains("widget-card"), pos = settingsFor(id, preview);
  card.classList.add("micrographics-card", "no-header");
  card.setAttribute("role", "group");
  card.querySelectorAll(".resize-handle, .mg-grid, .mg-version-switcher, .mg-location-request").forEach(e => e.remove());
  const grid = document.createElement("div"); grid.className = "mg-grid"; card.appendChild(grid);
  const pillLayer = document.createElement("div"); pillLayer.className = "mg-pills"; grid.appendChild(pillLayer);
  const recovery = document.createElement("span"); recovery.className = "mg-atom mg-recovery";
  recovery.textContent = "MICROGRAPHICS"; recovery.hidden = true; grid.appendChild(recovery);
  const corner = document.createElement("button"); corner.type = "button";
  corner.className = "resize-handle corner"; corner.dataset.testid = "micrographics-scale";
  corner.setAttribute("aria-label", "Размер Micrographics"); card.appendChild(corner);
  const switcher = document.createElement("button"); switcher.type = "button";
  switcher.className = "version-switcher mg-version-switcher";
  switcher.dataset.testid = "micrographics-style"; switcher.textContent = "↻"; card.appendChild(switcher);
  const locationButton = document.createElement("button"); locationButton.type = "button";
  locationButton.className = "mg-location-request"; locationButton.dataset.testid = "micrographics-location";
  locationButton.textContent = "Определить место"; locationButton.hidden = preview; card.appendChild(locationButton);
  const nodes = new Map(), cleanups = [];
  let alive = true, frame = 0, needsLayout = true, gesture = null, signature = "", footprint = "";
  let previousLayout = null, anchorSignature = "";
  let weather = null, volume = null, weatherRevision = 0, volumeRevision = 0;
  let locationPending = false, locationError = "";
  let docks = preview ? [] : getDockGeometry();
  const valid = () => alive && card.isConnected;
  const viewport = () => ({ w: preview ? Math.max(640, card.parentElement.clientWidth) : card.parentElement.clientWidth,
    h: preview ? 400 : card.parentElement.clientHeight });
  const capabilities = area => [...new Set(docks.filter(d => d.x < area.w && d.y < area.h && d.x + d.w > 0 && d.y + d.h > 0)
    .flatMap(d => d.capabilities || []))];
  function schedule(layout = false) {
    needsLayout ||= layout;
    if (frame || !valid()) return;
    frame = requestAnimationFrame(() => { frame = 0; if (valid()) paint(); });
  }
  function paint() {
    // Freeze data reconciliation as well as routing while pointer capture is active.
    if (gesture) return;
    locationButton.disabled = locationPending || weather?.locationStatus === "requesting";
    const wasHidden = locationButton.hidden;
    locationButton.hidden = preview || ["available", "retrying"].includes(weather?.locationStatus);
    needsLayout ||= wasHidden !== locationButton.hidden;
    locationButton.textContent = locationButton.disabled ? "Определяю…" : "Определить место";
    locationButton.setAttribute("aria-label", locationError || locationButton.textContent);
    const area = viewport();
    const specs = buildTokens(new Date(), { weather, volume, online: navigator.onLine !== false,
      capabilities: capabilities(area), style: pos.mgStyle });
    // Metadata such as atom provenance/age text must not trigger geometric measurement.
    const next = JSON.stringify(specs.map(({ title, ...geometry }) => geometry));
    if (next !== signature) {
      signature = next; needsLayout = true;
      const ids = new Set(specs.map(s => s.id));
      for (const [key, node] of nodes) if (!ids.has(key)) { node.remove(); nodes.delete(key); }
      for (const spec of specs) {
        let node = nodes.get(spec.id);
        if (!node) {
          node = makeCell(spec.k); node.dataset.tokenId = spec.id;
          node.dataset.testid = `micrographics-${spec.id.replaceAll(":", "-")}`;
          node.setAttribute("aria-hidden", "true"); grid.appendChild(node); nodes.set(spec.id, node);
        }
        paintCell(node, spec); node.classList.add("mg-atom");
        node.classList.toggle("mg-secondary", spec.secondary);
        node.dataset.group = spec.group;
        node.style.setProperty("--mg-ink", `var(--ink-tint-mg-${spec.group})`);
      }
    }
    grid.setAttribute("role", "img");
    card.dataset.mgStyle = pos.mgStyle;
    switcher.setAttribute("aria-label", `Композиция: ${STYLE_NAMES[pos.mgStyle]}. Сменить`);
    if (!needsLayout) return;
    needsLayout = false;
    card.style.setProperty("--mg-scale", String(pos.mgScale));
    const scrollPosition = { left: grid.scrollLeft, top: grid.scrollTop };
    grid.style.width = ""; grid.style.height = "";
    card.classList.remove("mg-overflow", "mg-overlay");
    for (const node of nodes.values()) {
      node.hidden = false;
      node.style.maxWidth = `${Math.max(1, area.w - 16)}px`;
    }
    // Batch intrinsic reads before final coordinate writes.
    const elements = specs.map(spec => ({ id: spec.id, group: spec.group, breakBefore: spec.breakBefore, node: nodes.get(spec.id),
      w: nodes.get(spec.id).offsetWidth, h: nodes.get(spec.id).offsetHeight }));
    const pills = pillsEnabled();
    const anchorKey = JSON.stringify([pos.x, pos.y, pos.mgScale, pos.mgStyle, pills]);
    if (anchorKey !== anchorSignature) previousLayout = null;
    anchorSignature = anchorKey;
    const placed = routeElements(elements, { x: pos.x, y: pos.y, scale: pos.mgScale,
      style: pos.mgStyle, previous: previousLayout, pills }, area, docks);
    previousLayout = placed.layout;
    const bounds = placed.layout;
    const key = JSON.stringify([placed.map(e => [e.id, e.x, e.y, e.w, e.h]), bounds, locationButton.hidden, pills]);
    card.classList.toggle("mg-overflow", bounds.overflow);
    card.classList.toggle("mg-overlay", bounds.overlay);
    grid.style.width = `${bounds.contentW}px`; grid.style.height = `${bounds.contentH}px`;
    grid.tabIndex = bounds.overflow ? 0 : -1;
    grid.scrollLeft = scrollPosition.left; grid.scrollTop = scrollPosition.top;
    if (key !== footprint) {
      footprint = key;
      const x = bounds.x, y = bounds.y;
      const w = placed.length ? bounds.w : 180, h = placed.length ? bounds.h : 36;
      Object.assign(card.style, { left: `${x}px`, top: `${y}px`, width: `${w}px`, height: `${h}px` });
      for (const e of placed) { e.node.style.left = `${e.x - x}px`; e.node.style.top = `${e.y - y}px`; }
      if (pills) drawPills(pillLayer, placed, x, y, pos.mgScale); else pillLayer.replaceChildren();
      // Controls belong to a free surface, never to the broad flow bounding box.
      const first = placed[0] || {x,y,w:180,h:36}, last = placed.at(-1) || first;
      const controlW = locationButton.hidden ? 64 : 220, controlH = 28;
      const starts = [first.x,8,...docks.map(d=>d.x+d.w+8)];
      const bands = [Math.max(0,first.y-32),0,...placed.map(e=>e.y+e.h+4)];
      let toolbar = null;
      for (const cy of bands) for (const cx of starts) {
        if (toolbar || cx+controlW>area.w || cy+controlH>area.h) continue;
        if (![...docks,...(bounds.overflow ? [bounds] : placed)].some(d=>cx<d.x+d.w && cx+controlW>d.x && cy<d.y+d.h && cy+controlH>d.y)) toolbar={x:cx,y:cy};
      }
      toolbar ||= {x:first.x,y:first.y};
      Object.assign(switcher.style,{left:`${toolbar.x-x}px`,right:"auto",top:`${toolbar.y-y}px`});
      Object.assign(locationButton.style,{left:`${toolbar.x-x+32}px`,bottom:"auto",top:`${toolbar.y-y}px`});
      Object.assign(corner.style,{left:`${bounds.overflow ? Math.max(0,w-20) : last.x-x+Math.max(0,last.w-20)}px`,top:`${bounds.overflow ? Math.max(0,h-20) : last.y-y+Math.max(0,last.h-20)}px`,right:"auto",bottom:"auto"});
      const remove=card.querySelector(".widget-instance-remove");
      if(remove) Object.assign(remove.style,{left:`${toolbar.x-x+(locationButton.hidden?32:194)}px`,top:`${toolbar.y-y}px`,right:"auto"});
    }
    const spokenGlyph = { thermometer: "Температура", feels: "Ощущаемая температура", wind: "Ветер", sunrise: "Восход", sunset: "Закат", daylength: "Длительность светового дня", rain: "Осадки", signal: "Сеть", volume: "Системная громкость", mute: "Системная громкость", offline: "Сеть" };
    const mgSpoken = specs.map(s => s.id === "geo:pin" ? s.title : s.id === "forecast:chance" ? `Максимальная вероятность отдельного часа: до ${s.v}` : s.k === "daytrack" ? `Ход светового дня ${s.v}%` : s.v || spokenGlyph[s.g] || "").filter(Boolean);
    // Atom provenance, accuracy and age text moved here from per-cell hover tooltips:
    // the popups are gone, the details stay in the grid's accessible description.
    const mgDetails = specs.map(s => (s.id === "geo:pin" ? "" : s.title)).filter(Boolean);
    grid.setAttribute("aria-label", [...mgSpoken, ...mgDetails].join(" "));
    recovery.hidden = placed.length > 0 || preview;
    card.classList.toggle("mg-ready", placed.length > 0);
    card.classList.toggle("mg-empty", placed.length === 0);
  }
  switcher.addEventListener("click", e => {
    e.stopPropagation();
    if (!document.body.classList.contains("edit-mode") || gesture) return;
    pos.mgStyle = STYLES[(STYLES.indexOf(pos.mgStyle) + 1) % STYLES.length];
    if (!preview) saveConfig(); schedule(true);
  });
  locationButton.addEventListener("click", async e => {
    e.stopPropagation();
    if (preview || locationPending || gesture || !document.body.classList.contains("edit-mode")) return;
    locationPending = true; locationError = ""; schedule();
    try { await invoke("weather_location_request"); }
    catch (error) { if (valid()) locationError = `Не удалось запросить геолокацию: ${String(error)}`; }
    finally { if (valid()) { locationPending = false; schedule(); } }
  });
  corner.addEventListener("keydown", e => {
    if (!["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown"].includes(e.key) ||
        !document.body.classList.contains("edit-mode") || gesture) return;
    e.preventDefault();
    pos.mgScale = clamp(pos.mgScale + (["ArrowRight", "ArrowUp"].includes(e.key) ? 0.05 : -0.05), 0.55, 3);
    if (!preview) saveConfig(); schedule(true);
  });
  function begin(e) {
    if (gesture || !valid() || e.button !== 0 || !document.body.classList.contains("edit-mode") ||
        e.target.closest(".mg-version-switcher, .mg-location-request")) return;
    const resizing = Boolean(e.target.closest(".resize-handle.corner"));
    if (!resizing && !e.target.closest(".mg-atom")) return;
    e.preventDefault(); if (resizing) corner.focus({ preventScroll: true });
    const origin = { x: pos.x, y: pos.y, scale: pos.mgScale, left: parseFloat(card.style.left),
      top: parseFloat(card.style.top), w: card.offsetWidth, h: card.offsetHeight };
    const sx = e.clientX, sy = e.clientY, pointerId = e.pointerId;
    const capture = resizing ? corner : card;
    gesture = { origin }; card.classList.add("dragging");
    let queued = null, moveFrame = 0;
    const apply = ev => {
      const dx = ev.clientX - sx, dy = ev.clientY - sy;
      if (resizing) {
        const ratio = 1 + (dx * origin.w + dy * origin.h) / Math.max(1, origin.w ** 2 + origin.h ** 2);
        pos.mgScale = clamp(origin.scale * Math.max(0.1, ratio), 0.55, 3);
        const previewScale = pos.mgScale / origin.scale;
        grid.style.transformOrigin = "top left";
        grid.style.transform = `scale(${previewScale})`;
        card.style.width = `${origin.w * previewScale}px`;
        card.style.height = `${origin.h * previewScale}px`;
      } else {
        const area = viewport();
        pos.x = clamp(origin.x + dx, 8, Math.max(8, area.w - 64));
        pos.y = clamp(origin.y + dy, 8, Math.max(8, area.h - 40));
        card.style.left = `${origin.left + pos.x - origin.x}px`;
        card.style.top = `${origin.top + pos.y - origin.y}px`;
      }
    };
    const move = ev => {
      if (ev.pointerId !== pointerId) return;
      queued = ev;
      if (!moveFrame) moveFrame = requestAnimationFrame(() => { moveFrame = 0; if (queued) apply(queued); queued = null; });
    };
    const finish = (ev, cancel = false) => {
      if (!gesture || ev.pointerId !== pointerId) return;
      if (moveFrame) cancelAnimationFrame(moveFrame);
      if (cancel) Object.assign(pos, { x: origin.x, y: origin.y, mgScale: origin.scale }); else apply(ev);
      document.removeEventListener("pointermove", move); document.removeEventListener("pointerup", up);
      document.removeEventListener("pointercancel", cancelled); capture.removeEventListener("lostpointercapture", lost);
      gesture = null; card.classList.remove("dragging");
      grid.style.transform = "";
      try { capture.releasePointerCapture(pointerId); } catch {}
      if (!cancel && !preview) saveConfig();
      footprint = ""; schedule(true);
    };
    const up = ev => finish(ev), cancelled = ev => finish(ev, true), lost = ev => finish(ev, true);
    document.addEventListener("pointermove", move); document.addEventListener("pointerup", up);
    document.addEventListener("pointercancel", cancelled); capture.addEventListener("lostpointercapture", lost);
    try { capture.setPointerCapture(pointerId); } catch {}
    gesture.cancel = () => finish({ pointerId }, true);
  }
  function relayout() { docks = preview ? [] : getDockGeometry(); schedule(true); }
  function dispose() {
    if (!alive) return;
    alive = false; gesture?.cancel(); if (frame) cancelAnimationFrame(frame);
    cleanups.forEach(fn => fn()); runtimes.delete(card); delete card._geometryController;
  }
  const runtime = { begin, relayout, dispose }; runtimes.set(card, runtime);
  if (!preview) {
    card._geometryController = runtime;
    const geometry = e => { if (["load", "drag", "relayout", "dock-size", "dock-content"].includes(e.detail?.reason)) relayout(); };
    window.addEventListener("unidtk-geometry-changed", geometry);
    cleanups.push(() => window.removeEventListener("unidtk-geometry-changed", geometry));
    const themed = () => schedule(true);
    window.addEventListener("unidtk-theme-changed", themed);
    cleanups.push(() => window.removeEventListener("unidtk-theme-changed", themed));
    const network = () => schedule(); window.addEventListener("online", network); window.addEventListener("offline", network);
    cleanups.push(() => { window.removeEventListener("online", network); window.removeEventListener("offline", network); });
    const subscribe = async (name, cmd, accept, revision) => {
      try {
        const unlisten = await listen(name, event => { if (valid()) accept(event?.payload || null); });
        if (!valid()) { unlisten(); return; } cleanups.push(unlisten);
      } catch { if (!valid()) return; }
      // Managed state can exist before its background store/feed is ready.
      for (const delay of [0, 250, 1000, 3000]) {
        if (delay) await new Promise(resolve => setTimeout(resolve, delay));
        if (!valid()) return;
        const seedRevision = revision();
        try {
          const value = await invoke(cmd);
          if (valid() && seedRevision === revision()) accept(value);
          return;
        } catch { if (!valid()) return; }
      }
    };
    subscribe("weather-update", "weather_current", v => { weatherRevision++; weather = v; schedule(); }, () => weatherRevision);
    subscribe("system-volume", "get_system_volume", v => { volumeRevision++; volume = v; schedule(); }, () => volumeRevision);
    const timer = setInterval(() => { if (!card.isConnected) dispose(); else if (!document.hidden) schedule(); }, 1000);
    cleanups.push(() => clearInterval(timer));
  }
  (document.fonts?.ready || Promise.resolve()).then(() => {
    if (valid()) { footprint = ""; needsLayout = true; paint(); }
    if (preview) dispose();
  });
  paint();
}
export function disposeMicrographics() { for (const runtime of [...runtimes.values()]) runtime.dispose(); }
