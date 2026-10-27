/** Read-only live fixture: same saved settings, isolated viewport, no gestures. */
import { renderMicrographics } from "../ui/widget.js";
import { config } from "../../../core/ui/unidtk-core.js";

export async function runLiveDiagnostics() {
  let checks = 0;
  const assert = (ok, message) => { checks++; if (!ok) throw new Error(message); };
  const pause = ms => new Promise(resolve => setTimeout(resolve, ms));
  const actual = document.querySelector("#widgetsCanvas .micrographics-card") || document.querySelector(".micrographics-card");
  assert(Boolean(actual), "live widget exists");
  const id = actual.dataset.id, saved = JSON.stringify(config.positions[id]);
  const host = document.createElement("div");
  Object.assign(host.style, { position: "fixed", left: "-10000px", top: "0", width: "220px", height: "160px" });
  const card = document.createElement("div"); card.className = "widget-card"; host.appendChild(card); document.body.appendChild(host);
  try {
    renderMicrographics(card, id);
    await pause(600);
    const grid = card.querySelector(".mg-grid"), atoms = [...grid.querySelectorAll(".mg-atom:not(.mg-recovery)")];
    assert(card.classList.contains("mg-overflow"), "tight viewport uses scroll");
    assert(atoms.length > 0 && atoms.every(e => !e.hidden), "no atom hidden in tight viewport");
    assert(new Set(atoms.map(e => { const s=getComputedStyle(e); return `${s.fontFamily}/${s.fontSize}/${s.lineHeight}`; })).size===1, "all atom text has one typography system");
    const chance=card.querySelector(".mg-chance");
    if(chance){const ring=getComputedStyle(chance,"::before");assert(ring.width===ring.height,"precipitation graphic is circular");assert(parseFloat(chance.style.getPropertyValue("--mg-chance"))===parseFloat(chance.textContent),"precipitation graphic matches percentage");}
    assert(!card.querySelector('[data-glyph="sunrise"]') || grid.getAttribute("aria-label").includes("Восход"), "graphic sunrise retains accessible meaning");
    assert(getComputedStyle(grid).pointerEvents === "auto", "scroll surface accepts normal input");
    assert(grid.tabIndex === 0, "scroll surface is keyboard reachable");
    const corner = card.querySelector(".resize-handle.corner");
    assert(parseFloat(corner.style.top) >= 0 && parseFloat(corner.style.top) < card.clientHeight &&
      parseFloat(corner.style.left) >= 0 && parseFloat(corner.style.left) < card.clientWidth, "scale control stays in visible scroll viewport");
    assert(grid.scrollHeight > grid.clientHeight, "scroll contains all content");
    const last = atoms.reduce((a, b) => a.offsetTop + a.offsetHeight > b.offsetTop + b.offsetHeight ? a : b);
    grid.scrollTop = grid.scrollHeight;
    await pause(40);
    assert(last.getBoundingClientRect().bottom <= grid.getBoundingClientRect().bottom + 2, "last value reachable");
    const offset = grid.scrollTop;
    card._geometryController.relayout();
    await pause(40);
    assert(atoms.every(e => e.isConnected && card.contains(e)), "relayout reuses DOM nodes");
    assert(Math.abs(grid.scrollTop - offset) <= 1, "relayout preserves reading position");
    assert(JSON.stringify(config.positions[id]) === saved, "fixture leaves user settings untouched");
    return { ok: true, checks, atoms: atoms.length, scrollHeight: grid.scrollHeight, clientHeight: grid.clientHeight };
  } finally {
    card._geometryController?.dispose(); host.remove();
  }
}
