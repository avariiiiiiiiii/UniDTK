// Real WebView layout/WAAPI regression checks; no saved user settings are changed.
const checks = [];
const check = (name, ok) => checks.push({ name, ok: !!ok });
const frame = () => new Promise(resolve => requestAnimationFrame(resolve));
const sameRect = (a, b) => ["left", "top", "width", "height"].every(key => Math.abs(a[key] - b[key]) < 0.15);
const versions = ["rings", "digital", "analogue", "grid-second", "gooey", "nothing-watch-pro", "gothic"];
const fixtures = [];
try {
  for (const scale of [1, 1.5]) for (const version of versions) {
    const card = document.createElement("div");
    card.className = "widget-card clock-widget";
    card.style.cssText = `position:fixed;left:30px;top:250px;width:210px;height:190px;transform:scale(${scale});transform-origin:top left;pointer-events:none;z-index:2600`;
    card.innerHTML = `<div class="clock-container" data-version="${version}"></div><div class="widget-body"></div>`;
    document.body.appendChild(card);
    fixtures.push(card);
    const source = card.firstElementChild;
    const body = card.lastElementChild;
    deps.clock.updateClockFace(source, version);
    await frame();
    const seed = {source, originRect:rectSnapshot(source.getBoundingClientRect()),sourceOpacity:saveInlineProperty(source,"opacity"),faces:captureClockMorphFaces(source)};
    body.innerHTML = `<div class="clock-container dash-clock" data-version="${version}"></div>`;
    const target = body.firstElementChild;
    deps.clock.updateClockFace(target, version);
    target.style.cssText = "position:fixed;left:500px;top:80px;width:180px;height:180px;display:flex";
    card.classList.add("expanded");
    const start = performance.now();
    animateClockMorph(card, body, true, 40, seed);
    check(`${version}@${scale}: clone setup <100ms`, performance.now() - start < 100);
    const state = card._clockMorph;
    check(`${version}@${scale}: both endpoints hidden`, getComputedStyle(source).visibility === "hidden" && getComputedStyle(target).visibility === "hidden");
    state.timeline.finish();
    await state.timeline.finished;
    finishClockMorph(card);
    animateClockMorph(card, body, false, 100);
    const closing = state.timeline;
    check(`${version}@${scale}: only transform tracks`, closing.tracks.every(track => track.keyframes.every(key => !("opacity" in key))));
    closing.finish();
    await closing.finished;
    check(`${version}@${scale}: exact dock ink geometry`, state.planes.every((plane,index) => sameRect(plane.element.getBoundingClientRect(),seed.faces[index].rect)));
    check(`${version}@${scale}: plane opacity stays 1`, state.planes.every(plane => getComputedStyle(plane.element).opacity === "1"));
    card.classList.remove("expanded");
    finishClockMorph(card,true);
    check(`${version}@${scale}: atomic opaque handoff`, !card._clockMorph && !state.clone.isConnected && getComputedStyle(source).opacity === "1" && getComputedStyle(source).visibility === "visible");
    await frame();
    check(`${version}@${scale}: no second CSS fade`, getComputedStyle(source).opacity === "1" && source.getAnimations().length === 0);

    // Reverse a live close, including a dashboard node replacement.
    card.classList.add("expanded");
    animateClockMorph(card,body,true,100,seed);
    animateClockMorph(card,body,false,100);
    const superseded = card._clockMorph.timeline;
    const liveClone = card._clockMorph.clone;
    animateClockMorph(card,body,true,100);
    check(`${version}@${scale}: reversal reuses one clone`, card._clockMorph.clone === liveClone && superseded.state === "cancelled");
    const reopened = card._clockMorph.timeline;
    reopened.finish();
    await reopened.finished;
    finishClockMorph(card);
    animateClockMorph(card,body,false,100);
    card.remove();
    finishClockMorph(card,true);
    check(`${version}@${scale}: unmount removes plane`, !liveClone.isConnected && !card._clockMorph);
  }
  // Dynamic SVG ink must not inherit old CSS path data after a minute change.
  const card = document.createElement("div");
  card.className = "widget-card clock-widget";
  card.style.cssText = "position:fixed;left:30px;top:250px;width:210px;height:190px";
  card.innerHTML = '<div class="clock-container"></div><div class="widget-body"><div class="clock-container dash-clock"></div></div>';
  document.body.appendChild(card);fixtures.push(card);
  const source = card.firstElementChild, target = card.lastElementChild.firstElementChild;
  deps.clock.updateClockFace(source,"gooey");deps.clock.updateClockFace(target,"gooey");
  const seed={source,originRect:rectSnapshot(source.getBoundingClientRect()),sourceOpacity:saveInlineProperty(source,"opacity"),faces:captureClockMorphFaces(source)};
  const {updateGooeyClock}=await import('http://tauri.localhost/modules/clock/gooey.js');
  updateGooeyClock(source.firstElementChild,new Date(2026,9,4,11,11));
  // A minute rollover starts from settled old ink. Finish its entry effect so
  // unchanged slots reflect that state instead of an artificial fresh render.
  source.querySelectorAll('.gooey-clock-ink').forEach(ink=>ink.getAnimations().forEach(animation=>animation.finish()));
  animateClockMorph(card,card.lastElementChild,true,100,seed);
  const paths=card._clockMorph.clone.querySelectorAll('.gooey-clock-slot path');
  check('Gooey minute change preserves new ink', paths.length===4 && Array.from(paths).every(path=>!path.style.getPropertyValue('d')));
  check('Gooey clone retains updater context', !!card._clockMorph.clone.querySelector('.gooey-clock-face'));
  const nextMinute=new Date(2026,9,4,12,22);
  updateGooeyClock(source.firstElementChild,nextMinute);
  updateGooeyClock(card._clockMorph.clone.querySelector('.gooey-clock-face'),nextMinute);
  await frame();
  const sourceInk=Array.from(source.querySelectorAll('.gooey-clock-ink'));
  const cloneInk=Array.from(card._clockMorph.clone.querySelectorAll('.gooey-clock-ink'));
  const inkGeometry=ink=>JSON.stringify(Array.from(ink.querySelectorAll('path,circle')).map(node=>
    [node.localName,...['d','cx','cy','r'].map(attribute=>node.getAttribute(attribute))]));
  check('Gooey rollover keeps live ink and opacity in sync', sourceInk.length===cloneInk.length && sourceInk.every((ink,index)=>
    inkGeometry(ink)===inkGeometry(cloneInk[index]) && Math.abs(Number(getComputedStyle(ink).opacity)-Number(getComputedStyle(cloneInk[index]).opacity))<0.01));
  finishClockMorph(card,true);
  const oldScale=deps.motion.getMotionScale();
  try {
    deps.motion.setMotionScale(0);
    animateClockMorph(card,card.lastElementChild,true,100,seed);
    await card._clockMorph.timeline.finished;
    finishClockMorph(card);
    animateClockMorph(card,card.lastElementChild,false,100);
    await card._clockMorph.timeline.finished;
    finishClockMorph(card,true);
    check('zero motion settles and removes clone', !card._clockMorph);
  } finally {deps.motion.setMotionScale(oldScale);}
} finally {
  for(const card of fixtures) {finishClockMorph(card,true);card.remove();}
}
return {passed:checks.filter(check=>check.ok).length,total:checks.length,failed:checks.filter(check=>!check.ok)};
