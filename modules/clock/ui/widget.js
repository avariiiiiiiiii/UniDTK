import { saveConfig, config, notifyGeometryChanged } from '../../unidtk-core.js';
import { initVpnPanel } from '../vpn/vpn-panel.js';
import { renderGooeyClock, updateGooeyClock } from './gooey.js';

// gooey.css is loaded via a <link> in core/ui/index.html (stage 3 split);
// the old runtime injection here was removed to keep one canonical load path.

function renderClock(card, widgetId) {
  card.classList.add("clock-widget");
  card.classList.add("no-header");
  const pos = config.positions[widgetId] || (config.positions[widgetId] = {});
  const version = pos.version || "digital";

  const container = document.createElement("div");
  container.className = "clock-container";
  container.dataset.version = version;
  
  // Add switcher
  const switcher = document.createElement("div");
  switcher.className = "version-switcher";
  switcher.setAttribute("aria-label", "Change Style");
  switcher.innerHTML = `
    <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
      <path d="M21.5 2v6h-6M2.5 22v-6h6M2 12c0-4.4 3.6-8 8-8 2.2 0 4.2.9 5.7 2.3L21.5 8M22 12c0 4.4-3.6 8-8 8-2.2 0-4.2-.9-5.7-2.3L2.5 16" />
    </svg>
  `;
  switcher.addEventListener("click", (e) => {
    e.stopPropagation();
    const versions = ["digital", "gooey", "digital-stacked", "digital-vertical", "analogue", "analogue-dot", "analogue-bezel", "analogue-cmf", "rings", "minimal-lines", "grid-second", "nothing-watch-pro"];
    const curIdx = versions.indexOf(container.dataset.version);
    const nextIdx = (curIdx + 1) % versions.length;
    const nextVer = versions[nextIdx];
    
    container.dataset.version = nextVer;
    config.positions[widgetId].version = nextVer;
    saveConfig();
    updateClockFace(container, nextVer);
    notifyGeometryChanged("dock-content");
  });

  card.appendChild(switcher);
  card.appendChild(container);
  
  updateClockFace(container, version);
}

