/** Real data → stable, individually routed pieces of mechanical typography. */
export const STYLES = ["mechanical", "outline", "symbols"];
export const STYLE_NAMES = { mechanical: "Механика", outline: "Контуры", symbols: "Символы" };
const MONTHS = ["ЯНВ", "ФЕВ", "МАР", "АПР", "МАЙ", "ИЮН", "ИЮЛ", "АВГ", "СЕН", "ОКТ", "НОЯ", "ДЕК"];
const DAYS = ["ВС", "ПН", "ВТ", "СР", "ЧТ", "ПТ", "СБ"];
const WEATHER = {
  0: ["ЯСНО", "clear"], 1: ["МАЛООБЛАЧНО", "partly"],
  2: ["ПЕРЕМЕННАЯ ОБЛАЧНОСТЬ", "partly"], 3: ["ПАСМУРНО", "cloud"],
  45: ["ТУМАН", "fog"], 48: ["ИЗМОРОЗЬ", "fog"],
  51: ["МОРОСЬ", "drizzle"], 53: ["МОРОСЬ", "drizzle"], 55: ["СИЛЬНАЯ МОРОСЬ", "drizzle"],
  56: ["ЛЕДЯНАЯ МОРОСЬ", "sleet"], 57: ["ЛЕДЯНАЯ МОРОСЬ", "sleet"],
  61: ["ДОЖДЬ", "rain"], 63: ["ДОЖДЬ", "rain"], 65: ["СИЛЬНЫЙ ДОЖДЬ", "rain"],
  66: ["ЛЕДЯНОЙ ДОЖДЬ", "sleet"], 67: ["ЛЕДЯНОЙ ДОЖДЬ", "sleet"],
  71: ["СНЕГ", "snow"], 73: ["СНЕГ", "snow"], 75: ["СИЛЬНЫЙ СНЕГ", "snow"],
  77: ["СНЕЖНЫЕ ЗЁРНА", "sleet"], 80: ["ЛИВЕНЬ", "rain"],
  81: ["ЛИВЕНЬ", "rain"], 82: ["СИЛЬНЫЙ ЛИВЕНЬ", "rain"],
  85: ["СНЕЖНЫЙ ЗАРЯД", "snow"], 86: ["СНЕЖНЫЙ ЗАРЯД", "snow"],
  95: ["ГРОЗА", "thunder"], 96: ["ГРОЗА С ГРАДОМ", "thunder"], 99: ["ГРОЗА С ГРАДОМ", "thunder"],
};
const signed = value => `${value > 0 ? "+" : ""}${value}`;

/** Who named the place, said the way the panel must say it. */
const NAME_SOURCES = {
  bigdatacloud: "Название: BigDataCloud.",
  openstreetmap: "Название: OpenStreetMap. © OpenStreetMap contributors.",
  windows: "",
};
function nameSource(weather) {
  const provider = weather?.locationNameProvider;
  if (provider && Object.prototype.hasOwnProperty.call(NAME_SOURCES, provider)) return NAME_SOURCES[provider];
  // A name cached before the provider was recorded can only come from the
  // first provider; every other name says who it is.
  return "Название: BigDataCloud.";
}

export function freshness(weather, now) {
  const valid = Number.isFinite(weather?.observedAt) && weather.observedAt > 0;
  const minutes = valid ? Math.max(0, Math.floor((now.getTime() / 1000 - weather.observedAt) / 60)) : null;
  const age = minutes === null ? "время неизвестно" : minutes < 60 ? `${minutes} мин` :
    `${Math.floor(minutes / 60)} ч${minutes % 60 ? ` ${minutes % 60} мин` : ""}`;
  return { stale: Boolean(weather?.stale) || minutes === null || now.getTime() / 1000 - weather.observedAt > 1200, age };
}

