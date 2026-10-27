import { invoke, convertFileSrc, listen } from '@unidtk/bridge';
import { saveConfig, showDebugMessage, editMode, widgetsCanvas, config, collapseActiveDesktopWidget, escapeHtml } from '../../unidtk-core.js';
import { surfaceManager } from '../../surface-manager.js';
import { setupMusicAudioArcs } from './audio-arcs.js';

let musicTrackCache = null;
let musicTrackCachePromise = null;
// After one miss-driven refresh, stop re-fetching until cache is explicitly cleared.
let musicTrackCacheMissRetried = false;
let currentMusicVolume = 100;
let musicVolumeToastTimer = null;
let lastTrackId = null;

// Theme token cache (stage 4): canvas 2D cannot resolve CSS custom properties,
// so the channels the AI DJ graph needs are read once at mount and refreshed on
// the `unidtk-theme-changed` contract event — never per frame. DOM inline styles
// below use var() directly and need no cache.
const graphThemeTokens = {
  accent: "#d71921",
  accentRgb: "215, 25, 33",
  fgRgb: "255, 255, 255",
  surfaceDeepRgb: "12, 12, 16",
};
let graphThemeHooked = false;

function refreshGraphThemeTokens() {
  const cs = getComputedStyle(document.documentElement);
  const accent = cs.getPropertyValue("--theme-accent").trim();
  const accentRgb = cs.getPropertyValue("--theme-accent-rgb").trim();
  const fgRgb = cs.getPropertyValue("--fg-rgb").trim();
  const surfaceDeepRgb = cs.getPropertyValue("--bg-surface-deep-rgb").trim();
  if (accent) graphThemeTokens.accent = accent;
  if (accentRgb) graphThemeTokens.accentRgb = accentRgb;
  if (fgRgb) graphThemeTokens.fgRgb = fgRgb;
  if (surfaceDeepRgb) graphThemeTokens.surfaceDeepRgb = surfaceDeepRgb;
}

function hookGraphThemeEvents() {
  if (graphThemeHooked) return;
  graphThemeHooked = true;
  refreshGraphThemeTokens();
  window.addEventListener("unidtk-theme-changed", refreshGraphThemeTokens);
}

function showMusicVolumeToast(card, vol) {
  const titleEl = card?.querySelector(".music-title");
  if (!titleEl) return;
  if (!card._volumeToastActive) {
    card._cachedTitleBeforeVolume = titleEl.textContent;
    card._volumeToastActive = true;
  }
  titleEl.textContent = `VOL: ${Math.round(vol)}%`;
  clearTimeout(musicVolumeToastTimer);
  musicVolumeToastTimer = setTimeout(() => {
    card._volumeToastActive = false;
    if (card._lastTrackTitle) {
      titleEl.textContent = card._lastTrackTitle;
    } else if (card._cachedTitleBeforeVolume) {
      titleEl.textContent = card._cachedTitleBeforeVolume;
    }
  }, 1400);
}

async function nudgeMusicVolume(delta, card) {
  try {
    const st = await invoke("player_status").catch(() => null);
    if (st && typeof st.volume === "number") {
      currentMusicVolume = st.volume;
    }
    const next = Math.max(0, Math.min(100, Math.round(currentMusicVolume + delta)));
    currentMusicVolume = next;
    await invoke("player_set_volume", { volume: next });
    const targetCard = card || document.querySelector(".music-widget");
    if (targetCard) showMusicVolumeToast(targetCard, next);
    // Also sync menu slider if open
    const slider = document.querySelector(".music-volume-slider");
    const valEl = document.querySelector(".music-volume-value");
    if (slider) slider.value = next;
    if (valEl) valEl.textContent = `${next}%`;
  } catch (err) {
    console.error("nudgeMusicVolume failed:", err);
  }
}

async function runMusicCommand(command, args = {}) {
  try {
    return await invoke(command, args);
  } catch (err) {
    showDebugMessage(`Music: ${err.message || err}`);
    return null;
  }
}

function isYoutubeId(id) {
  return typeof id === "string" && /^[a-zA-Z0-9_-]{11}$/.test(id);
}

function formatMusicTitle(track) {
  const title = track?.title || "---";
  return title.length > 20 ? title.slice(0, 20) + "..." : title;
}

function musicCoverUrl(track) {
  if (track?.thumbnail_path) {
    return musicMediaUrl(track.thumbnail_path);
  }
  if (track?.id && isYoutubeId(track.id)) {
    return `https://i.ytimg.com/vi/${track.id}/hqdefault.jpg`;
  }
  return "";
}

