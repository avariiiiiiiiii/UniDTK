// Real WebView layout/WAAPI regression checks of the clock's container transform; no saved
// user settings are changed. The fixture card goes through the real surface manager and the
// real shared-flight engine, so these checks cover the same code the desk uses.
const checks = [];
const check = (name, ok) => checks.push({ name, ok: !!ok });
const frame = () => new Promise(resolve => requestAnimationFrame(resolve));
const sameRect = (a, b) => ["left", "top", "width", "height"].every(key => Math.abs(a[key] - b[key]) < 0.2);
const versions = ["rings", "digital", "analogue", "grid-second", "gooey", "nothing-watch-pro", "gothic"];
const fixtures = [];
const sm = deps.sm;
const layerOf = card => card.querySelector(".surface-flight-layer");
const leftovers = () => document.querySelectorAll("[data-flight-hidden]").length + document.querySelectorAll(".surface-flight-layer").length;

function makeCard(version, scale) {
  const card = document.createElement("div");
  card.className = "widget-card clock-widget";
  card.style.cssText = `position:fixed;left:30px;top:250px;width:620px;height:300px;transform:scale(${scale});transform-origin:top left;pointer-events:none;z-index:2600`;
  card.innerHTML = `<div class="clock-container" data-version="${version}" style="flex:none;width:210px;height:190px"></div><div class="widget-body"></div>`;
  document.body.appendChild(card);
  fixtures.push(card);
  const source = card.firstElementChild;
  const body = card.lastElementChild;
  deps.clock.updateClockFace(source, version);
  body.innerHTML = `<div class="clock-container dash-clock" data-version="${version}"></div>`;
  const target = body.firstElementChild;
  deps.clock.updateClockFace(target, version);
  target.style.cssText = "position:absolute;left:380px;top:40px;width:180px;height:180px;display:flex";
  return { card, source, body, target };
}

// What the desk does: freeze the tile, then restyle the card, then open the surface.
function openCard(card, body) {
  const snapshot = sm.captureOrigin(card, { shared: deps.clock.clockSharedElements(card, body) });
  card.classList.add("expanded");
  const opened = sm.openSurface(card, {
    snapshot, morph: "container", suppressContentAnimation: true,
    closeOnEscape: false, closeOnOutsideClick: false, duration: 60,
    onClosed: () => card.classList.remove("expanded"),
  });
  return { snapshot, opened, record: sm.getRecord(card) };
}

