/**
 * UniDTK — customization panel (menu.js)
 *
 * A side panel on the right of the live desktop, inside the wallpaper window:
 *   1. «Стол»  — which widgets show, arranging them on the desk, backend modules
 *   2. «Папки» — desktop folders and their shortcuts, added from Start, a system
 *               dialog or the clipboard — never by typing a path
 *   3. «Вид»   — theme, colours, everything else folded into «Тонкая настройка»
 * plus the VPN view, reached from «Стол» (the VPN widget may be hidden).
 */

import { invoke } from "@unidtk/bridge";
import { getWidgets, getDefaultWidgetConfig, getDisabledModules, setDisabledModules } from "./widgets-api.js";
import { config, saveConfig, syncWidgets, relayoutWidgets, escapeHtml, enterArrangeMode } from "./unidtk-core.js";
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
} from "./theme.js";
import { showToast, showUndo } from "./toast.js";
import {
  FOLDER_ICONS, shortcutCount, listFolders, createFolder, updateFolder, deleteFolder,
  addItems, removeItem, renameItem, replaceItemTarget, missingTargets, pickTarget, clipboardDrafts,
} from "./folders.js";
import { hydrateShortcutIcons } from "./shortcut-icons.js";

// One submission per click: the control stays disabled while its action is
// in flight, so a double click cannot create a folder or shortcut twice.
async function runOnce(control, action) {
  if (!control || control.disabled) return;
  control.disabled = true;
  try {
    await action();
  } catch (err) {
    showToast(`Не получилось: ${err?.message || err}`, { tone: "error" });
  } finally {
    if (control.isConnected) control.disabled = false;
  }
}

const TABS = [
  { id: "desk", label: "Стол" },
  { id: "folders", label: "Папки" },
  { id: "look", label: "Вид" },
];

// Modules with a Rust half, so the user can switch the background work off.
// `clock` is absent on purpose: it has no crate, there is nothing to stop.
const BACKEND_MODULES = [
  { id: "mp3-player", label: "Плеер" },
  { id: "telegram", label: "Telegram" },
  { id: "glyph-bar", label: "Громкость (Glyph Bar)" },
  { id: "micrographics", label: "Микрографика" },
  { id: "vpn", label: "VPN" },
  { id: "explorer", label: "Проводник" },
];

let panel = null;
let opener = null;
let currentView = "desk";
// A module switch flipped this session: it applies on the next start.
let modulesPending = false;

export function initMenu() {
  panel = document.getElementById("menuContainer");
  opener = document.getElementById("openMenu");
  if (!panel) return;
  renderShell();
  panel.addEventListener("keydown", onPanelKeydown);
}

function renderShell() {
  panel.innerHTML = `
    <header class="menu-header">
      <h2 class="menu-title" id="menuTitle">Настройки</h2>
      <button type="button" class="menu-icon-btn" id="menuCloseBtn" aria-label="Закрыть (Esc)">
        <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" aria-hidden="true">
          <line x1="18" y1="6" x2="6" y2="18"></line><line x1="6" y1="6" x2="18" y2="18"></line>
        </svg>
      </button>
    </header>
    <nav class="menu-tabs" role="tablist" aria-label="Разделы">
      ${TABS.map((t) => `
        <button type="button" class="menu-tab" role="tab" id="tab-${t.id}" data-tab="${t.id}"
          aria-controls="pane-${t.id}" aria-selected="false" tabindex="-1">${t.label}</button>
      `).join("")}
    </nav>
    <div class="menu-body">
      ${TABS.map((t) => `<section class="menu-pane" role="tabpanel" id="pane-${t.id}" aria-labelledby="tab-${t.id}" tabindex="0" hidden></section>`).join("")}
      <section class="menu-pane" id="pane-vpn" aria-label="VPN" hidden></section>
    </div>
    <footer class="menu-footer">
      <button type="button" class="menu-btn menu-btn-quiet" id="menuQuit">Выйти из UniDTK</button>
    </footer>
  `;

  panel.querySelectorAll(".menu-tab").forEach((tab) => {
    tab.addEventListener("click", () => showView(tab.dataset.tab));
  });
  panel.querySelector("#menuCloseBtn").addEventListener("click", () => closeMenu());
  const quit = panel.querySelector("#menuQuit");
  quit.addEventListener("click", () => runOnce(quit, () => invoke("quit_app")));
}

function onPanelKeydown(e) {
  if (e.key === "Escape") {
    e.preventDefault();
    if (currentView === "vpn") showView("desk");
    else closeMenu();
    return;
  }
  // Tabs: arrows move between them (WAI-ARIA tabs pattern).
  const tab = e.target.closest?.(".menu-tab");
  if (tab && (e.key === "ArrowRight" || e.key === "ArrowLeft")) {
    e.preventDefault();
    const ids = TABS.map((t) => t.id);
    const next = ids[(ids.indexOf(tab.dataset.tab) + (e.key === "ArrowRight" ? 1 : ids.length - 1)) % ids.length];
    showView(next);
    panel.querySelector(`#tab-${next}`)?.focus();
  }
  // Enter submits the small inline forms (rename, new folder, search).
  if (e.key === "Enter" && e.target.matches?.("input[data-submit]")) {
    e.preventDefault();
    panel.querySelector(`#${e.target.dataset.submit}`)?.click();
  }
}

function showView(view) {
  currentView = view;
  panel.querySelectorAll(".menu-tab").forEach((t) => {
    const active = t.dataset.tab === view;
    t.setAttribute("aria-selected", String(active));
    t.tabIndex = active ? 0 : -1;
  });
  panel.querySelectorAll(".menu-pane").forEach((p) => {
    p.hidden = p.id !== `pane-${view}`;
  });
  panel.querySelector(".menu-tabs").hidden = view === "vpn";
  if (view === "desk") renderDesk();
  else if (view === "folders") renderFolders();
  else if (view === "look") renderLook();
  else if (view === "vpn") renderVpn();
}

export function isMenuOpen() {
  return Boolean(panel && !panel.hidden && !panel.classList.contains("closing"));
}

