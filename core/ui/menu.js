/**
 * UniDTK — Core Fullscreen Menu (menu.js)
 *
 * Core overlay surface providing:
 *   1. Widgets and desktop layout editor (visibility, coordinate editing, reordering, reset)
 *   2. Desktop folders management (create, delete, rename, add/remove shortcuts) via redb
 *   3. VPN configuration (subscription URL, auto-connect, isolated Home Server submodule)
 */

import { invoke } from "@unidtk/bridge";
import { surfaceManager } from "./surface-manager.js";
import { getWidgets, getDefaultWidgetConfig, getDisabledModules, setDisabledModules } from "./widgets-api.js";
import { config, saveConfig, syncWidgets, relayoutWidgets, escapeHtml } from "./unidtk-core.js";
import {
  getActiveTheme,
  getAllPresets,
  setLiveToken,
  commitLiveChange,
  selectPreset,
  saveCustomPreset,
  deleteCustomPreset,
  resetSizesToPreset,
  PALETTE_SLOTS,
  TINT_TARGETS,
  shuffleColors,
  restorePreviousColors,
  hasPreviousColors,
} from "./theme.js";
import { animate, spring } from "./motion.js";

// Backend-module toggles share one stored list; see the handler.
let moduleToggleChain = Promise.resolve();

// One submission per click: the control stays disabled while its action is
// in flight, so a double click cannot create a folder or shortcut twice.
async function runOnce(control, action) {
  if (!control || control.disabled) return;
  control.disabled = true;
  try {
    await action();
  } finally {
    if (control.isConnected) control.disabled = false;
  }
}

const FOLDER_ICONS = {
  folder: `<svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M22 19a2 2 0 01-2 2H4a2 2 0 01-2-2V5a2 2 0 012-2h5l2 3h9a2 2 0 012 2z"/></svg>`,
  terminal: `<svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><polyline points="4 17 10 11 4 5"/><line x1="12" y1="19" x2="20" y2="19"/></svg>`,
  gamepad: `<svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><line x1="6" y1="12" x2="10" y2="12"/><line x1="8" y1="10" x2="8" y2="14"/><line x1="15" y1="13" x2="15.01" y2="13"/><line x1="18" y1="11" x2="18.01" y2="11"/><rect x="2" y="6" width="20" height="12" rx="6"/></svg>`,
  code: `<svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><polyline points="16 18 22 12 16 6"/><polyline points="8 6 2 12 8 18"/></svg>`,
  globe: `<svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="10"/><line x1="2" y1="12" x2="22" y2="12"/><path d="M12 2a15.3 15.3 0 0 1 4 10 15.3 15.3 0 0 1-4 10 15.3 15.3 0 0 1-4-10 15.3 15.3 0 0 1 4-10z"/></svg>`,
  music: `<svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M9 18V5l12-2v13"/><circle cx="6" cy="18" r="3"/><circle cx="18" cy="16" r="3"/></svg>`,
  tools: `<svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M14.7 6.3a1 1 0 0 0 0 1.4l1.6 1.6a1 1 0 0 0 1.4 0l3.77-3.77a6 6 0 0 1-7.94 7.94l-6.91 6.91a2.12 2.12 0 0 1-3-3l6.91-6.91a6 6 0 0 1 7.94-7.94l-3.76 3.76z"/></svg>`,
  book: `<svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M4 19.5A2.5 2.5 0 0 1 6.5 17H20"/><path d="M6.5 2H20v20H6.5A2.5 2.5 0 0 1 4 19.5v-15A2.5 2.5 0 0 1 6.5 2z"/></svg>`,
  star: `<svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><polygon points="12 2 15.09 8.26 22 9.27 17 14.14 18.18 21.02 12 17.77 5.82 21.02 7 14.14 2 9.27 8.91 8.26 12 2"/></svg>`,
};

let currentTab = "widgets";
let menuOverlay = null;
let menuContainer = null;

export function initMenu() {
  menuOverlay = document.getElementById("menuOverlay");
  menuContainer = document.getElementById("menuContainer");
  if (!menuOverlay || !menuContainer) return;

  renderShell();
  setupEvents();
}

function renderShell() {
  menuContainer.innerHTML = `
    <div class="menu-header">
      <div class="menu-brand">
        <div class="menu-brand-icon">
          <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
            <line x1="3" y1="12" x2="21" y2="12"></line>
            <line x1="3" y1="6" x2="21" y2="6"></line>
            <line x1="3" y1="18" x2="21" y2="18"></line>
          </svg>
        </div>
        <div class="menu-title-block">
          <h2>UniDTK // Menu</h2>
          <p>Конфигурация модулей, рабочего стола и сетевых параметров</p>
        </div>
      </div>
      <button class="menu-close-btn" id="menuCloseBtn" aria-label="Закрыть меню (Esc)">
        <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round">
          <line x1="18" y1="6" x2="6" y2="18"></line>
          <line x1="6" y1="6" x2="18" y2="18"></line>
        </svg>
      </button>
    </div>

    <div class="menu-tabs">
      <button class="menu-tab-btn active" data-tab="widgets">
        <span class="tab-dot"></span>
        Виджеты и стол
      </button>
      <button class="menu-tab-btn" data-tab="folders">
        <span class="tab-dot"></span>
        Папки ярлыков
      </button>
      <button class="menu-tab-btn" data-tab="vpn">
        <span class="tab-dot"></span>
        VPN и Сеть
      </button>
      <button class="menu-tab-btn" data-tab="theme">
        <span class="tab-dot"></span>
        Оформление
      </button>
    </div>

    <div class="menu-body" id="menuBody">
      <div class="menu-tab-pane active" id="pane-widgets"></div>
      <div class="menu-tab-pane" id="pane-folders"></div>
      <div class="menu-tab-pane" id="pane-vpn"></div>
      <div class="menu-tab-pane" id="pane-theme"></div>
    </div>
  `;
}

function setupEvents() {
  menuContainer.querySelectorAll(".menu-tab-btn").forEach((btn) => {
    btn.addEventListener("click", () => {
      const tab = btn.dataset.tab;
      switchTab(tab);
    });
  });

  menuContainer.querySelector("#menuCloseBtn")?.addEventListener("click", () => {
    closeMenu();
  });

  menuOverlay.addEventListener("click", (e) => {
    if (e.target === menuOverlay) {
      closeMenu();
    }
  });
}

function switchTab(tab) {
  currentTab = tab;
  menuContainer.querySelectorAll(".menu-tab-btn").forEach((b) => {
    b.classList.toggle("active", b.dataset.tab === tab);
  });
  menuContainer.querySelectorAll(".menu-tab-pane").forEach((p) => {
    p.classList.toggle("active", p.id === `pane-${tab}`);
  });

  if (tab === "widgets") renderWidgetsTab();
  else if (tab === "folders") renderFoldersTab();
  else if (tab === "vpn") renderVpnTab();
  else if (tab === "theme") renderThemeTab();
}

export function openMenu(triggerOrigin = null) {
  if (!menuOverlay || !menuContainer) return;

  surfaceManager.openSurface(menuContainer, {
    backdrop: menuOverlay,
    origin: triggerOrigin || null,
    originRadius: "16px",
    content: ".menu-header, .menu-tabs, .menu-body",
    elevation: "var(--z-surface-overlay, 2000)",
    onOpen: () => {
      menuOverlay.classList.add("visible");
      switchTab(currentTab);
    },
    onClosed: () => {
      menuOverlay.classList.remove("visible");
    },
  });
}

export function closeMenu() {
  if (menuContainer && surfaceManager.isOpen(menuContainer)) {
    surfaceManager.closeSurface(menuContainer);
  } else {
    menuOverlay?.classList.remove("visible");
  }
}

// ═══════════════════════════════════════════════════════════════════════════
// TAB 1: Виджеты и стол
// ═══════════════════════════════════════════════════════════════════════════