const placeFormats = new Map();
function placeTime(weather, now) {
  if (!weather.timezone) return null;
  try {
    let format = placeFormats.get(weather.timezone);
    if (!format) {
      format = { clock: new Intl.DateTimeFormat("ru-RU", {timeZone:weather.timezone,hour:"2-digit",minute:"2-digit",hourCycle:"h23"}),
        day: new Intl.DateTimeFormat("en-CA", {timeZone:weather.timezone,year:"numeric",month:"2-digit",day:"2-digit"}) };
      if (placeFormats.size >= 8) placeFormats.clear();
      placeFormats.set(weather.timezone,format);
    }
    return {clock: epoch=>format.clock.format(new Date(epoch*1000)),today: epoch=>Number.isFinite(epoch)&&epoch>0&&format.day.format(new Date(epoch*1000))===format.day.format(now)};
  } catch { return null; }
}

export function buildTokens(now, { weather, volume, online = true, capabilities = [], style = "mechanical" }) {
  const result = [];
  const method = weather?.locationMethod;
  const approximate = ["ip", "coarse", "default"].includes(method) || weather?.accuracyM > 1000;
  const positioning = {ip:"Оценка Windows по сети",coarse:"Приблизительное место Windows",default:"Место задано в Windows",wifi:"Место Windows по Wi-Fi",cellular:"Место Windows по сотовой сети",satellite:"Место Windows по спутникам"}[method] || "Местоположение Windows";
  const accuracy = Number.isFinite(weather?.accuracyM) ? ` Заявленная точность: ${Math.round(weather.accuracyM)} м.` : "";
  const add = (id, group, k, v, title = "") => result.push({ id, group, k, ...(k === "ic" ? { g: v } : { v }), title });
  const chip = style === "outline" ? "outline" : style === "symbols" ? "lbl" : "chip";
  if (!capabilities.includes("date")) {
    add("date:weekday", "date", style === "symbols" ? "outline" : "lbl", DAYS[now.getDay()]);
    add("date:day", "date", style === "symbols" ? "disk" : chip, String(now.getDate()));
    add("date:month", "date", "outline", MONTHS[now.getMonth()]);
  }
  if (!capabilities.includes("time")) {
    const hh = String(now.getHours()).padStart(2, "0"), mm = String(now.getMinutes()).padStart(2, "0");
    add("time:clock", "time", "val", `${hh}:${mm}`);
    const offsets = [new Date(now.getFullYear(), 0, 1), new Date(now.getFullYear(), 6, 1)].map(d => d.getTimezoneOffset());
    if (offsets[0] !== offsets[1] && now.getTimezoneOffset() < Math.max(...offsets)) add("time:dst", "time", "burst", "DST");
  }
  if (weather?.weatherAvailable && Number.isFinite(weather.tempC)) {
    const f = freshness(weather, now);
    const source = `${positioning}.${accuracy} Обновлено ${f.age} назад. Open-Meteo.`;
    if (f.stale) add("weather:stale", "freshness", "warning", `УСТАРЕЛО · ${f.age}`, source);
    add("weather:thermometer", "temperature", "ic", "thermometer", source);
    add("weather:temperature", "temperature", f.stale ? "off" : "val", `${signed(weather.tempC)}°C`, source);
    const spec = WEATHER[weather.code];
    let glyph = spec?.[1];
    if (glyph === "clear" && weather.isDay === false) glyph = "moon";
    if (glyph === "partly" && weather.isDay === false) glyph = "partly-night";
    if (glyph) add("weather:symbol", "condition", "ic", glyph, source);
    const label = glyph === "moon" ? "ЯСНАЯ НОЧЬ" : spec?.[0] || "ПОГОДА НЕИЗВЕСТНА";
    // Each word can wrap independently, but keeps the condition's visual grammar.
    label.split(" ").forEach((word, i) => add(`weather:label:${i}`, "condition", chip, word, source));
    add("weather:feels-label", "feels", "ic", "feels", `Ощущаемая температура. ${source}`);
    add("weather:feels", "feels", f.stale ? "off" : "val", Number.isFinite(weather.feelsC) ? `${signed(weather.feelsC)}°C` : "НЕТ ДАННЫХ", source);
    add("weather:wind-symbol", "wind", "ic", "wind", source);
    add("weather:wind", "wind", f.stale ? "off" : "val", Number.isFinite(weather.windKmh) ? `${weather.windKmh} км/ч` : "НЕТ ДАННЫХ", `Ветер. ${source}`);
  }
  if (weather?.weatherAvailable) {
    const f=freshness(weather,now), second=now.getTime()/1000;
    const source="Open-Meteo. Прогноз трёх ближайших часовых интервалов; первый интервал уже идёт. Вероятность — максимум отдельного часа, объём приблизительный.";
    const hours=Array.isArray(weather.precipitationForecast) ? weather.precipitationForecast.filter(h=>Number.isFinite(h.at)&&h.at>second&&h.at<=second+10800).slice(0,3) : [];
    const complete=hours.length===3 && hours.every((h,i)=>!i || h.at-hours[i-1].at===3600);
    const probabilities=complete && hours.every(h=>Number.isFinite(h.probability)&&h.probability>=0&&h.probability<=100);
    const amounts=complete && hours.every(h=>Number.isFinite(h.amountMm)&&h.amountMm>=0);
    add("forecast:symbol","precipitation","ic","rain",source);
    if (complete) {
      const local = placeTime(weather, now);
      const startSec = hours[0].at - 3600;
      const endSec = hours[2].at;
      const startTime = local ? local.clock(startSec) : `${String(new Date(startSec * 1000).getHours()).padStart(2, "0")}:00`;
      const endTime = local ? local.clock(endSec) : `${String(new Date(endSec * 1000).getHours()).padStart(2, "0")}:00`;
      const isToday = local ? local.today(startSec) : new Date(startSec * 1000).toDateString() === now.toDateString();
      const datePrefix = isToday ? "" : `${DAYS[new Date(startSec * 1000).getDay()]} `;
      const windowStr = `${datePrefix}${startTime}–${endTime}`;
      add("forecast:window", "precipitation", "outline", windowStr, `Интервал прогноза осадков: ${windowStr} (по времени места)`);
    }
    const chanceText = probabilities ? `${Math.max(...hours.map(h=>h.probability))}%` : (f.stale ? "--%" : "НЕТ ДАННЫХ");
    add("forecast:chance","precipitation",(f.stale || !probabilities)?"off":"chance",chanceText,source);
    const amountText = amounts ? `≈${hours.reduce((n,h)=>n+h.amountMm,0).toLocaleString("ru-RU",{maximumFractionDigits:1})} ММ` : (f.stale ? "ОЖИДАНИЕ ОБНОВЛЕНИЯ" : "ОБЪЁМ НЕДОСТУПЕН");
    add("forecast:amount","precipitation-amount",(f.stale || !amounts)?"off":"lbl",amountText,source);
    const local=placeTime(weather,now),rise=weather.sunriseAt,set=weather.sunsetAt;
    const hasSolar=Number.isFinite(rise) && Number.isFinite(set) && set>rise;
    const isTodaySolar=hasSolar && local && local.today(rise) && local.today(set);
    const solarSource="Восход и закат по времени места. Расчёт Open-Meteo.";
    add("sun:rise-label","sunrise","ic","sunrise",`Восход. ${solarSource}`);
    add("sun:rise","sunrise",isTodaySolar && !f.stale ? "lbl" : "off",hasSolar && local ? local.clock(rise) : "НЕТ ДАННЫХ",solarSource);
    add("sun:set-label","sunset","ic","sunset",`Закат. ${solarSource}`);
    add("sun:set","sunset",isTodaySolar && !f.stale ? "lbl" : "off",hasSolar && local ? local.clock(set) : "НЕТ ДАННЫХ",solarSource);
    const duration=Number.isFinite(weather.daylightSeconds) && weather.daylightSeconds>=0 && weather.daylightSeconds<=86400 ? Math.round(weather.daylightSeconds/60) : null;
    add("sun:duration-symbol","daylight","ic","daylength",`Длительность светового дня. ${solarSource}`);
    add("sun:duration","daylight",isTodaySolar && !f.stale ? "lbl" : "off",duration===null ? "НЕТ ДАННЫХ" : `${Math.floor(duration/60)}ч ${duration%60}м`,`Длительность светового дня. ${solarSource}`);
    if(isTodaySolar) add("sun:progress","daylight","daytrack",String(Math.floor(Math.max(0,Math.min(1,(second-rise)/(set-rise)))*100)),"Ход светового дня от восхода до заката.");
  }
  if (weather && !["available", "retrying"].includes(weather.locationStatus)) {
    const status = { "permission-required": "РАЗРЕШИ ГЕОЛОКАЦИЮ", requesting: "ОПРЕДЕЛЯЮ МЕСТО", denied: "ГЕОЛОКАЦИЯ ОТКЛЮЧЕНА", unavailable: "МЕСТО НЕДОСТУПНО" };
    add("geo:status", "geolocation", "outline", status[weather.locationStatus] || "ОПРЕДЕЛЯЮ МЕСТО",
      "Погода использует геолокацию Windows. Открой режим редактирования и нажми «Определить место». Доступ к местоположению управляется в Параметрах Windows.");
  }
  if (weather?.locationStatus === "available" && !weather.weatherAvailable) {
    add("geo:weather-unavailable", "geolocation", "outline", "ПОГОДА НЕДОСТУПНА",
      "Windows определила местоположение, но погодный сервис пока не ответил. Обновление повторится автоматически.");
  }
  if (weather?.locationSource === "windows" || weather?.locationNameStatus === "available" || (weather?.location && weather.location.length > 0)) {
    const name = weather.location || "";
    const placeText = name || (weather.locationNameStatus === "resolving" ? "ОПРЕДЕЛЯЮ НАЗВАНИЕ" : "НАЗВАНИЕ МЕСТА НЕДОСТУПНО");
    const source = `${positioning}.${accuracy}${method === "default" ? "" : ` ${nameSource(weather)}`}`;
    add("geo:pin", "location", "ic", "pin", source);
    add("geo:place", "location", "lbl", `${approximate ? "≈ " : ""}${placeText}`, source);
    if (method === "ip") add("geo:method", "location", "ic", "signal", source);
  }
  if (weather?.ipLocation) {
    const value = weather.ipLocation;
    const ip = { ...value, stale: Boolean(value.stale) || !Number.isFinite(value.resolvedAt) || now.getTime() / 1000 - value.resolvedAt > 1800 };
    const source = `${[ip.city, ip.country].filter(Boolean).join(", ")}. Местоположение выхода в интернет по IP${ip.stale ? ". Данные устарели" : ""}.`;
    add("ip:symbol", "ip-location", "ic", "signal", source);
    (ip.city || ip.country || "IP").split(/\s+/).forEach((word, i) => add(`ip:location:${i}`, "ip-location", "lbl", word, source));
    add("ip:source", "ip-location", "outline", ip.stale ? "IP · УСТАРЕЛ" : "IP", source);
  }
  if (!online) {
    add("network:symbol", "network", "ic", "offline", "Браузер сообщает отсутствие сетевого подключения");
    add("network:offline", "network", "warning", "НЕТ ПОДКЛЮЧЕНИЯ");
  }
  if (volume && !capabilities.includes("system-volume")) {
    add("volume:symbol", "volume", "ic", volume.muted ? "mute" : "volume", "Системная громкость");
    add("volume:level", "volume", volume.muted ? "outline" : "val", volume.muted ? "ЗВУК ВЫКЛ" : `${volume.level}%`, "Системная громкость");
  }
  return result.map(token => ({ ...token,
    secondary: ["feels", "wind", "location", "ip-location", "geolocation", "network", "precipitation", "precipitation-amount", "sunrise", "sunset", "daylight"].includes(token.group),
    breakBefore: token.id === "weather:thermometer" || token.id === "weather:feels-label" ||
      token.id === "ip:symbol" || token.id === "geo:status" || token.id === "weather:stale" ||
      style !== "mechanical" && ["condition", "wind", "volume"].includes(token.group) &&
        result.find(t => t.group === token.group)?.id === token.id,
  }));
}