export function openMenu() {
  if (!panel) return;
  if (isMenuOpen()) {
    closeMenu();
    return;
  }
  panel.classList.remove("closing");
  panel.hidden = false;
  opener?.setAttribute("aria-expanded", "true");
  // Next frame: the slide-in transition needs a rendered start state.
  requestAnimationFrame(() => panel.classList.add("open"));
  showView(currentView === "vpn" ? "desk" : currentView);
  panel.querySelector(`#tab-${currentView}`)?.focus();
}

/** Open «Папки» with the Start list of one folder already unfolded. */
export function openFoldersFor(folderId) {
  startPickerFor = folderId;
  currentView = "folders";
  if (isMenuOpen()) showView("folders");
  else openMenu();
}

export function closeMenu({ restoreFocus = true } = {}) {
  if (!isMenuOpen()) return;
  clearHighlight();
  panel.classList.remove("open");
  panel.classList.add("closing");
  opener?.setAttribute("aria-expanded", "false");
  const done = () => {
    if (!panel.classList.contains("closing")) return;
    panel.classList.remove("closing");
    panel.hidden = true;
  };
  panel.addEventListener("transitionend", done, { once: true });
  // Reduced motion or a zero scale may skip the transition entirely.
  setTimeout(done, 700);
  if (restoreFocus) opener?.focus();
}

// ═══════════════════════════════════════════════════════════════════════════
// «Стол»
// ═══════════════════════════════════════════════════════════════════════════

function highlight(id) {
  clearHighlight();
  document.querySelector(`.widget-card[data-id="${CSS.escape(id)}"]`)?.classList.add("menu-highlight");
}

function clearHighlight() {
  document.querySelectorAll(".widget-card.menu-highlight").forEach((c) => c.classList.remove("menu-highlight"));
}

async function renderDesk() {
  const pane = panel.querySelector("#pane-desk");
  pane.innerHTML = `<div class="menu-empty">Загрузка…</div>`;

  let widgets;
  let disabled;
  try {
    [widgets, disabled] = await Promise.all([getWidgets(), getDisabledModules()]);
  } catch (err) {
    pane.innerHTML = `
      <div class="menu-empty menu-empty-error">
        Не удалось загрузить список виджетов: ${escapeHtml(String(err?.message || err))}
        <button type="button" class="menu-btn" data-retry>Повторить</button>
      </div>`;
    pane.querySelector("[data-retry]").addEventListener("click", renderDesk);
    return;
  }
  const hidden = new Set(config.hidden || []);
  const disabledSet = new Set(disabled);
  const modules = widgets.filter((w) => !w.isFolder);
  const folders = widgets.filter((w) => w.isFolder);

  const row = (w) => `
    <li class="menu-row" data-widget-id="${escapeHtml(w.id)}">
      <span class="menu-row-icon" aria-hidden="true">${w.iconSvg || ""}</span>
      <span class="menu-row-text">
        <span class="menu-row-name">${escapeHtml(w.name)}</span>
        ${w.isFolder ? `<span class="menu-row-sub">${shortcutCount((w.items || []).length)}</span>` : ""}
      </span>
      <label class="menu-switch" title="Показывать на столе">
        <input type="checkbox" class="widget-vis-toggle" data-id="${escapeHtml(w.id)}" ${hidden.has(w.id) ? "" : "checked"}
          aria-label="Показывать «${escapeHtml(w.name)}»">
        <span class="menu-slider" aria-hidden="true"></span>
      </label>
    </li>`;

  pane.innerHTML = `
    <div class="menu-actions">
      <button type="button" class="menu-btn menu-btn-primary" id="btnArrange">Расставить</button>
      <button type="button" class="menu-btn" id="btnResetPositions">Исходная раскладка</button>
    </div>

    <h3 class="menu-heading">Виджеты</h3>
    <ul class="menu-list">${modules.map(row).join("")}</ul>

    ${folders.length ? `
      <h3 class="menu-heading">Папки</h3>
      <ul class="menu-list">${folders.map(row).join("")}</ul>
    ` : ""}

    <ul class="menu-list">
      <li class="menu-row">
        <span class="menu-row-text">
          <span class="menu-row-name">VPN</span>
          <span class="menu-row-sub">Подписка, автоподключение, домашний сервер</span>
        </span>
        <button type="button" class="menu-btn" id="btnVpnSettings">Настроить…</button>
      </li>
    </ul>

    <h3 class="menu-heading">Модули</h3>
    <p class="menu-note">Выключенный модуль не тратит ресурсы: его фоновая работа не запускается. Изменение вступает в силу после перезапуска.</p>
    <ul class="menu-list">
      ${BACKEND_MODULES.map((m) => `
        <li class="menu-row">
          <span class="menu-row-text"><span class="menu-row-name">${m.label}</span></span>
          <label class="menu-switch">
            <input type="checkbox" class="mod-toggle" data-id="${m.id}" ${disabledSet.has(m.id) ? "" : "checked"} aria-label="Модуль «${m.label}» включён">
            <span class="menu-slider" aria-hidden="true"></span>
          </label>
        </li>
      `).join("")}
    </ul>
    <div class="menu-restart" ${modulesPending ? "" : "hidden"}>
      <p class="menu-note">Плеер и Telegram перезапустятся, музыка остановится.</p>
      <button type="button" class="menu-btn menu-btn-primary" id="btnRestart">Перезапустить сейчас</button>
    </div>
  `;

  pane.querySelectorAll(".menu-row[data-widget-id]").forEach((li) => {
    li.addEventListener("mouseenter", () => highlight(li.dataset.widgetId));
    li.addEventListener("mouseleave", clearHighlight);
  });

  pane.querySelectorAll(".widget-vis-toggle").forEach((toggle) => {
    toggle.addEventListener("change", () => runOnce(toggle, async () => {
      const id = toggle.dataset.id;
      const previous = [...(config.hidden || [])];
      config.hidden = toggle.checked ? previous.filter((h) => h !== id) : [...new Set([...previous, id])];
      if (!(await saveConfig())) {
        config.hidden = previous;
        toggle.checked = !toggle.checked;
        return;
      }
      await syncWidgets();
      if (toggle.checked) highlight(id);
    }));
  });

  // Read-modify-write of one stored list: chained so a second switch reads the
  // first one's result instead of the same stale list.
  let chain = Promise.resolve();
  pane.querySelectorAll(".mod-toggle").forEach((tgl) => {
    tgl.addEventListener("change", () => {
      const { id } = tgl.dataset;
      const on = tgl.checked;
      chain = chain.then(async () => {
        const current = new Set(await getDisabledModules());
        if (on) current.delete(id);
        else current.add(id);
        if (!(await setDisabledModules([...current]))) throw new Error("список модулей не сохранён");
        modulesPending = true;
        pane.querySelector(".menu-restart").hidden = false;
      }).catch((err) => {
        tgl.checked = !on;
        showToast(`Не получилось: ${err?.message || err}`, { tone: "error" });
      });
    });
  });

  const restart = pane.querySelector("#btnRestart");
  restart.addEventListener("click", () => runOnce(restart, () => invoke("restart_app")));

  pane.querySelector("#btnVpnSettings").addEventListener("click", () => showView("vpn"));

  pane.querySelector("#btnArrange").addEventListener("click", () => {
    closeMenu({ restoreFocus: false });
    enterArrangeMode(() => openMenu());
  });

  const resetBtn = pane.querySelector("#btnResetPositions");
  resetBtn.addEventListener("click", () => runOnce(resetBtn, async () => {
    const before = structuredClone(config.positions || {});
    const defaults = (await getDefaultWidgetConfig()).positions || {};
    const canvas = document.getElementById("widgets");
    const maxX = Math.max(0, (canvas?.clientWidth || window.innerWidth) - 120);
    const maxY = Math.max(0, (canvas?.clientHeight || window.innerHeight) - 64);
    // Geometry only: settings stored next to it (clock face, AI DJ,
    // micrographics style) are the user's and survive a layout reset.
    Object.entries(config.positions || {}).forEach(([id, pos]) => {
      delete pos.anchor;
      delete pos.ax;
      delete pos.ay;
      const def = defaults[id];
      if (def) {
        Object.assign(pos, { x: def.x, y: def.y });
        if (def.w) pos.w = def.w;
        if (def.h) pos.h = def.h;
        return;
      }
      // No kernel default (folders, instances): keep the place, only pull it back on screen.
      if (typeof pos.x === "number") pos.x = Math.min(Math.max(0, pos.x), maxX);
      if (typeof pos.y === "number") pos.y = Math.min(Math.max(0, pos.y), maxY);
    });
    await applyPositions();
    showUndo("Раскладка сброшена", async () => {
      config.positions = before;
      await applyPositions();
    });
  }));
}