async function renderWidgetsTab() {
  const pane = document.getElementById("pane-widgets");
  if (!pane) return;

  pane.innerHTML = `<div class="menu-empty-state">Загрузка виджетов...</div>`;

  const widgets = await getWidgets().catch(() => []);
  if (!config.positions) config.positions = {};
  if (!Array.isArray(config.hidden)) config.hidden = [];

  const hiddenSet = new Set(config.hidden);
  const disabledSet = new Set(await getDisabledModules());
  // Modules with a Rust half, so the user can switch the background work off.
  // `clock` is absent on purpose: it has no crate, there is nothing to stop.
  const BACKEND_MODULE_IDS = ["mp3-player", "telegram", "glyph-bar", "vpn", "explorer", "micrographics"];
  const nonWidgetModules = ["vpn", "explorer"].filter((id) => !widgets.some((w) => w.id === id));

  pane.innerHTML = `
    <div class="menu-section">
      <div class="menu-section-header">
        <div>
          <div class="menu-section-title">
            <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
              <rect x="3" y="3" width="7" height="7"></rect>
              <rect x="14" y="3" width="7" height="7"></rect>
              <rect x="14" y="14" width="7" height="7"></rect>
              <rect x="3" y="14" width="7" height="7"></rect>
            </svg>
            Управление виджетами и положение на экране
          </div>
          <div class="menu-section-desc">Включение/выключение отображения на рабочем столе и точная настройка координат (px)</div>
        </div>
        <div style="display: flex; gap: 10px;">
          <button class="menu-btn" id="btnInteractiveEdit">
            🎯 Переместить на столе
          </button>
          <button class="menu-btn" id="btnResetPositions">
            ↺ Сброс позиций
          </button>
        </div>
      </div>

      <div class="widgets-mgmt-grid" id="widgetsGrid">
        ${widgets.map((w) => {
          const isHidden = hiddenSet.has(w.id);
          const pos = config.positions[w.id] || { x: 28, y: 28 };
          return `
            <div class="widget-mgmt-card" data-widget-id="${w.id}">
              <div class="widget-mgmt-header">
                <div class="widget-mgmt-info">
                  <div class="widget-mgmt-icon">${w.iconSvg || ""}</div>
                  <div>
                    <div class="widget-mgmt-name">${escapeHtml(w.name)}</div>
                    <div class="widget-mgmt-type">${w.isFolder ? "Пользовательская папка" : "Модуль системы"}</div>
                  </div>
                </div>
                <label class="menu-switch">
                  <input type="checkbox" class="widget-vis-toggle" ${!isHidden ? "checked" : ""} data-id="${w.id}">
                  <span class="menu-slider"></span>
                </label>
              </div>

              ${BACKEND_MODULE_IDS.includes(w.id) ? `
                <div class="widget-mgmt-module" style="margin-top: 8px; display: flex; align-items: center; gap: 8px;">
                  <span style="font-size: 11px; color: rgba(var(--fg-rgb), 0.55); flex: 1;">Модуль (бэкенд)</span>
                  <label class="menu-switch">
                    <input type="checkbox" class="mod-disable-toggle" data-id="${w.id}" ${disabledSet.has(w.id) ? "" : "checked"}>
                    <span class="menu-slider"></span>
                  </label>
                </div>
                <div class="mod-note" data-mod-note="${w.id}" style="${disabledSet.has(w.id) ? "" : "display:none;"} font-size: 11px; color: rgba(var(--fg-rgb), 0.35); padding: 4px 0 0;">Модуль выключен — бэкенд остановится, применится после перезапуска</div>
              ` : ""}

              ${!isHidden ? `
                <div class="widget-mgmt-coords">
                  <span style="font-size: 11px; color: rgba(var(--fg-rgb), 0.4);">Координаты:</span>
                  <div class="coord-group">
                    <span class="coord-label">X</span>
                    <input type="number" class="menu-input coord-input coord-x" data-id="${w.id}" value="${pos.x ?? 28}" step="10">
                  </div>
                  <div class="coord-group">
                    <span class="coord-label">Y</span>
                    <input type="number" class="menu-input coord-input coord-y" data-id="${w.id}" value="${pos.y ?? 28}" step="10">
                  </div>
                </div>
              ` : `
                <div style="font-size: 11px; color: rgba(var(--fg-rgb), 0.3); padding: 6px 0;">Виджет скрыт с рабочего стола</div>
              `}
            </div>
          `;
        }).join("")}
      </div>

      ${nonWidgetModules.length ? `
        <div style="margin-top: 20px;">
          <div class="menu-section-title">Прочие модули (бэкенд)</div>
          <div style="margin-top: 10px; display: flex; flex-direction: column; gap: 10px;">
            ${nonWidgetModules.map((id) => `
              <div style="display: flex; align-items: center; gap: 10px; padding: 10px 14px; background: rgba(var(--scrim-rgb), 0.3); border-radius: 12px; border: 1px solid rgba(var(--bg-elev-rgb), 0.1);">
                <span style="flex: 1; font-size: 13px;">${escapeHtml(id)}</span>
                <label class="menu-switch">
                  <input type="checkbox" class="mod-disable-toggle" data-id="${id}" ${disabledSet.has(id) ? "" : "checked"}>
                  <span class="menu-slider"></span>
                </label>
                <span data-mod-note="${id}" style="${disabledSet.has(id) ? "" : "display:none;"} font-size: 11px; color: rgba(var(--fg-rgb), 0.35);">после перезапуска</span>
              </div>
            `).join("")}
          </div>
        </div>
      ` : ""}
    </div>
  `;

  // Visibility toggle
  pane.querySelectorAll(".widget-vis-toggle").forEach((toggle) => {
    toggle.addEventListener("change", () => runOnce(toggle, async () => {
      const id = toggle.dataset.id;
      const previous = [...(config.hidden || [])];
      config.hidden = toggle.checked
        ? previous.filter((h) => h !== id)
        : [...new Set([...previous, id])];
      if (!(await saveConfig())) {
        config.hidden = previous;
        toggle.checked = !toggle.checked;
        return;
      }
      await syncWidgets();
      renderWidgetsTab();
    }));
  });

  // Module disable toggle (backend off). Persisted; takes effect on the next
  // app start (the module is not initialized / skips its heavy background).
  pane.querySelectorAll(".mod-disable-toggle").forEach((tgl) => {
    tgl.addEventListener("change", () => {
      const id = tgl.dataset.id;
      const checked = tgl.checked;
      // Read-modify-write of one list: chained so a second toggle reads the
      // first one's result instead of the same stale list.
      moduleToggleChain = moduleToggleChain.then(async () => {
        const current = new Set(await getDisabledModules());
        if (checked) current.delete(id);
        else current.add(id);
        const ok = await setDisabledModules([...current]);
        if (!ok) tgl.checked = !checked;
      }).catch((err) => {
        console.error("module toggle failed:", err);
        tgl.checked = !checked;
      }).finally(() => renderWidgetsTab());
    });
  });

  // Position inputs
  pane.querySelectorAll(".coord-input").forEach((inp) => {
    inp.addEventListener("change", async (e) => {
      const id = inp.dataset.id;
      const isX = inp.classList.contains("coord-x");
      const val = parseInt(inp.value, 10) || 0;
      if (!config.positions[id]) config.positions[id] = { x: 28, y: 28 };
      if (isX) config.positions[id].x = val;
      else config.positions[id].y = val;

      const card = document.querySelector(`.widget-card[data-id="${id}"]`);
      if (card) {
        if (isX) card.style.left = val + "px";
        else card.style.top = val + "px";
      }

      await saveConfig();
      relayoutWidgets();
    });
  });

  // Interactive edit button (opens edit mode on desktop)
  pane.querySelector("#btnInteractiveEdit")?.addEventListener("click", () => {
    closeMenu();
    const editBtn = document.getElementById("editMode");
    if (editBtn && !document.body.classList.contains("edit-mode")) {
      editBtn.click();
    }
  });

  // Reset positions button
  const resetBtn = pane.querySelector("#btnResetPositions");
  resetBtn?.addEventListener("click", () => runOnce(resetBtn, async () => {
    const defaults = (await getDefaultWidgetConfig()).positions || {};
    const canvas = document.getElementById("widgets");
    const maxX = Math.max(0, (canvas?.clientWidth || window.innerWidth) - 120);
    const maxY = Math.max(0, (canvas?.clientHeight || window.innerHeight) - 64);
    // Geometry only: settings stored next to it (clock face, AI DJ,
    // micrographics style) are the user's and survive a layout reset.
    Object.entries(config.positions || {}).forEach(([id, pos]) => {
      const def = defaults[id];
      if (def) {
        pos.x = def.x;
        pos.y = def.y;
        if (def.w) pos.w = def.w;
        if (def.h) pos.h = def.h;
        return;
      }
      // No kernel default (folders, instances, satellites): keep the place,
      // only pull it back on screen.
      if (typeof pos.x === "number") pos.x = Math.min(Math.max(0, pos.x), maxX);
      if (typeof pos.y === "number") pos.y = Math.min(Math.max(0, pos.y), maxY);
    });
    await saveConfig();
    await syncWidgets();
    renderWidgetsTab();
  }));
}

