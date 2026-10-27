/** Run through `udtk gui js` in the real WebView; no external JS runtime. */
import { buildTokens, routeElements, STYLES, PILL_PAD } from "../ui/composition.js";

export function runCompositionDiagnostics() {
  let checks = 0, worstMs = 0;
  const assert = (ok, message) => { checks++; if (!ok) throw new Error(message); };
  const now = new Date("2026-10-03T12:00:00Z");
  const weather = { weatherAvailable: true, tempC: 13, feelsC: 13, windKmh: 0,
    code: 3, isDay: true, observedAt: now.getTime() / 1000, locationStatus: "available" };
  for (const style of STYLES) {
    const tokens = buildTokens(now, { weather, style });
    assert(tokens.some(t => t.id === "weather:feels" && t.v === "+13°C"), "equal feels must remain");
    assert(tokens.some(t => t.id === "weather:wind" && t.v === "0 км/ч"), "known calm must remain");
    assert(!tokens.some(t => t.v === "WINDOWS"), "technical Windows label must not occupy composition");
    const unknown = buildTokens(now, { weather: { ...weather, feelsC: null, windKmh: null, code: null, isDay: null }, style });
    assert(unknown.some(t => t.id === "weather:feels" && t.v === "НЕТ ДАННЫХ"), "unknown feels stays visible");
    assert(unknown.some(t => t.id === "weather:wind" && t.v === "НЕТ ДАННЫХ"), "unknown wind stays visible");
    assert(!unknown.some(t => ["clear", "moon"].includes(t.g)), "unknown code cannot claim clear weather");
    const stale = buildTokens(now, { weather: { ...weather, observedAt: now.getTime() / 1000 - 1800 }, style });
    assert(stale.some(t => t.id === "weather:stale"), "stale warning required");
    const suppressed = buildTokens(now, { weather, capabilities: ["date", "time", "system-volume"], volume: { level: 50 }, style });
    assert(!suppressed.some(t => ["date", "time", "volume"].includes(t.group)), "capability contract preserved");
  }
  let seed = 7241;
  const random = () => { seed = (seed * 1664525 + 1013904223) >>> 0; return seed / 4294967296; };
  const overlap = (a, b) => a.x < b.x + b.w - .01 && a.x + a.w > b.x + .01 && a.y < b.y + b.h - .01 && a.y + a.h > b.y + .01;
  for (let trial = 0; trial < 350; trial++) {
    const viewport = { w: 160 + random() * 1600, h: 100 + random() * 900 };
    const style = STYLES[trial % STYLES.length], scale = .55 + random() * 2.45;
    const tokens = buildTokens(now, { weather: { ...weather, code: 2 }, style });
    const elements = tokens.map(t => ({ ...t, w: Math.min(viewport.w - 16, (t.v?.length || 2) * 12 * scale + 8), h: (t.secondary ? 25 : 38) * scale }));
    const docks = Array.from({ length: trial % 12 }, () => ({ x: random() * viewport.w, y: random() * viewport.h, w: 30 + random() * 300, h: 30 + random() * 200 }));
    const start = performance.now();
    const placed = routeElements(elements, { x: random() * viewport.w * 1.5, y: random() * viewport.h * 1.5, scale, style }, viewport, docks);
    worstMs = Math.max(worstMs, performance.now() - start);
    assert(placed.length === elements.length, `all values retained ${trial}`);
    assert(new Set(placed.map(e => e.id)).size === elements.length, `unique atoms ${trial}`);
    for (const e of placed) {
      assert(Number.isFinite(e.x) && Number.isFinite(e.y), `finite position ${trial}`);
      for (const other of placed) if (e !== other) assert(!overlap(e, other), `no atom overlap ${trial}`);
      if (!placed.layout.overflow && !placed.layout.overlay) {
        assert(e.x >= 8 && e.y >= -8 && e.x + e.w <= viewport.w - 8 + .01 && e.y + e.h <= viewport.h - 8 + .01, `viewport ${trial}`);
        for (const dock of docks) assert(!overlap(e, dock), `dock avoided ${trial}`);
      }
    }
    if (placed.layout.overflow && !placed.layout.overlay) for (const dock of docks) assert(!overlap(placed.layout, dock), `scroll viewport avoids docks ${trial}`);
  }
  const obstacle = {x:180,y:30,w:160,h:160};
  const flowing = routeElements(Array.from({length:24}, (_,i)=>({id:`flow:${i}`,group:`g:${i}`,w:70,h:40})),
    {x:8,y:36,scale:1}, {w:540,h:420}, [obstacle]);
  assert(!flowing.layout.overflow, "flow fits around central dock");
  assert(flowing.some(e=>e.x+e.w<obstacle.x && e.y<190), "flow uses left interval");
  assert(flowing.some(e=>e.x>obstacle.x+obstacle.w && e.y<190), "flow uses right interval");
  assert(flowing.some(e=>e.y>obstacle.y+obstacle.h), "flow continues below dock");
  for (const e of flowing) assert(!overlap(e,obstacle), "flow avoids central dock");
  const place = buildTokens(now,{weather:{...weather,locationSource:"windows",locationNameStatus:"available",location:"Москва",weatherAvailable:false}});
  assert(place.some(t=>t.id==="geo:place" && t.v==="Москва"), "place independent of weather");
  const packed = routeElements([{id:"date",group:"date",w:130,h:35},{id:"temperature",group:"temperature",w:180,h:40},{id:"wind",group:"wind",w:90,h:25}],
    {x:8,y:8,scale:1},{w:600,h:300},[{x:260,y:0,w:120,h:180}]);
  assert(!packed.layout.overflow && packed.find(e=>e.id==="wind").x<253, "later group backfills left gap");
  assert(packed.find(e=>e.id==="wind").y<48,"backfill stays in first row");
  assert(packed.layout.y===0,"top aligns with neighboring dock");
  const aligned=routeElements([{id:"date",group:"date",w:130,h:35},{id:"ip:symbol",group:"ip",w:25,h:25},{id:"ip:network",group:"ip",w:90,h:25},{id:"ip:source",group:"ip",w:260,h:25}],
    {x:8,y:8,scale:1},{w:900,h:300},[{x:370,y:0,w:64,h:64},{x:513,y:-3,w:210,h:190}]);
  assert(aligned.layout.y===-3,"top follows clock at negative local position");
  assert(Math.abs(Math.max(...aligned.filter(e=>e.y<32&&e.x<370).map(e=>e.x+e.w))-363)<.01,"left row closes gap to dock");
  assert(aligned.find(e=>e.id==="ip:symbol").y===aligned.find(e=>e.id==="ip:network").y,"network symbol retains first caption");
  const estimate=buildTokens(now,{weather:{...weather,locationSource:"windows",locationMethod:"ip",accuracyM:1500,locationNameStatus:"available",location:"Санкт-Петербург"}});
  assert(estimate.some(t=>t.id==="geo:place"&&t.v==="≈ Санкт-Петербург"),"Windows network estimate visibly approximate");
  const osm=buildTokens(now,{weather:{...weather,locationSource:"windows",locationMethod:"ip",accuracyM:1500,locationNameStatus:"available",location:"Санкт-Петербург",locationNameProvider:"openstreetmap"}});
  assert(osm.find(t=>t.id==="geo:place").title.includes("OpenStreetMap"),"the provider that named the place is named");
  assert(osm.find(t=>t.id==="geo:place").title.includes("© OpenStreetMap contributors"),"openstreetmap attribution is shown");
  const bdc=buildTokens(now,{weather:{...weather,locationSource:"windows",locationMethod:"ip",accuracyM:1500,locationNameStatus:"available",location:"Санкт-Петербург",locationNameProvider:"bigdatacloud"}});
  assert(bdc.find(t=>t.id==="geo:place").title.includes("BigDataCloud"),"bigdatacloud attribution is shown");
  assert(!bdc.find(t=>t.id==="geo:place").title.includes("OpenStreetMap"),"attribution never mixes providers");
  const manual=buildTokens(now,{weather:{...weather,locationSource:"windows",locationMethod:"default",locationNameStatus:"available",location:"Дом",locationNameProvider:"windows"}});
  assert(!manual.find(t=>t.id==="geo:place").title.includes("Название:"),"a place Windows provides needs no geocoder attribution");
  assert(estimate.find(t=>t.id==="weather:temperature").title.includes("по сети"),"weather describes actual positioning method");
  assert(!estimate.some(t=>/ОЩУЩАЕТСЯ|ВОСХОД|ЗАКАТ|ОЦЕНКА WINDOWS|ПО IP|СЕТЬ/.test(t.v||"")),"repeated prose replaced with symbols");
  assert(estimate.some(t=>t.id==="weather:feels-label"&&t.g==="feels"),"feels keeps graphic meaning");
  const between=routeElements([{id:"left",group:"left",w:130,h:35},{id:"right",group:"right",w:100,h:35}],
    {x:8,y:8,scale:1},{w:700,h:260},[{x:150,y:0,w:100,h:200},{x:500,y:0,w:100,h:200}]);
  assert(between.find(e=>e.id==="right").x===257,"between docks attaches to left dock instead of far right edge");
  const repeat = routeElements([{id:"date",group:"date",w:130,h:35},{id:"temperature",group:"temperature",w:180,h:40},{id:"wind",group:"wind",w:90,h:25}],
    {x:8,y:8,scale:1},{w:600,h:300},[{x:260,y:0,w:120,h:180}]);
  assert(JSON.stringify(packed)===JSON.stringify(repeat),"backfill deterministic");
  const tallDock={x:0,y:45,w:210,h:100};
  const tall=routeElements([{id:"short",group:"short",w:100,h:20},{id:"tall",group:"tall",w:130,h:65}],
    {x:8,y:8,scale:1},{w:500,h:300},[tallDock]);
  assert(tall.every(e=>!overlap(e,tallDock)),"late tall group does not push earlier atom into dock");
  const nextEnd=Math.floor(now.getTime()/3600000)*3600+3600;
  const rich={...weather,timezone:"Europe/Moscow",sunriseAt:Date.parse("2026-10-03T04:00:00Z")/1000,sunsetAt:Date.parse("2026-10-03T15:00:00Z")/1000,daylightSeconds:39600,
    precipitationForecast:[1,2,3].map((_,i)=>({at:nextEnd+i*3600,probability:0,amountMm:0}))};
  const richTokens=buildTokens(now,{weather:rich});
  assert(richTokens.some(t=>t.id==="forecast:chance"&&t.v==="0%"),"known zero precipitation stays zero");
  assert(richTokens.some(t=>t.id==="sun:rise"&&t.v==="07:00"),"sunrise uses place timezone");
  assert(richTokens.some(t=>t.id==="sun:duration"&&t.v==="11ч 0м"),"day duration represented");
  const expired=buildTokens(new Date(now.getTime()+86400000),{weather:rich});
  assert(expired.some(t=>t.id==="forecast:chance"&&t.v==="НЕТ ДАННЫХ"),"expired forecast never looks upcoming");
  assert(expired.some(t=>t.id==="sun:rise"&&t.v==="НЕТ ДАННЫХ"),"yesterday sunrise never looks today");
  const buffered={...rich,precipitationForecast:[0,1,2,3].map(i=>({at:nextEnd+i*3600,probability:10+i,amountMm:0}))};
  const afterBoundary=buildTokens(new Date(nextEnd*1000+1000),{weather:buffered});
  assert(afterBoundary.some(t=>t.id==="forecast:chance"&&t.v==="13%"),"reserve hour keeps next three windows after hour boundary");
  const beforeBoundary=buildTokens(new Date(nextEnd*1000-1000),{weather:buffered});
  assert(beforeBoundary.some(t=>t.id==="forecast:chance"&&t.v==="12%"),"reserve hour is not included before boundary");
  const unknownExtra=buildTokens(now,{weather:{...rich,timezone:"invalid/zone",precipitationForecast:[{at:nextEnd,probability:null,amountMm:null}],sunriseAt:null,sunsetAt:null}});
  assert(unknownExtra.some(t=>t.id==="forecast:chance"&&t.v==="НЕТ ДАННЫХ"),"partial unknown forecast not dry");
  assert(unknownExtra.some(t=>t.id==="sun:rise"&&t.v==="НЕТ ДАННЫХ"),"invalid zone/polar no invented sunrise");
  const crowded = routeElements([{ id: "wind", group: "wind", w: 180, h: 120 }, { id: "feels", group: "feels", w: 180, h: 120 }],
    { x: 8, y: 8, scale: 3 }, { w: 220, h: 140 }, [{ x: 0, y: 0, w: 220, h: 140 }]);
  assert(crowded.length === 2 && crowded.layout.overflow && crowded.layout.overlay, "fully occupied screen has full scroll fallback");
  return { ok: true, checks, layouts: 351, worstMs: Math.round(worstMs * 100) / 100 };
}