async function applyPositions() {
  await saveConfig();
  document.querySelectorAll("#widgets > .widget-card").forEach((card) => {
    const pos = config.positions?.[card.dataset.id];
    if (!pos || card._geometryController) return;
    if (pos.w) card.style.width = pos.w + "px";
    if (pos.h) card.style.height = pos.h + "px";
    card._glyphBarGeometry?.layout();
  });
  relayoutWidgets();
}

// ═══════════════════════════════════════════════════════════════════════════
// «Папки»
// ═══════════════════════════════════════════════════════════════════════════

const ICON_LABELS = {
  folder: "Папка", terminal: "Терминал", gamepad: "Игры", code: "Код", globe: "Сайты",
  music: "Музыка", tools: "Инструменты", book: "Книги", star: "Избранное",
};

let startMenuCache = null;
// Which folder has its «Из Пуска» list open (one at a time).
let startPickerFor = null;
let creatingFolder = false;

async function renderFolders() {
  const pane = panel.querySelector("#pane-folders");
  if (!pane.childElementCount) pane.innerHTML = `<div class="menu-empty">Загрузка…</div>`;

  let folders;
  try {
    folders = await listFolders();
  } catch (err) {
    pane.innerHTML = `
      <div class="menu-empty menu-empty-error">
        Не удалось загрузить папки: ${escapeHtml(String(err?.message || err))}
        <button type="button" class="menu-btn" data-retry>Повторить</button>
      </div>`;
    pane.querySelector("[data-retry]").addEventListener("click", renderFolders);
    return;
  }
  const missing = await missingTargets(folders.flatMap((f) => f.items || []));

  pane.innerHTML = `
    <div class="menu-actions">
      <button type="button" class="menu-btn menu-btn-primary" id="btnNewFolder" aria-expanded="${creatingFolder}">+ Папка</button>
    </div>
    <form class="menu-new-folder" id="newFolderForm" ${creatingFolder ? "" : "hidden"}>
      <input type="text" class="menu-input" id="newFolderName" placeholder="Название, например «Игры»" maxlength="40" aria-label="Название папки">
      <div class="menu-icon-pick" role="radiogroup" aria-label="Значок папки">
        ${Object.entries(FOLDER_ICONS).map(([key, svg], i) => `
          <button type="button" class="menu-icon-choice" role="radio" data-icon="${key}" aria-checked="${i === 0}" aria-label="${ICON_LABELS[key] || key}">${svg}</button>
        `).join("")}
      </div>
      <div class="menu-actions">
        <button type="submit" class="menu-btn menu-btn-primary">Создать</button>
        <button type="button" class="menu-btn" id="btnCancelFolder">Отмена</button>
      </div>
    </form>

    ${folders.length ? folders.map((f) => folderCard(f, missing)).join("") : `
      <div class="menu-empty">Папок пока нет. Папка — это значок на столе, внутри которого лежат ваши программы, файлы и сайты.</div>
    `}
  `;

  hydrateShortcutIcons(pane);
  bindFolderEvents(pane, folders);
}