// ═══════════════════════════════════════════════════════════════════════════
// TAB 2: Папки ярлыков (redb)
// ═══════════════════════════════════════════════════════════════════════════

let isCreatingFolder = false;
let selectedFolderIcon = "folder";

async function renderFoldersTab() {
  const pane = document.getElementById("pane-folders");
  if (!pane) return;

  pane.innerHTML = `<div class="menu-empty-state">Загрузка папок...</div>`;

  let folders = [];
  try {
    folders = await invoke("get_folders");
  } catch (e) {
    console.error("get_folders error:", e);
  }

  pane.innerHTML = `
    <div class="menu-section">
      <div class="menu-section-header">
        <div>
          <div class="menu-section-title">
            <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
              <path d="M22 19a2 2 0 01-2 2H4a2 2 0 01-2-2V5a2 2 0 012-2h5l2 3h9a2 2 0 012 2z"></path>
            </svg>
            Пользовательские папки
          </div>
          <div class="menu-section-desc">Создавайте тематические папки с веб-ссылками, файлами и приложениями. Данные хранятся в базе redb.</div>
        </div>
        <button class="menu-btn menu-btn-primary" id="btnToggleCreateFolder">
          + Создать папку
        </button>
      </div>

      <div id="createFolderBox" class="${isCreatingFolder ? "" : "hidden"}" style="margin-bottom: 20px; padding: 18px 20px; background: rgba(var(--scrim-rgb), 0.35); border-radius: 14px; border: 1px solid rgba(var(--bg-elev-rgb), 0.12);">
        <div style="font-size: 13px; font-weight: 600; margin-bottom: 12px; color: rgb(var(--fg-rgb));">Новая папка на рабочем столе</div>
        <div style="display: flex; gap: 12px; margin-bottom: 14px;">
          <input type="text" class="menu-input" id="newFolderName" placeholder="Название папки (например: Игры, Работа, Ссылки)" style="flex: 1;">
          <button class="menu-btn menu-btn-primary" id="btnConfirmCreateFolder">Создать</button>
          <button class="menu-btn" id="btnCancelCreateFolder">Отмена</button>
        </div>
        <div style="font-size: 11px; color: rgba(var(--fg-rgb), 0.45); margin-bottom: 8px;">Выберите иконку папки:</div>
        <div style="display: flex; gap: 8px; flex-wrap: wrap;" id="iconPicker">
          ${Object.entries(FOLDER_ICONS).map(([key, svg]) => `
            <button type="button" class="menu-btn icon-pick-btn ${selectedFolderIcon === key ? "active" : ""}" data-icon="${key}" style="padding: 6px 10px; ${selectedFolderIcon === key ? "border-color: rgb(var(--fg-rgb)); background: rgba(var(--bg-elev-rgb), 0.2);" : ""}">
              ${svg}
            </button>
          `).join("")}
        </div>
      </div>

      ${(!folders || folders.length === 0) ? `
        <div class="menu-empty-state">
          <div class="menu-empty-icon">
            <svg width="40" height="40" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.2">
              <path d="M22 19a2 2 0 01-2 2H4a2 2 0 01-2-2V5a2 2 0 012-2h5l2 3h9a2 2 0 012 2z"></path>
            </svg>
          </div>
          <div class="menu-empty-title">Нет созданных папок</div>
          <div class="menu-empty-desc">У вас пока нет папок с ярлыками. Нажмите «+ Создать папку», чтобы добавить первую папку на рабочий стол.</div>
        </div>
      ` : `
        <div class="folders-list">
          ${folders.map((f) => `
            <div class="folder-mgmt-card" data-folder-id="${f.id}">
              <div class="folder-top-row">
                <div class="folder-title-wrap">
                  <div class="folder-icon-box">${f.icon_svg || FOLDER_ICONS.folder}</div>
                  <input type="text" class="menu-input folder-name-input" data-id="${f.id}" value="${escapeHtml(f.name)}">
                  <span style="font-size: 12px; color: rgba(var(--fg-rgb), 0.38);">(${(f.items || []).length} ярлыков)</span>
                </div>
                <button class="menu-btn menu-btn-danger btn-delete-folder" data-id="${f.id}">
                  <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                    <polyline points="3 6 5 6 21 6"></polyline>
                    <path d="M19 6v14a2 2 0 01-2 2H7a2 2 0 01-2-2V6m3 0V4a2 2 0 012-2h4a2 2 0 012 2v2"></path>
                  </svg>
                  Удалить
                </button>
              </div>

              ${(!f.items || f.items.length === 0) ? `
                <div style="font-size: 12px; color: rgba(var(--fg-rgb), 0.35); padding: 8px 0;">В папке пока нет ярлыков</div>
              ` : `
                <table class="shortcuts-table">
                  <thead>
                    <tr>
                      <th style="width: 25%;">Название</th>
                      <th style="width: 15%;">Тип</th>
                      <th style="width: 50%;">Цель (URL или Путь)</th>
                      <th style="width: 10%;"></th>
                    </tr>
                  </thead>
                  <tbody>
                    ${f.items.map((it) => `
                      <tr data-item-id="${it.id}">
                        <td><strong>${escapeHtml(it.name)}</strong></td>
                        <td><span class="shortcut-kind-pill">${escapeHtml(it.kind || "file")}</span></td>
                        <td class="shortcut-target-cell" aria-label="${escapeHtml(it.target)}">${escapeHtml(it.target)}</td>
                        <td style="text-align: right;">
                          <button class="menu-btn menu-btn-danger btn-delete-shortcut" data-folder-id="${f.id}" data-item-id="${it.id}" style="padding: 4px 8px;" aria-label="Удалить ярлык">
                            ✕
                          </button>
                        </td>
                      </tr>
                    `).join("")}
                  </tbody>
                </table>
              `}

              <!-- Add shortcut inline row -->
              <div class="add-shortcut-bar" data-folder-id="${f.id}">
                <input type="text" class="menu-input add-sc-name" placeholder="Имя ярлыка (YouTube)" />
                <select class="menu-select add-sc-kind">
                  <option value="url">Веб (URL)</option>
                  <option value="folder">Папка</option>
                  <option value="file">Файл / ПО</option>
                </select>
                <input type="text" class="menu-input add-sc-target" placeholder="https://youtube.com или C:\\..." />
                <button class="menu-btn menu-btn-primary btn-add-shortcut" data-folder-id="${f.id}">
                  + Добавить
                </button>
              </div>
            </div>
          `).join("")}
        </div>
      `}
    </div>
  `;

  // Toggle create folder box
  pane.querySelector("#btnToggleCreateFolder")?.addEventListener("click", () => {
    isCreatingFolder = !isCreatingFolder;
    renderFoldersTab();
  });

  pane.querySelector("#btnCancelCreateFolder")?.addEventListener("click", () => {
    isCreatingFolder = false;
    renderFoldersTab();
  });

  // Icon picker
  pane.querySelectorAll(".icon-pick-btn").forEach((btn) => {
    btn.addEventListener("click", () => {
      selectedFolderIcon = btn.dataset.icon;
      renderFoldersTab();
    });
  });

  // Create folder confirm
  const createBtn = pane.querySelector("#btnConfirmCreateFolder");
  createBtn?.addEventListener("click", () => runOnce(createBtn, async () => {
    const nameInp = pane.querySelector("#newFolderName");
    const name = (nameInp?.value || "").trim();
    if (!name) return;

    const newFolder = {
      id: `folder_${crypto.randomUUID()}`,
      name,
      icon_svg: FOLDER_ICONS[selectedFolderIcon] || FOLDER_ICONS.folder,
      items: [],
    };

    try {
      await invoke("save_folder", { folder: newFolder });
      isCreatingFolder = false;
      await syncWidgets();
      renderFoldersTab();
    } catch (e) {
      alert(`Ошибка создания папки: ${e}`);
    }
  }));

  // Rename folder on change
  pane.querySelectorAll(".folder-name-input").forEach((inp) => {
    inp.addEventListener("change", async () => {
      const id = inp.dataset.id;
      const newName = inp.value.trim();
      const target = folders.find((f) => f.id === id);
      if (!target) return;
      if (!newName) {
        inp.value = target.name;
        return;
      }
      try {
        await invoke("save_folder", { folder: { ...target, name: newName } });
        target.name = newName;
        await syncWidgets();
      } catch (e) {
        inp.value = target.name;
        alert(`Ошибка переименования: ${e}`);
      }
    });
  });

  // Delete folder
  pane.querySelectorAll(".btn-delete-folder").forEach((btn) => {
    btn.addEventListener("click", async () => {
      const id = btn.dataset.id;
      if (!confirm("Удалить эту папку и все её ярлыки?")) return;
      try {
        await invoke("delete_folder", { id });
        delete config.positions?.[id];
        config.hidden = (config.hidden || []).filter((h) => h !== id);
        config.instances = (config.instances || []).filter((it) => it.widgetType !== id);
        await saveConfig();
        await syncWidgets();
        renderFoldersTab();
      } catch (e) {
        alert(`Ошибка удаления: ${e}`);
      }
    });
  });

  // Add shortcut
  pane.querySelectorAll(".btn-add-shortcut").forEach((btn) => {
    btn.addEventListener("click", () => runOnce(btn, async () => {
      const folderId = btn.dataset.folderId;
      const card = btn.closest(".folder-mgmt-card");
      if (!card) return;

      const nameInp = card.querySelector(".add-sc-name");
      const kindSel = card.querySelector(".add-sc-kind");
      const targetInp = card.querySelector(".add-sc-target");

      const scName = (nameInp?.value || "").trim();
      const scKind = (kindSel?.value || "file").trim();
      const scTarget = (targetInp?.value || "").trim();

      if (!scName || !scTarget) {
        alert("Укажите название ярлыка и путь/URL");
        return;
      }

      const targetFolder = folders.find((f) => f.id === folderId);
      if (!targetFolder) return;

      const items = [...(targetFolder.items || []), {
        id: `item_${crypto.randomUUID()}`,
        name: scName,
        kind: scKind,
        target: scTarget,
      }];

      try {
        await invoke("save_folder", { folder: { ...targetFolder, items } });
        targetFolder.items = items;
        await syncWidgets();
        renderFoldersTab();
      } catch (e) {
        alert(`Ошибка сохранения: ${e}`);
      }
    }));
  });

  // Delete shortcut
  pane.querySelectorAll(".btn-delete-shortcut").forEach((btn) => {
    btn.addEventListener("click", () => runOnce(btn, async () => {
      const folderId = btn.dataset.folderId;
      const itemId = btn.dataset.itemId;
      const targetFolder = folders.find((f) => f.id === folderId);
      if (!targetFolder || !targetFolder.items) return;

      const items = targetFolder.items.filter((it) => it.id !== itemId);
      try {
        await invoke("save_folder", { folder: { ...targetFolder, items } });
        targetFolder.items = items;
        await syncWidgets();
        renderFoldersTab();
      } catch (e) {
        alert(`Ошибка удаления: ${e}`);
      }
    }));
  });
}