function musicMediaUrl(path) {
  if (!path) return "";
  const value = String(path);
  if (/^https?:\/\//i.test(value)) return value;
  return convertFileSrc(value.replace(/\\/g, "/"));
}

async function getMusicTrackById(trackId) {
  if (!trackId) return null;
  if (musicTrackCache && musicTrackCache.has(trackId)) {
    return musicTrackCache.get(trackId);
  }
  try {
    const single = await invoke("get_track", { trackId });
    if (single) {
      if (!musicTrackCache) musicTrackCache = new Map();
      musicTrackCache.set(single.id, single);
      return single;
    }
  } catch {}

  const ensureCache = () => {
    if (!musicTrackCachePromise) {
      musicTrackCachePromise = invoke("list_tracks", { search: null })
        .then((tracks) => {
          musicTrackCache = new Map(tracks.map((track) => [track.id, track]));
          return musicTrackCache;
        })
        .catch((err) => {
          musicTrackCachePromise = null;
          throw err;
        });
    }
    return musicTrackCachePromise;
  };
  try {
    let cache = musicTrackCache || await ensureCache();
    if (cache && cache.has(trackId)) return cache.get(trackId);
    musicTrackCache = null;
    musicTrackCachePromise = null;
    cache = await ensureCache();
    return cache.get(trackId) || null;
  } catch {
    return null;
  }
}

// ─── Music In-Widget Popups (Library & Settings) ───
// Попапы являются дочерними элементами карточки плеера (.music-widget),
// центрируются точно по плееру и перемещаются вместе с ним.

function openMusicPopup(card, type) {
  if (editMode || !card) return;
  const library = card.querySelector(".music-library-popup");
  const settings = card.querySelector(".music-settings-popup");
  const targetPopup = type === "library" ? library : settings;
  const otherPopup = type === "library" ? settings : library;
  const triggerZone = type === "library"
    ? (card.querySelector(".zone-top") || card)
    : (card.querySelector(".zone-bottom") || card);

  if (!targetPopup) return;

  if (otherPopup && (surfaceManager.isOpen(otherPopup) || otherPopup.classList.contains("expanded"))) {
    closePopupElement(otherPopup);
  }

  // Widget mutual exclusion (the same rule toggleWidget enforces): an expanded
  // desktop widget — above all the Telegram panel, which is a surface with
  // closeOnOutsideClick:false — folds before this popup opens, so the two
  // surfaces never overlap.
  collapseActiveDesktopWidget(card);

  card.style.zIndex = "var(--z-surface-panel, 250)";

  const body = targetPopup.querySelector(".music-popup-body");
  let releaseDashboardRender = null;
  if (type === "library") {
    const renderGate = new Promise((resolve) => { releaseDashboardRender = resolve; });
    renderMusicDashboard(body, card.dataset.id, { beforeCommit: () => renderGate });
  } else {
    renderMusicMenu(body, card.dataset.id);
  }

  surfaceManager.openSurface(targetPopup, {
    origin: triggerZone,
    originRadius: "75px",
    duration: "motion",
    openOpacity: 1,
    closeOpacity: 1,
    elevation: "var(--z-surface-panel, 250)",
    ignoreOutsideElements: [triggerZone],
    onOpened: () => {
      releaseDashboardRender?.();
      releaseDashboardRender = null;
    },
    onOpen: () => {
      targetPopup.classList.add("expanded");
    },
    onClosed: () => {
      if (body) body._dashboardRenderGeneration = (body._dashboardRenderGeneration || 0) + 1;
      releaseDashboardRender?.();
      releaseDashboardRender = null;
      targetPopup.classList.remove("expanded");
      const anyRemaining = card && (card.querySelector(".music-popup.expanded") || card.querySelector(".music-popup.surface-open") || card.querySelector(".music-popup.surface-active"));
      if (!anyRemaining && card) {
        card.style.zIndex = "";
      }
      if (body?._musicTasksInterval) {
        clearInterval(body._musicTasksInterval);
        body._musicTasksInterval = null;
      }
      if (typeof body?._aiDjCleanup === "function") {
        body._aiDjCleanup();
        body._aiDjCleanup = null;
      }
      if (typeof body?._dashboardCleanup === "function") {
        body._dashboardCleanup();
        body._dashboardCleanup = null;
      }
    }
  });
}

function closePopupElement(popup) {
  if (!popup) return;
  surfaceManager.closeSurface(popup);
}

function closeAllMusicPopups(card) {
  if (!card) {
    document.querySelectorAll(".music-widget").forEach(closeAllMusicPopups);
    return;
  }
  card.querySelectorAll(".music-popup").forEach((popup) => {
    if (surfaceManager.isOpen(popup) || popup.classList.contains("expanded")) {
      closePopupElement(popup);
    }
  });
  card.style.zIndex = "";
}

// Алиасы для обратной совместимости
function openMusicSatellite(card, sat) {
  openMusicPopup(card, "library");
}

function closeMusicSatellite(card, sat) {
  if (sat) closePopupElement(sat);
  else closeAllMusicPopups(card);
}

function openMusicMenu(card, menu) {
  openMusicPopup(card, "settings");
}

function syncTaskLabel(task) {
  const progress = Number.isFinite(task?.progress_pct) ? ` ${Math.round(task.progress_pct)}%` : "";
  const status = task?.status || "queued";
  const label = task?.label || "Sync";
  return `${label}: ${status}${progress}`;
}

async function renderMusicMenu(body, musicId) {
  if (typeof body?._aiDjCleanup === "function") {
    body._aiDjCleanup();
    body._aiDjCleanup = null;
  }

  const autoShuffle = !!config.positions?.[musicId]?.autoShuffle;
  const aiDj = !!config.positions?.[musicId]?.aiDj;
  let activeMode = "normal";
  if (aiDj) activeMode = "ai_dj";
  else if (autoShuffle) activeMode = "shuffle";

  body.innerHTML = `
    <div class="music-mode-selector">
      <button type="button" class="music-mode-btn ${activeMode === 'normal' ? 'active' : ''}" data-mode="normal">NORMAL</button>
      <button type="button" class="music-mode-btn ${activeMode === 'shuffle' ? 'active' : ''}" data-mode="shuffle">SHUFFLE</button>
      <button type="button" class="music-mode-btn ${activeMode === 'ai_dj' ? 'active' : ''}" data-mode="ai_dj">AI DJ</button>
    </div>
    <div class="music-ai-panel">
      <div class="music-ai-status-bar">
        <span class="music-ai-count">Ready: 0 / 0</span>
        <button type="button" class="music-ai-graph-btn">Graph</button>
      </div>
      <div class="ai-dj-graph-container" style="display: none; position: relative; width: 100%; height: 230px; margin-top: 8px; border-radius: 12px; background: rgb(var(--bg-surface-deep-rgb)); border: 1px solid rgba(var(--bg-elev-rgb), 0.08); overflow: hidden; user-select: none; touch-action: none;">
        <canvas class="ai-dj-graph-canvas" style="width: 100%; height: 100%; display: block; cursor: grab;"></canvas>
        <div class="ai-dj-graph-tooltip" style="position: absolute; pointer-events: none; display: none; padding: 6px 10px; background: rgba(var(--bg-surface-rgb), 0.94); border: 1px solid rgba(var(--bg-elev-rgb), 0.18); border-radius: 8px; font-size: 11px; color: rgb(var(--fg-rgb)); font-family: inherit; z-index: 10; backdrop-filter: blur(8px); box-shadow: 0 4px 16px rgba(0,0,0,0.6);">
          <div class="ai-dj-tooltip-title" style="font-weight: 600; white-space: nowrap; max-width: 200px; overflow: hidden; text-overflow: ellipsis;"></div>
          <div class="ai-dj-tooltip-meta" style="font-size: 10px; color: rgba(var(--fg-rgb), 0.55); margin-top: 2px;"></div>
        </div>
        <div class="ai-dj-graph-hint" style="position: absolute; bottom: 6px; left: 8px; font-size: 9px; color: rgba(var(--fg-rgb), 0.3); pointer-events: none; font-family: var(--font-mono,monospace);">
          Drag to Pan • Wheel to Zoom • Click to Play
        </div>
        <div class="ai-dj-graph-loading" style="position: absolute; inset: 0; display: flex; align-items: center; justify-content: center; background: rgba(var(--scrim-rgb), 0.85); color: rgba(var(--bg-elev-rgb), 0.85); font-size: 11px; font-family: var(--font-mono,monospace);">
          Computing Graph...
        </div>
      </div>
      <button type="button" class="music-ai-index-btn">Index Library</button>
      <div class="music-ai-progress-wrap" style="display: none;">
        <div class="music-ai-progress-info">
          <span class="music-ai-progress-track">Analyzing...</span>
          <span class="music-ai-progress-pct">0%</span>
        </div>
        <div class="music-ai-progress-bar">
          <div class="music-ai-progress-fill"></div>
        </div>
      </div>
      <div class="music-ai-model-wrap" style="display: none;">
        <div class="music-ai-model-error" style="display: none;"></div>
        <div class="music-ai-model-progress" style="display: none;">
          <div class="music-ai-model-info">
            <span class="music-ai-model-file"></span>
            <span class="music-ai-model-pct">0%</span>
          </div>
          <div class="music-ai-model-bar">
            <div class="music-ai-model-fill" style="width: 0%;"></div>
          </div>
        </div>
      </div>
    </div>
  `;

  const card = body.closest(".music-widget");
  const countEl = body.querySelector(".music-ai-count");
  const indexBtn = body.querySelector(".music-ai-index-btn");
  const progressWrap = body.querySelector(".music-ai-progress-wrap");
  const progressTrack = body.querySelector(".music-ai-progress-track");
  const progressPct = body.querySelector(".music-ai-progress-pct");
  const progressFill = body.querySelector(".music-ai-progress-fill");

  // Докачка AI DJ модели — отдельный ход со своей поверхностью: не смешивается
  // с прогрессом анализа библиотеки (music-ai-progress-wrap).
  const modelWrap = body.querySelector(".music-ai-model-wrap");
  const modelError = body.querySelector(".music-ai-model-error");
  const modelProgress = body.querySelector(".music-ai-model-progress");
  const modelFile = body.querySelector(".music-ai-model-file");
  const modelPct = body.querySelector(".music-ai-model-pct");
  const modelFill = body.querySelector(".music-ai-model-fill");
  let modelState = "checking"; // checking | hidden | offer | downloading | error
  let modelCurrentFile = "";

  const MB = 1024 * 1024;
  const mbText = (bytes) => (bytes / MB).toFixed(1);
  const MODEL_LABEL = "Скачать модель (331 МБ)";
  const MODEL_LABEL_RETRY = "Повторить скачивание";

  const modelSetBtn = (label, disabled) => {
    if (!indexBtn) return;
    indexBtn.textContent = label;
    indexBtn.disabled = !!disabled;
  };

  const hideModelError = () => {
    if (modelError) modelError.style.display = "none";
  };

  // Поверхность докачки не всплывает под открытым графом: состояния
  // обновляются и при закрытом поповере, но show только когда граф спрятан.
  const modelUiVisible = () => !graphContainer || graphContainer.style.display === "none";

  const showModelProgress = (fileName, bytesDone, bytesTotal) => {
    if (!modelWrap || !body.isConnected) return;
    modelState = "downloading";
    if (fileName) modelCurrentFile = fileName;
    hideModelError();
    if (modelProgress) modelProgress.style.display = "flex";
    if (modelFile) {
      modelFile.textContent = bytesTotal > 0
        ? `${modelCurrentFile || "модель"} · ${mbText(bytesDone)} / ${mbText(bytesTotal)} МБ`
        : (modelCurrentFile || "модель");
    }
    if (bytesTotal > 0) {
      const pct = Math.min(100, Math.round((bytesDone / bytesTotal) * 100));
      if (modelPct) modelPct.textContent = `${pct}%`;
      if (modelFill) modelFill.style.width = `${pct}%`;
    } else if (modelPct) {
      modelPct.textContent = bytesDone > 0 ? `${mbText(bytesDone)} МБ` : "0%";
    }
    modelSetBtn("Скачивание модели…", true);
    if (modelUiVisible()) modelWrap.style.display = "flex";
  };

  const showModelOffer = () => {
    if (!modelWrap || !body.isConnected) return;
    modelState = "offer";
    hideModelError();
    if (modelProgress) modelProgress.style.display = "none";
    if (modelFill) modelFill.style.width = "0%";
    if (modelPct) modelPct.textContent = "0%";
    if (modelFile) modelFile.textContent = "";
    modelSetBtn(MODEL_LABEL, false);
    if (modelUiVisible()) modelWrap.style.display = "flex";
  };

  const showModelError = (message) => {
    if (!modelWrap || !body.isConnected) return;
    modelState = "error";
    if (modelProgress) modelProgress.style.display = "none";
    if (modelError) {
      modelError.textContent = String(message);
      modelError.style.display = "block";
    }
    modelSetBtn(MODEL_LABEL_RETRY, false);
    if (modelUiVisible()) modelWrap.style.display = "flex";
  };

  const hideModelUi = () => {
    modelState = "hidden";
    if (modelWrap) modelWrap.style.display = "none";
    hideModelError();
    if (modelProgress) modelProgress.style.display = "none";
    if (modelFill) modelFill.style.width = "0%";
    if (modelPct) modelPct.textContent = "0%";
    if (modelFile) modelFile.textContent = "";
  };

  const graphBtn = body.querySelector(".music-ai-graph-btn");
  const graphContainer = body.querySelector(".ai-dj-graph-container");
  const canvas = body.querySelector(".ai-dj-graph-canvas");
  const tooltip = body.querySelector(".ai-dj-graph-tooltip");
  const tooltipTitle = body.querySelector(".ai-dj-tooltip-title");
  const tooltipMeta = body.querySelector(".ai-dj-tooltip-meta");
  const loadingOverlay = body.querySelector(".ai-dj-graph-loading");

  let lastStatus = null;

  // Refresh status from backend
  const updateAiDjStatus = async () => {
    try {
      const status = await invoke("ai_dj_status");
      lastStatus = status;
      if (!body.isConnected) return;

      const total = typeof status?.track_count === "number" ? status.track_count : 0;
      const reachable = typeof status?.reachable_count === "number" ? status.reachable_count : total;
      const analyzed = typeof status?.analyzed_count === "number" ? status.analyzed_count : 0;

      if (countEl) {
        countEl.textContent = reachable > 0 ? `Ready: ${analyzed} / ${reachable}` : "0 tracks";
      }

      // Update indicator on widget if available
      const ind = card?.querySelector(".ai-dj-indicator");
      const indCount = card?.querySelector(".ai-dj-count");
      if (indCount) indCount.textContent = String(analyzed);

      if (status?.is_analyzing) {
        if (indexBtn) {
          indexBtn.disabled = true;
          indexBtn.textContent = "Analyzing...";
        }
        if (progressWrap && (!graphContainer || graphContainer.style.display === "none")) {
          progressWrap.style.display = "flex";
        }
        if (progressTrack && status.current_analyzing_track) {
          progressTrack.textContent = status.current_analyzing_track;
        }
        if (reachable > 0) {
          const pct = Math.min(100, Math.round((analyzed / reachable) * 100));
          if (progressPct) progressPct.textContent = `${pct}%`;
          if (progressFill) progressFill.style.width = `${pct}%`;
        }
      } else if (status?.model_present === false) {
        // Модели нет: кнопка индекса работает кнопкой скачивания, текст
        // задаёт статус докачки (offer / downloading / error).
        if (status?.model_downloading) {
          const bytesDone = typeof status.model_download_bytes_done === "number"
            ? status.model_download_bytes_done : 0;
          const bytesTotal = typeof status.model_download_bytes_total === "number"
            ? status.model_download_bytes_total : 0;
          showModelProgress(modelCurrentFile, bytesDone, bytesTotal);
        } else if (modelState !== "error") {
          // После ошибки интерфейс не сбрасываем: там retry-кнопка и текст.
          showModelOffer();
        }
      } else {
        if (indexBtn) {
          indexBtn.disabled = false;
          if (reachable > 0 && analyzed >= reachable) {
            indexBtn.textContent = "Re-analyze Library";
          } else {
            indexBtn.textContent = "Index Library";
          }
        }
        if (modelState !== "hidden") {
          hideModelUi();
        }
      }
    } catch (err) {
      console.warn("AI DJ status update failed:", err);
    }
  };

  await updateAiDjStatus();

  // Mode button clicks
  body.querySelectorAll(".music-mode-btn").forEach((btn) => {
    btn.addEventListener("click", async (e) => {
      e.stopPropagation();
      if (editMode) return;
      const mode = btn.dataset.mode;
      if (!config.positions[musicId]) config.positions[musicId] = {};

      if (mode === "normal") {
        config.positions[musicId].autoShuffle = false;
        config.positions[musicId].aiDj = false;
        invoke("toggle_ai_dj", { enabled: false }).catch(() => {});
      } else if (mode === "shuffle") {
        config.positions[musicId].autoShuffle = true;
        config.positions[musicId].aiDj = false;
        invoke("toggle_ai_dj", { enabled: false }).catch(() => {});
      } else if (mode === "ai_dj") {
        config.positions[musicId].autoShuffle = false;
        config.positions[musicId].aiDj = true;
        invoke("toggle_ai_dj", { enabled: true }).catch(() => {});
      }
      saveConfig();

      body.querySelectorAll(".music-mode-btn").forEach((b) => {
        b.classList.toggle("active", b.dataset.mode === mode);
      });

      const ind = card?.querySelector(".ai-dj-indicator");
      if (ind) ind.classList.toggle("active", mode === "ai_dj");

      if (mode === "ai_dj") {
        setTimeout(updateAiDjStatus, 150);
      }
    });
  });

  // Index / Re-analyze button click
  indexBtn?.addEventListener("click", async (e) => {
    e.stopPropagation();
    if (editMode || indexBtn.disabled) return;

    // Без модели кнопка индекса — это кнопка докачки: сначала модель,
    // анализ без неё невозможен (бэкенд отвечает той же ошибкой).
    if (modelState === "offer" || modelState === "error") {
      indexBtn.disabled = true;
      indexBtn.textContent = "Скачивание модели…";
      try {
        const outcome = await invoke("ai_dj_model_download");
        if (outcome?.result === "already_present") {
          // Появилась, пока был интерфейс — вернуться к обычной разметке.
          hideModelUi();
          await updateAiDjStatus();
        } else {
          showModelProgress(
            typeof outcome?.file === "string" ? outcome.file : "",
            typeof outcome?.bytesDone === "number" ? outcome.bytesDone : 0,
            typeof outcome?.bytesTotal === "number" ? outcome.bytesTotal : 0,
          );
        }
      } catch (err) {
        showModelError(err && err.message ? err.message : String(err));
        indexBtn.disabled = false;
      }
      return;
    }

    const reachable = (lastStatus && typeof lastStatus.reachable_count === "number") ? lastStatus.reachable_count : (lastStatus?.track_count || 0);
    const isFullReanalysis = reachable > 0 && (lastStatus?.analyzed_count || 0) >= reachable;
    indexBtn.disabled = true;
    indexBtn.textContent = "Analyzing...";
    if (progressWrap) progressWrap.style.display = "flex";
    if (progressTrack) progressTrack.textContent = isFullReanalysis ? "Re-analyzing library..." : "Scanning library...";
    if (progressPct) progressPct.textContent = "0%";
    if (progressFill) progressFill.style.width = "0%";

    try {
      const command = isFullReanalysis ? "reanalyze_tracks" : "analyze_tracks";
      const analyzed = await invoke(command);
      if (progressTrack) progressTrack.textContent = `Completed (${analyzed} tracks)`;
      if (progressPct) progressPct.textContent = "100%";
      if (progressFill) progressFill.style.width = "100%";
      await updateAiDjStatus();
      setTimeout(() => {
        if (progressWrap && body.isConnected && !lastStatus?.is_analyzing) {
          progressWrap.style.display = "none";
        }
      }, 2500);
    } catch (err) {
      console.error("Analysis command failed:", err);
      if (progressTrack) progressTrack.textContent = `Error: ${err.message || err}`;
      indexBtn.disabled = false;
      indexBtn.textContent = "Retry Index";
      setTimeout(() => {
        if (progressWrap && body.isConnected) {
          progressWrap.style.display = "none";
        }
      }, 3500);
    }
  });

  // Tauri event listener for real-time progress
  let unlistenProgress = null;
  listen("ai-dj-progress", (event) => {
    if (!body.isConnected) return;
    const { current, total, track } = event.payload || {};
    if (progressWrap && (!graphContainer || graphContainer.style.display === "none")) {
      progressWrap.style.display = "flex";
    }
    if (progressTrack && track) progressTrack.textContent = track;
    if (total > 0) {
      const pct = Math.min(100, Math.round((current / total) * 100));
      if (progressPct) progressPct.textContent = `${pct}%`;
      if (progressFill) progressFill.style.width = `${pct}%`;
      if (countEl) countEl.textContent = `Ready: ${current} / ${total}`;
    }
    if (indexBtn && !indexBtn.disabled) {
      indexBtn.disabled = true;
      indexBtn.textContent = "Analyzing...";
    }
    if (current >= total && total > 0) {
      if (indexBtn) {
        indexBtn.disabled = false;
        indexBtn.textContent = "Re-analyze Library";
      }
      setTimeout(() => {
        if (progressWrap && body.isConnected) progressWrap.style.display = "none";
      }, 2500);
    }
  }).then((unsub) => {
    unlistenProgress = unsub;
  }).catch((err) => {
    console.warn("Failed to listen to ai-dj-progress:", err);
  });

  // Докачка модели: свой ход и своё событие — не смешивается с ai-dj-progress
  // (там сканируется библиотека).
  let unlistenModel = null;
  listen("ai-dj-model", (event) => {
    if (!body.isConnected) return;
    const p = event.payload || {};
    const bytesDone = typeof p.bytesDone === "number" ? p.bytesDone : 0;
    const bytesTotal = typeof p.bytesTotal === "number" ? p.bytesTotal : 0;
    if (p.stage === "download") {
      showModelProgress(typeof p.fileName === "string" ? p.fileName : "", bytesDone, bytesTotal);
    } else if (p.stage === "error") {
      showModelError(p.message || "Скачивание модели не удалось");
    } else if (p.stage === "done") {
      // Модель валидна и на месте — обычная разметка и статус из бэкенда.
      hideModelUi();
      updateAiDjStatus();
    }
  }).then((unsub) => {
    unlistenModel = unsub;
  }).catch((err) => {
    console.warn("Failed to listen to ai-dj-model:", err);
  });

  // --- AI DJ Similarity Graph Canvas Engine ---
  let graphData = null;
  let graphNodes = [];
  let graphEdges = [];
  let simAnimId = null;
  let isSimulating = false;
  let isGraphAborted = false;
  let panX = 0;
  let panY = 0;
  let zoom = 1.0;
  let hoveredNode = null;
  let isDragging = false;
  let dragStartX = 0;
  let dragStartY = 0;
  let dragStartPanX = 0;
  let dragStartPanY = 0;
  let hasDragged = false;

  const drawGraph = () => {
    if (!canvas || !graphContainer) return;
    const ctx = canvas.getContext("2d");
    if (!ctx) return;

    const rect = graphContainer.getBoundingClientRect();
    const dpr = window.devicePixelRatio || 1;
    const w = rect.width || 280;
    const h = rect.height || 230;

    if (canvas.width !== Math.round(w * dpr) || canvas.height !== Math.round(h * dpr)) {
      canvas.width = Math.round(w * dpr);
      canvas.height = Math.round(h * dpr);
    }

    ctx.save();
    ctx.scale(dpr, dpr);
    ctx.clearRect(0, 0, w, h);

    // Deep dark background (theme surface channel via token cache)
    ctx.fillStyle = `rgb(${graphThemeTokens.surfaceDeepRgb})`;
    ctx.fillRect(0, 0, w, h);

    // Pan & zoom transform
    ctx.translate(w / 2 + panX, h / 2 + panY);
    ctx.scale(zoom, zoom);

    const activeTrackId = lastTrackId;
    const hoveredIdx = hoveredNode ? graphNodes.indexOf(hoveredNode) : -1;

    // Draw edges
    for (let i = 0; i < graphEdges.length; i++) {
      const [u, v, score] = graphEdges[i];
      const n1 = graphNodes[u];
      const n2 = graphNodes[v];
      if (!n1 || !n2) continue;

      const isConnectedToHover = hoveredIdx >= 0 && (u === hoveredIdx || v === hoveredIdx);
      if (isConnectedToHover) {
        ctx.strokeStyle = `rgba(${graphThemeTokens.fgRgb}, 0.55)`;
        ctx.lineWidth = 1.4 / zoom;
      } else {
        const alpha = Math.min(0.22, Math.max(0.04, score * 0.18));
        ctx.strokeStyle = `rgba(${graphThemeTokens.fgRgb}, ${alpha})`;
        ctx.lineWidth = 0.7 / zoom;
      }

      ctx.beginPath();
      ctx.moveTo(n1.x, n1.y);
      ctx.lineTo(n2.x, n2.y);
      ctx.stroke();
    }

    // Draw nodes
    for (let i = 0; i < graphNodes.length; i++) {
      const node = graphNodes[i];
      const isHovered = node === hoveredNode;
      const isPlaying = activeTrackId && node.id === activeTrackId;

      ctx.beginPath();
      if (isPlaying) {
        // Theme accent halo & solid dot (token cache — no per-frame CSS reads)
        const accent = graphThemeTokens.accent;
        const accentRgb = graphThemeTokens.accentRgb;
        ctx.arc(node.x, node.y, 6 / zoom, 0, Math.PI * 2);
        ctx.fillStyle = accent;
        ctx.fill();

        ctx.beginPath();
        ctx.arc(node.x, node.y, 10 / zoom, 0, Math.PI * 2);
        ctx.strokeStyle = `rgba(${accentRgb}, 0.45)`;
        ctx.lineWidth = 1.5 / zoom;
        ctx.stroke();
      } else if (isHovered) {
        ctx.arc(node.x, node.y, 5 / zoom, 0, Math.PI * 2);
        ctx.fillStyle = `rgb(${graphThemeTokens.fgRgb})`;
        ctx.fill();
      } else {
        ctx.arc(node.x, node.y, 2.5 / zoom, 0, Math.PI * 2);
        ctx.fillStyle = `rgba(${graphThemeTokens.fgRgb}, 0.4)`;
        ctx.fill();
      }
    }

    ctx.restore();
  };

  const screenToWorld = (sx, sy) => {
    const rect = canvas.getBoundingClientRect();
    const w = rect.width;
    const h = rect.height;
    const wx = (sx - rect.left - (w / 2 + panX)) / zoom;
    const wy = (sy - rect.top - (h / 2 + panY)) / zoom;
    return { wx, wy };
  };

  const findNodeAt = (sx, sy) => {
    const { wx, wy } = screenToWorld(sx, sy);
    const threshold = 12 / zoom;
    let closest = null;
    let minD = threshold * threshold;
    for (let i = 0; i < graphNodes.length; i++) {
      const n = graphNodes[i];
      const dx = n.x - wx;
      const dy = n.y - wy;
      const d = dx * dx + dy * dy;
      if (d < minD) {
        minD = d;
        closest = n;
      }
    }
    return closest;
  };

  // Вписать облако узлов в кадр: вызывается сразу после раскладки и на фиксации.
  const fitView = () => {
    if (!graphContainer || graphNodes.length === 0) return;
    let minX = Infinity, maxX = -Infinity, minY = Infinity, maxY = -Infinity;
    for (let i = 0; i < graphNodes.length; i++) {
      const n = graphNodes[i];
      if (n.x < minX) minX = n.x;
      if (n.x > maxX) maxX = n.x;
      if (n.y < minY) minY = n.y;
      if (n.y > maxY) maxY = n.y;
    }
    const rect = graphContainer.getBoundingClientRect();
    const gw = rect.width || 340;
    const gh = rect.height || 300;
    const bboxW = (maxX - minX) || 200;
    const bboxH = (maxY - minY) || 200;
    zoom = Math.min(2.0, Math.max(0.10, Math.min((gw - 36) / bboxW, (gh - 36) / bboxH)));
    panX = -((minX + maxX) / 2) * zoom;
    panY = -((minY + maxY) / 2) * zoom;
  };

  // Очередь для запуска из графа: обход по его рёбрам. Пока у текущего узла
  // есть непройденные соседи — идём в самого похожего из них; когда локальный
  // кластер исчерпан, подключаемся по сильнейшему оставшемуся ребру к уже
  // пройденным (каждый следующий трек — сосед в графе, а не случайный).
  const buildGraphPlaybackQueue = (startId) => {
    const nodes = graphData?.nodes || [];
    const ids = nodes.map((node) => node.id);
    if (!ids.includes(startId)) return [startId];

    const adjacency = new Map();
    for (const [u, v, score] of (graphData.edges || [])) {
      const a = ids[u];
      const b = ids[v];
      if (!a || !b) continue;
      if (!adjacency.has(a)) adjacency.set(a, []);
      if (!adjacency.has(b)) adjacency.set(b, []);
      adjacency.get(a).push([b, score]);
      adjacency.get(b).push([a, score]);
    }

    const visited = new Set([startId]);
    const queue = [startId];
    const frontier = new Map(); // id -> лучший счёт ребра к пройденным
    const consider = (from) => {
      for (const [nid, score] of adjacency.get(from) || []) {
        if (visited.has(nid)) continue;
        const best = frontier.get(nid);
        if (best === undefined || score > best) frontier.set(nid, score);
      }
    };
    consider(startId);

    const maxWalk = 60;
    while (queue.length < maxWalk && frontier.size) {
      let bestId = null;
      let bestScore = -Infinity;
      for (const [nid, score] of frontier) {
        if (score > bestScore) {
          bestScore = score;
          bestId = nid;
        }
      }
      visited.add(bestId);
      frontier.delete(bestId);
      queue.push(bestId);
      consider(bestId);
    }
    for (const id of ids) {
      if (!visited.has(id)) queue.push(id);
    }
    return queue;
  };

  const initGraph = async () => {
    hookGraphThemeEvents();
    if (graphData) return;
    if (loadingOverlay) loadingOverlay.style.display = "flex";
    isGraphAborted = false;
    try {
      const data = await invoke("ai_dj_graph");
      if (isGraphAborted || !body.isConnected || !graphContainer || graphContainer.style.display === "none") {
        return;
      }
      if (!data || !data.nodes || data.nodes.length === 0) {
        if (loadingOverlay) loadingOverlay.textContent = "No analyzed tracks yet";
        return;
      }
      graphData = data;
      const N = data.nodes.length;
      if (loadingOverlay) loadingOverlay.style.display = "none";

      // Seeded random dispersion: no circular prior — the shape comes from the
      // edges, not from a starting spiral.
      let seed = (0x9e3779b9 ^ N) >>> 0;
      const rand = () => {
        seed = (seed * 1664525 + 1013904223) >>> 0;
        return seed / 4294967296;
      };
      const R = Math.min(900, Math.max(320, Math.sqrt(N) * 34.0));
      graphNodes = data.nodes.map((node) => {
        const angle = rand() * Math.PI * 2;
        const radius = Math.sqrt(rand()) * R;
        return {
          id: node.id,
          title: node.title,
          artist: node.artist,
          bpm: node.bpm,
          key: node.key,
          x: Math.cos(angle) * radius,
          y: Math.sin(angle) * radius,
          vx: 0,
          vy: 0,
        };
      });

      graphEdges = data.edges || [];

      // Сразу вписываем облако в кадр: симуляция идёт на глазах, без «пустого»
      // ожидания и без прыжка зума в конце.
      fitView();

      let tick = 0;
      const maxTicks = 320;
      // Fruchterman–Reingold: k — «идеальная» дистанция. Отталкивание ~k²/d
      // раскрывает граф, притяжение по рёбрам ~d²/k держит топологию; форму
      // задаёт баланс сил, а не стартовая раскладка. Кулинга нет — иначе
      // симуляция застывала бы в не-равновесии.
      const k = 62;
      const repulsionCutoff = k * 3;
      const forceScale = 0.012;
      isSimulating = true;

      const step = () => {
        if (isGraphAborted || !body.isConnected || !graphContainer || graphContainer.style.display === "none") {
          isSimulating = false;
          simAnimId = null;
          return;
        }

        // 1. Слабая гравитация к центру — только чтобы изолированные куски не
        //    улетали; стягиванием форму не задаём.
        const kCenter = 0.00025;
        for (let i = 0; i < N; i++) {
          const n = graphNodes[i];
          n.vx -= n.x * kCenter;
          n.vy -= n.y * kCenter;
        }

        // 2. Притяжение по рёбрам (d²/k): ближе по вкусу — короче ребро.
        for (let e = 0; e < graphEdges.length; e++) {
          const [u, v, score] = graphEdges[e];
          const n1 = graphNodes[u];
          const n2 = graphNodes[v];
          if (!n1 || !n2) continue;
          const dx = n2.x - n1.x;
          const dy = n2.y - n1.y;
          const dist = Math.sqrt(dx * dx + dy * dy) || 1;
          const rawForce = ((dist * dist) / k) * (score + 0.15) * forceScale;
          const force = Math.min(9.0, rawForce);
          const fx = (dx / dist) * force;
          const fy = (dy / dist) * force;
          n1.vx += fx;
          n1.vy += fy;
          n2.vx -= fx;
          n2.vy -= fy;
        }

        // 3. Пространственная сетка с целочисленными ключами (без строк и GC).
        const cellSize = repulsionCutoff;
        const grid = new Map();
        for (let i = 0; i < N; i++) {
          const n = graphNodes[i];
          const cx = Math.floor(n.x / cellSize);
          const cy = Math.floor(n.y / cellSize);
          const key = ((cx & 0xFFFF) << 16) | (cy & 0xFFFF);
          let cell = grid.get(key);
          if (!cell) {
            cell = [];
            grid.set(key, cell);
          }
          cell.push(i);
        }

        // 4. Отталкивание (k²/d): вблизи сильнее притяжения — именно оно
        //    раскрывает граф вместо стягивания в круг.
        for (let i = 0; i < N; i++) {
          const n1 = graphNodes[i];
          const cx = Math.floor(n1.x / cellSize);
          const cy = Math.floor(n1.y / cellSize);
          for (let ox = -1; ox <= 1; ox++) {
            for (let oy = -1; oy <= 1; oy++) {
              const key = (((cx + ox) & 0xFFFF) << 16) | ((cy + oy) & 0xFFFF);
              const cell = grid.get(key);
              if (!cell) continue;
              for (let ci = 0; ci < cell.length; ci++) {
                const j = cell[ci];
                if (j <= i) continue;
                const n2 = graphNodes[j];
                const dx = n1.x - n2.x;
                const dy = n1.y - n2.y;
                const distSq = dx * dx + dy * dy || 1;
                if (distSq < repulsionCutoff * repulsionCutoff) {
                  const dist = Math.sqrt(distSq);
                  const f = Math.min(9.0, ((k * k) / (dist + 4)) * forceScale);
                  const fx = (dx / dist) * f;
                  const fy = (dy / dist) * f;
                  n1.vx += fx;
                  n1.vy += fy;
                  n2.vx -= fx;
                  n2.vy -= fy;
                }
              }
            }
          }
        }

        // 5. Интеграция: демпфирование и ограничение скорости. Равновесие
        //    достигается балансом сил, а не затуханием к нулю.
        const maxSpeed = 9.0;
        const damping = 0.85;
        let totalSpeed = 0;
        for (let i = 0; i < N; i++) {
          const n = graphNodes[i];
          n.vx *= damping;
          n.vy *= damping;
          const spd = Math.sqrt(n.vx * n.vx + n.vy * n.vy);
          if (spd > maxSpeed) {
            n.vx = (n.vx / spd) * maxSpeed;
            n.vy = (n.vy / spd) * maxSpeed;
          }
          n.x += n.vx;
          n.y += n.vy;
          totalSpeed += spd;
        }

        drawGraph();

        tick++;
        const avgSpeed = totalSpeed / N;
        const isSettled = tick > 80 && avgSpeed < 0.12;

        if (tick < maxTicks && !isSettled) {
          simAnimId = requestAnimationFrame(step);
        } else {
          isSimulating = false;
          simAnimId = null;
          if (loadingOverlay) loadingOverlay.style.display = "none";

          // Auto-fit bounding box on freeze
          fitView();

          drawGraph(); // Clean final render at 0% idle CPU
        }
      };

      simAnimId = requestAnimationFrame(step);
    } catch (err) {
      console.error("Failed to load AI DJ graph:", err);
      if (loadingOverlay) loadingOverlay.textContent = "Failed to load graph";
    }
  };

  if (canvas) {
    canvas.addEventListener("pointerdown", (e) => {
      e.stopPropagation();
      isDragging = true;
      hasDragged = false;
      dragStartX = e.clientX;
      dragStartY = e.clientY;
      dragStartPanX = panX;
      dragStartPanY = panY;
      canvas.style.cursor = "grabbing";
      try { canvas.setPointerCapture(e.pointerId); } catch {}
    });

    canvas.addEventListener("pointermove", (e) => {
      if (isDragging) {
        const dx = e.clientX - dragStartX;
        const dy = e.clientY - dragStartY;
        if (Math.abs(dx) > 3 || Math.abs(dy) > 3) hasDragged = true;
        panX = dragStartPanX + dx;
        panY = dragStartPanY + dy;
        if (!isSimulating) drawGraph();
        if (tooltip) tooltip.style.display = "none";
        return;
      }

      const prevHovered = hoveredNode;
      hoveredNode = findNodeAt(e.clientX, e.clientY);
      if (hoveredNode !== prevHovered) {
        if (!isSimulating) drawGraph();
      }

      if (hoveredNode) {
        canvas.style.cursor = "pointer";
        if (tooltip && tooltipTitle && tooltipMeta) {
          tooltipTitle.textContent = hoveredNode.title || hoveredNode.id;
          const artist = hoveredNode.artist || "Unknown Artist";
          const bpm = hoveredNode.bpm ? ` · ${Math.round(hoveredNode.bpm)} BPM` : "";
          const key = hoveredNode.key ? ` · ${hoveredNode.key}` : "";
          tooltipMeta.textContent = `${artist}${bpm}${key}`;
          tooltip.style.display = "block";

          const rect = graphContainer.getBoundingClientRect();
          let tx = e.clientX - rect.left + 12;
          let ty = e.clientY - rect.top - 12;
          if (tx + 210 > rect.width) tx = rect.width - 215;
          if (ty < 8) ty = 8;
          tooltip.style.left = `${Math.max(4, tx)}px`;
          tooltip.style.top = `${ty}px`;
        }
      } else {
        canvas.style.cursor = "grab";
        if (tooltip) tooltip.style.display = "none";
      }
    });

    const onPointerUp = async (e) => {
      if (!isDragging) return;
      isDragging = false;
      canvas.style.cursor = "grab";
      try { canvas.releasePointerCapture(e.pointerId); } catch {}

      if (!hasDragged && hoveredNode) {
        const targetId = hoveredNode.id;
        // Очередь — обход по рёбрам графа: next/prev ходят по соседям, а не
        // крутят один трек (player_load_queue с одним id зацикливался).
        const trackIds = buildGraphPlaybackQueue(targetId);
        showDebugMessage(`AI DJ: ${hoveredNode.title || targetId} (${trackIds.length} в очереди по графу)`);
        try {
          const state = await invoke("player_load_queue", { trackIds, startIndex: 0 });
          const musicCard = card || document.querySelector(".music-widget");
          if (state && musicCard && typeof musicCard._applyMusicStatus === "function") {
            await musicCard._applyMusicStatus(state);
          } else if (musicCard && typeof musicCard._setPlaying === "function") {
            musicCard._setPlaying(true);
          }
          if (!isSimulating) drawGraph();
        } catch (err) {
          console.error("Failed to play track from graph:", err);
        }
      }
    };

    canvas.addEventListener("pointerup", onPointerUp);
    canvas.addEventListener("pointercancel", () => {
      isDragging = false;
      canvas.style.cursor = "grab";
    });

    canvas._handleWheelZoom = (delta, cursorX, cursorY) => {
      if (!delta) return;
      const rect = canvas.getBoundingClientRect();
      if (!rect.width || !rect.height) return;

      const zoomFactor = delta > 0 ? 1.15 : 0.87;
      const newZoom = Math.max(0.25, Math.min(5.0, zoom * zoomFactor));
      if (newZoom === zoom) return;

      const mx = cursorX - rect.left - rect.width / 2;
      const my = cursorY - rect.top - rect.height / 2;

      panX = mx - (mx - panX) * (newZoom / zoom);
      panY = my - (my - panY) * (newZoom / zoom);
      zoom = newZoom;

      if (isDragging) {
        dragStartPanX = panX;
        dragStartPanY = panY;
        dragStartX = cursorX;
        dragStartY = cursorY;
      }

      if (!isSimulating) drawGraph();
      if (tooltip) tooltip.style.display = "none";
    };

    canvas.addEventListener("wheel", (e) => {
      e.preventDefault();
      e.stopPropagation();
      canvas._handleWheelZoom(-e.deltaY, e.clientX, e.clientY);
    }, { passive: false });
  }

  // Handle ResizeObserver to redraw graph dynamically when popup expands/collapses
  let resizeObserver = null;
  if (typeof ResizeObserver !== "undefined" && graphContainer) {
    resizeObserver = new ResizeObserver(() => {
      if (graphContainer.style.display !== "none" && !isSimulating) {
        drawGraph();
      }
    });
    resizeObserver.observe(graphContainer);
  }

  graphBtn?.addEventListener("click", async (e) => {
    e.stopPropagation();
    if (!graphContainer) return;
    const isVisible = graphContainer.style.display !== "none";
    const popup = body.closest(".music-popup");
    if (isVisible) {
      graphContainer.style.display = "none";
      graphBtn.classList.remove("active");
      popup?.classList.remove("graph-view");
      if (indexBtn) indexBtn.style.display = "";
      if (progressWrap && lastStatus?.is_analyzing) progressWrap.style.display = "flex";
      if (modelWrap && modelState !== "hidden") modelWrap.style.display = "flex";
      if (simAnimId) {
        cancelAnimationFrame(simAnimId);
        simAnimId = null;
        isSimulating = false;
      }
    } else {
      graphContainer.style.display = "block";
      graphBtn.classList.add("active");
      popup?.classList.add("graph-view");
      if (indexBtn) indexBtn.style.display = "none";
      if (progressWrap) progressWrap.style.display = "none";
      if (modelWrap) modelWrap.style.display = "none";
      await initGraph();
      if (!isSimulating) drawGraph();
    }
  });

  // Store cleanup callback
  body._aiDjCleanup = () => {
    isGraphAborted = true;
    if (typeof unlistenProgress === "function") {
      unlistenProgress();
      unlistenProgress = null;
    }
    if (typeof unlistenModel === "function") {
      unlistenModel();
      unlistenModel = null;
    }
    if (simAnimId) {
      cancelAnimationFrame(simAnimId);
      simAnimId = null;
    }
    isSimulating = false;
    if (resizeObserver) {
      resizeObserver.disconnect();
      resizeObserver = null;
    }
    const popup = body.closest(".music-popup");
    popup?.classList.remove("graph-view");
  };

}

function renderMusicPlayer(card, widgetId) {
  card.classList.add("music-widget");
  card.classList.add("no-header");
  
  card.insertAdjacentHTML("beforeend", `
    <div class="music-player-container">
      <div class="music-ring">
        <svg viewBox="0 0 100 100">
          <circle class="ring-bg" cx="50" cy="50" r="48" />
          <circle class="ring-progress" cx="50" cy="50" r="48" />
        </svg>
      </div>
      <div class="music-audio-arcs">
        <svg viewBox="0 0 100 100">
          <path class="ac-arc bass" />
          <path class="ac-arc mid" />
          <path class="ac-arc treble" />
        </svg>
      </div>
      <div class="music-cover">
        <img alt="" />
      </div>
      <div class="ring-accent-dot"></div>
      <div class="music-title">---</div>
      <div class="music-player-error" style="display: none; position: absolute; bottom: 4px; left: 50%; transform: translateX(-50%); font-size: 8px; color: rgba(var(--status-error-rgb), 0.95); white-space: nowrap; pointer-events: none;"></div>
      <div class="ai-dj-indicator">
        <span class="ai-dj-icon">AI</span>
        <span class="ai-dj-count">0</span>
      </div>
      <div class="music-touch-zones">
        <div class="touch-zone zone-prev"></div>
        <div class="middle-column">
          <div class="touch-zone zone-top"></div>
          <div class="touch-zone zone-play"></div>
          <div class="touch-zone zone-bottom"></div>
        </div>
        <div class="touch-zone zone-next"></div>
      </div>
    </div>

    <div class="music-popup music-library-popup">
      <div class="music-popup-header">
        <div class="music-popup-info">
          <div class="music-popup-title">LIBRARY</div>
          <div class="music-popup-desc">Ваши плейлисты</div>
        </div>
        <button type="button" class="music-popup-close" aria-label="Закрыть">
          <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round">
            <path d="M18 6L6 18M6 6l12 12"/>
          </svg>
        </button>
      </div>
      <div class="music-popup-body"></div>
    </div>

    <div class="music-popup music-settings-popup">
      <div class="music-popup-header">
        <div class="music-popup-info">
          <div class="music-popup-title">SETTINGS</div>
          <div class="music-popup-desc">Параметры воспроизведения</div>
        </div>
        <button type="button" class="music-popup-close" aria-label="Закрыть">
          <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round">
            <path d="M18 6L6 18M6 6l12 12"/>
          </svg>
        </button>
      </div>
      <div class="music-popup-body"></div>
    </div>
  `);

  setupMusicAudioArcs(card);

  // Stop clicks inside popups from propagating to parent or touch zones
  card.querySelectorAll(".music-popup").forEach((popup) => {
    popup.addEventListener("click", (e) => e.stopPropagation());
    popup.addEventListener("pointerdown", (e) => e.stopPropagation());
    const header = popup.querySelector(".music-popup-header");
    if (header) {
      header.addEventListener("click", (e) => {
        const interactive = e.target.closest("button, input, select, a, [data-prevent-close]");
        if (interactive && !interactive.classList.contains("music-popup-close")) {
          return;
        }
        e.stopPropagation();
        closePopupElement(popup);
      });
    }
  });

  const container = card.querySelector(".music-player-container");
  if (container) {
    container.addEventListener("wheel", (e) => {
      if (editMode) return;
      e.preventDefault();
      e.stopPropagation();
      const dir = e.deltaY < 0 ? 1 : -1;
      nudgeMusicVolume(dir * 5, card);
    }, { passive: false });
  }

  card.querySelector(".zone-prev").addEventListener("click", (e) => {
    e.stopPropagation();
    if (editMode || card._isNavigating) return;
    card._isNavigating = true;
    flashZone(e.currentTarget);
    runMusicCommand("player_previous").then((state) => {
      if (state && typeof card._applyMusicStatus === "function") card._applyMusicStatus(state);
    }).finally(() => {
      setTimeout(() => { card._isNavigating = false; }, 250);
    });
  });

  card.querySelector(".zone-top").addEventListener("click", (e) => {
    e.stopPropagation();
    if (editMode) return;

    const lib = card.querySelector(".music-library-popup");
    if (lib && (surfaceManager.isOpen(lib) || lib.classList.contains("expanded"))) {
      closePopupElement(lib);
    } else {
      openMusicPopup(card, "library");
    }
  });

  card.querySelector(".zone-play").addEventListener("click", (e) => {
    e.stopPropagation();
    if (editMode || card._isTogglingPlay) return;
    card._isTogglingPlay = true;
    flashZone(e.currentTarget);
    const next = !card._isPlaying;
    card._isPlaying = next;
    if (typeof card._setPlaying === "function") card._setPlaying(next);
    runMusicCommand("player_toggle_pause").then((state) => {
      if (state && typeof card._applyMusicStatus === "function") card._applyMusicStatus(state);
      else if (state && typeof card._setPlaying === "function") card._setPlaying(!state.paused);
    }).finally(() => {
      setTimeout(() => { card._isTogglingPlay = false; }, 250);
    });
  });

  card.querySelector(".zone-next").addEventListener("click", (e) => {
    e.stopPropagation();
    if (editMode || card._isNavigating) return;
    card._isNavigating = true;
    flashZone(e.currentTarget);
    runMusicCommand("player_next").then((state) => {
      if (state && typeof card._applyMusicStatus === "function") card._applyMusicStatus(state);
    }).finally(() => {
      setTimeout(() => { card._isNavigating = false; }, 250);
    });
  });

  card.querySelector(".zone-bottom").addEventListener("click", (e) => {
    e.stopPropagation();
    if (editMode) return;
    const settings = card.querySelector(".music-settings-popup");
    if (settings && (surfaceManager.isOpen(settings) || settings.classList.contains("expanded"))) {
      closePopupElement(settings);
    } else {
      openMusicPopup(card, "settings");
    }
  });

  // Start metadata sync loop
  startMusicMetadataLoop(card);
}

function flashZone(el) {
  if (!el) return;
  el.style.background = "rgba(var(--bg-elev-rgb), 0.2)";
  setTimeout(() => { el.style.background = ""; }, 120);
}

/** Stops this card's own ticker/sync loop and its visibility listener. */
function stopMusicCardLoops(card) {
  if (card._musicTickerInterval) {
    clearInterval(card._musicTickerInterval);
    card._musicTickerInterval = null;
  }
  if (card._musicSyncInterval) {
    clearInterval(card._musicSyncInterval);
    card._musicSyncInterval = null;
  }
  if (card._visibilityHandler) {
    document.removeEventListener("visibilitychange", card._visibilityHandler);
    card._visibilityHandler = null;
  }
}

async function startMusicMetadataLoop(card) {
  const ring = card.querySelector(".ring-progress");
  const cover = card.querySelector(".music-cover img");
  const titleEl = card.querySelector(".music-title");
  const CIRCUMFERENCE = 2 * Math.PI * 48;

  // Loops live on the card, never on module-level variables: a second instance
  // or the diagnostic sandbox must not be able to take over the live player.
  stopMusicCardLoops(card);

  if (ring) {
    ring.style.strokeDasharray = CIRCUMFERENCE;
    ring.style.strokeDashoffset = CIRCUMFERENCE;
    ring.style.transition = "stroke-dashoffset 0.25s linear";
  }

  let elapsed = 0;
  let duration = 0;
  let playing = false;
  let anchorElapsed = 0;
  let anchorTime = performance.now();

  if (cover) {
    cover.onerror = () => {
      if (lastTrackId && isYoutubeId(lastTrackId)) {
        const ytFallback = `https://i.ytimg.com/vi/${lastTrackId}/hqdefault.jpg`;
        if (cover.getAttribute("src") !== ytFallback) {
          cover.src = ytFallback;
          cover.style.display = "block";
          return;
        }
      }
      cover.removeAttribute("src");
      cover.style.display = "none";
    };
    cover.onload = () => {
      cover.style.display = "block";
    };
  }

  const paintRing = () => {
    if (!ring) return;
    if (!Number.isFinite(duration) || duration <= 0) {
      ring.style.strokeDashoffset = CIRCUMFERENCE;
      return;
    }
    const ratio = Math.max(0, Math.min(1, elapsed / duration));
    ring.style.strokeDashoffset = (CIRCUMFERENCE * (1 - ratio)).toFixed(2);
  };

  const applyStatus = async (data) => {
    const trackId = data?.current_track_id || null;
    if (typeof data?.volume === "number") {
      currentMusicVolume = data.volume;
    }

    if (!trackId) {
      lastTrackId = null;
      elapsed = 0;
      duration = 0;
      anchorElapsed = 0;
      anchorTime = performance.now();
      playing = false;
      card._isPlaying = false;
      card._lastTrackTitle = "---";
      if (cover) cover.removeAttribute("src");
      if (titleEl && !card._volumeToastActive) titleEl.textContent = "---";
      paintRing();
      return;
    }

    const track = await getMusicTrackById(trackId);
    const apiElapsed = Number(data?.time_pos);
    const rawDuration = Number(data?.duration);
    const trackDuration = Number(track?.duration_s);
    const apiDuration = (Number.isFinite(rawDuration) && rawDuration > 0)
      ? rawDuration
      : ((Number.isFinite(trackDuration) && trackDuration > 0) ? trackDuration : 0);

    const trackChanged = trackId !== lastTrackId;
    const currentCoverSrc = cover?.getAttribute("src");
    const missingCover = !currentCoverSrc || currentCoverSrc === "";
    const missingTitle = titleEl && (titleEl.textContent === "---" || titleEl.textContent === "");

    if (trackChanged || ((missingCover || missingTitle) && track)) {
      lastTrackId = trackId;
      elapsed = Number.isFinite(apiElapsed) ? apiElapsed : 0;
      anchorElapsed = elapsed;
      anchorTime = performance.now();
      duration = apiDuration;
      const coverUrl = musicCoverUrl(track) || (isYoutubeId(trackId) ? `https://i.ytimg.com/vi/${trackId}/hqdefault.jpg` : "");
      if (cover) {
        if (coverUrl) {
          if (cover.getAttribute("src") !== coverUrl) {
            cover.src = coverUrl;
            cover.style.display = "block";
          }
        } else {
          cover.removeAttribute("src");
        }
      }
      const formattedTitle = formatMusicTitle(track || { title: trackId });
      card._lastTrackTitle = formattedTitle;
      if (titleEl && !card._volumeToastActive) {
        titleEl.textContent = formattedTitle;
      }
    } else if (Number.isFinite(apiElapsed)) {
      const error = apiElapsed - elapsed;
      if (Math.abs(error) >= 0.5) {
        elapsed = apiElapsed;
        anchorElapsed = apiElapsed;
        anchorTime = performance.now();
      } else if (error > 0) {
        elapsed += error * 0.35;
        anchorElapsed = elapsed;
        anchorTime = performance.now();
      }
    }

    playing = !!data?.running && !data?.paused && !!trackId;
    if (!playing && Number.isFinite(apiElapsed)) {
      elapsed = apiElapsed;
      anchorElapsed = apiElapsed;
      anchorTime = performance.now();
    }

    // Surface mpv/player errors instead of pretending to play.
    const playerError = data?.last_error || null;
    const errorEl = card.querySelector(".music-player-error");
    if (playerError) {
      playing = false;
      card._isPlaying = false;
      if (titleEl) titleEl.textContent = "mpv error";
      showDebugMessage(`Music: ${playerError}`);
      if (errorEl) {
        errorEl.textContent = playerError;
        errorEl.style.display = "block";
      }
    } else if (errorEl) {
      errorEl.style.display = "none";
      errorEl.textContent = "";
    }

    // Update AI DJ indicator
    const aiDjIndicator = card.querySelector(".ai-dj-indicator");
    const aiDjCount = card.querySelector(".ai-dj-count");
    if (aiDjIndicator && aiDjCount) {
      const aiDjEnabled = !!data?.ai_dj_enabled;
      aiDjIndicator.classList.toggle("active", aiDjEnabled);
      // Fetch AI DJ track count if enabled
      if (aiDjEnabled && !card._aiDjCountFetched) {
        card._aiDjCountFetched = true;
        invoke("ai_dj_status").then((status) => {
          if (status && typeof status.track_count === "number") {
            aiDjCount.textContent = status.track_count;
          }
        }).catch(() => {});
      } else if (!aiDjEnabled) {
        card._aiDjCountFetched = false;
        aiDjCount.textContent = "0";
      }
    }
    card._isPlaying = playing;
    if (apiDuration > 0) {
      duration = apiDuration;
    }
    paintRing();
  };

  card._setPlaying = (val) => {
    playing = val;
    card._isPlaying = val;
    anchorElapsed = elapsed;
    anchorTime = performance.now();
  };
  // Exposed so playlist load / next / prev can resync ticker state immediately.
  card._applyMusicStatus = applyStatus;
  card._isPlaying = playing;
  runMusicCommand("player_status").then(async (state) => {
    if (state) await applyStatus(state);
  });

  // A 4 Hz progress ticker using dead-reckoning against high-resolution performance.now()
  card._musicTickerInterval = setInterval(() => {
    // Detached card (widget removed, sandbox torn down): stop its loop for good.
    if (!card.isConnected) { stopMusicCardLoops(card); return; }
    if (document.hidden) return;
    if (!playing || !Number.isFinite(duration) || duration <= 0) return;
    const now = performance.now();
    const dt = (now - anchorTime) / 1000;
    elapsed = Math.min(duration, anchorElapsed + dt);
    paintRing();
  }, 250);

  // Backend sync (2s)
  card._musicSyncInterval = setInterval(async () => {
    if (!card.isConnected) { stopMusicCardLoops(card); return; }
    if (document.hidden) return;
    try {
      const data = await invoke("player_status");
      await applyStatus(data);
    } catch (e) {
      playing = false;
      card._isPlaying = false;
    }
  }, 2000);

  const onVisibilityChange = () => {
    if (!document.hidden && card) {
      runMusicCommand("player_status").then((state) => {
        if (state && typeof card._applyMusicStatus === "function") {
          card._applyMusicStatus(state);
        }
      }).catch(() => {});
    }
  };
  document.addEventListener("visibilitychange", onVisibilityChange);
  card._visibilityHandler = onVisibilityChange;
}

// в”Ђв”Ђв”Ђ Special Widget: Glyph (Nothing Phone-style light strip) в”Ђв”Ђв”Ђ
//
// Используем АБСОЛЮТНЫЕ пиксельные координаты в SVG (без viewBox), а не растягиваемый
// viewBox — тогда stroke-width в px не масштабируется при изменении размера виджета.
// Толщина линии всегда константна (GLYPH_STROKE), а длина/форма меняется при resize.

async function renderMusicDashboard(body, widgetId, options = {}) {
  if (!body) return;
  if (typeof body?._dashboardCleanup === "function") {
    body._dashboardCleanup();
    body._dashboardCleanup = null;
  }

  const renderGeneration = (body._dashboardRenderGeneration || 0) + 1;
  body._dashboardRenderGeneration = renderGeneration;
  const isCurrentRender = () => body.isConnected && body._dashboardRenderGeneration === renderGeneration;
  if (!body.querySelector(".music-dashboard-view")) {
    body.innerHTML = '<div class="music-dashboard-loading" role="status">Загрузка плейлистов…</div>';
  }

  const musicId = (widgetId || "").replace(/-playlists$/, "");

  let playlists = [];
  try {
    const res = await invoke("list_playlists");
    if (!isCurrentRender()) return;
    playlists = Array.isArray(res) ? res : [];
  } catch (e) {
    if (!isCurrentRender()) return;
    console.error("Dashboard fetch error:", e);
    body.innerHTML = `<div class="widget-empty" style="padding: 16px; text-align: center; color: rgba(var(--fg-rgb), 0.7); font-size: 10px;">Ошибка загрузки плейлистов: ${e.message || e}</div>`;
    return;
  }

  // Названия плейлистов приходят из локальной qbc-библиотеки.
  if (typeof options.beforeCommit === "function") {
    await options.beforeCommit();
    if (!isCurrentRender()) return;
  }

  body.innerHTML = `
    <div class="music-dashboard-view">
      <div class="ytm-grid-scroll" style="width: 100%; box-sizing: border-box;">
        <div class="shortcuts-grid" style="padding: 8px; display: grid; grid-template-columns: repeat(3, 1fr); gap: 10px; width: 100%; box-sizing: border-box;">
          <button class="shortcut-item music-playlist-add-card" type="button">
            <div class="shortcut-icon">
              <svg width="20" height="20" viewBox="0 0 24 24" fill="none" style="stroke: rgba(var(--fg-rgb), 0.85);" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round">
                <path d="M12 5v14M5 12h14"/>
              </svg>
            </div>
            <span class="shortcut-name">Add</span>
          </button>
          ${playlists.map((p, i) => `
            <button class="shortcut-item music-playlist-item" data-idx="${i}" data-id="${p.id}" style="width: auto; height: auto; background: none; border: none; padding: 0; cursor: pointer;">
              <div class="shortcut-icon" data-icon="${i}" style="width: 48px; height: 48px; margin: 0 auto; background: rgba(var(--bg-elev-rgb), 0.05); border: 1px solid rgba(var(--bg-elev-rgb), 0.1); border-radius: 10px; display: grid; place-items: center; overflow: hidden; position: relative;">
                <div class="shortcut-fallback-icon" style="display: flex; width: 100%; height: 100%; align-items: center; justify-content: center; background: rgba(var(--bg-elev-rgb), 0.06); border-radius: 9px;">
                  <svg width="22" height="22" viewBox="0 0 24 24" fill="none" style="stroke: rgba(var(--fg-rgb), 0.7);" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M9 18V5l12-2v13"/><circle cx="6" cy="18" r="3"/><circle cx="18" cy="16" r="3"/></svg>
                </div>
                <img class="shortcut-cover-img" style="display: none; width: 100%; height: 100%; object-fit: cover; border-radius: 9px;" />
              </div>
              <span class="shortcut-name" data-name="${i}" style="opacity: 1; color: rgb(var(--fg-rgb)); font-size: 9px; margin-top: 5px; display: block; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; text-align: center;">${escapeHtml(musicPlaylistName(p, i))}</span>
            </button>
          `).join("")}
        </div>
      </div>
    </div>

    <div class="music-add-view" style="display: none;">
      <div class="music-add-top">
        <button class="music-add-back-btn" type="button">
          <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round">
            <path d="M19 12H5M12 19l-7-7 7-7"/>
          </svg>
          <span>Back</span>
        </button>
        <span class="music-add-title">Add Playlist</span>
      </div>
      <form class="music-add-form">
        <div class="music-add-input-wrap">
          <input class="music-sync-url" type="text" placeholder="YouTube Music URL" autocomplete="off" spellcheck="false" data-prevent-escape-surface-close="true" />
          <button class="music-sync-submit" type="submit">Sync</button>
        </div>
        <div class="music-sync-hint">Paste YouTube Music playlist URL to sync offline tracks</div>
      </form>
      <div class="music-sync-tasks" style="display: none;"></div>
    </div>
  `;

  // Views & Task state
  const dashboardView = body.querySelector(".music-dashboard-view");
  const addView = body.querySelector(".music-add-view");
  const addCard = body.querySelector(".music-playlist-add-card");
  const backBtn = body.querySelector(".music-add-back-btn");
  const urlInput = body.querySelector(".music-sync-url");
  const addForm = body.querySelector(".music-add-form");
  const syncBtn = body.querySelector(".music-sync-submit");
  const tasksEl = body.querySelector(".music-sync-tasks");

  let taskInterval = null;
  let didQueueSync = false;

  const renderTasks = async () => {
    if (!tasksEl || !body.isConnected) return;
    try {
      const tasks = await invoke("sync_tasks");
      if (!Array.isArray(tasks) || tasks.length === 0) {
        tasksEl.style.display = "none";
        tasksEl.innerHTML = "";
        return;
      }
      tasksEl.style.display = "block";
      tasksEl.innerHTML = "";
      tasks.slice(0, 4).forEach((task) => {
        const row = document.createElement("div");
        row.style.whiteSpace = "nowrap";
        row.style.overflow = "hidden";
        row.style.textOverflow = "ellipsis";
        row.textContent = syncTaskLabel(task);
        if (task.message || task.current) row.setAttribute("aria-label", task.message || task.current);
        tasksEl.appendChild(row);
      });
    } catch {
      // silently ignore sync_tasks error
    }
  };

  const startTaskPolling = () => {
    if (taskInterval) clearInterval(taskInterval);
    renderTasks();
    taskInterval = setInterval(renderTasks, 2000);
  };

  const stopTaskPolling = () => {
    if (taskInterval) {
      clearInterval(taskInterval);
      taskInterval = null;
    }
  };

  const openAddView = () => {
    dashboardView.style.display = "none";
    addView.style.display = "flex";
    if (urlInput) {
      urlInput.value = "";
      urlInput.focus();
    }
    startTaskPolling();
  };

  const closeAddView = async () => {
    stopTaskPolling();
    addView.style.display = "none";
    dashboardView.style.display = "flex";
    if (didQueueSync) {
      didQueueSync = false;
      // Re-render dashboard to pick up newly synced playlists
      renderMusicDashboard(body, widgetId);
    }
  };

  // Add card click
  addCard?.addEventListener("click", (e) => {
    e.stopPropagation();
    openAddView();
  });

  // Back button click
  backBtn?.addEventListener("click", (e) => {
    e.stopPropagation();
    closeAddView();
  });

  // Escape key handling inside input: cancel and go back to playlist grid
  urlInput?.addEventListener("keydown", (e) => {
    if (e.key === "Escape" || e.key === "Esc") {
      e.preventDefault();
      e.stopPropagation();
      urlInput.blur();
      closeAddView();
    }
  });

  // Form submit: queue sync
  addForm?.addEventListener("submit", async (e) => {
    e.preventDefault();
    e.stopPropagation();
    const url = urlInput?.value?.trim();
    if (!url) return;
    if (syncBtn) {
      syncBtn.disabled = true;
      syncBtn.textContent = "...";
    }
    try {
      await invoke("sync_url", { url, name: null });
      musicTrackCache = null;
      musicTrackCachePromise = null;
      musicTrackCacheMissRetried = false;
      didQueueSync = true;
      if (urlInput) urlInput.value = "";
      showDebugMessage("Sync queued");
      await renderTasks();
    } catch (err) {
      showDebugMessage(`Sync: ${err.message || err}`);
    } finally {
      if (syncBtn) {
        syncBtn.disabled = false;
        syncBtn.textContent = "Sync";
      }
    }
  });

  // Безопасно заполняем обложки без мутации структуры DOM
  body.querySelectorAll(".music-playlist-item").forEach(async (item) => {
    const i = Number(item.dataset.idx);
    const p = playlists[i];
    if (!p) return;
    const displayName = musicPlaylistName(p, i);
    item.setAttribute("aria-label", `${displayName} · ${p.track_count || 0} tracks\n${p.source_url || ""}`);
    const imgEl = item.querySelector(".shortcut-cover-img");
    const fallbackEl = item.querySelector(".shortcut-fallback-icon");
    if (!imgEl) return;

    let localThumb = p.thumbnail_path;
    if (!localThumb || localThumb.startsWith("http")) {
      try {
        const tracks = await invoke("list_playlist_tracks", { playlistId: p.id });
        const first = tracks.find(t => t.thumbnail_path && !t.thumbnail_path.startsWith("http"));
        if (first?.thumbnail_path) localThumb = first.thumbnail_path;
      } catch {}
    }

    const coverUrl = localThumb ? musicMediaUrl(localThumb) : "";
    if (coverUrl) {
      imgEl.src = coverUrl;
      imgEl.onload = () => {
        imgEl.style.display = "block";
        if (fallbackEl) fallbackEl.style.display = "none";
      };
      imgEl.onerror = () => {
        imgEl.style.display = "none";
        if (fallbackEl) fallbackEl.style.display = "flex";
      };
    }
  });

  // ── ПКМ-меню плейлиста ────────────────────────────────────────────────────
  // Свои классы (.music-context-menu/.music-cm-*), общий стиль с меню проводника,
  // но без зависимости от его CSS/модуля. Показывает число треков, локальное
  // переименование и удаление (файлы остаются на диске).
  // Меню живёт на уровне документа (клиентские координаты) и открывается через
  // SurfaceManager — как меню проводника: анимация, слои, жизненный цикл.
  document.querySelectorAll(".music-context-menu").forEach((el) => el.remove());
  const playlistMenu = document.createElement("div");
  playlistMenu.className = "music-context-menu";
  playlistMenu.dataset.surfaceTint = "mp3-player"; // lives in <body>, outside the card
  document.body.appendChild(playlistMenu);

  const closePlaylistMenu = () => {
    if (surfaceManager.isOpen(playlistMenu)) {
      surfaceManager.closeSurface(playlistMenu);
    } else {
      playlistMenu.remove();
    }
  };

  const playlistDisplayName = (playlist) =>
    musicPlaylistName(playlist, playlists.indexOf(playlist));

  const startPlaylistRename = (playlist) => {
    const item = body.querySelector(`.music-playlist-item[data-id="${playlist.id}"]`);
    const label = item?.querySelector(".shortcut-name");
    if (!label) return;
    const input = document.createElement("input");
    input.type = "text";
    input.value = playlistDisplayName(playlist);
    input.style.cssText = "width:100%; font-size:9px; text-align:center; background:rgba(var(--bg-elev-rgb), 0.08); border:1px solid rgba(var(--bg-elev-rgb), 0.25); border-radius:6px; color:rgb(var(--fg-rgb)); outline:none;";
    label.style.display = "none";
    label.parentNode.insertBefore(input, label.nextSibling);
    input.focus();
    input.select();

    let finished = false;
    const finish = async (commit) => {
      if (finished) return;
      finished = true;
      const value = input.value.trim();
      input.remove();
      label.style.display = "";
      if (!commit || !value || value === label.textContent) return;
      try {
        await invoke("rename_playlist", { playlistId: playlist.id, name: value });
        showDebugMessage(`Плейлист переименован: ${value}`);
        await renderMusicDashboard(body, widgetId);
      } catch (err) {
        showDebugMessage(`Переименовать не удалось: ${err.message || err}`);
      }
    };
    input.addEventListener("keydown", (e) => {
      e.stopPropagation();
      if (e.key === "Enter") finish(true);
      else if (e.key === "Escape") finish(false);
    });
    input.addEventListener("blur", () => finish(true));
  };

  const confirmPlaylistDelete = (playlist) => {
    playlistMenu.innerHTML = `
      <div class="music-cm-item info">
        <span class="music-cm-label">Удалить «${escapeHtml(playlistDisplayName(playlist))}»?</span>
        <span class="music-cm-label dim">Треки и файлы останутся в библиотеке</span>
      </div>
      <div class="music-cm-sep"></div>
      <div class="music-cm-item" data-action="delete-confirm"><span class="music-cm-label danger">Удалить</span></div>
      <div class="music-cm-item" data-action="cancel"><span class="music-cm-label">Отмена</span></div>
    `;
    playlistMenu.querySelector('[data-action="delete-confirm"]')?.addEventListener("click", async (e) => {
      e.stopPropagation();
      try {
        await invoke("delete_playlist", { playlistId: playlist.id });
        showDebugMessage(`Плейлист «${playlistDisplayName(playlist)}» удалён`);
        closePlaylistMenu();
        await renderMusicDashboard(body, widgetId);
      } catch (err) {
        showDebugMessage(`Удалить не удалось: ${err.message || err}`);
      }
    });
    playlistMenu.querySelector('[data-action="cancel"]')?.addEventListener("click", (e) => {
      e.stopPropagation();
      closePlaylistMenu();
    });
  };

  let detachMenuDismiss = null;

  const openPlaylistMenu = (x, y, playlist) => {
    playlistMenu.innerHTML = `
      <div class="music-cm-item info">
        <span class="music-cm-label">${escapeHtml(playlistDisplayName(playlist))}</span>
        <span class="music-cm-label dim">${playlist.track_count || 0} треков</span>
      </div>
      <div class="music-cm-sep"></div>
      <div class="music-cm-item" data-action="rename"><span class="music-cm-label">Переименовать</span></div>
      <div class="music-cm-item" data-action="delete"><span class="music-cm-label danger">Удалить плейлист</span></div>
    `;

    if (!playlistMenu.isConnected) document.body.appendChild(playlistMenu);

    // Измеряем и вписываем в экран до открытия серфейса.
    playlistMenu.style.left = "0px";
    playlistMenu.style.top = "0px";
    playlistMenu.style.display = "block";
    const menuW = playlistMenu.offsetWidth || 210;
    const menuH = playlistMenu.offsetHeight || 120;
    playlistMenu.style.display = "";
    playlistMenu.style.left = `${Math.max(6, Math.min(x, Math.max(6, window.innerWidth - menuW - 6)))}px`;
    playlistMenu.style.top = `${Math.max(6, Math.min(y, Math.max(6, window.innerHeight - menuH - 6)))}px`;

    if (detachMenuDismiss) detachMenuDismiss();
    const onDocPointerDown = (e) => {
      if (!e.target.closest(".music-context-menu")) closePlaylistMenu();
    };
    const onDocKeyDown = (e) => {
      if (e.key === "Escape") closePlaylistMenu();
    };
    document.addEventListener("pointerdown", onDocPointerDown, true);
    document.addEventListener("keydown", onDocKeyDown, true);
    detachMenuDismiss = () => {
      document.removeEventListener("pointerdown", onDocPointerDown, true);
      document.removeEventListener("keydown", onDocKeyDown, true);
      detachMenuDismiss = null;
    };

    surfaceManager.openSurface(playlistMenu, {
      origin: { left: x, top: y, width: 4, height: 4 },
      originRadius: "8px",
      elevation: "var(--z-surface-menu, 4000)",
      display: "block",
      onClosed: () => {
        if (detachMenuDismiss) detachMenuDismiss();
      },
    });

    playlistMenu.querySelector('[data-action="rename"]')?.addEventListener("click", (e) => {
      e.stopPropagation();
      closePlaylistMenu();
      startPlaylistRename(playlist);
    });
    playlistMenu.querySelector('[data-action="delete"]')?.addEventListener("click", (e) => {
      e.stopPropagation();
      confirmPlaylistDelete(playlist);
    });
  };

  // Единое делегирование кликов на уровне сетки (Event Delegation)
  const grid = body.querySelector(".shortcuts-grid");
  if (grid) {
    grid.addEventListener("contextmenu", (e) => {
      const item = e.target.closest(".music-playlist-item");
      if (!item || editMode) return;
      const playlist = playlists[Number(item.dataset.idx)];
      if (!playlist) return;
      e.preventDefault();
      e.stopPropagation();
      openPlaylistMenu(e.clientX, e.clientY, playlist);
    });

    grid.addEventListener("click", async (e) => {
      const item = e.target.closest(".music-playlist-item");
      if (!item || editMode || item.classList.contains("is-loading")) return;
      e.stopPropagation();
      const pid = item.dataset.id;
      if (!pid) return;

      const wantShuffle = !!config.positions?.[musicId]?.autoShuffle;
      const wantAiDj = !!config.positions?.[musicId]?.aiDj;
      item.classList.add("is-loading");
      grid.querySelectorAll(".music-playlist-item").forEach(o => {
        if (o !== item) o.style.opacity = "0.4";
      });
      const cleanup = () => {
        item.classList.remove("is-loading");
        grid.querySelectorAll(".music-playlist-item").forEach(o => { o.style.opacity = ""; });
      };
      try {
        const tracks = await invoke("list_playlist_tracks", { playlistId: pid });
        let trackIds = tracks.map((track) => track.id).filter(Boolean);
        if (wantShuffle) {
          trackIds = trackIds
            .map((id) => ({ id, sort: Math.random() }))
            .sort((a, b) => a.sort - b.sort)
            .map((item) => item.id);
        } else if (wantAiDj) {
          // Pick the session seed before building transitions so fallback keeps it too.
          if (trackIds.length > 1) {
            const startIndex = Math.floor(Math.random() * trackIds.length);
            const [startId] = trackIds.splice(startIndex, 1);
            trackIds.unshift(startId);
          }
          // AI DJ mode: build queue using similarity graph from the chosen seed.
          try {
            const aiQueue = await invoke("ai_dj_build_queue", { trackIds });
            if (aiQueue && aiQueue.length > 0) {
              trackIds = aiQueue;
            }
          } catch (aiErr) {
            console.warn("AI DJ queue build failed, keeping random start:", aiErr);
          }
        }
        if (!trackIds.length) {
          showDebugMessage("В плейлисте пока нет скачанных треков");
          cleanup();
          return;
        }
        const state = await invoke("player_load_queue", { trackIds, startIndex: 0 });
        const musicCard = document.querySelector(`.widget-card[data-id="${musicId}"]`) || body.closest(".music-widget");
        musicTrackCache = null;
        musicTrackCachePromise = null;
        musicTrackCacheMissRetried = false;
        if (state && musicCard && typeof musicCard._applyMusicStatus === "function") {
          await musicCard._applyMusicStatus(state);
        } else if (musicCard && typeof musicCard._setPlaying === "function") {
          musicCard._setPlaying(true);
        }
        cleanup();
        const popup = body.closest(".music-popup");
        if (popup) closePopupElement(popup);
      } catch (err) {
        cleanup();
        showDebugMessage(`Плейлист: ${err.message || err}`);
      }
    });
  }

  body._dashboardCleanup = () => {
    stopTaskPolling();
  };
}

function musicPlaylistName(playlist, index) {
  const rawName = String(playlist?.name || "").trim();
  if (rawName && !/^https?:\/\//i.test(rawName)) return rawName;
  const source = String(playlist?.source_url || rawName || "");
  const count = Number(playlist?.track_count || 0);
  const suffix = count > 0 ? `${count} tracks` : `#${index + 1}`;
  try {
    const url = new URL(source);
    const listId = url.searchParams.get("list");
    const videoId = url.searchParams.get("v");
    if (listId) return `Playlist ${suffix}`;
    if (videoId) return `Track ${index + 1}`;
  } catch {}
  return `Playlist ${index + 1}`;
}

function resetMusicIntervals() {
  document.querySelectorAll(".music-widget").forEach(stopMusicCardLoops);
}

export {
  renderMusicPlayer,
  renderMusicDashboard,
  renderMusicMenu,
  openMusicPopup,
  closePopupElement,
  closeAllMusicPopups,
  closeMusicSatellite,
  openMusicSatellite,
  startMusicMetadataLoop,
  resetMusicIntervals,
  musicTrackCache,
  musicTrackCachePromise,
  musicTrackCacheMissRetried,
  nudgeMusicVolume
};