function folderCard(f, missing) {
  const items = f.items || [];
  const picker = startPickerFor === f.id;
  return `
    <article class="menu-folder" data-folder-id="${escapeHtml(f.id)}">
      <header class="menu-folder-head">
        <span class="menu-folder-icon" aria-hidden="true">${f.icon_svg || FOLDER_ICONS.folder}</span>
        <input type="text" class="menu-input menu-folder-name" value="${escapeHtml(f.name)}" maxlength="40" aria-label="Название папки">
        <span class="menu-row-sub">${shortcutCount(items.length)}</span>
        <button type="button" class="menu-icon-btn" data-delete-folder aria-label="Удалить папку «${escapeHtml(f.name)}»">
          <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true"><polyline points="3 6 5 6 21 6"></polyline><path d="M19 6v14a2 2 0 01-2 2H7a2 2 0 01-2-2V6m3 0V4a2 2 0 012-2h4a2 2 0 012 2v2"></path></svg>
        </button>
      </header>
      ${items.length ? `
        <ul class="menu-list menu-shortcuts">
          ${items.map((it) => {
            const gone = missing.has(it.target);
            return `
            <li class="menu-row ${gone ? "is-missing" : ""}" data-item-id="${escapeHtml(it.id)}">
              <span class="menu-row-icon">
                <img alt="" hidden data-icon-target="${escapeHtml(it.target)}">
                <span class="menu-row-glyph" aria-hidden="true">${it.kind === "url" ? FOLDER_ICONS.globe : it.kind === "folder" ? FOLDER_ICONS.folder : ""}</span>
              </span>
              <span class="menu-row-text">
                <input type="text" class="menu-input menu-inline-input" value="${escapeHtml(it.name)}" maxlength="60" aria-label="Название ярлыка">
                ${gone ? `<span class="menu-row-sub menu-warn">Недоступен — файл перемещён или удалён</span>` : ""}
              </span>
              ${gone ? `<button type="button" class="menu-btn" data-relink>Найти…</button>` : ""}
              <button type="button" class="menu-icon-btn" data-remove-item aria-label="Убрать «${escapeHtml(it.name)}»">
                <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" aria-hidden="true"><line x1="18" y1="6" x2="6" y2="18"></line><line x1="6" y1="6" x2="18" y2="18"></line></svg>
              </button>
            </li>`;
          }).join("")}
        </ul>
      ` : `<p class="menu-note">Пусто. Добавьте программу из Пуска, файл или папку.</p>`}
      <div class="menu-actions">
        <button type="button" class="menu-btn" data-from-start aria-expanded="${picker}">Из Пуска</button>
        <button type="button" class="menu-btn" data-pick-file>Файл…</button>
        <button type="button" class="menu-btn" data-pick-folder>Папку…</button>
        <button type="button" class="menu-btn" data-paste title="То, что скопировано: файлы или ссылка">Вставить</button>
      </div>
      ${picker ? `
        <div class="menu-start" data-start-picker>
          <input type="search" class="menu-input" data-start-search placeholder="Поиск программы" aria-label="Поиск программы">
          <ul class="menu-list menu-start-list" role="listbox" aria-label="Программы из Пуска">
            <li class="menu-empty">Загрузка…</li>
          </ul>
        </div>
      ` : ""}
    </article>`;
}

function bindFolderEvents(pane, folders) {
  const byId = (id) => folders.find((f) => f.id === id);
  const refresh = () => renderFolders();

  const form = pane.querySelector("#newFolderForm");
  let iconKey = "folder";
  pane.querySelector("#btnNewFolder").addEventListener("click", () => {
    creatingFolder = !creatingFolder;
    form.hidden = !creatingFolder;
    if (creatingFolder) form.querySelector("#newFolderName").focus();
  });
  pane.querySelector("#btnCancelFolder").addEventListener("click", () => {
    creatingFolder = false;
    form.hidden = true;
  });
  form.querySelectorAll(".menu-icon-choice").forEach((btn) => {
    btn.addEventListener("click", () => {
      iconKey = btn.dataset.icon;
      form.querySelectorAll(".menu-icon-choice").forEach((b) => b.setAttribute("aria-checked", String(b === btn)));
    });
  });
  form.addEventListener("submit", (e) => {
    e.preventDefault();
    const submit = form.querySelector('[type="submit"]');
    runOnce(submit, async () => {
      const folder = await createFolder(form.querySelector("#newFolderName").value, iconKey);
      creatingFolder = false;
      showToast(`Папка «${folder.name}» появилась на столе`);
      await refresh();
    });
  });

  pane.querySelectorAll(".menu-folder").forEach((card) => {
    const folder = byId(card.dataset.folderId);
    if (!folder) return;

    const nameInput = card.querySelector(".menu-folder-name");
    nameInput.addEventListener("change", () => {
      const name = nameInput.value.trim();
      if (!name) {
        nameInput.value = folder.name;
        return;
      }
      updateFolder(folder, { name }).then((next) => Object.assign(folder, next))
        .catch((err) => {
          nameInput.value = folder.name;
          showToast(`Не переименовано: ${err?.message || err}`, { tone: "error" });
        });
    });
    nameInput.addEventListener("keydown", (e) => {
      if (e.key === "Enter") nameInput.blur();
    });

    const del = card.querySelector("[data-delete-folder]");
    del.addEventListener("click", () => runOnce(del, async () => {
      await deleteFolder(folder, refresh);
      await refresh();
    }));

    card.querySelectorAll(".menu-row[data-item-id]").forEach((li) => {
      const itemId = li.dataset.itemId;
      const input = li.querySelector(".menu-inline-input");
      input.addEventListener("change", () => {
        renameItem(folder, itemId, input.value).then((next) => Object.assign(folder, next))
          .catch((err) => showToast(`Не переименовано: ${err?.message || err}`, { tone: "error" }));
      });
      input.addEventListener("keydown", (e) => {
        if (e.key === "Enter") input.blur();
      });
      const remove = li.querySelector("[data-remove-item]");
      remove.addEventListener("click", () => runOnce(remove, async () => {
        await removeItem(folder, itemId, refresh);
        await refresh();
      }));
      const relink = li.querySelector("[data-relink]");
      relink?.addEventListener("click", () => runOnce(relink, async () => {
        const item = folder.items.find((it) => it.id === itemId);
        const draft = await pickTarget(item?.kind === "folder");
        if (!draft) return;
        await replaceItemTarget(folder, itemId, draft);
        await refresh();
      }));
    });

    const add = (drafts) => addItems(folder, drafts).then(refresh);
    const pickFile = card.querySelector("[data-pick-file]");
    pickFile.addEventListener("click", () => runOnce(pickFile, async () => {
      const draft = await pickTarget(false);
      if (draft) await add([draft]);
    }));
    const pickFolder = card.querySelector("[data-pick-folder]");
    pickFolder.addEventListener("click", () => runOnce(pickFolder, async () => {
      const draft = await pickTarget(true);
      if (draft) await add([draft]);
    }));
    const paste = card.querySelector("[data-paste]");
    paste.addEventListener("click", () => runOnce(paste, async () => {
      const drafts = await clipboardDrafts();
      if (!drafts.length) {
        showToast("В буфере обмена нет ни файлов, ни ссылки");
        return;
      }
      await add(drafts);
    }));

    card.querySelector("[data-from-start]").addEventListener("click", () => {
      startPickerFor = startPickerFor === folder.id ? null : folder.id;
      refresh();
    });
    const picker = card.querySelector("[data-start-picker]");
    if (picker) bindStartPicker(picker, (draft) => add([draft]));
  });
}