// ═══════════════════════════════════════════════════════════════════════════
// TAB 3: VPN и Сеть
// ═══════════════════════════════════════════════════════════════════════════

async function renderVpnTab() {
  const pane = document.getElementById("pane-vpn");
  if (!pane) return;

  pane.innerHTML = `<div class="menu-empty-state">Загрузка настроек VPN...</div>`;

  let settings = null;
  let status = null;
  try {
    [settings, status] = await Promise.all([
      invoke("vpn_settings"),
      invoke("vpn_status").catch(() => null),
    ]);
  } catch (e) {
    console.error("vpn status/settings error:", e);
  }

  // Without the stored settings the form would show blanks, and saving it
  // would overwrite the real subscription and server with them.
  if (!settings) {
    pane.innerHTML = `
      <div class="menu-empty-state">
        Не удалось загрузить настройки VPN. Форма скрыта, чтобы не затереть сохранённую подписку.
        <div style="margin-top: 12px;"><button class="menu-btn" id="btnVpnRetry">Повторить</button></div>
      </div>`;
    pane.querySelector("#btnVpnRetry")?.addEventListener("click", () => renderVpnTab());
    return;
  }

  const home = settings.home_server || {
    enabled: false,
    host: "",
    port: 443,
    uuid: "",
    public_key: "",
    short_id: "",
    sni: "",
    subnet: "10.10.0.0/16",
  };

  const isConnected = !!status?.connected;
  const statusLabel = !status ? "Статус неизвестен" : isConnected ? "Подключено" : "Отключено";

  pane.innerHTML = `
    <!-- VPN Subscription Section -->
    <div class="menu-section">
      <div class="menu-section-header">
        <div>
          <div class="menu-section-title">
            <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
              <path d="M12 22s8-4 8-10V5l-8-3-8 3v7c0 6 8 10 8 10z"></path>
            </svg>
            Подписка VPN
          </div>
          <div class="menu-section-desc">Настройка внешней VLESS / Singbox подписки и автоматического подключения</div>
        </div>
        <div>
          <span class="menu-status-badge ${isConnected ? "active" : ""}">
            <span style="width: 6px; height: 6px; border-radius: 50%; background: ${isConnected ? "var(--status-ok)" : "rgba(var(--fg-rgb), 0.53)"};"></span>
            ${statusLabel}
          </span>
        </div>
      </div>

      <div class="vpn-field-row">
        <input type="text" class="menu-input" id="vpnSubUrl" placeholder="https://your-provider.com/sub/..." value="${escapeHtml(settings?.subscription_url || "")}" />
        <button class="menu-btn menu-btn-primary" id="btnSaveVpnSub">Сохранить подписку</button>
      </div>

      <div style="margin-top: 16px;">
        <label class="menu-switch-label">
          <div>
            <div style="font-size: 13px; font-weight: 500; color: rgb(var(--fg-rgb));">Автоматическое подключение при старте</div>
            <div style="font-size: 11px; color: rgba(var(--fg-rgb), 0.45);">Автоматически запускать VPN при запуске UniDTK</div>
          </div>
          <div class="menu-switch">
            <input type="checkbox" id="vpnAutoConnectToggle" ${settings?.auto_connect ? "checked" : ""}>
            <span class="menu-slider"></span>
          </div>
        </label>
      </div>

      <div class="menu-toast" id="vpnSubToast"></div>
    </div>

    <!-- Home Server Submodule Section -->
    <div class="menu-section">
      <div class="menu-section-header">
        <div>
          <div class="menu-section-title">
            <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
              <rect x="2" y="2" width="20" height="8" rx="2" ry="2"></rect>
              <rect x="2" y="14" width="20" height="8" rx="2" ry="2"></rect>
              <line x1="6" y1="6" x2="6.01" y2="6"></line>
              <line x1="6" y1="18" x2="6.01" y2="18"></line>
            </svg>
            Подмодуль: Домашний сервер (VLESS)
          </div>
          <div class="menu-section-desc">Изолированный отключаемый подмодуль для прямого VLESS Reality подключения к личному серверу</div>
        </div>
        <label class="menu-switch">
          <input type="checkbox" id="homeServerToggle" ${home.enabled ? "checked" : ""}>
          <span class="menu-slider"></span>
        </label>
      </div>

      <div id="homeServerFieldsBox" class="${home.enabled ? "" : "hidden"}">
        <div class="vpn-submodule-box">
          <div class="vpn-submodule-fields">
            <div class="vpn-field-group">
              <label>Хост / IP</label>
              <input type="text" class="menu-input" id="hsHost" placeholder="213.108.x.x или domain" value="${escapeHtml(home.host || "")}">
            </div>
            <div class="vpn-field-group">
              <label>Порт</label>
              <input type="number" class="menu-input" id="hsPort" placeholder="443" value="${home.port || 443}">
            </div>
            <div class="vpn-field-group">
              <label>UUID пользователя</label>
              <input type="text" class="menu-input" id="hsUuid" placeholder="xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx" value="${escapeHtml(home.uuid || "")}">
            </div>
            <div class="vpn-field-group">
              <label>Public Key</label>
              <input type="text" class="menu-input" id="hsPublicKey" placeholder="Reality public key" value="${escapeHtml(home.public_key || "")}">
            </div>
            <div class="vpn-field-group">
              <label>Short ID</label>
              <input type="text" class="menu-input" id="hsShortId" placeholder="Reality short ID" value="${escapeHtml(home.short_id || "")}">
            </div>
            <div class="vpn-field-group">
              <label>SNI (Server Name)</label>
              <input type="text" class="menu-input" id="hsSni" placeholder="example.com" value="${escapeHtml(home.sni || "")}">
            </div>
            <div class="vpn-field-group">
              <label>LAN Подсеть / Subnet</label>
              <input type="text" class="menu-input" id="hsSubnet" placeholder="10.10.0.0/16" value="${escapeHtml(home.subnet || "10.10.0.0/16")}">
            </div>
          </div>

          <div style="margin-top: 16px; display: flex; justify-content: flex-end;">
            <button class="menu-btn menu-btn-primary" id="btnSaveHomeServer">
              Сохранить конфигурацию сервера
            </button>
          </div>
        </div>
      </div>

      <div class="menu-toast" id="homeServerToast"></div>
    </div>
  `;

  // Subscription save
  const saveSubBtn = pane.querySelector("#btnSaveVpnSub");
  saveSubBtn?.addEventListener("click", () => runOnce(saveSubBtn, async () => {
    const url = (pane.querySelector("#vpnSubUrl")?.value || "").trim();
    const toast = pane.querySelector("#vpnSubToast");
    try {
      await invoke("vpn_set_subscription_url", { url });
      if (toast) {
        toast.className = "menu-toast visible success";
        toast.textContent = "Ссылка на подписку сохранена. Список серверов обновлен.";
        setTimeout(() => toast.classList.remove("visible"), 4000);
      }
    } catch (e) {
      if (toast) {
        toast.className = "menu-toast visible error";
        toast.textContent = `Ошибка сохранения: ${e}`;
      }
    }
  }));

  // Auto connect toggle
  pane.querySelector("#vpnAutoConnectToggle")?.addEventListener("change", async (e) => {
    const toggle = e.target;
    try {
      await invoke("vpn_set_auto_connect", { enabled: toggle.checked });
    } catch (err) {
      console.error("vpn_set_auto_connect failed:", err);
      toggle.checked = !toggle.checked;
      const toast = pane.querySelector("#vpnSubToast");
      if (toast) {
        toast.className = "menu-toast visible error";
        toast.textContent = `Автоподключение не сохранено: ${err}`;
      }
    }
  });

  // Home server toggle
  pane.querySelector("#homeServerToggle")?.addEventListener("change", (e) => {
    const box = pane.querySelector("#homeServerFieldsBox");
    if (box) box.classList.toggle("hidden", !e.target.checked);
  });

  // Home server save
  const saveHomeBtn = pane.querySelector("#btnSaveHomeServer");
  saveHomeBtn?.addEventListener("click", () => runOnce(saveHomeBtn, async () => {
    const isEnabled = pane.querySelector("#homeServerToggle")?.checked ?? false;
    const config = {
      enabled: isEnabled,
      host: (pane.querySelector("#hsHost")?.value || "").trim(),
      port: parseInt(pane.querySelector("#hsPort")?.value || "443", 10) || 443,
      uuid: (pane.querySelector("#hsUuid")?.value || "").trim(),
      public_key: (pane.querySelector("#hsPublicKey")?.value || "").trim(),
      short_id: (pane.querySelector("#hsShortId")?.value || "").trim(),
      sni: (pane.querySelector("#hsSni")?.value || "").trim(),
      subnet: (pane.querySelector("#hsSubnet")?.value || "10.10.0.0/16").trim(),
    };

    const toast = pane.querySelector("#homeServerToast");
    try {
      await invoke("vpn_set_home_server_config", { config });
      if (toast) {
        toast.className = "menu-toast visible success";
        toast.textContent = "Настройки домашнего сервера успешно сохранены в redb.";
        setTimeout(() => toast.classList.remove("visible"), 4000);
      }
    } catch (e) {
      if (toast) {
        toast.className = "menu-toast visible error";
        toast.textContent = `Ошибка сохранения: ${e}`;
      }
    }
  }));
}