/** Pack every atom, keeping semantic groups together whenever a row allows it. */
function pack(elements, width, gap, lineGap) {
  const placed = [];
  let x = 0, y = 0, rowHeight = 0;
  const newline = () => { if (rowHeight) y += rowHeight + lineGap; x = 0; rowHeight = 0; };
  for (let i = 0; i < elements.length; i++) {
    const e = elements[i];
    if (e.w > width || e.w <= 0 || e.h <= 0) return null;
    if (e.breakBefore) newline();
    if (i === 0 || e.group !== elements[i - 1].group) {
      const group = [];
      for (let j = i; j < elements.length && elements[j].group === e.group; j++) group.push(elements[j]);
      const groupWidth = group.reduce((sum, atom) => sum + atom.w, 0) + (group.length - 1) * gap;
      if (groupWidth <= width && x + groupWidth > width) newline();
    }
    if (x + e.w > width) newline();
    placed.push({ ...e, x, y });
    x += e.w + gap; rowHeight = Math.max(rowHeight, e.h);
  }
  const heights = new Map();
  for (const e of placed) heights.set(e.y, Math.max(heights.get(e.y) || 0, e.h));
  for (const e of placed) e.y += (heights.get(e.y) - e.h) / 2;
  return { placed, h: y + rowHeight, w: Math.max(0, ...placed.map(e => e.x + e.w)) };
}