async function bindStartPicker(picker, onPick) {
  const list = picker.querySelector(".menu-start-list");
  const search = picker.querySelector("[data-start-search]");
  search.focus();

  const load = async () => {
    list.innerHTML = `<li class="menu-empty">Загрузка…</li>`;
    try {
      startMenuCache = startMenuCache || await invoke("start_menu_list");
      draw();
    } catch (err) {
      startMenuCache = null;
      list.innerHTML = `<li class="menu-empty menu-empty-error">Не удалось прочитать меню Пуск: ${escapeHtml(String(err?.message || err))}
        <button type="button" class="menu-btn" data-retry>Повторить</button></li>`;
      list.querySelector("[data-retry]").addEventListener("click", load);
    }
  };

  const draw = () => {
    const q = search.value.trim().toLowerCase();
    const apps = startMenuCache.filter((a) => !q || a.name.toLowerCase().includes(q));
    if (!startMenuCache.length) {
      list.innerHTML = `<li class="menu-empty">В меню Пуск нет программ</li>`;
      return;
    }
    if (!apps.length) {
      list.innerHTML = `<li class="menu-empty">Ничего не нашлось</li>`;
      return;
    }
    list.innerHTML = apps.slice(0, 200).map((a) => `
      <li>
        <button type="button" class="menu-start-app" role="option" data-index="${startMenuCache.indexOf(a)}">
          <span class="menu-row-icon"><img alt="" hidden data-icon-target="${escapeHtml(a.target)}"><span class="menu-row-glyph" aria-hidden="true"></span></span>
          <span class="menu-row-name">${escapeHtml(a.name)}</span>
        </button>
      </li>
    `).join("");
    hydrateShortcutIcons(list);
  };

  search.addEventListener("input", () => {
    if (startMenuCache) draw();
  });
  list.addEventListener("click", (e) => {
    const btn = e.target.closest(".menu-start-app");
    if (!btn) return;
    const app = startMenuCache[Number(btn.dataset.index)];
    runOnce(btn, () => onPick({ name: app.name, target: app.target, args: app.args, kind: app.kind }));
  });
  await load();
}

// ═══════════════════════════════════════════════════════════════════════════
// «Вид»
// ═══════════════════════════════════════════════════════════════════════════

const SLOT_LABELS = {
  neutral: "Нейтральный", blue: "Цвет 1", aqua: "Цвет 2", mint: "Цвет 3",
  lilac: "Цвет 4", peach: "Цвет 5", magenta: "Цвет 6", coral: "Коралловый",
};

const TINT_LABELS = {
  panel: "Панели: меню, проводник", folder: "Папки", clock: "Часы", telegram: "Telegram",
  "mp3-player": "Плеер", "glyph-bar": "Громкость",
  "mg.date": "Дата", "mg.time": "Время", "mg.freshness": "Устаревание", "mg.temperature": "Температура",
  "mg.condition": "Погода", "mg.feels": "Ощущается", "mg.wind": "Ветер", "mg.precipitation": "Осадки",
  "mg.precipitation-amount": "Объём осадков", "mg.sunrise": "Восход", "mg.sunset": "Закат",
  "mg.daylight": "Световой день", "mg.geolocation": "Геолокация", "mg.location": "Место",
  "mg.ip-location": "Место по IP", "mg.network": "Сеть", "mg.volume": "Громкость",
};

const PALETTE_FIELDS = [
  { field: "fgRgb", label: "Текст и значки", kind: "rgb" },
  { field: "bgElevRgb", label: "Оттенок элементов", kind: "rgb" },
  { field: "surfaceRgb", label: "Фон панелей", kind: "rgb" },
  { field: "surfaceDeepRgb", label: "Глубокий фон", kind: "rgb" },
  { field: "scrimRgb", label: "Затемнение", kind: "rgb" },
  { field: "statusOk", label: "Статус: норма", kind: "hex" },
  { field: "statusWarn", label: "Статус: внимание", kind: "hex" },
  { field: "statusError", label: "Статус: ошибка", kind: "hex" },
  { field: "statusInfo", label: "Статус: сведения", kind: "hex" },
];

const pct = (v) => `${Math.round(v * 100)}%`;
// One generic live-preview/commit handler serves every slider.
// radiusStrip shows its top value as a full pill (999 px).
const SLIDERS = [
  { field: "glassOpacity", label: "Плотность стекла", min: 0.4, max: 1, step: 0.02, fmt: pct },
  { field: "blur", label: "Размытие фона", min: 0, max: 36, step: 4, fmt: (v) => `${v} пикс.` },
  { field: "radiusCard", label: "Скругление углов", min: 0, max: 28, step: 2, fmt: (v) => `${v} пикс.`,
    also: (v) => setLiveToken("radiusSurface", v + 8) },
  { field: "motionScale", label: "Длительность анимаций", min: 0, max: 2, step: 0.05, fmt: (v) => (v === 0 ? "без анимаций" : `×${Number(v).toFixed(2)}`) },
  { field: "glassGrain", label: "Зерно", min: 0, max: 1, step: 0.02, fmt: pct },
  { field: "glassDepth", label: "Тёмная кромка", min: 0, max: 1, step: 0.05, fmt: pct },
  { field: "glassSheen", label: "Блик", min: 0, max: 1, step: 0.05, fmt: pct },
  { field: "glassRim", label: "Светлый ободок", min: 0, max: 1, step: 0.05, fmt: pct },
  { field: "glassShadow", label: "Тень", min: 0, max: 1, step: 0.05, fmt: pct },
  { field: "shadowOffset", label: "Сдвиг тени", min: 0, max: 24, step: 1, fmt: (v) => `${v} пикс.` },
  { field: "inkDepth", label: "Светлота текста", min: 0.15, max: 0.38, step: 0.01, fmt: (v) => Number(v).toFixed(2) },
  { field: "surfaceTint", label: "Оттенок раскрытых окон", min: 0, max: 1, step: 0.05, fmt: pct },
  { field: "radiusStrip", label: "Скругление полос", min: 0, max: 64, step: 2, fmt: (v) => (v >= 64 ? "пилюля" : `${v} пикс.`),
    toUi: (v) => Math.min(64, v), fromUi: (v) => (v >= 64 ? 999 : v) },
];