// ═══════════════════════════════════════════════════════════════════════════
// TAB 4: Оформление и Анимации (theme.js & motion.js)
// ═══════════════════════════════════════════════════════════════════════════

// Palette fields of theme schema v2 (theme.js). kind "rgb" fields hold an
// "r, g, b" string and need hex conversion for <input type="color">.
const PALETTE_FIELDS = [
  { field: "fgRgb", label: "Текст и иконки", kind: "rgb" },
  { field: "bgElevRgb", label: "Тинт элементов", kind: "rgb" },
  { field: "surfaceRgb", label: "Фон поверхностей", kind: "rgb" },
  { field: "surfaceDeepRgb", label: "Глубокий фон", kind: "rgb" },
  { field: "scrimRgb", label: "Затемнение", kind: "rgb" },
  { field: "statusOk", label: "Статус: норма", kind: "hex" },
  { field: "statusWarn", label: "Статус: внимание", kind: "hex" },
  { field: "statusError", label: "Статус: ошибка", kind: "hex" },
  { field: "statusInfo", label: "Статус: инфо", kind: "hex" },
];

// Material section (theme schema v4). Labels live here; keys come from theme.js.
// Slot keys are stable ids; their hues are random per user, so labels are
// neutral numbers rather than color names. Coral keeps its hue (warm accent).
const SLOT_LABELS = {
  neutral: "Нейтральный", blue: "Цвет 1", aqua: "Цвет 2", mint: "Цвет 3",
  lilac: "Цвет 4", peach: "Цвет 5", magenta: "Цвет 6", coral: "Коралловый",
};

const TINT_LABELS = {
  panel: "Панели: меню, проводник", folder: "Папки", clock: "Часы", telegram: "Telegram",
  "mp3-player": "Плеер", "glyph-bar": "Glyph Bar",
  "mg.date": "Дата", "mg.time": "Время", "mg.freshness": "Устаревание", "mg.temperature": "Температура",
  "mg.condition": "Погода", "mg.feels": "Ощущается", "mg.wind": "Ветер", "mg.precipitation": "Осадки",
  "mg.precipitation-amount": "Объём осадков", "mg.sunrise": "Восход", "mg.sunset": "Закат",
  "mg.daylight": "Световой день", "mg.geolocation": "Геолокация", "mg.location": "Место",
  "mg.ip-location": "Место по IP", "mg.network": "Сеть", "mg.volume": "Громкость",
};

// Fine tuning sliders: one generic live-preview/commit handler serves all.
// radiusStrip shows its top value as a full pill (999 px).
const MATERIAL_SLIDERS = [
  { field: "glassGrain", label: "Зерно", min: 0, max: 1, step: 0.02, fmt: (v) => `${Math.round(v * 100)}%` },
  { field: "glassDepth", label: "Глубина (тёмная кромка)", min: 0, max: 1, step: 0.05, fmt: (v) => `${Math.round(v * 100)}%` },
  { field: "glassSheen", label: "Блик", min: 0, max: 1, step: 0.05, fmt: (v) => `${Math.round(v * 100)}%` },
  { field: "glassRim", label: "Светлый ободок", min: 0, max: 1, step: 0.05, fmt: (v) => `${Math.round(v * 100)}%` },
  { field: "glassShadow", label: "Тень", min: 0, max: 1, step: 0.05, fmt: (v) => `${Math.round(v * 100)}%` },
  { field: "shadowOffset", label: "Сдвиг тени", min: 0, max: 24, step: 1, fmt: (v) => `${v}px` },
  { field: "inkDepth", label: "Светлота текста (меньше — глубже)", min: 0.15, max: 0.38, step: 0.01, fmt: (v) => v.toFixed(2) },
  { field: "surfaceTint", label: "Оттенок раскрытых окон", min: 0, max: 1, step: 0.05, fmt: (v) => `${Math.round(v * 100)}%` },
  { field: "radiusStrip", label: "Скругление полос", min: 0, max: 64, step: 2, fmt: (v) => (v >= 64 ? "пилюля" : `${v}px`),
    toUi: (v) => Math.min(64, v), fromUi: (v) => (v >= 64 ? 999 : v) },
];

let materialDetailsOpen = { mg: false, fine: false };

function renderTintRow(target, current) {
  const slot = current.tintMap[target];
  return `
    <div class="theme-tint-row">
      <span class="theme-tint-name">${TINT_LABELS[target] || target}</span>
      <div class="theme-tint-dots">
        ${PALETTE_SLOTS.map((s) => `
          <button type="button" class="theme-tint-dot ${s === slot ? "active" : ""}" data-target="${target}" data-slot="${s}"
            style="background: ${safeColor(current.palette[s], "#ffffff")};" title="${SLOT_LABELS[s]}" aria-label="${TINT_LABELS[target]}: ${SLOT_LABELS[s]}"></button>
        `).join("")}
      </div>
    </div>`;
}

