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
  const faces=container=>Array.from((container.querySelector(':scope > .gooey-clock-face')||container).children);
  const rect=r=>({left:r.left,top:r.top,width:r.width,height:r.height});
  try {
    if(core.surfaceManager.getRecord(card))await close();
    const deployed=await fetch('http://tauri.localhost/unidtk-core.js').then(r=>r.text());
    check('installed code contains plane fix',deployed.includes('function clockMorphLayoutNodes(')&&!deployed.includes('timeline.add(source, [{ opacity: 0 }, { opacity: 1 }]'));
    for(const version of ['rings','digital','analogue','grid-second','gooey','nothing-watch-pro','gothic']){
      position.version=version;source.dataset.version=version;clock.updateClockFace(source,version);await frame();
      await open();
      const samples=[],handoffs=[];
      core.surfaceManager.closeSurface(card);
      const clone=card._clockMorph.clone;
      const planes=card._clockMorph.planes;
      clone.remove=function(){
        const sourceFaces=faces(source);
        handoffs.push({sourceOpacity:getComputedStyle(source).opacity,sourceVisibility:getComputedStyle(source).visibility,
          rects:planes.map((plane,index)=>({clone:rect(plane.element.getBoundingClientRect()),source:rect(sourceFaces[index].getBoundingClientRect())}))});
        Element.prototype.remove.call(this);
      };
      const start=performance.now();
      while(core.surfaceManager.getRecord(card)){
        const rendered=card._clockMorph?.clone?.isConnected;
        samples.push({t:performance.now()-start,sourceOpacity:getComputedStyle(source).opacity,sourceVisibility:getComputedStyle(source).visibility,
          clone:!!rendered,planeOpacities:rendered?planes.map(p=>Number(getComputedStyle(p.element).opacity)):[]});
        await frame();
      }
      await frame();
      check(`${version}: single opaque owner on closing frames`,samples.every(s=>s.clone&&s.sourceVisibility==='hidden'&&s.planeOpacities.every(o=>o===1)));
      check(`${version}: handoff opacity is 1`,handoffs.length===1&&handoffs[0].sourceOpacity==='1'&&handoffs[0].sourceVisibility==='visible');
      const geometryError=handoffs.length?Math.max(...handoffs[0].rects.flatMap(pair=>['left','top','width','height'].map(k=>Math.abs(pair.clone[k]-pair.source[k])))):Infinity;
      check(`${version}: matching ink at handoff`,geometryError<0.2);
      check(`${version}: no fade or leftover clone`,getComputedStyle(source).opacity==='1'&&source.getAnimations().length===0&&!document.querySelector('.clock-morph-clone'));
      results.push({version,frames:samples.length,closeMs:Math.round(performance.now()-start),geometryError});
    }
    await open();
    core.surfaceManager.closeSurface(card);await frame();
    const reversing=card._clockMorph.clone;
    card.click();await waitFor(()=>core.surfaceManager.getRecord(card)?.state==='open');
    check('quick reopen finishes cleanly',!reversing.isConnected&&!document.querySelector('.clock-morph-clone'));
    await close();
    motion.setMotionScale(0);
    await open();await close();
    check('zero motion installed lifecycle',!card._clockMorph&&getComputedStyle(source).opacity==='1');
  } finally {
    if(core.surfaceManager.getRecord(card))await close();
    motion.setMotionScale(savedScale);
    if(savedVersion)Object.defineProperty(position,'version',savedVersion);else delete position.version;
    source.dataset.version=originalVersion;clock.updateClockFace(source,originalVersion);
    core.notifyGeometryChanged('clock-runtime-test-restore');
  }
  return {passed:checkList.filter(c=>c.ok).length,total:checkList.length,failed:checkList.filter(c=>!c.ok),results};
})()