const CHOICES = [
  { field: "motionProfile", label: "Характер анимаций", options: [["responsive", "Отзывчивый"], ["balanced", "Сбалансированный"], ["fluid", "Плавный"]] },
  // Must stay in sync with FONT_BODY_OPTIONS in theme.js (exact strings).
  { field: "fontBody", label: "Шрифт текста", options: [["'Geist', sans-serif", "Geist"], ["'Segoe UI', 'Geist', sans-serif", "Segoe UI"], ["system-ui, sans-serif", "Системный"], ["'Ndot', monospace", "Ndot"]] },
  { field: "material", label: "Материал карточек", options: [["flat", "Плоское стекло"], ["jelly", "Морское стекло"]] },
  { field: "mgLayout", label: "Микрографика", options: [["panel", "Одной панелью"], ["pills", "Пилюлями"]] },
  { field: "avatarColor", label: "Аватары Telegram", options: [[false, "Чёрно-белые"], [true, "Цветные"]] },
];

let fineOpen = false;

function rgbToHex(rgb) {
  const m = String(rgb || "").match(/(\d{1,3})\s*,\s*(\d{1,3})\s*,\s*(\d{1,3})/);
  if (!m) return "#ffffff";
  const part = (n) => Math.max(0, Math.min(255, Number(n))).toString(16).padStart(2, "0");
  return `#${part(m[1])}${part(m[2])}${part(m[3])}`;
}

function hexToRgb(hex) {
  const h = String(hex || "").replace("#", "");
  if (!/^[0-9a-fA-F]{6}$/.test(h)) return null;
  return `${parseInt(h.slice(0, 2), 16)}, ${parseInt(h.slice(2, 4), 16)}, ${parseInt(h.slice(4, 6), 16)}`;
}

// Only validated hex values may enter HTML attributes (defense in depth —
// normalizeTheme already guarantees the shape for everything it returns).
function safeColor(value, fallback = "#ffffff") {
  return /^#[0-9a-fA-F]{6}$/.test(value) ? value : fallback;
}