function rgbToHexPicker(rgb) {
  const m = String(rgb || "").match(/(\d{1,3})\s*,\s*(\d{1,3})\s*,\s*(\d{1,3})/);
  if (!m) return "#ffffff";
  const part = (n) => Math.max(0, Math.min(255, Number(n))).toString(16).padStart(2, "0");
  return `#${part(m[1])}${part(m[2])}${part(m[3])}`;
}

function hexToRgbString(hex) {
  const h = String(hex || "").replace("#", "");
  if (!/^[0-9a-fA-F]{6}$/.test(h)) return null;
  return `${parseInt(h.slice(0, 2), 16)}, ${parseInt(h.slice(2, 4), 16)}, ${parseInt(h.slice(4, 6), 16)}`;
}

// Only validated hex values may enter HTML attributes (defense in depth —
// normalizeTheme already guarantees the shape for everything it returns).
function safeColor(value, fallback) {
  return /^#[0-9a-fA-F]{6}$/.test(value) ? value : fallback;
}

function renderThemeTab() {
  const pane = document.getElementById("pane-theme");
  if (!pane) return;

  const current = getActiveTheme();
  const presets = getAllPresets();

  const COLOR_CHIPS = [
    { label: "Nothing Red", val: "#d71921" },
    { label: "Amber", val: "#ffaa00" },
    { label: "Cyan", val: "#00d2ff" },
    { label: "Emerald", val: "#10b981" },
    { label: "Purple", val: "#a855f7" },
    { label: "White", val: "#ffffff" },
    { label: "Slate", val: "#a0a5b5" },
  ];

  const MOTION_PROFILES = [
    { id: "responsive", label: "Отзывчивый" },
    { id: "balanced", label: "Сбалансированный" },
    { id: "fluid", label: "Плавный" },
  ];

  // Must stay in sync with FONT_BODY_OPTIONS in theme.js (exact strings).
  const FONT_BODY_OPTIONS = [
    { value: "'Geist', sans-serif", label: "Geist" },
    { value: "'Segoe UI', 'Geist', sans-serif", label: "Segoe UI" },
    { value: "system-ui, sans-serif", label: "Системный" },
    { value: "'Ndot', monospace", label: "Ndot" },
  ];

  pane.innerHTML = `
    <div class="theme-tab-layout">
      <!-- Section 1: Presets -->
      <div class="theme-section">
        <div class="theme-section-header">
          <div class="theme-section-title">
            <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
              <path d="M12 2l3.09 6.26L22 9.27l-5 4.87 1.18 6.88L12 17.77l-6.18 3.25L7 14.14 2 9.27l6.91-1.01L12 2z"></path>
            </svg>
            Пресеты стиля и анимаций
          </div>
          <span style="font-size: 11px; color: rgba(var(--fg-rgb), 0.45);">Мгновенное переключение без перезапуска</span>
        </div>

        <div class="theme-presets-grid" id="themePresetsGrid">
          ${presets.map((p) => {
            const accent = safeColor(p.accent, "#d71921");
            return `
            <div class="theme-preset-card ${p.id === current.id ? "active" : ""}" data-preset-id="${p.id}">
              <div class="theme-preset-info">
                <span class="theme-color-dot" style="background: ${accent}; ${accent === '#ffffff' ? 'box-shadow: 0 0 8px rgba(255,255,255,0.8);' : ''}"></span>
                <span class="theme-preset-name">${escapeHtml(p.name)}</span>
                ${p.isBuiltin ? '' : '<span class="theme-preset-badge">Свой</span>'}
              </div>
              ${p.isBuiltin ? '' : `
                <button type="button" class="theme-preset-delete-btn" data-delete-id="${p.id}" aria-label="Удалить пользовательский пресет">
                  <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                    <line x1="18" y1="6" x2="6" y2="18"></line>
                    <line x1="6" y1="6" x2="18" y2="18"></line>
                  </svg>
                </button>
              `}
            </div>
          `;}).join("")}
        </div>

        <div class="theme-save-preset-row">
          <input type="text" class="theme-save-input" id="newPresetName" placeholder="Название нового пресета (например: Мой Киберпанк)..." maxlength="32">
          <button type="button" class="theme-save-btn" id="btnSavePreset">
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
              <line x1="12" y1="5" x2="12" y2="19"></line>
              <line x1="5" y1="12" x2="19" y2="12"></line>
            </svg>
            Сохранить стиль
          </button>
        </div>

        <div class="theme-save-preset-row">
          <span style="font-size: 11px; color: rgba(var(--fg-rgb), 0.55);">Размеры виджетов — из активного пресета. Свои правки переживают смену стиля.</span>
          <button type="button" class="theme-save-btn" id="btnResetSizes">
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
              <polyline points="1 4 1 10 7 10"></polyline>
              <path d="M3.51 15a9 9 0 105.64-11.36L1 10"></path>
            </svg>
            Сбросить размеры
          </button>
        </div>
      </div>

      <!-- Section 2: Interactive Controls & Live Preview -->
      <div class="theme-section">
        <div class="theme-section-header">
          <div class="theme-section-title">
            <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
              <circle cx="12" cy="12" r="3"></circle>
              <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1 0 2.83 2 2 0 0 1-2.83 0l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-2 2 2 2 0 0 1-2-2v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83 0 2 2 0 0 1 0-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1-2-2 2 2 0 0 1 2-2h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 0-2.83 2 2 0 0 1 2.83 0l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 2-2 2 2 0 0 1 2 2v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 0 2 2 0 0 1 0 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 2 2 2 2 0 0 1-2 2h-.09a1.65 1.65 0 0 0-1.51 1z"></path>
            </svg>
            Параметры стилей и Live-превью
          </div>
        </div>

        <div class="theme-interactive-grid">
          <!-- Sliders Column -->
          <div class="theme-sliders-col">
            <!-- Accent Color -->
            <div class="theme-slider-row">
              <div class="theme-slider-label">
                <span>Акцентный цвет интерфейса</span>
                <span class="theme-slider-val" id="valAccent">${current.accent}</span>
              </div>
              <div class="theme-color-chips-row" id="themeColorChipsRow">
                ${COLOR_CHIPS.map((c) => `
                  <button type="button" class="theme-color-chip ${c.val.toLowerCase() === current.accent.toLowerCase() ? "active" : ""}"
                    style="background: ${c.val};" data-color="${c.val}" aria-label="${c.label}">
                  </button>
                `).join("")}
                <label class="theme-custom-color-btn">
                  <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                    <circle cx="12" cy="12" r="10"></circle>
                    <line x1="12" y1="8" x2="12" y2="16"></line>
                    <line x1="8" y1="12" x2="16" y2="12"></line>
                  </svg>
                  Спектр…
                  <input type="color" class="theme-native-color-picker" id="pickerCustomColor" value="${current.accent}">
                </label>
              </div>
            </div>

            <!-- Glass Opacity -->
            <div class="theme-slider-row">
              <div class="theme-slider-label">
                <span>Непрозрачность стекла (Glass Opacity)</span>
                <span class="theme-slider-val" id="valOpacity">${Math.round(current.glassOpacity * 100)}%</span>
              </div>
              <input type="range" class="theme-range-input" id="sliderOpacity"
                min="0.4" max="1.0" step="0.02" value="${current.glassOpacity}">
            </div>

            <!-- Blur Strength -->
            <div class="theme-slider-row">
              <div class="theme-slider-label">
                <span>Сила размытия фона (Blur Strength)</span>
                <span class="theme-slider-val" id="valBlur">${current.blur}px</span>
              </div>
              <input type="range" class="theme-range-input" id="sliderBlur"
                min="0" max="36" step="4" value="${current.blur}">
            </div>

            <!-- Corner Radius -->
            <div class="theme-slider-row">
              <div class="theme-slider-label">
                <span>Скругление углов карточек (Corner Radius)</span>
                <span class="theme-slider-val" id="valRadius">${current.radiusCard}px</span>
              </div>
              <input type="range" class="theme-range-input" id="sliderRadius"
                min="0" max="28" step="2" value="${current.radiusCard}">
            </div>

            <!-- Motion Scale -->
            <div class="theme-slider-row">
              <div class="theme-slider-label">
                <span>Скорость анимаций (Motion Duration Scale)</span>
                <span class="theme-slider-val" id="valMotion">${Number(current.motionScale ?? 1.0).toFixed(2)}x</span>
              </div>
              <input type="range" class="theme-range-input" id="sliderMotion"
                min="0.0" max="2.0" step="0.05" value="${current.motionScale ?? 1.0}">
            </div>

            <!-- Motion Profiles -->
            <div class="theme-slider-row">
              <div class="theme-slider-label">
                <span>Профиль динамики переходов</span>
              </div>
              <div class="theme-motion-pills" id="themeMotionPillsRow">
                ${MOTION_PROFILES.map((mp) => `
                  <button type="button" class="theme-motion-pill ${mp.id === (current.motionProfile || "balanced") ? "active" : ""}"
                    data-profile="${mp.id}">
                    ${mp.label}
                  </button>
                `).join("")}
              </div>
            </div>
            <!-- Font Family (body) -->
            <div class="theme-slider-row">
              <div class="theme-slider-label">
                <span>Шрифт текста (пресетный токен)</span>
                <span class="theme-slider-val" id="valFontBody">Aa</span>
              </div>
              <div class="theme-motion-pills" id="themeFontBodyRow">
                ${FONT_BODY_OPTIONS.map((f) => `
                  <button type="button" class="theme-motion-pill ${f.value === current.fontBody ? "active" : ""}"
                    data-font="${f.value}" title="${f.label}">
                    ${f.label}
                  </button>
                `).join("")}
              </div>
            </div>
          </div>

          <!-- Preview Column -->
          <div class="theme-preview-col">
            <div class="theme-preview-card" id="themePreviewCard">
              <div class="theme-preview-header">
                <div class="theme-preview-brand">
                  <span class="theme-preview-dot" id="themePreviewDot" style="background: ${current.accent};"></span>
                  <span>Виджет // Превью</span>
                </div>
                <span style="font-size: 11px; opacity: 0.5;">Live</span>
              </div>

              <div class="theme-preview-mock-content">
                <div style="display: flex; justify-content: space-between; font-size: 11px; opacity: 0.8;">
                  <span>Уровень сигнала</span>
                  <span id="previewPercentText">64%</span>
                </div>
                <div class="theme-preview-mock-bar">
                  <div class="theme-preview-mock-fill" id="previewMockFill" style="background: ${current.accent};"></div>
                </div>
              </div>

              <div class="theme-preview-actions">
                <button type="button" class="theme-preview-btn primary" id="btnTestAnimation" style="background: ${current.accent}; border-color: ${current.accent};">
                  ▶ Тест анимации
                </button>
              </div>

              <div class="theme-test-anim-target" id="themeTestAnimTarget">
                Нажмите «Тест анимации» для проверки кривой и скорости
              </div>
            </div>
          </div>
        </div>
      </div>

      <!-- Section 3: Palette (theme schema v2 color channels) -->
      <div class="theme-section">
        <div class="theme-section-header">
          <div class="theme-section-title">
            <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
              <circle cx="13.5" cy="6.5" r=".5" fill="currentColor"></circle>
              <circle cx="17.5" cy="10.5" r=".5" fill="currentColor"></circle>
              <circle cx="8.5" cy="7.5" r=".5" fill="currentColor"></circle>
              <circle cx="6.5" cy="12.5" r=".5" fill="currentColor"></circle>
              <path d="M12 2C6.5 2 2 6.5 2 12s4.5 10 10 10c.926 0 1.648-.746 1.648-1.688 0-.437-.18-.835-.437-1.125-.29-.289-.438-.652-.438-1.125a1.64 1.64 0 011.668-1.668h1.996c3.051 0 5.555-2.503 5.555-5.554C21.965 6.012 17.461 2 12 2z"></path>
            </svg>
            Палитра текста, фонов и статусов
          </div>
          <span style="font-size: 11px; color: rgba(var(--fg-rgb), 0.45);">Живой предпросмотр — применяется сразу</span>
        </div>

        <div class="theme-palette-grid" id="themePaletteGrid">
          ${PALETTE_FIELDS.map((f) => {
            const value = f.kind === "rgb" ? rgbToHexPicker(current[f.field]) : safeColor(current[f.field], "#ffffff");
            return `
            <label class="theme-palette-item">
              <span class="theme-palette-name">${f.label}</span>
              <input type="color" class="theme-native-color-picker theme-palette-picker" data-field="${f.field}" data-kind="${f.kind}" value="${value}" aria-label="${f.label}">
            </label>
          `;}).join("")}
        </div>
      </div>

      <!-- Section 4: Material (theme schema v4) -->
      <div class="theme-section">
        <div class="theme-section-header">
          <div class="theme-section-title">Материал и оттенки виджетов</div>
          <span style="font-size: 11px; color: rgba(var(--fg-rgb), 0.45);">Живой предпросмотр — применяется сразу</span>
        </div>

        <div class="theme-slider-row">
          <div class="theme-slider-label"><span>Материал карточек</span></div>
          <div class="theme-motion-pills">
            <button type="button" class="theme-motion-pill theme-enum-pill ${current.material === "flat" ? "active" : ""}" data-field="material" data-value="flat">Плоское стекло</button>
            <button type="button" class="theme-motion-pill theme-enum-pill ${current.material === "jelly" ? "active" : ""}" data-field="material" data-value="jelly">Морское стекло (желе)</button>
          </div>
        </div>

        <div class="theme-slider-row">
          <div class="theme-slider-label"><span>Микрографика</span></div>
          <div class="theme-motion-pills">
            <button type="button" class="theme-motion-pill theme-enum-pill ${current.mgLayout === "panel" ? "active" : ""}" data-field="mgLayout" data-value="panel">Одной панелью</button>
            <button type="button" class="theme-motion-pill theme-enum-pill ${current.mgLayout === "pills" ? "active" : ""}" data-field="mgLayout" data-value="pills">Пилюлями по группам</button>
          </div>
        </div>

        <div class="theme-slider-row">
          <div class="theme-slider-label"><span>Аватары Telegram</span></div>
          <div class="theme-motion-pills">
            <button type="button" class="theme-motion-pill theme-enum-pill ${current.avatarColor ? "" : "active"}" data-field="avatarColor" data-value="false">Чёрно-белые</button>
            <button type="button" class="theme-motion-pill theme-enum-pill ${current.avatarColor ? "active" : ""}" data-field="avatarColor" data-value="true">Цветные</button>
          </div>
        </div>

        <div class="theme-slider-label" style="margin-top: 6px;">
          <span>Палитра оттенков</span>
          <span class="theme-motion-pills">
            <button type="button" class="theme-motion-pill" id="btnShuffleColors">Перемешать цвета</button>
            <button type="button" class="theme-motion-pill" id="btnRestoreColors" ${hasPreviousColors() ? "" : "disabled"}>Вернуть прошлые</button>
          </span>
        </div>
        <div class="theme-palette-grid">
          ${PALETTE_SLOTS.map((slot) => `
            <label class="theme-palette-item">
              <span class="theme-palette-name">${SLOT_LABELS[slot]}</span>
              <input type="color" class="theme-native-color-picker theme-slot-picker" data-slot="${slot}" value="${safeColor(current.palette[slot], "#ffffff")}" aria-label="${SLOT_LABELS[slot]}">
            </label>
          `).join("")}
        </div>

        <div class="theme-slider-label" style="margin-top: 14px;"><span>Оттенок виджетов</span></div>
        <div class="theme-tint-list">
          ${TINT_TARGETS.filter((t) => !t.startsWith("mg.")).map((t) => renderTintRow(t, current)).join("")}
        </div>

        <details class="theme-details" data-details="mg" ${materialDetailsOpen.mg ? "open" : ""}>
          <summary>Оттенки групп микрографики</summary>
          <div class="theme-tint-list">
            ${TINT_TARGETS.filter((t) => t.startsWith("mg.")).map((t) => renderTintRow(t, current)).join("")}
          </div>
        </details>

        <details class="theme-details" data-details="fine" ${materialDetailsOpen.fine ? "open" : ""}>
          <summary>Тонкая настройка материала</summary>
          ${MATERIAL_SLIDERS.map((sl) => {
            const v = Number(current[sl.field]);
            const ui = sl.toUi ? sl.toUi(v) : v;
            return `
            <div class="theme-slider-row">
              <div class="theme-slider-label">
                <span>${sl.label}</span>
                <span class="theme-slider-val" data-val-for="${sl.field}">${sl.fmt(ui)}</span>
              </div>
              <input type="range" class="theme-range-input theme-material-slider" data-field="${sl.field}"
                min="${sl.min}" max="${sl.max}" step="${sl.step}" value="${ui}">
            </div>`;
          }).join("")}
        </details>
      </div>
    </div>
  `;

  setupThemeTabEvents(pane);
}