/** Padding of a micrographics pill around its group run, in unscaled px. */
export const PILL_PAD = { x: 7, y: 3 };

/** Scan free row intervals; semantic groups cross a dock only as separate rows.
 *  `gap` clears docks, `itemGap` spaces atoms inside a group, `groupGap` separates groups. */
function flow(elements, start, area, docks, gap, lineGap, itemGap = gap, groupGap = gap) {
  const placed = [], pending = [];
  for (const e of elements) {
    if (pending.at(-1)?.[0].group === e.group) pending.at(-1).push(e);
    else pending.push([e]);
  }
  let y=start.y, rowHeight=0, row=[], firstRow=true;
  const overlap=(a,b)=>a.x<b.x+b.w && a.x+a.w>b.x && a.y<b.y+b.h && a.y+a.h>b.y;
  const spans=height=>{
    let free=[[firstRow?Math.max(8,start.x):8,area.w-8]];
    for(const d of docks){
      if(y+height<=d.y-gap || y>=d.y+d.h+gap) continue;
      free=free.flatMap(([l,r])=>d.x-gap>=r || d.x+d.w+gap<=l ? [[l,r]] :
        [[l,Math.min(r,d.x-gap)],[Math.max(l,d.x+d.w+gap),r]].filter(([a,b])=>b>a));
    }
    return free;
  };
  const finishRow=()=>{
    // Close the gap beside a dock without stretching the typography inside a
    // segment. Use the completed row height: a later tall atom can change which
    // dock bounds this interval.
    for(const [l,r] of spans(rowHeight)){
      // An interval following a dock attaches to that dock's right edge.
      // Only an interval without a dock on its left attaches to the next dock.
      const leftDock=docks.some(d=>y+rowHeight>d.y-gap && y<d.y+d.h+gap && Math.abs(l-(d.x+d.w+gap))<.01);
      if(r>=area.w-8 || leftDock)continue;
      const segment=row.filter(e=>e.x>=l-.01 && e.x+e.w<=r+.01);
      if(!segment.length)continue;
      const shift=Math.max(0,r-Math.max(...segment.map(e=>e.x+e.w)));
      for(const e of segment)e.x+=shift;
    }
    for(const e of row)e.y+=(rowHeight-e.h)/2;
    y+=rowHeight+lineGap;row=[];rowHeight=0;firstRow=false;
  };
  while(pending.length && y<area.h-8){
    let choice=null;
    for(let i=0;i<pending.length;i++){
      const group=pending[i], gh=Math.max(...group.map(e=>e.h));
      const gw=group.reduce((n,e)=>n+e.w,0)+(group.length-1)*itemGap;
      const together=spans(Math.max(rowHeight,gh)).some(([l,r])=>r-l>=gw);
      // A symbol must travel with its first caption even when a long group
      // needs more than one row. If this pair has no slot yet, try another group
      // or a later row rather than leaving the symbol beside unrelated data.
      const symbol=group[0].node?.classList.contains("mg-ic") || /:(symbol|thermometer|wind-symbol|pin)$/.test(group[0].id);
      const chunk=together?group:group.slice(0,symbol?2:1);
      const width=chunk.reduce((n,e)=>n+e.w,0)+(chunk.length-1)*itemGap;
      const height=Math.max(rowHeight,...chunk.map(e=>e.h));
      if(y+height>area.h-8)continue;
      if(row.some(e=>docks.some(d=>overlap({x:e.x,y,w:e.w,h:height},{x:d.x-gap,y:d.y-gap,w:d.w+2*gap,h:d.h+2*gap}))))continue;
      let free=spans(height);
      for(const e of row) free=free.flatMap(([l,r])=>e.x-groupGap>=r || e.x+e.w+groupGap<=l ? [[l,r]] :
        [[l,Math.min(r,e.x-groupGap)],[Math.max(l,e.x+e.w+groupGap),r]].filter(([a,b])=>b>a));
      const slot=free.find(([l,r])=>r-l>=width);
      if(slot){choice={i,chunk,height,x:slot[0]};break;}
    }
    if(!choice){
      if(rowHeight){finishRow();continue;}
      const edges=docks.map(d=>d.y+d.h+gap).filter(v=>v>y);
      const minHeight=Math.min(...pending.flat().map(e=>e.h));
      y=Math.min(y+Math.max(1,minHeight)+lineGap,...edges);firstRow=false;continue;
    }
    let x=choice.x;
    for(const e of choice.chunk){const item={...e,x,y};placed.push(item);row.push(item);x+=e.w+itemGap;}
    rowHeight=choice.height;
    pending[choice.i]=pending[choice.i].slice(choice.chunk.length);
    if(!pending[choice.i].length)pending.splice(choice.i,1);
  }
  if(pending.length)return null;
  finishRow();
  const x=Math.min(...placed.map(e=>e.x)),top=Math.min(...placed.map(e=>e.y));
  const w=Math.max(...placed.map(e=>e.x+e.w))-x,h=Math.max(...placed.map(e=>e.y+e.h))-top;
  placed.layout={x,y:top,w,h,contentW:w,contentH:h,overflow:false,overlay:false};
  return placed;
}