function renderLook() {
  const pane = panel.querySelector("#pane-look");
  const t = getActiveTheme();
  const presets = getAllPresets();
  const accents = [...new Set(PALETTE_SLOTS.map((s) => safeColor(t.palette?.[s], "")).filter(Boolean))];

  pane.innerHTML = `
    <h3 class="menu-heading">Тема</h3>
    <div class="menu-themes" role="radiogroup" aria-label="Тема">
      ${presets.map((p) => `
        <button type="button" class="menu-theme" role="radio" aria-checked="${p.id === t.id}" data-preset="${escapeHtml(p.id)}"
          data-material="${p.material === "jelly" ? "jelly" : "flat"}"
          style="--theme-card-bg: rgb(${escapeHtml(p.surfaceRgb || "18, 18, 24")}); --theme-card-fg: rgb(${escapeHtml(p.fgRgb || "255, 255, 255")}); --theme-card-accent: ${safeColor(p.accent, "#d71921")};">
          <span class="menu-theme-swatch" aria-hidden="true"><span></span><span></span><span></span></span>
          <span class="menu-theme-name">${escapeHtml(p.name)}</span>
        </button>
      `).join("")}
    </div>

    <h3 class="menu-heading">Цвета</h3>
    <div class="menu-accents" role="radiogroup" aria-label="Акцентный цвет">
      ${accents.map((c) => `
        <button type="button" class="menu-accent" role="radio" aria-checked="${c.toLowerCase() === String(t.accent).toLowerCase()}"
          data-accent="${c}" style="background: ${c};" aria-label="Акцент ${c}"></button>
      `).join("")}
    </div>
    <div class="menu-actions">
      <button type="button" class="menu-btn" id="btnShuffle">Перемешать цвета</button>
    </div>

    <details class="menu-details" id="lookFine" ${fineOpen ? "open" : ""}>
      <summary>Тонкая настройка</summary>
      ${CHOICES.map((c) => `
        <div class="menu-field">
          <span class="menu-field-label">${c.label}</span>
          <div class="menu-pills" role="radiogroup" aria-label="${c.label}">
            ${c.options.map(([value, label]) => `
              <button type="button" class="menu-pill" role="radio" data-choice="${c.field}" data-value="${escapeHtml(String(value))}"
                aria-checked="${String(t[c.field] ?? "") === String(value)}">${label}</button>
            `).join("")}
          </div>
        </div>
      `).join("")}

      ${SLIDERS.map((sl) => {
        const v = Number(t[sl.field] ?? 0);
        const ui = sl.toUi ? sl.toUi(v) : v;
        return `
          <label class="menu-field">
            <span class="menu-field-label">${sl.label}<output data-out="${sl.field}">${sl.fmt(ui)}</output></span>
            <input type="range" class="menu-range" data-slider="${sl.field}" min="${sl.min}" max="${sl.max}" step="${sl.step}" value="${ui}">
          </label>`;
      }).join("")}

      <span class="menu-field-label">Палитра оттенков</span>
      <div class="menu-colors">
        ${PALETTE_SLOTS.map((slot) => `
          <label class="menu-color"><input type="color" data-slot="${slot}" value="${safeColor(t.palette?.[slot])}"><span>${SLOT_LABELS[slot]}</span></label>
        `).join("")}
      </div>

      <span class="menu-field-label">Каким цветом окрашено</span>
      <div class="menu-tints">
        ${TINT_TARGETS.map((target) => `
          <div class="menu-tint">
            <span>${TINT_LABELS[target] || target}</span>
            <div class="menu-tint-dots" role="radiogroup" aria-label="${TINT_LABELS[target] || target}">
              ${PALETTE_SLOTS.map((s) => `
                <button type="button" class="menu-tint-dot" role="radio" data-target="${target}" data-slot="${s}"
                  aria-checked="${t.tintMap?.[target] === s}" aria-label="${SLOT_LABELS[s]}"
                  style="background: ${safeColor(t.palette?.[s])};"></button>
              `).join("")}
            </div>
          </div>
        `).join("")}
      </div>

      <span class="menu-field-label">Текст, фоны и статусы</span>
      <div class="menu-colors">
        ${PALETTE_FIELDS.map((f) => `
          <label class="menu-color"><input type="color" data-field="${f.field}" data-kind="${f.kind}"
            value="${f.kind === "rgb" ? rgbToHex(t[f.field]) : safeColor(t[f.field])}"><span>${f.label}</span></label>
        `).join("")}
      </div>

      <span class="menu-field-label">Свои темы</span>
      <form class="menu-inline-form" id="presetForm">
        <input type="text" class="menu-input" id="presetName" placeholder="Название темы" maxlength="32" aria-label="Название темы">
        <button type="submit" class="menu-btn">Сохранить текущую</button>
      </form>
      ${presets.filter((p) => !p.isBuiltin).map((p) => `
        <div class="menu-row">
          <span class="menu-row-text"><span class="menu-row-name">${escapeHtml(p.name)}</span></span>
          <button type="button" class="menu-icon-btn" data-delete-preset="${escapeHtml(p.id)}" aria-label="Удалить тему «${escapeHtml(p.name)}»">
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" aria-hidden="true"><line x1="18" y1="6" x2="6" y2="18"></line><line x1="6" y1="6" x2="18" y2="18"></line></svg>
          </button>
        </div>
      `).join("")}
      <div class="menu-actions">
        <button type="button" class="menu-btn" id="btnResetSizes">Размеры виджетов как в теме</button>
      </div>
    </details>
  `;

  pane.querySelectorAll("[data-preset]").forEach((btn) => {
    btn.addEventListener("click", () => {
      const before = getActiveTheme().id;
      selectPreset(btn.dataset.preset);
      renderLook();
      if (before !== btn.dataset.preset) {
        showUndo(`Тема «${getActiveTheme().name}»`, () => {
          selectPreset(before);
          renderLook();
        });
      }
    });
  });

  pane.querySelectorAll("[data-accent]").forEach((btn) => {
    btn.addEventListener("click", () => {
      setLiveToken("accent", btn.dataset.accent);
      commitLiveChange();
      renderLook();
    });
  });

  pane.querySelector("#btnShuffle").addEventListener("click", () => {
    shuffleColors();
    renderLook();
    showUndo("Цвета перемешаны", () => {
      restorePreviousColors();
      renderLook();
    });
  });

  pane.querySelector("#lookFine").addEventListener("toggle", (e) => {
    fineOpen = e.currentTarget.open;
  });

  pane.querySelectorAll("[data-choice]").forEach((pill) => {
    pill.addEventListener("click", () => {
      const field = pill.dataset.choice;
      const raw = pill.dataset.value;
      setLiveToken(field, field === "avatarColor" ? raw === "true" : raw);
      commitLiveChange();
      renderLook();
    });
  });

  pane.querySelectorAll("[data-slider]").forEach((input) => {
    const spec = SLIDERS.find((sl) => sl.field === input.dataset.slider);
    input.addEventListener("input", () => {
      const ui = parseFloat(input.value);
      const value = spec.fromUi ? spec.fromUi(ui) : ui;
      setLiveToken(spec.field, value);
      spec.also?.(value);
      pane.querySelector(`[data-out="${spec.field}"]`).textContent = spec.fmt(ui);
    });
    input.addEventListener("change", () => commitLiveChange());
  });

  pane.querySelectorAll("input[data-slot]").forEach((input) => {
    input.addEventListener("input", () => {
      setLiveToken("palette", { ...getActiveTheme().palette, [input.dataset.slot]: input.value });
    });
    input.addEventListener("change", () => {
      commitLiveChange();
      renderLook();
    });
  });

  pane.querySelectorAll(".menu-tint-dot").forEach((dot) => {
    dot.addEventListener("click", () => {
      setLiveToken("tintMap", { ...getActiveTheme().tintMap, [dot.dataset.target]: dot.dataset.slot });
      commitLiveChange();
      renderLook();
    });
  });

  pane.querySelectorAll("input[data-field]").forEach((input) => {
    input.addEventListener("input", () => {
      const value = input.dataset.kind === "rgb" ? hexToRgb(input.value) : input.value;
      if (value !== null) setLiveToken(input.dataset.field, value);
    });
    input.addEventListener("change", () => commitLiveChange());
  });

  pane.querySelector("#presetForm").addEventListener("submit", (e) => {
    e.preventDefault();
    const name = pane.querySelector("#presetName").value.trim();
    if (!name) return;
    saveCustomPreset(name, getActiveTheme());
    showToast(`Тема «${name}» сохранена`);
    renderLook();
  });

  pane.querySelectorAll("[data-delete-preset]").forEach((btn) => {
    btn.addEventListener("click", () => {
      const preset = getAllPresets().find((p) => p.id === btn.dataset.deletePreset);
      deleteCustomPreset(btn.dataset.deletePreset);
      renderLook();
      if (preset) {
        showUndo(`Тема «${preset.name}» удалена`, () => {
          saveCustomPreset(preset.name, preset);
          renderLook();
        });
      }
    });
  });

  pane.querySelector("#btnResetSizes").addEventListener("click", () => {
    resetSizesToPreset();
    showToast("Размеры виджетов — как в теме");
  });
}

// ═══════════════════════════════════════════════════════════════════════════
// VPN
// ═══════════════════════════════════════════════════════════════════════════