function setupThemeTabEvents(pane) {
  // Preset Selection
  pane.querySelectorAll(".theme-preset-card").forEach((card) => {
    card.addEventListener("click", (e) => {
      if (e.target.closest(".theme-preset-delete-btn")) return;
      const id = card.dataset.presetId;
      selectPreset(id);
      renderThemeTab();
    });
  });

  // Preset Deletion
  pane.querySelectorAll(".theme-preset-delete-btn").forEach((btn) => {
    btn.addEventListener("click", (e) => {
      e.stopPropagation();
      const id = btn.dataset.deleteId;
      deleteCustomPreset(id);
      renderThemeTab();
    });
  });

  // Save Custom Preset
  pane.querySelector("#btnSavePreset")?.addEventListener("click", () => {
    const input = pane.querySelector("#newPresetName");
    const name = (input?.value || "").trim();
    saveCustomPreset(name, getActiveTheme());
    renderThemeTab();
  });

  // Reset Sizes To Preset
  pane.querySelector("#btnResetSizes")?.addEventListener("click", () => {
    resetSizesToPreset();
    renderThemeTab();
  });

  // Font Family (body token): live preview + commit
  pane.querySelectorAll("#themeFontBodyRow .theme-motion-pill").forEach((pill) => {
    pill.addEventListener("click", () => {
      setLiveToken("fontBody", pill.dataset.font);
      commitLiveChange();
      renderThemeTab();
    });
  });

  // Color Chips
  pane.querySelectorAll(".theme-color-chip").forEach((chip) => {
    chip.addEventListener("click", () => {
      const color = chip.dataset.color;
      setLiveToken("accent", color);
      commitLiveChange();
      renderThemeTab();
    });
  });

  // Custom Color Picker
  const picker = pane.querySelector("#pickerCustomColor");
  if (picker) {
    picker.addEventListener("input", (e) => {
      const color = e.target.value;
      setLiveToken("accent", color);
      pane.querySelector("#valAccent").textContent = color;
      const previewDot = pane.querySelector("#themePreviewDot");
      const previewFill = pane.querySelector("#previewMockFill");
      const previewBtn = pane.querySelector("#btnTestAnimation");
      if (previewDot) previewDot.style.background = color;
      if (previewFill) previewFill.style.background = color;
      if (previewBtn) {
        previewBtn.style.background = color;
        previewBtn.style.borderColor = color;
      }
    });
    picker.addEventListener("change", () => {
      commitLiveChange();
    });
  }

  // Sliders: Glass Opacity
  const sliderOpacity = pane.querySelector("#sliderOpacity");
  if (sliderOpacity) {
    sliderOpacity.addEventListener("input", (e) => {
      const val = parseFloat(e.target.value);
      setLiveToken("glassOpacity", val);
      pane.querySelector("#valOpacity").textContent = `${Math.round(val * 100)}%`;
    });
    sliderOpacity.addEventListener("change", () => {
      commitLiveChange();
    });
  }

  // Sliders: Blur Strength
  const sliderBlur = pane.querySelector("#sliderBlur");
  if (sliderBlur) {
    sliderBlur.addEventListener("input", (e) => {
      const val = parseInt(e.target.value, 10);
      setLiveToken("blur", val);
      pane.querySelector("#valBlur").textContent = `${val}px`;
    });
    sliderBlur.addEventListener("change", () => {
      commitLiveChange();
    });
  }

  // Sliders: Corner Radius
  const sliderRadius = pane.querySelector("#sliderRadius");
  if (sliderRadius) {
    sliderRadius.addEventListener("input", (e) => {
      const val = parseInt(e.target.value, 10);
      setLiveToken("radiusCard", val);
      setLiveToken("radiusSurface", val + 8);
      pane.querySelector("#valRadius").textContent = `${val}px`;
    });
    sliderRadius.addEventListener("change", () => {
      commitLiveChange();
    });
  }

  // Sliders: Motion Scale
  const sliderMotion = pane.querySelector("#sliderMotion");
  if (sliderMotion) {
    sliderMotion.addEventListener("input", (e) => {
      const val = parseFloat(e.target.value);
      setLiveToken("motionScale", val);
      pane.querySelector("#valMotion").textContent = `${val.toFixed(2)}x`;
    });
    sliderMotion.addEventListener("change", () => {
      commitLiveChange();
    });
  }

  // Motion Profile Pills
  // `.theme-motion-pill` is only the pill look (font, enum and shuffle buttons
  // share it); the profile handler must bind to profile pills alone.
  pane.querySelectorAll("#themeMotionPillsRow [data-profile]").forEach((pill) => {
    pill.addEventListener("click", () => {
      const profile = pill.dataset.profile;
      setLiveToken("motionProfile", profile);
      commitLiveChange();
      renderThemeTab();
    });
  });

  // Palette pickers (theme schema v2 channels): live preview on input,
  // validation + persistence on change (see commitLiveChange).
  pane.querySelectorAll(".theme-palette-picker").forEach((input) => {
    const field = input.dataset.field;
    const kind = input.dataset.kind;
    if (!field || !kind) return;
    input.addEventListener("input", (e) => {
      const hex = e.target.value;
      const value = kind === "rgb" ? hexToRgbString(hex) : hex;
      if (value === null) return;
      setLiveToken(field, value);
    });
    input.addEventListener("change", () => {
      commitLiveChange();
    });
  });

  // Material: enum pills (material, mgLayout, avatarColor)
  pane.querySelectorAll(".theme-enum-pill").forEach((pill) => {
    pill.addEventListener("click", () => {
      const { field, value } = pill.dataset;
      setLiveToken(field, field === "avatarColor" ? value === "true" : value);
      commitLiveChange();
      renderThemeTab();
    });
  });

  // Material: a new random coloring / undo the last one
  pane.querySelector("#btnShuffleColors")?.addEventListener("click", () => {
    shuffleColors();
    renderThemeTab();
  });
  pane.querySelector("#btnRestoreColors")?.addEventListener("click", () => {
    restorePreviousColors();
    renderThemeTab();
  });

  // Material: palette slots (live on input, commit on change)
  pane.querySelectorAll(".theme-slot-picker").forEach((input) => {
    input.addEventListener("input", (e) => {
      setLiveToken("palette", { ...getActiveTheme().palette, [input.dataset.slot]: e.target.value });
    });
    input.addEventListener("change", () => {
      commitLiveChange();
      renderThemeTab();
    });
  });

  // Material: which slot tints which surface
  pane.querySelectorAll(".theme-tint-dot").forEach((dot) => {
    dot.addEventListener("click", () => {
      setLiveToken("tintMap", { ...getActiveTheme().tintMap, [dot.dataset.target]: dot.dataset.slot });
      commitLiveChange();
      renderThemeTab();
    });
  });

  // Material: fine tuning sliders
  pane.querySelectorAll(".theme-material-slider").forEach((input) => {
    const spec = MATERIAL_SLIDERS.find((sl) => sl.field === input.dataset.field);
    if (!spec) return;
    input.addEventListener("input", (e) => {
      const ui = parseFloat(e.target.value);
      setLiveToken(spec.field, spec.fromUi ? spec.fromUi(ui) : ui);
      const label = pane.querySelector(`[data-val-for="${spec.field}"]`);
      if (label) label.textContent = spec.fmt(ui);
    });
    input.addEventListener("change", () => {
      commitLiveChange();
    });
  });

  // Keep collapsible blocks open across re-renders.
  pane.querySelectorAll(".theme-details").forEach((el) => {
    el.addEventListener("toggle", () => {
      materialDetailsOpen = { ...materialDetailsOpen, [el.dataset.details]: el.open };
    });
  });

  // Test Animation Trigger
  pane.querySelector("#btnTestAnimation")?.addEventListener("click", () => {
    const target = pane.querySelector("#themeTestAnimTarget");
    if (!target) return;
    target.textContent = "Анимация запущена…";
    spring(target, [
      { transform: "scale(0.92)", opacity: 0.5 },
      { transform: "scale(1.05)", opacity: 1 },
      { transform: "scale(1)", opacity: 1 },
    ], { duration: "motion" }).then(() => {
      target.textContent = "Физика отработана успешно ✓";
      setTimeout(() => {
        if (target) target.textContent = "Нажмите «Тест анимации» для проверки кривой и скорости";
      }, 2500);
    });
  });
}