function updateClockFace(container, version) {
  container.innerHTML = "";
  // Version-specific classes must be toggled off when switching styles; otherwise
  // CSS + updateClockTime selectors (e.g. .nothing-watch-pro → HH:MM:SS) stick.
  container.classList.toggle("gothic-clock-container", version === "gothic");
  container.classList.toggle("grid-second", version === "grid-second");
  container.classList.toggle("nothing-watch-pro", version === "nothing-watch-pro");
  container.classList.toggle("gooey-clock-container", version === "gooey");
  if (version === "gothic") {
    container.innerHTML = `
      <div class="gothic-clock">
        <svg viewBox="0 0 190 170" aria-hidden="true">
          <path class="gothic-clock-arch" d="M24 151V69C24 38 51 19 95 7C139 19 166 38 166 69V151" />
          <path class="gothic-clock-arch inner" d="M34 147V72C34 47 56 31 95 19C134 31 156 47 156 72V147" />
          <path class="gothic-clock-base" d="M18 151H172M35 158H155" />
          <path class="gothic-clock-tracery" d="M24 69C43 67 57 57 64 39M166 69C147 67 133 57 126 39" />
          <path class="gothic-clock-finial" d="M95 5L90 16L95 12L100 16Z" />
          <path class="gothic-clock-divider" d="M55 105H78M112 105H135" />
          <circle class="gothic-clock-gem" cx="95" cy="105" r="2.5" />
        </svg>
        <div class="clock-digital gothic-time">00:00</div>
        <div class="gothic-date"></div>
      </div>
    `;
  } else if (version === "digital") {
    container.innerHTML = `<div class="clock-digital">00:00</div>`;
  } else if (version === "gooey") {
    renderGooeyClock(container);
  } else if (version === "digital-stacked") {
    container.innerHTML = `
      <div class="clock-digital-stacked">
        <div class="hour">00</div>
        <div class="min">00</div>
      </div>
    `;
  } else if (version === "analogue") {
    container.innerHTML = `
      <div class="clock-analogue">
        <div class="clock-center"></div>
        <div class="hand hour"></div>
        <div class="hand minute"></div>
        <div class="hand second"></div>
      </div>
    `;
  } else if (version === "analogue-dot") {
     container.innerHTML = `
      <div class="clock-analogue dot-style">
        <div class="dot-digital-overlay clock-digital" style="font-size: 0.7rem; top: 38%;">00:00</div>
        <div class="dot-red-accent"></div>
        <div class="clock-center" style="width: 8px; height: 8px; background: rgba(var(--fg-rgb), 0.2); z-index: 10;"></div>
        <div class="hand hour" style="width: 4px; height: 28%; background: rgba(var(--fg-rgb), 0.2); border-radius: 2px; z-index: 8;"></div>
        <div class="hand minute" style="width: 2px; height: 38%; background: rgba(var(--fg-rgb), 0.33); border-radius: 1px; z-index: 7;"></div>
        <div class="dot-stats-overlay" style="bottom: 22%;">
          SAT 23<br>
          156 kcal<br>
          6,240 steps
        </div>
      </div>
    `;
  } else if (version === "minimal-lines") {
    container.innerHTML = `
      <div class="clock-analogue" style="border: none;">
        <div class="hand hour" style="width: 6px; height: 25%; background: rgb(var(--fg-rgb));"></div>
        <div class="hand minute" style="width: 6px; height: 40%; background: rgb(var(--fg-rgb));"></div>
      </div>
    `;
  } else if (version === "grid-second") {
    container.innerHTML = `
      <div class="clock-digital" style="font-size: 2.5rem;">00:00</div>
      <div class="clock-analogue" style="position: absolute; border: 2px dashed rgba(var(--bg-elev-rgb), 0.1); width: 140px; height: 140px;">
        <div class="hand second" style="width: 4px; height: 8px; background: rgb(var(--fg-rgb)); bottom: 95%;"></div>
      </div>
    `;
  } else if (version === "nothing-watch-pro") {
     container.innerHTML = `
      <div class="clock-analogue" style="background: rgb(var(--scrim-rgb)); border: 2px solid rgba(var(--bg-elev-rgb), 0.13);">
        <div class="clock-digital" style="font-size: 1.2rem; position: absolute; top: 20%;">00:00:00</div>
        <div class="hand hour" style="width: 3px; height: 20%; background: rgb(var(--fg-rgb));"></div>
        <div class="hand minute" style="width: 3px; height: 30%; background: rgb(var(--fg-rgb));"></div>
        <div class="dot-red-accent" style="top: 50%; right: 20%;"></div>
        <div style="position: absolute; bottom: 15%; font-family: var(--font-accent); font-size: 0.6rem; color: var(--theme-accent);">CMF BY NOTHING</div>
      </div>
    `;
  } else if (version === "digital-vertical") {
    container.innerHTML = `
      <div class="clock-digital-vertical">
        <div class="hour">00</div>
        <div class="min">00</div>
        <div class="sec">00</div>
      </div>
    `;
  } else if (version === "analogue-bezel") {
    container.innerHTML = `
      <div class="clock-analogue bezel-style">
        <div class="clock-center"></div>
        <div class="hand hour"></div>
        <div class="hand minute"></div>
        <div class="hand second"></div>
      </div>
    `;
  } else if (version === "analogue-cmf") {
    container.innerHTML = `
      <div class="clock-analogue cmf-style">
        <div class="clock-center"></div>
        <div class="hand hour"></div>
        <div class="hand minute"></div>
        <div class="hand second"></div>
      </div>
    `;
  } else if (version === "rings") {
    container.innerHTML = `
      <div class="clock-rings">
        <div class="ring outer"></div>
        <div class="ring inner"></div>
        <div class="time-display">00:00</div>
      </div>
    `;
  }
  updateClockTime();
}

function startClockInterval() {
  if (window.clockInterval) clearInterval(window.clockInterval);
  window.clockInterval = setInterval(() => {
    if (document.hidden) return;
    updateClockTime();
  }, 1000);
  updateClockTime();
}

function updateClockTime() {
  const now = new Date();
  const h = now.getHours();
  const m = now.getMinutes();
  const s = now.getSeconds();
  const timeStr = `${h.toString().padStart(2, '0')}:${m.toString().padStart(2, '0')}`;
  const dateStr = now.toLocaleDateString("ru-RU", {
    weekday: "short",
    day: "2-digit",
    month: "short",
  }).replaceAll(".", "");

  document.querySelectorAll('.gooey-clock-face').forEach(face => {
    updateGooeyClock(face, now);
  });

  // Digital
  document.querySelectorAll(".clock-digital").forEach(el => {
    el.textContent = timeStr;
  });

  document.querySelectorAll(".gothic-date").forEach(el => {
    el.textContent = dateStr;
  });

  document.querySelectorAll(".clock-digital-stacked").forEach(el => {
    el.querySelector(".hour").textContent = h.toString().padStart(2, '0');
    el.querySelector(".min").textContent = m.toString().padStart(2, '0');
  });

  document.querySelectorAll(".clock-digital-vertical").forEach(el => {
    el.querySelector(".hour").textContent = h.toString().padStart(2, '0');
    el.querySelector(".min").textContent = m.toString().padStart(2, '0');
    el.querySelector(".sec").textContent = s.toString().padStart(2, '0');
  });

  document.querySelectorAll(".clock-rings").forEach(el => {
    const outer = el.querySelector(".ring.outer");
    const inner = el.querySelector(".ring.inner");
    const display = el.querySelector(".time-display");
    if (outer) outer.style.transform = `rotate(${m * 6}deg)`;
    if (inner) inner.style.transform = `rotate(${s * 6}deg)`;
    if (display) display.textContent = timeStr;
  });

  document.querySelectorAll(".grid-second").forEach(el => {
    const secondHand = el.querySelector(".hand.second");
    if (secondHand) secondHand.style.transform = `translateX(-50%) rotate(${s * 6}deg)`;
  });

  document.querySelectorAll(".nothing-watch-pro").forEach(el => {
    const digital = el.querySelector(".clock-digital");
    if (digital) {
      digital.textContent = `${h.toString().padStart(2, '0')}:${m.toString().padStart(2, '0')}:${s.toString().padStart(2, '0')}`;
    }
  });

  // Analogue
  const hDeg = (h % 12) * 30 + m * 0.5;
  const mDeg = m * 6;
  const sDeg = s * 6;

  document.querySelectorAll(".clock-analogue").forEach(el => {
    const hourHand = el.querySelector(".hand.hour");
    const minuteHand = el.querySelector(".hand.minute");
    const secondHand = el.querySelector(".hand.second");
    if (hourHand) hourHand.style.transform = `translateX(-50%) rotate(${hDeg}deg)`;
    if (minuteHand) minuteHand.style.transform = `translateX(-50%) rotate(${mDeg}deg)`;
    if (secondHand) secondHand.style.transform = `translateX(-50%) rotate(${sDeg}deg)`;
  });
}