async function renderVpn() {
  const pane = panel.querySelector("#pane-vpn");
  pane.innerHTML = `<div class="menu-empty">Загрузка…</div>`;

  let settings = null;
  let status = null;
  try {
    [settings, status] = await Promise.all([
      invoke("vpn_settings"),
      invoke("vpn_status").catch(() => null),
    ]);
  } catch (err) {
    console.error("vpn settings error:", err);
  }

  const back = `
    <button type="button" class="menu-back" data-back>
      <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" aria-hidden="true"><polyline points="15 18 9 12 15 6"></polyline></svg>
      Стол
    </button>`;

  // Without the stored settings the form would show blanks, and saving it
  // would overwrite the real subscription and server with them.
  if (!settings) {
    pane.innerHTML = `${back}
      <div class="menu-empty menu-empty-error">
        Не удалось загрузить настройки VPN. Форма скрыта, чтобы не затереть сохранённую подписку.
        <button type="button" class="menu-btn" data-retry>Повторить</button>
      </div>`;
    pane.querySelector("[data-back]").addEventListener("click", () => showView("desk"));
    pane.querySelector("[data-retry]").addEventListener("click", renderVpn);
    return;
  }

  const home = settings.home_server || {
    enabled: false, host: "", port: 443, uuid: "", public_key: "", short_id: "", sni: "", subnet: "10.10.0.0/16",
  };
  const statusLabel = !status ? "Состояние неизвестно" : status.connected ? "Подключено" : "Отключено";
  const field = (id, label, value, placeholder, type = "text") => `
    <label class="menu-field">
      <span class="menu-field-label">${label}</span>
      <input type="${type}" class="menu-input" id="${id}" value="${escapeHtml(String(value ?? ""))}" placeholder="${placeholder}">
    </label>`;

  pane.innerHTML = `${back}
    <h3 class="menu-heading">VPN <span class="menu-status ${status?.connected ? "is-on" : ""}">${statusLabel}</span></h3>

    <form class="menu-form" id="vpnSubForm">
      ${field("vpnSubUrl", "Ссылка на подписку", settings.subscription_url || "", "https://…")}
      <button type="submit" class="menu-btn menu-btn-primary">Сохранить подписку</button>
    </form>

    <div class="menu-row">
      <span class="menu-row-text">
        <span class="menu-row-name">Подключаться при запуске</span>
        <span class="menu-row-sub">VPN включится сам, когда запустится UniDTK</span>
      </span>
      <label class="menu-switch">
        <input type="checkbox" id="vpnAutoConnect" ${settings.auto_connect ? "checked" : ""} aria-label="Подключаться при запуске">
        <span class="menu-slider" aria-hidden="true"></span>
      </label>
    </div>

    <h3 class="menu-heading">Домашний сервер</h3>
    <div class="menu-row">
      <span class="menu-row-text">
        <span class="menu-row-name">Прямое подключение к своему серверу</span>
        <span class="menu-row-sub">Отдельно от подписки, протокол VLESS Reality</span>
      </span>
      <label class="menu-switch">
        <input type="checkbox" id="homeServerToggle" ${home.enabled ? "checked" : ""} aria-label="Домашний сервер">
        <span class="menu-slider" aria-hidden="true"></span>
      </label>
    </div>
    <form class="menu-form" id="homeServerForm" ${home.enabled ? "" : "hidden"}>
      ${field("hsHost", "Адрес сервера", home.host, "IP или домен")}
      ${field("hsPort", "Порт", home.port || 443, "443", "number")}
      ${field("hsUuid", "UUID пользователя", home.uuid, "xxxxxxxx-xxxx-…")}
      ${field("hsPublicKey", "Открытый ключ", home.public_key, "Reality public key")}
      ${field("hsShortId", "Короткий ID", home.short_id, "Reality short id")}
      ${field("hsSni", "Имя сервера (SNI)", home.sni, "example.com")}
      ${field("hsSubnet", "Домашняя подсеть", home.subnet || "10.10.0.0/16", "10.10.0.0/16")}
      <button type="submit" class="menu-btn menu-btn-primary">Сохранить сервер</button>
    </form>
  `;

  pane.querySelector("[data-back]").addEventListener("click", () => {
    showView("desk");
    panel.querySelector("#btnVpnSettings")?.focus();
  });

  const subForm = pane.querySelector("#vpnSubForm");
  subForm.addEventListener("submit", (e) => {
    e.preventDefault();
    runOnce(subForm.querySelector('[type="submit"]'), async () => {
      await invoke("vpn_set_subscription_url", { url: pane.querySelector("#vpnSubUrl").value.trim() });
      showToast("Подписка сохранена, список серверов обновлён");
    });
  });

  pane.querySelector("#vpnAutoConnect").addEventListener("change", async (e) => {
    const toggle = e.target;
    try {
      await invoke("vpn_set_auto_connect", { enabled: toggle.checked });
    } catch (err) {
      toggle.checked = !toggle.checked;
      showToast(`Автоподключение не сохранено: ${err?.message || err}`, { tone: "error" });
    }
  });

  const homeForm = pane.querySelector("#homeServerForm");
  const homeToggle = pane.querySelector("#homeServerToggle");
  const saveHome = async () => {
    await invoke("vpn_set_home_server_config", {
      config: {
        enabled: homeToggle.checked,
        host: pane.querySelector("#hsHost").value.trim(),
        port: parseInt(pane.querySelector("#hsPort").value, 10) || 443,
        uuid: pane.querySelector("#hsUuid").value.trim(),
        public_key: pane.querySelector("#hsPublicKey").value.trim(),
        short_id: pane.querySelector("#hsShortId").value.trim(),
        sni: pane.querySelector("#hsSni").value.trim(),
        subnet: pane.querySelector("#hsSubnet").value.trim() || "10.10.0.0/16",
      },
    });
  };
  // The switch saves by itself: turning the server off must not wait for a
  // second click on a form that is now hidden.
  homeToggle.addEventListener("change", () => {
    homeForm.hidden = !homeToggle.checked;
    runOnce(homeToggle, async () => {
      await saveHome();
      showToast(homeToggle.checked ? "Домашний сервер включён" : "Домашний сервер выключен");
    });
  });
  homeForm.addEventListener("submit", (e) => {
    e.preventDefault();
    runOnce(homeForm.querySelector('[type="submit"]'), async () => {
      await saveHome();
      showToast("Настройки сервера сохранены");
    });
  });
}