/** Jelly pills: a group's pill (atoms grown by PILL_PAD) never touches another group's pill. */
export function runPillDiagnostics() {
  let checks = 0;
  const assert = (ok, message) => { checks++; if (!ok) throw new Error(message); };
  const now = new Date("2026-10-03T12:00:00Z");
  const weather = { weatherAvailable: true, tempC: 13, feelsC: 13, windKmh: 3, code: 3, isDay: true,
    observedAt: now.getTime() / 1000, locationStatus: "available", location: "Рим", locationSource: "windows",
    ipLocation: { city: "Milan", resolvedAt: now.getTime() / 1000 } };
  const { x: px, y: py } = PILL_PAD;
  for (let trial = 0; trial < 200; trial++) {
    const tokens = buildTokens(now, { weather, style: "mechanical" });
    const elements = tokens.map((t, i) => ({ id: t.id, group: t.group, breakBefore: t.breakBefore, w: 30 + ((i * 37 + trial * 11) % 90), h: 33 }));
    const placed = routeElements(elements, { x: 8, y: 0, scale: 1, style: "mechanical", pills: true },
      { w: 500 + trial * 6, h: 400 }, [{ x: 300, y: 0, w: 120, h: 120 }]);
    assert(placed.length === elements.length, "pills keep every atom");
    if (placed.layout.overflow) continue;
    for (const a of placed) for (const b of placed) {
      if (a === b || a.group === b.group) continue;
      const touch = a.x - px < b.x + b.w + px - .01 && a.x + a.w + px > b.x - px + .01 &&
        a.y - py < b.y + b.h + py - .01 && a.y + a.h + py > b.y - py + .01;
      assert(!touch, `pill of ${a.id} overlaps pill of ${b.id}`);
    }
  }
  return { checks };
}