try {
  for (const scale of [1, 1.5]) for (const version of versions) {
    const { card, source, body, target } = makeCard(version, scale);
    await frame();

    const start = performance.now();
    const first = openCard(card, body);
    check(`${version}@${scale}: flight setup <100ms`, performance.now() - start < 100);
    const planes = first.record.flight.planes.map(plane => plane.element);
    check(`${version}@${scale}: one plane per face`, planes.length === first.snapshot.shared.length && planes.length > 0);
    check(`${version}@${scale}: both endpoints hidden`, getComputedStyle(source).visibility === "hidden" && getComputedStyle(target).visibility === "hidden");
    await first.opened;
    check(`${version}@${scale}: open hands over cleanly`, leftovers() === 0 && getComputedStyle(target).visibility === "visible");

    // Close: only transforms move the planes, they stay opaque, and they end on the dock's ink.
    const closing = sm.getRecord(card);
    sm.closeSurface(card);
    const closePlanes = closing.flight.planes.map(plane => plane.element);
    check(`${version}@${scale}: only transform tracks`, closePlanes.every(el => el.getAnimations().every(a => a.effect.getKeyframes().every(key => !("opacity" in key)))));
    const opacities = [];
    while (sm.getRecord(card)) {
      for (const el of closePlanes) opacities.push(getComputedStyle(el).opacity);
      await frame();
    }
    check(`${version}@${scale}: plane opacity stays 1`, opacities.length > 0 && opacities.every(value => value === "1"));
    await frame();
    check(`${version}@${scale}: atomic opaque handoff`, leftovers() === 0 && getComputedStyle(source).opacity === "1" && getComputedStyle(source).visibility === "visible");
    check(`${version}@${scale}: no second CSS fade`, source.getAnimations().length === 0);

    // The last frame of a close puts every plane on the dock's ink, exactly.
    const exact = openCard(card, body);
    await exact.opened;
    sm.closeSurface(card);
    sm.getRecord(card).motionGroup.finish();
    check(`${version}@${scale}: exact dock ink geometry`, exact.record.flight.planes.every((plane, index) =>
      sameRect(plane.element.getBoundingClientRect(), exact.snapshot.shared[index].rect)));
    while (sm.getRecord(card)) await frame();

    // Reverse a live close, then let it settle: the same planes carry on.
    const again = openCard(card, body);
    sm.closeSurface(card);
    const reused = again.record.flight.planes.map(plane => plane.element);
    sm.openSurface(card);
    check(`${version}@${scale}: reversal reuses the planes`, again.record.flight.planes.every((plane, index) => plane.element === reused[index]));
    await again.opened;
    await new Promise(resolve => setTimeout(resolve, 120));
    check(`${version}@${scale}: reversal settles clean`, leftovers() === 0 && sm.getRecord(card)?.state === "open");
    sm.closeSurface(card);
    while (sm.getRecord(card)) await frame();

    // Unmount in flight: nothing may be left on the stack or on screen.
    openCard(card, body);
    card.remove();
    sm._cleanupDeadNodes();
    check(`${version}@${scale}: unmount frees the surface`, !sm.activeStack.some(record => record.element === card) && leftovers() === 0);
  }

  // Dynamic SVG ink must not inherit old CSS path data after a minute change.
  {
    const { card, source, body } = makeCard("gooey", 1);
    const { updateGooeyClock } = await import("http://tauri.localhost/modules/clock/gooey.js");
    updateGooeyClock(source.firstElementChild, new Date(2026, 9, 4, 11, 11));
    // A minute rollover starts from settled old ink. Finish its entry effect so
    // unchanged slots reflect that state instead of an artificial fresh render.
    source.querySelectorAll(".gooey-clock-ink").forEach(ink => ink.getAnimations().forEach(animation => animation.finish()));
    const flight = openCard(card, body);
    const layer = layerOf(card);
    const paths = layer.querySelectorAll(".gooey-clock-slot path");
    check("Gooey minute change preserves new ink", paths.length === 4 && Array.from(paths).every(path => !path.style.getPropertyValue("d")));
    check("Gooey plane retains updater context", !!layer.querySelector(".gooey-clock-face"));
    const nextMinute = new Date(2026, 9, 4, 12, 22);
    updateGooeyClock(source.firstElementChild, nextMinute);
    updateGooeyClock(layer.querySelector(".gooey-clock-face"), nextMinute);
    await frame();
    const sourceInk = Array.from(source.querySelectorAll(".gooey-clock-ink"));
    const planeInk = Array.from(layer.querySelectorAll(".gooey-clock-ink"));
    const inkGeometry = ink => JSON.stringify(Array.from(ink.querySelectorAll("path,circle")).map(node =>
      [node.localName, ...["d", "cx", "cy", "r"].map(attribute => node.getAttribute(attribute))]));
    check("Gooey rollover keeps live ink and opacity in sync", sourceInk.length === planeInk.length && sourceInk.every((ink, index) =>
      inkGeometry(ink) === inkGeometry(planeInk[index]) && Math.abs(Number(getComputedStyle(ink).opacity) - Number(getComputedStyle(planeInk[index]).opacity)) < 0.01));
    await flight.opened;
    sm.closeSurface(card);
    while (sm.getRecord(card)) await frame();

    const oldScale = deps.motion.getMotionScale();
    try {
      deps.motion.setMotionScale(0);
      const instant = openCard(card, body);
      await instant.opened;
      sm.closeSurface(card);
      while (sm.getRecord(card)) await frame();
      check("zero motion settles and frees the planes", leftovers() === 0 && sm.activeStack.every(record => record.element !== card));
    } finally { deps.motion.setMotionScale(oldScale); }
  }
} finally {
  for (const card of fixtures) {
    if (sm.getRecord(card)) sm.closeSurface(card);
    card.remove();
  }
  sm._cleanupDeadNodes();
}
return { passed: checks.filter(check => check.ok).length, total: checks.length, failed: checks.filter(check => !check.ok) };
