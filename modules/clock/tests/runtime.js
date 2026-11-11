(async()=>{
  const [core,clock,motion]=await Promise.all([
    import('http://tauri.localhost/unidtk-core.js'),
    import('http://tauri.localhost/modules/clock/widget.js'),
    import('http://tauri.localhost/motion.js')
  ]);
  const card=document.querySelector('.clock-widget');
  const source=card.querySelector(':scope > .clock-container');
  const position=core.config.positions[card.dataset.id];
  const savedVersion=Object.getOwnPropertyDescriptor(position,'version');
  const originalVersion=source.dataset.version;
  const savedScale=motion.getMotionScale();
  const checkList=[],results=[];
  const check=(name,ok)=>checkList.push({name,ok:!!ok});
  const frame=()=>new Promise(resolve=>requestAnimationFrame(resolve));
  const waitFor=async(condition)=>{
    const start=performance.now();
    while(!condition()){if(performance.now()-start>4000)throw new Error('Clock transition timeout');await frame();}
  };
  const open=async()=>{card.click();await waitFor(()=>core.surfaceManager.getRecord(card)?.state==='open');};
  const close=async()=>{core.surfaceManager.closeSurface(card);await waitFor(()=>!core.surfaceManager.getRecord(card)&&!card.classList.contains('expanded'));};
  const leftovers=()=>document.querySelectorAll('[data-flight-hidden]').length+document.querySelectorAll('.surface-flight-layer').length;
  try {
    if(core.surfaceManager.getRecord(card))await close();
    const deployed=await fetch('http://tauri.localhost/shared-flight.js').then(r=>r.text());
    check('installed code contains the flight engine',deployed.includes('class SharedFlight')&&!document.querySelector('.clock-morph-clone'));
    for(const version of ['rings','digital','analogue','grid-second','gooey','nothing-watch-pro','gothic']){
      position.version=version;source.dataset.version=version;clock.updateClockFace(source,version);await frame();
      await open();
      const samples=[];
      core.surfaceManager.closeSurface(card);
      const record=core.surfaceManager.getRecord(card);
      const planes=record.flight.planes.map(plane=>plane.element);
      const start=performance.now();
      while(core.surfaceManager.getRecord(card)){
        samples.push({t:performance.now()-start,sourceHidden:source.hasAttribute('data-flight-hidden'),
          planeOpacities:planes.map(el=>Number(getComputedStyle(el).opacity))});
        await frame();
      }
      await frame();
      check(`${version}: single opaque owner on closing frames`,planes.length>0&&samples.every(s=>s.sourceHidden&&s.planeOpacities.every(o=>o===1)));
      check(`${version}: source is visible and opaque after the handoff`,getComputedStyle(source).opacity==='1'&&getComputedStyle(source).visibility==='visible');
      check(`${version}: no fade or leftover plane`,source.getAnimations().length===0&&leftovers()===0);
      results.push({version,frames:samples.length,closeMs:Math.round(performance.now()-start)});
    }
    await open();
    core.surfaceManager.closeSurface(card);await frame();
    const reversing=core.surfaceManager.getRecord(card).flight.planes.map(plane=>plane.element);
    card.click();await waitFor(()=>core.surfaceManager.getRecord(card)?.state==='open');
    check('quick reopen finishes cleanly',reversing.every(el=>!el.isConnected)&&leftovers()===0);
    await close();
    motion.setMotionScale(0);
    await open();await close();
    check('zero motion installed lifecycle',leftovers()===0&&getComputedStyle(source).opacity==='1');
  } finally {
    if(core.surfaceManager.getRecord(card))await close();
    motion.setMotionScale(savedScale);
    if(savedVersion)Object.defineProperty(position,'version',savedVersion);else delete position.version;
    source.dataset.version=originalVersion;clock.updateClockFace(source,originalVersion);
    core.notifyGeometryChanged('clock-runtime-test-restore');
  }
  return {passed:checkList.filter(c=>c.ok).length,total:checkList.length,failed:checkList.filter(c=>!c.ok),results};
})()