// в”Ђв”Ђв”Ђ Expand / Collapse with Dashboard/Folder в”Ђв”Ђв”Ђ

async function renderClockDashboard(body, widgetId) {
  const version = config.positions[widgetId]?.version || "digital";
  const now = new Date();
  const days = ["S", "M", "T", "W", "T", "F", "S"];
  const currentDay = now.getDate();
  const currentMonth = now.getMonth();
  const currentYear = now.getFullYear();

  const firstDay = new Date(currentYear, currentMonth, 1).getDay();
  const lastDate = new Date(currentYear, currentMonth + 1, 0).getDate();

  let calendarHtml = `
    <div class="dash-calendar">
      <div class="cal-header">${now.toLocaleString('default', { month: 'long' })}</div>
      <div class="cal-grid">
        ${days.map(d => `<div class="cal-day-name">${d}</div>`).join("")}
        ${Array(firstDay).fill("").map(() => `<div></div>`).join("")}
        ${Array(lastDate).fill(0).map((_, i) => `
          <div class="cal-date ${i + 1 === currentDay ? 'today' : ''}">${i + 1}</div>
        `).join("")}
      </div>
    </div>
  `;

  const isOnline = navigator.onLine;

  body.innerHTML = `
    <div class="clock-dash-layout">
      <div class="dash-left">
        <div class="clock-container dash-clock" data-version="${version}"></div>
      </div>
      <div class="dash-right">
        ${calendarHtml}
        <div class="dash-internet">
          <div class="net-icon ${isOnline ? 'online' : 'offline'}">
            <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
              <path d="M5 12.55a11 11 0 0 1 14.08 0" /><path d="M1.42 9a16 16 0 0 1 21.16 0" /><path d="M8.53 16.11a6 6 0 0 1 6.95 0" /><line x1="12" y1="20" x2="12.01" y2="20" />
            </svg>
          </div>
          <div class="net-status">${isOnline ? 'Connected' : 'Offline'}</div>
        </div>
      </div>
    </div>
    <div class="dash-vpn">
      <div class="vpn-row">
        <div class="vpn-icon off">
          <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <path d="M12 2 4 6v6c0 5 3.4 8.7 8 10 4.6-1.3 8-5 8-10V6l-8-4Z" />
          </svg>
        </div>
        <div class="vpn-info">
          <div class="vpn-label">VPN: Off</div>
          <div class="vpn-server-name">—</div>
        </div>
        <label class="vpn-toggle">
          <input type="checkbox" class="vpn-toggle-input" />
          <span class="vpn-toggle-slider"></span>
        </label>
      </div>
      <select class="vpn-server-select" disabled>
        <option>Загрузка...</option>
      </select>
      <div class="vpn-server-picker">
        <button type="button" class="vpn-server-button" disabled>Loading...</button>
        <div class="vpn-server-menu">
          <div class="vpn-menu-head">
            <span class="vpn-menu-title">SERVERS</span>
            <button type="button" class="vpn-ping-btn">PING ALL</button>
          </div>
          <div class="vpn-menu-list"></div>
        </div>
      </div>
      <div class="vpn-auto-row">
        <span class="vpn-auto-label">AUTO</span>
        <label class="vpn-toggle vpn-auto-toggle">
          <input type="checkbox" class="vpn-auto-toggle-input" />
          <span class="vpn-toggle-slider"></span>
        </label>
      </div>
    </div>
  `;

  const container = body.querySelector(".dash-clock");
  updateClockFace(container, version);

  initVpnPanel(body);
}

// в”Ђв”Ђв”Ђ VPN panel в”Ђв”Ђв”Ђ

export { renderClock, updateClockFace, updateClockTime, startClockInterval, renderClockDashboard };