/** Flow around permanent docks; confined scrolling retains every atom if crowded. */
export function routeElements(elements, anchor, viewport, obstacles) {
  // Pill backdrops (jelly) grow each group run by PILL_PAD: groups, rows and
  // docks keep that much more clearance. Without pills every gap is as before.
  const pad = anchor.pills ? PILL_PAD.x * anchor.scale : 0, padY = anchor.pills ? PILL_PAD.y * anchor.scale : 0;
  const margin = 8, gap = 7 * anchor.scale, lineGap = 10 * anchor.scale + 2 * padY;
  const top = Math.max(-8,Math.min(0,...obstacles.filter(o=>o.w>0 && o.h>0 && o.x<viewport.w && o.x+o.w>0 && o.y+o.h>0).map(o=>o.y)));
  const right = Math.max(margin + 1, viewport.w - margin), bottom = Math.max(top + 1, viewport.h - margin);
  const maxAtom = Math.max(1, ...elements.map(e => e.w));
  const preferred = Math.max(maxAtom, ({ mechanical: 430, outline: 340, symbols: 290 }[anchor.style] || 430) * anchor.scale);
  const ax = Math.max(margin, Math.min(anchor.x, right - Math.min(maxAtom, right - margin)));
  const ay = top;
  const docks = obstacles.filter(o => o.w > 0 && o.h > 0 && o.x < right && o.y < bottom && o.x + o.w > margin && o.y + o.h > top);
  if (elements.length) {
    for (const start of [{x:ax,y:ay},{x:margin,y:ay}]) {
      const routed = flow(elements,start,viewport,docks,gap+pad,lineGap,gap,gap+2*pad);
      if (routed) return routed;
    }
  }
  const unique = values => [...new Set(values)].sort((a, b) => a - b);
  const previous = anchor.previous;
  const xs = unique([ax, margin, previous?.x, ...docks.map(o => o.x + o.w + gap)].filter(Number.isFinite)).filter(x => x >= margin && x + maxAtom <= right);
  const ys = unique([ay, top, previous?.y, ...docks.map(o => o.y + o.h + gap)].filter(Number.isFinite)).filter(y => y >= top && y < bottom);
  const ends = unique([right, ...docks.map(o => o.x - gap)]);
  let best = null, fallback = null;
  for (const x of xs) for (const y of ys) {
    // Candidate widths only use meaningful boundaries; no pixel or permutation search.
    const widths = unique([Math.min(preferred, right - x), previous?.candidateW, ...ends.map(end => end - x)].filter(Number.isFinite)).filter(w => w >= maxAtom && x + w <= right);
    for (const width of widths) {
      let limit = bottom;
      for (const o of docks) {
        if (x >= o.x + o.w + gap || x + width <= o.x - gap) continue;
        if (y >= o.y + o.h + gap) continue;
        limit = Math.min(limit, o.y - gap);
      }
      if (limit <= y) continue;
      const layout = pack(elements, width, gap, lineGap);
      if (!layout) continue;
      const distance = Math.hypot(x - ax, y - ay);
      const stable = previous && x === previous.x && y === previous.y && width === previous.candidateW;
      const score = distance + Math.abs(width - preferred) * .2 + layout.h * .08 - (stable ? 24 : 0);
      const candidate = { x, y, width, available: limit - y, layout, score };
      if (layout.h <= candidate.available && (!best || score < best.score)) best = candidate;
      const overflow = Math.max(0, layout.h - candidate.available);
      candidate.scrollScore = overflow * 2 + score;
      if (!fallback || candidate.scrollScore < fallback.scrollScore) fallback = candidate;
    }
  }
  // If every free region is too short, retain the entire composition in a
  // scrollable region. A completely occupied desktop uses an explicit overlay.
  const chosen = best || fallback || { x: margin, y: top,
    width: right - margin, available: bottom - top,
    layout: pack(elements, Math.max(maxAtom, right - margin), gap, lineGap), overlay: true };
  const scrolling = chosen.layout && chosen.layout.h > chosen.available;
  const layout = scrolling && chosen.width - 8 >= maxAtom ?
    pack(elements, chosen.width - 8, gap, lineGap) : chosen.layout || { placed: [], w: 0, h: 0 };
  const placed = layout.placed.map(e => ({ ...e, x: e.x + chosen.x, y: e.y + chosen.y }));
  placed.layout = { x: chosen.x, y: chosen.y, w: scrolling ? chosen.width : Math.min(chosen.width, Math.max(1, layout.w)),
    h: Math.min(chosen.available, Math.max(1, layout.h)), candidateW: chosen.width, contentW: layout.w, contentH: layout.h,
    overflow: !best && (layout.h > chosen.available || layout.w > chosen.width), overlay: Boolean(chosen.overlay) };
  return placed;
}
