import { convertFileSrc, invoke } from '@unidtk/bridge';
import { getFileIconName, getFolderIconName, getFileTypeName } from '../../file-icons.js';
import { calculateDuration, getDuration, EASINGS, motionTimeline } from '../../motion.js';
import { explorerAsset } from './asset.js';
import { ContextMenu } from './context-menu.js';
import { PreviewPanel } from './preview.js';
import { TabManager } from './tabs.js';

// Items beyond this index render without the staggered appear animation.
const APPEAR_LIMIT = 24;
// Large folders render the first screenful at once, the rest per frame
// (~0.6 ms per item: 48 items keep each frame's slice near 30 ms).
const RENDER_FIRST = 48;
const RENDER_CHUNK = 48;

class NothingExplorer {
    constructor() {
        this.tabManager = new TabManager(this);
        this.previewPanel = new PreviewPanel(this);
        this.contextMenu = new ContextMenu(this);
        this.currentView = 'grid';
        this.selectedFiles = new Set();
        this.clipboard = { files: [], cut: false };
        this.sortColumn = 'name';
        this.sortAscending = true;
        this._favorites = [];
        this._places = [];
        // Async boot is exposed so the shell can wait until the first tab is ready.
        this.ready = this.init();
    }

    async init() {
        try {
            this.bindEvents();
        } catch (e) {
            console.error('bindEvents failed:', e);
        }

        // Restore saved sort order from unified redb
        try {
            const savedSort = await invoke('explorer_sort_get');
            if (savedSort) {
                const col = Array.isArray(savedSort) ? savedSort[0] : savedSort.column;
                const asc = Array.isArray(savedSort) ? savedSort[1] : savedSort.ascending;
                if (col) this.sortColumn = String(col);
                if (typeof asc === 'boolean') this.sortAscending = asc;
                this.updateSortHeader();
            }
        } catch (e) {
            console.warn('Failed to restore explorer sort:', e);
        }

        // Restore saved tabs FIRST (reads redb, sets _restored) so any
        // startup addTab from openExplorer cannot overwrite them. If nothing
        // was saved, fall back to a single default tab.
        const restored = await this.tabManager.restoreTabs();
        if (!restored) {
            await this.tabManager.addTab('::MY_PC::');
        } else {
            const activeTab = this.tabManager.getActiveTab();
            if (activeTab) {
                if (activeTab.path === '::MY_PC::') {
                    await this.tabManager.showThisPC();
                } else {
                    await this.navigateToSilent(activeTab.path);
                }
            }
        }

        await Promise.all([
            this.loadPlaces().catch((e) => console.error('loadPlaces', e)),
            this.loadFavorites().catch((e) => console.error('loadFavorites', e)),
            this.loadDrives().catch((e) => console.error('loadDrives', e)),
        ]);

        // Refresh This PC once richer sidebar/drive data is available.
        if (this.tabManager.getActiveTab()?.path === '::MY_PC::') {
            this.renderMyPc();
        }
        try {
            await invoke('frontend_log', { message: 'explorer init ok' });
        } catch {}
    }

    async invokeWithTimeout(command, args = {}, timeoutMs = 1500) {
        let timer;
        try {
            return await Promise.race([
                invoke(command, args),
                new Promise((_, reject) => {
                    timer = setTimeout(
                        () => reject(new Error(`${command} timed out after ${timeoutMs} ms`)),
                        timeoutMs,
                    );
                }),
            ]);
        } finally {
            clearTimeout(timer);
        }
    }

    async scanDirectory(path) {
        const items = await invoke('scan_directory', { path });
        return Array.isArray(items) ? items : [];
    }

    async getDrives() {
        if (Array.isArray(this._cachedDrives) && this._cachedDrives.length) {
            return this._cachedDrives;
        }
        try {
            const drives = await this.invokeWithTimeout('get_drives');
            if (Array.isArray(drives) && drives.length) {
                this._cachedDrives = drives;
                return drives;
            }
        } catch (e) {
            console.error('getDrives error:', e);
        }
        try {
            const raw = await this.invokeWithTimeout('get_raw_drives', {}, 1000);
            const drives = (raw || []).map((p) => ({
                display: p,
                path: p,
                is_removable: false,
                total_space: null,
                free_space: null,
            }));
            this._cachedDrives = drives;
            return drives;
        } catch (e) {
            console.error('get_raw_drives error:', e);
            return [];
        }
    }

    async searchFiles(query) {
        try {
            return await invoke('search_files', { query });
        } catch (e) {
            console.error('searchFiles error:', e);
            return [];
        }
    }

    async getFilePreview(path) {
        try {
            return await invoke('get_file_preview', { path });
        } catch (e) {
            console.error('getFilePreview error:', e);
            return null;
        }
    }

    async hashFile(path) {
        try {
            return await invoke('hash_file', { path });
        } catch (e) {
            console.error('hashFile error:', e);
            return null;
        }
    }

    sortItems(items, column = this.sortColumn, ascending = this.sortAscending) {
        if (!Array.isArray(items)) return [];
        const list = [...items];
        if (!this._nameCollator) {
            this._nameCollator = new Intl.Collator(undefined, { numeric: true, sensitivity: 'base' });
        }
        const collator = this._nameCollator;
        list.sort((a, b) => {
            if (!!a.is_dir !== !!b.is_dir) return a.is_dir ? -1 : 1;
            let cmp = 0;
            if (column === 'size') {
                cmp = (a.file_size || 0) - (b.file_size || 0);
            } else if (column === 'type') {
                const aExt = (a.file_type || a.name?.split('.').pop() || '').toLowerCase();
                const bExt = (b.file_type || b.name?.split('.').pop() || '').toLowerCase();
                cmp = collator.compare(aExt, bExt);
            } else if (column === 'modified') {
                const aTime = a.modified_time_raw || 0;
                const bTime = b.modified_time_raw || 0;
                cmp = aTime - bTime;
            } else {
                cmp = collator.compare(String(a.name || ''), String(b.name || ''));
            }
            if (cmp === 0) {
                cmp = collator.compare(String(a.name || ''), String(b.name || ''));
            }
            return ascending ? cmp : -cmp;
        });
        return list;
    }

    updateSortHeader() {
        const header = document.getElementById('fileHeader');
        if (!header) return;
        header.querySelectorAll('[data-sort]').forEach((el) => {
            const col = el.dataset.sort;
            const indicator = el.querySelector('.sort-indicator');
            if (col === this.sortColumn) {
                el.classList.add('active-sort');
                if (indicator) indicator.textContent = this.sortAscending ? ' ▲' : ' ▼';
            } else {
                el.classList.remove('active-sort');
                if (indicator) indicator.textContent = '';
            }
        });
    }

    async sortFiles(files, column, ascending) {
        try {
            return await invoke('sort_files', { files, column, ascending });
        } catch (e) {
            console.error('sortFiles error:', e);
            return this.sortItems(files, column, ascending);
        }
    }

    async getFolderSize(path) {
        try {
            return await invoke('get_folder_size', { path });
        } catch (e) {
            console.error('getFolderSize error:', e);
            return 0;
        }
    }

    async getFileInfo(path) {
        try {
            return await invoke('get_file_info', { path });
        } catch (e) {
            console.error('getFileInfo error:', e);
            return null;
        }
    }

    async isZipFile(path) {
        try {
            return await invoke('is_zip_file', { path });
        } catch (e) {
            console.error('isZipFile error:', e);
            return false;
        }
    }

    async getArchiveInfo(path) {
        try {
            return await invoke('get_archive_info', { path });
        } catch (e) {
            console.error('getArchiveInfo error:', e);
            return null;
        }
    }

    async navigateTo(path, originEl = null, transitionType = null) {
        if (typeof path === 'string') {
            const trimmed = path.trim();
            if (/^[A-Za-z]$/.test(trimmed)) {
                path = trimmed.toUpperCase() + ':\\';
            } else if (/^[A-Za-z]:$/.test(trimmed)) {
                path = trimmed.toUpperCase() + '\\';
            } else if (/^[A-Za-z]:[\\/]+$/.test(trimmed)) {
                path = trimmed[0].toUpperCase() + ':\\';
            }
        }
        const tab = this.tabManager.getActiveTab();
        if (!tab) return;

        this.highlightSidebar(path);

        let originRect = null;
        let originRadius = null;
        if (originEl instanceof HTMLElement) {
            originRect = originEl.getBoundingClientRect();
            originRadius = getComputedStyle(originEl).borderRadius;
        } else if (originEl && typeof originEl === 'object' && 'top' in originEl) {
            originRect = originEl;
        }

        const navToken = (this._navToken || 0) + 1;
        this._navToken = navToken;

        const fileArea = document.getElementById('fileArea');
        const pathChanged = fileArea && tab.path && tab.path !== path;

        if (path === '::MY_PC::') {
            tab.path = path;
            tab.title = 'This PC';
            tab.history.push(path);
            tab.historyIndex = tab.history.length - 1;
            tab.items = [];
            this.updatePathBar(path);
            this.updateNavButtons();
            this.updateStatusBar([]);
            this.previewPanel.clear();
            this.tabManager.renderTabs();
            this.tabManager._saveTabs();

            if (fileArea && originRect && pathChanged) {
                this.morphToThisPc(originRect, () => {
                    this.renderMyPc();
                }, originRadius);
            } else if (fileArea && pathChanged) {
                this.playTabTransition(transitionType || 'up', () => {
                    this.renderMyPc();
                });
            } else {
                this.renderMyPc();
            }
            return;
        }

        this.showLoading(true);
        this.selectedFiles.clear();

        try {
            const items = await this.scanDirectory(path);
            if (this._navToken !== navToken) return;
            const filtered = items.filter(i => String(i?.name || '').toLowerCase() !== 'desktop.ini');
            tab.path = path;
            tab.title = this.tabManager.getFolderName(path);
            tab.history.push(path);
            tab.historyIndex = tab.history.length - 1;

            const sorted = this.sortItems(filtered, this.sortColumn, this.sortAscending);
            tab.items = sorted;
            tab._sortCol = this.sortColumn;
            tab._sortAsc = this.sortAscending;
            this.updateSortHeader();
            this.updatePathBar(path);
            this.updateNavButtons();
            this.updateStatusBar(sorted);
            this.previewPanel.clear();
            this.tabManager.renderTabs();
            this.tabManager._saveTabs();

            if (fileArea && originRect && pathChanged) {
                // True surface morph: blooms out of clicked folder element over the old files
                this.morphToDirectory(sorted, originRect, () => {
                    if (this._navToken === navToken) {
                        this.renderFiles(sorted);
                    }
                }, originRadius);
            } else if (fileArea && pathChanged) {
                this.playTabTransition(transitionType || 'fade', () => {
                    if (this._navToken === navToken) {
                        this.renderFiles(sorted);
                    }
                });
            } else {
                this.renderFiles(sorted);
            }
        } catch (e) {
            console.error('Navigation error:', e);
            this.showEmpty(true, String(e?.message || e || 'Cannot access this location'));
        } finally {
            this.showLoading(false);
        }
    }

    morphToDirectory(items, originRect, onFinish, originRadius = null) {
        const fileArea = document.getElementById('fileArea');
        if (!fileArea) {
            if (typeof onFinish === 'function') onFinish();
            return;
        }

        if (this._activeSurfaceEl) {
            try { this._activeSurfaceAnim?.cancel(); } catch {}
            if (this._activeSurfaceEl.parentNode) this._activeSurfaceEl.remove();
            this._activeSurfaceEl = null;
        }

        const areaRect = fileArea.getBoundingClientRect();
        if (areaRect.width <= 0 || areaRect.height <= 0) {
            if (typeof onFinish === 'function') onFinish();
            return;
        }

        const surfaceEl = document.createElement('div');
        surfaceEl.className = 'file-area-surface';
        surfaceEl.style.position = 'absolute';
        surfaceEl.style.top = fileArea.scrollTop + 'px';
        surfaceEl.style.left = fileArea.scrollLeft + 'px';
        surfaceEl.style.width = fileArea.clientWidth + 'px';
        surfaceEl.style.height = fileArea.clientHeight + 'px';

        const filtered = (items || []).filter(item => {
            const name = String(item?.name || '').toLowerCase();
            return name !== 'desktop.ini';
        });

        if (filtered.length === 0) {
            surfaceEl.innerHTML = `
                <div class="empty-state">
                    <div class="empty-icon">
                        <svg width="48" height="48" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.2" stroke-linecap="round" stroke-linejoin="round">
                            <path d="M22 19a2 2 0 01-2 2H4a2 2 0 01-2-2V5a2 2 0 012-2h5l2 3h9a2 2 0 012 2z"/>
                        </svg>
                    </div>
                    <div class="empty-text">This folder is empty</div>
                </div>
            `;
        } else if (this.currentView === 'grid') {
            const grid = document.createElement('div');
            grid.className = 'file-grid';
            this._renderFirstScreen(() => this.renderGrid(filtered, grid));
            surfaceEl.appendChild(grid);
        } else {
            const list = document.createElement('div');
            list.className = 'file-list';
            list.innerHTML = `
                <div class="file-header">
                    <div class="col-name"><span>Name</span></div>
                    <div class="col-size"><span>Size</span></div>
                    <div class="col-type"><span>Type</span></div>
                    <div class="col-modified"><span>Modified</span></div>
                </div>
                <div class="file-list-body"></div>
            `;
            const body = list.querySelector('.file-list-body');
            this._renderFirstScreen(() => this.renderList(filtered, body));
            surfaceEl.appendChild(list);
        }

        fileArea.appendChild(surfaceEl);
        this._activeSurfaceEl = surfaceEl;

        const topInset = Math.max(0, Math.min(areaRect.height - 24, Math.round(originRect.top - areaRect.top)));
        const bottomInset = Math.max(0, Math.min(areaRect.height - 24, Math.round(areaRect.bottom - (originRect.top + (originRect.height || 40)))));
        const leftInset = Math.max(0, Math.min(areaRect.width - 24, Math.round(originRect.left - areaRect.left)));
        const rightInset = Math.max(0, Math.min(areaRect.width - 24, Math.round(areaRect.right - (originRect.left + (originRect.width || 40)))));
        const clipRadius = originRadius || '12px';

        const dur = typeof calculateDuration === 'function'
            ? calculateDuration(areaRect.width, areaRect.height, originRect.width || 40, originRect.height || 40)
            : getDuration('motion');

        const contentEl = surfaceEl.querySelector('.file-grid, .file-list-body, .empty-state') || surfaceEl;
        let finished = false;
        let timeline = null;
        const finalize = () => {
            if (finished) return;
            finished = true;
            timeline?.cleanup();
            if (this._activeSurfaceAnim === timeline) {
                this._activeSurfaceAnim = null;
            }
            this._renderStatic(onFinish);
            fileArea.scrollTop = 0;
            if (surfaceEl.parentNode) surfaceEl.remove();
            if (this._activeSurfaceEl === surfaceEl) {
                this._activeSurfaceEl = null;
            }
        };
        timeline = motionTimeline({
            duration: dur,
            durationIsScaled: true,
            easing: EASINGS.expoOut,
            onFinish: finalize,
            onCancel: finalize,
        });
        timeline.add(surfaceEl, [
            {
                clipPath: `inset(${topInset}px ${rightInset}px ${bottomInset}px ${leftInset}px round ${clipRadius})`,
                opacity: 0.2,
                offset: 0,
            },
            {
                // Every edge moves 15% of the way to the file area, the left one included.
                clipPath: `inset(${Math.round(topInset * 0.85)}px ${Math.round(rightInset * 0.85)}px ${Math.round(bottomInset * 0.85)}px ${Math.round(leftInset * 0.85)}px round ${clipRadius})`,
                opacity: 1,
                offset: 0.15,
            },
            {
                clipPath: `inset(0px 0px 0px 0px round 0px)`,
                opacity: 1,
                offset: 1.0,
            }
        ]);
        if (contentEl) {
            timeline.add(contentEl, [
                { opacity: 0, transform: 'translateY(8px)', offset: 0 },
                { opacity: 0, transform: 'translateY(8px)', offset: 0.4 },
                { opacity: 1, transform: 'translateY(0px)', offset: 1.0 },
            ], { easing: EASINGS.content });
        }

        this._activeSurfaceAnim = timeline;
        timeline.play();
    }

    morphToThisPc(originRect, onFinish, originRadius = null) {
        const fileArea = document.getElementById('fileArea');
        if (!fileArea) {
            if (typeof onFinish === 'function') onFinish();
            return;
        }

        if (this._activeSurfaceEl) {
            try { this._activeSurfaceAnim?.cancel(); } catch {}
            if (this._activeSurfaceEl.parentNode) this._activeSurfaceEl.remove();
            this._activeSurfaceEl = null;
        }

        const areaRect = fileArea.getBoundingClientRect();
        if (areaRect.width <= 0 || areaRect.height <= 0) {
            if (typeof onFinish === 'function') onFinish();
            return;
        }

        const surfaceEl = document.createElement('div');
        surfaceEl.className = 'file-area-surface';
        surfaceEl.style.position = 'absolute';
        surfaceEl.style.top = fileArea.scrollTop + 'px';
        surfaceEl.style.left = fileArea.scrollLeft + 'px';
        surfaceEl.style.width = fileArea.clientWidth + 'px';
        surfaceEl.style.height = fileArea.clientHeight + 'px';

        if (this.currentView === 'grid') {
            const grid = document.createElement('div');
            grid.className = 'file-grid';
            this.renderMyPcGrid(grid);
            surfaceEl.appendChild(grid);
        } else {
            const list = document.createElement('div');
            list.className = 'file-list';
            list.innerHTML = `
                <div class="file-header">
                    <div class="col-name"><span>Name</span></div>
                    <div class="col-size"><span>Size</span></div>
                    <div class="col-type"><span>Type</span></div>
                    <div class="col-modified"><span>Modified</span></div>
                </div>
                <div class="file-list-body"></div>
            `;
            const body = list.querySelector('.file-list-body');
            this.renderMyPcList(body);
            surfaceEl.appendChild(list);
        }

        fileArea.appendChild(surfaceEl);
        this._activeSurfaceEl = surfaceEl;

        const topInset = Math.max(0, Math.min(areaRect.height - 24, Math.round(originRect.top - areaRect.top)));
        const bottomInset = Math.max(0, Math.min(areaRect.height - 24, Math.round(areaRect.bottom - (originRect.top + (originRect.height || 40)))));
        const leftInset = Math.max(0, Math.min(areaRect.width - 24, Math.round(originRect.left - areaRect.left)));
        const rightInset = Math.max(0, Math.min(areaRect.width - 24, Math.round(areaRect.right - (originRect.left + (originRect.width || 40)))));
        const clipRadius = originRadius || '12px';

        const dur = typeof calculateDuration === 'function'
            ? calculateDuration(areaRect.width, areaRect.height, originRect.width || 40, originRect.height || 40)
            : getDuration('motion');

        let finished = false;
        let timeline = null;
        const finalize = () => {
            if (finished) return;
            finished = true;
            timeline?.cleanup();
            if (this._activeSurfaceAnim === timeline) {
                this._activeSurfaceAnim = null;
            }
            this._renderStatic(onFinish);
            fileArea.scrollTop = 0;
            if (surfaceEl.parentNode) surfaceEl.remove();
            if (this._activeSurfaceEl === surfaceEl) {
                this._activeSurfaceEl = null;
            }
        };
        timeline = motionTimeline({
            duration: dur,
            durationIsScaled: true,
            easing: EASINGS.expoOut,
            onFinish: finalize,
            onCancel: finalize,
        });
        timeline.add(surfaceEl, [
            {
                clipPath: `inset(${topInset}px ${rightInset}px ${bottomInset}px ${leftInset}px round ${clipRadius})`,
                opacity: 0.2,
                offset: 0,
            },
            {
                // Every edge moves 15% of the way to the file area, the left one included.
                clipPath: `inset(${Math.round(topInset * 0.85)}px ${Math.round(rightInset * 0.85)}px ${Math.round(bottomInset * 0.85)}px ${Math.round(leftInset * 0.85)}px round ${clipRadius})`,
                opacity: 1,
                offset: 0.15,
            },
            {
                clipPath: `inset(0px 0px 0px 0px round 0px)`,
                opacity: 1,
                offset: 1.0,
            }
        ]);
        this._activeSurfaceAnim = timeline;
        timeline.play();
    }

    /** Pinned copy of what is on screen, for transitions that leave the old content
     *  visible while the new one arrives. Cloning a 1000-item folder costs hundreds of
     *  ms for content nobody sees, so each visible node is pinned at its viewport position. */
    _snapshotVisible(fileArea, areaRect) {
        const outgoingLayer = document.createElement('div');
        outgoingLayer.className = 'file-area-transition-layer outgoing';
        outgoingLayer.style.position = 'absolute';
        outgoingLayer.style.top = fileArea.scrollTop + 'px';
        outgoingLayer.style.left = fileArea.scrollLeft + 'px';
        outgoingLayer.style.width = fileArea.clientWidth + 'px';
        outgoingLayer.style.height = fileArea.clientHeight + 'px';
        outgoingLayer.style.zIndex = '8';
        outgoingLayer.style.pointerEvents = 'none';
        outgoingLayer.style.overflow = 'hidden';

        const pinClone = (node) => {
            const r = node.getBoundingClientRect();
            if (r.width === 0 || r.bottom < areaRect.top || r.top > areaRect.bottom) return r;
            const clone = node.cloneNode(true);
            Object.assign(clone.style, {
                position: 'absolute', margin: '0', animation: 'none',
                left: `${r.left - areaRect.left}px`, top: `${r.top - areaRect.top}px`,
                width: `${r.width}px`, height: `${r.height}px`,
            });
            outgoingLayer.appendChild(clone);
            return r;
        };
        Array.from(fileArea.children).forEach(child => {
            if (child.classList.contains('loading-overlay') ||
                child.classList.contains('drop-overlay') ||
                child.classList.contains('file-area-surface') ||
                child.classList.contains('file-area-transition-layer') ||
                child.getClientRects().length === 0) return;
            const items = child.querySelectorAll('.file-item, .file-row, .section-header');
            if (!items.length) { pinClone(child); return; }
            for (const item of items) {
                if (pinClone(item).top > areaRect.bottom) break;
            }
        });
        return outgoingLayer;
    }

    /** Tab morph in the Telegram manner (telegramMorphToThread): an opaque plane grows
     *  from the clicked tab to the file area and covers the old content, which stays in
     *  place underneath. The real containers move into the plane (ids and listeners kept)
     *  and go back when it lands. Returns false when there is nothing to morph from. */
    _morphFromOrigin(origin, renderFn) {
        const fileArea = document.getElementById('fileArea');
        const panel = fileArea?.closest('.explorer-widget');
        const src = origin?.rect;
        if (!fileArea || !panel || !src || src.width <= 0 || src.height <= 0 || typeof renderFn !== 'function') return false;

        const areaRect = fileArea.getBoundingClientRect();
        const bounds = panel.getBoundingClientRect();
        if (areaRect.width <= 0 || areaRect.height <= 0) return false;
        // The clicked tab may sit partly outside the panel: the plane starts from its visible part.
        const from = {
            left: Math.max(src.left, bounds.left), top: Math.max(src.top, bounds.top),
            right: Math.min(src.right, bounds.right), bottom: Math.min(src.bottom, bounds.bottom),
        };
        if (from.right <= from.left || from.bottom <= from.top) return false;

        const outgoingLayer = this._snapshotVisible(fileArea, areaRect);
        fileArea.appendChild(outgoingLayer);
        this._renderStatic(renderFn);
        fileArea.scrollTop = 0;

        const incomingEls = Array.from(fileArea.children).filter(child =>
            !child.classList.contains('loading-overlay') &&
            !child.classList.contains('drop-overlay') &&
            !child.classList.contains('file-area-transition-layer') &&
            child.getClientRects().length > 0);
        const incoming = document.createElement('div');
        incoming.className = 'explorer-morph-incoming';
        Object.assign(incoming.style, {
            position: 'absolute',
            left: `${areaRect.left - bounds.left}px`, top: `${areaRect.top - bounds.top}px`,
            width: `${areaRect.width}px`, height: `${areaRect.height}px`,
            display: 'flex', flexDirection: 'column', overflow: 'hidden',
        });
        incoming.append(...incomingEls);

        // A decorative copy of the clicked tab bridges the origin to the reveal.
        const sourceCopy = origin.node ? origin.node.cloneNode(true) : document.createElement('div');
        sourceCopy.removeAttribute('id');
        sourceCopy.querySelectorAll('[id]').forEach(node => node.removeAttribute('id'));
        Object.assign(sourceCopy.style, {
            position: 'absolute', margin: '0', boxSizing: 'border-box', pointerEvents: 'none',
            left: `${src.left - bounds.left}px`, top: `${src.top - bounds.top}px`,
            width: `${src.width}px`, height: `${src.height}px`,
        });
        const plane = document.createElement('div');
        plane.className = 'explorer-morph-plane';
        plane.inert = true;
        plane.setAttribute('aria-hidden', 'true');
        plane.append(incoming, sourceCopy);
        panel.append(plane);

        const insets = r => `inset(${r.top - bounds.top}px ${bounds.right - r.right}px ${bounds.bottom - r.bottom}px ${r.left - bounds.left}px`;
        const radius = origin.radius || '0px';
        const duration = calculateDuration(areaRect.width, areaRect.height, from.right - from.left, from.bottom - from.top);

        let timeline = null;
        let finished = false;
        const finalize = () => {
            if (finished) return;
            finished = true;
            timeline?.cleanup();
            if (this._activeTabTransition === timeline) this._activeTabTransition = null;
            outgoingLayer.remove();
            fileArea.append(...incomingEls);
            plane.remove();
            fileArea.scrollTop = 0;
        };
        timeline = motionTimeline({
            duration,
            durationIsScaled: true,
            easing: EASINGS.expoOut,
            onFinish: finalize,
            onCancel: finalize,
        });
        timeline.add(plane, [
            { clipPath: `${insets(from)} round ${radius})` },
            { clipPath: `${insets(areaRect)} round 0px)` },
        ]);
        timeline.add(sourceCopy, [{ opacity: 1 }, { opacity: 0, offset: 0.35 }, { opacity: 0 }], { easing: EASINGS.content });
        timeline.add(incoming, [{ opacity: 0 }, { opacity: 1, offset: 0.5 }, { opacity: 1 }], { easing: EASINGS.content });
        this._activeTabTransition = timeline;
        timeline.play();
        return true;
    }

    playTabTransition(direction, renderTargetFn, origin = null) {
        const fileArea = document.getElementById('fileArea');
        if (!fileArea) {
            if (typeof renderTargetFn === 'function') renderTargetFn();
            return;
        }

        if (this._activeTabTransition) {
            try { this._activeTabTransition.cancel(); } catch {}
            this._activeTabTransition = null;
        }

        if (origin && this._morphFromOrigin(origin, renderTargetFn)) return;

        const areaRect = fileArea.getBoundingClientRect();
        if (areaRect.width <= 0 || areaRect.height <= 0) {
            if (typeof renderTargetFn === 'function') renderTargetFn();
            return;
        }

        const outgoingLayer = this._snapshotVisible(fileArea, areaRect);
        fileArea.appendChild(outgoingLayer);

        // The incoming containers slide in as a whole; items must not run
        // their own appear animation on top of that.
        this._renderStatic(renderTargetFn);
        fileArea.scrollTop = 0;

        const incomingEls = Array.from(fileArea.children).filter(child =>
            !child.classList.contains('loading-overlay') &&
            !child.classList.contains('drop-overlay') &&
            !child.classList.contains('file-area-surface') &&
            !child.classList.contains('file-area-transition-layer')
        );

        let outX = 0, inX = 0, outY = 0, inY = 0;
        const offsetDist = 28;

        if (direction === 'tab-forward' || direction === 'forward') {
            outX = -offsetDist;
            inX = offsetDist;
        } else if (direction === 'tab-back' || direction === 'back') {
            outX = offsetDist;
            inX = -offsetDist;
        } else if (direction === 'up') {
            outY = offsetDist;
            inY = -offsetDist;
        } else {
            outX = -offsetDist;
            inX = offsetDist;
        }

        const dur = getDuration('transient') || 220;
        const easing = EASINGS.expoOut || 'cubic-bezier(0.30, 0.95, 0.35, 1.00)';

        let finished = false;
        let timeline = null;
        const finalize = () => {
            if (finished) return;
            finished = true;
            timeline?.cleanup();
            if (outgoingLayer.parentNode) outgoingLayer.remove();
            if (this._activeTabTransition === timeline) {
                this._activeTabTransition = null;
            }
            incomingEls.forEach(el => {
                el.style.transform = '';
                el.style.opacity = '';
            });
        };
        timeline = motionTimeline({
            duration: dur,
            durationIsScaled: true,
            easing,
            onFinish: finalize,
            onCancel: finalize,
        });
        timeline.add(outgoingLayer, [
            { opacity: 1, transform: 'translate(0px, 0px)' },
            { opacity: 0, transform: `translate(${outX}px, ${outY}px)` }
        ]);

        incomingEls.forEach(el => {
            timeline.add(el, [
                { opacity: 0, transform: `translate(${inX}px, ${inY}px)` },
                { opacity: 1, transform: 'translate(0px, 0px)' }
            ]);
        });

        this._activeTabTransition = timeline;
        timeline.play();
    }

    _playTransition(fileArea, transitionType = 'fade') {
        if (!fileArea || typeof fileArea.animate !== 'function') return;
        let keyframes;
        if (transitionType === 'back' || transitionType === 'tab-back') {
            keyframes = [
                { opacity: 0.35, transform: 'translateX(-8px)' },
                { opacity: 1, transform: 'translateX(0px)' }
            ];
        } else if (transitionType === 'forward' || transitionType === 'tab-forward') {
            keyframes = [
                { opacity: 0.35, transform: 'translateX(8px)' },
                { opacity: 1, transform: 'translateX(0px)' }
            ];
        } else if (transitionType === 'up') {
            keyframes = [
                { opacity: 0.35, transform: 'translateY(-6px)' },
                { opacity: 1, transform: 'translateY(0px)' }
            ];
        } else {
            keyframes = [
                { opacity: 0.35, transform: 'scale(0.99)' },
                { opacity: 1, transform: 'scale(1)' }
            ];
        }

        const timeline = motionTimeline({
            duration: getDuration('transient'),
            durationIsScaled: true,
            easing: EASINGS.expoOut,
            onFinish: (finishedTimeline) => {
                finishedTimeline.cleanup();
                fileArea.style.transform = '';
                fileArea.style.opacity = '';
            },
        });
        timeline.add(fileArea, keyframes).play();
    }

    renderMyPc() {
        const fileGrid = document.getElementById('fileGrid');
        const fileList = document.getElementById('fileList');
        const fileListBody = document.getElementById('fileListBody');
        const emptyState = document.getElementById('emptyState');

        const favItems = this.getFavoritesItems();
        const driveItems = this.getDrivesItems();
        const hasContent = (favItems && favItems.length > 0) || (driveItems && driveItems.length > 0);

        if (!hasContent) {
            fileGrid.classList.add('hidden');
            if (fileList) fileList.classList.add('hidden');
            emptyState.classList.remove('hidden');
            const txt = emptyState.querySelector('.empty-text');
            if (txt) txt.textContent = 'No drives or places found';
            return;
        }

        emptyState.classList.add('hidden');

        if (this.currentView === 'grid') {
            fileGrid.classList.remove('hidden');
            if (fileList) fileList.classList.add('hidden');
            this.renderMyPcGrid(fileGrid);
        } else {
            fileGrid.classList.add('hidden');
            if (fileList) fileList.classList.remove('hidden');
            this.renderMyPcList(fileListBody);
        }
    }

    renderMyPcGrid(container) {
        container.innerHTML = '';
        const sections = [
            { title: 'Favorites', items: this.getFavoritesItems() },
            { title: 'Drives', items: this.getDrivesItems() }
        ];

        sections.forEach((section, si) => {
            if (!section.items || section.items.length === 0) return;

            const header = document.createElement('div');
            header.className = 'section-header';
            header.textContent = section.title;
            container.appendChild(header);

            section.items.forEach((item, i) => {
                const el = document.createElement('div');
                el.className = 'file-item' + (this.selectedFiles.has(item.path) ? ' selected' : '');
                el.dataset.path = item.path;
                el.dataset.isDir = 'true';
                el.dataset.name = item.name;
                el.style.animationDelay = `${Math.min((si * 3 + i) * 15, 150)}ms`;

                const iconSrc = item.icon
                    ? (item.icon.includes('/') ? explorerAsset(item.icon) : explorerAsset('icons/ui/' + item.icon))
                    : this.fileIconSrc(item);

                const freeSpace = item.free_space ? this.formatSize(item.free_space) : '';
                const totalSpace = item.total_space ? this.formatSize(item.total_space) : '';

                el.innerHTML = `
                    <div class="file-icon ${item.total_space ? 'drive-icon' : ''}">
                        <img src="${iconSrc}" alt="" draggable="false">
                    </div>
                    <div class="file-name" aria-label="${item.name}">${item.name}</div>
                    ${freeSpace ? `<div class="file-size">${freeSpace} free</div>
                    <div class="drive-bar" aria-label="${freeSpace} free of ${totalSpace}"><span style="width:${item.total_space ? Math.max(4, Math.round(100 * (1 - (item.free_space || 0) / item.total_space))) : 0}%"></span></div>` : ''}
                `;

                el.addEventListener('click', (e) => this.selectFile({ ...item, is_dir: true }, e));
                el.addEventListener('dblclick', () => this.navigateTo(item.path, el));
                el.addEventListener('contextmenu', (e) => {
                    e.preventDefault();
                    e.stopPropagation();
                    if (!this.selectedFiles.has(item.path)) {
                        this.selectFile(item, e);
                    }
                    this.contextMenu.show(e.clientX, e.clientY, 'file', item);
                });

                container.appendChild(el);
            });
        });
    }

    renderMyPcList(container) {
        container.innerHTML = '';
        const sections = [
            { title: 'Favorites', items: this.getFavoritesItems() },
            { title: 'Drives', items: this.getDrivesItems() }
        ];

        sections.forEach((section, si) => {
            if (!section.items || section.items.length === 0) return;

            section.items.forEach((item, i) => {
                const row = document.createElement('div');
                row.className = 'file-row' + (this.selectedFiles.has(item.path) ? ' selected' : '');
                row.dataset.path = item.path;
                row.dataset.isDir = 'true';
                row.dataset.name = item.name;
                row.style.animationDelay = `${Math.min((si * 3 + i) * 10, 100)}ms`;

                const iconSrc = item.icon
                    ? (item.icon.includes('/') ? explorerAsset(item.icon) : explorerAsset('icons/ui/' + item.icon))
                    : this.fileIconSrc(item);

                const size = item.free_space && item.total_space ? `${this.formatSize(item.free_space)} free of ${this.formatSize(item.total_space)}` : '—';
                const type = item.type || (item.total_space ? 'Local Drive' : 'Folder');

                row.innerHTML = `
                    <div class="col-name">
                        <img src="${iconSrc}" class="row-icon" alt="" draggable="false">
                        <span class="row-name" aria-label="${item.name}">${item.name}</span>
                    </div>
                    <div class="col-size">${size}</div>
                    <div class="col-type">${type}</div>
                    <div class="col-modified">—</div>
                `;

                row.addEventListener('click', (e) => this.selectFile({ ...item, is_dir: true }, e));
                row.addEventListener('dblclick', () => this.navigateTo(item.path, row));
                row.addEventListener('contextmenu', (e) => {
                    e.preventDefault();
                    e.stopPropagation();
                    if (!this.selectedFiles.has(item.path)) {
                        this.selectFile(item, e);
                    }
                    this.contextMenu.show(e.clientX, e.clientY, 'file', item);
                });

                container.appendChild(row);
            });
        });
    }

    getFavoritesItems() {
        return this._favorites || [];
    }

    async resolveFavorites() {
        // Prefer backend places (no path-plugin ACL issues).
        try {
            const places = await invoke('get_user_places');
            if (Array.isArray(places) && places.length) {
                this._places = places;
                this._favorites = places
                    .filter((p) => p.kind === 'folder')
                    .map((p) => ({
                        name: p.name,
                        path: p.path,
                        icon: this.getFolderIcon({ name: p.name, path: p.path }),
                        type: 'Folder',
                    }));
                return this._favorites;
            }
        } catch (e) {
            console.error('get_user_places failed:', e);
        }
        this._favorites = [];
        return this._favorites;
    }

    getDrivesItems() {
        return this._cachedDrives || [];
    }

    renderFiles(items) {
        const fileGrid = document.getElementById('fileGrid');
        const fileList = document.getElementById('fileList');
        const fileListBody = document.getElementById('fileListBody');
        const emptyState = document.getElementById('emptyState');

        const filteredItems = (items || []).filter(item => {
            const name = String(item?.name || '').toLowerCase();
            return name !== 'desktop.ini';
        });

        if (filteredItems.length === 0) {
            fileGrid.classList.add('hidden');
            if (fileList) fileList.classList.add('hidden');
            emptyState.classList.remove('hidden');
            const txt = emptyState.querySelector('.empty-text');
            if (txt) txt.textContent = 'This folder is empty';
            return;
        }

        emptyState.classList.add('hidden');

        if (this.currentView === 'grid') {
            fileGrid.classList.remove('hidden');
            if (fileList) fileList.classList.add('hidden');
            this.renderGrid(filteredItems, fileGrid);
        } else {
            fileGrid.classList.add('hidden');
            if (fileList) fileList.classList.remove('hidden');
            this.renderList(filteredItems, fileListBody);
        }
    }

    isImageItem(item) {
        if (!item || item.is_dir || item.isDir) return false;
        const ext = (item.name || item.path || '').split('.').pop()?.toLowerCase() || '';
        return ['jpg', 'jpeg', 'png', 'gif', 'webp', 'bmp', 'ico', 'svg', 'avif'].includes(ext);
    }

    /** Render without per-item appear animations (transitions animate the
     *  containers instead). The flag is read synchronously by renderGrid /
     *  renderList, so no class has to survive a style flush. */
    _renderStatic(renderFn) {
        if (typeof renderFn !== 'function') return;
        this._suppressAppear = true;
        try {
            renderFn();
        } finally {
            this._suppressAppear = false;
        }
    }

    /** Render only the first screenful (morph stand-ins): the real render
     *  replaces it when the morph lands, so chunking it would only compete
     *  with the morph for frames. */
    _renderFirstScreen(renderFn) {
        this._firstScreenOnly = true;
        try {
            renderFn();
        } finally {
            this._firstScreenOnly = false;
        }
    }

    /** Only the first rows stagger in; a long list must not start hundreds of
     *  animations at once. */
    _appearStyle(el, i, stepMs, maxMs) {
        if (this._suppressAppear || i >= APPEAR_LIMIT) el.style.animation = 'none';
        else el.style.animationDelay = `${Math.min(i * stepMs, maxMs)}ms`;
    }

    /** Build items into `container`: the first screenful synchronously, the
     *  rest in per-frame chunks so a 1000-file folder never blocks the main
     *  thread in one long task. A newer render cancels the pending chunks. */
    _renderInChunks(items, container, build) {
        const token = (this._renderToken = (this._renderToken || 0) + 1);
        let i = 0;
        const step = (limit) => {
            const frag = document.createDocumentFragment();
            for (const end = Math.min(items.length, i + limit); i < end; i++) {
                const el = build(items[i], i);
                if (el) frag.appendChild(el);
            }
            container.appendChild(frag);
        };
        step(RENDER_FIRST);
        // A morph surface is a stand-in for one screen; it never grows further.
        if (this._firstScreenOnly) return;
        const next = () => {
            if (token !== this._renderToken || !container.isConnected) return;
            step(RENDER_CHUNK);
            this._applyPendingScroll();
            if (i < items.length) requestAnimationFrame(next);
        };
        if (i < items.length) requestAnimationFrame(next);
    }

    /** Restore a scroll offset that may lie beyond the chunks rendered so far:
     *  it is re-applied as chunks arrive until the content is tall enough. */
    _restoreScroll(scroller, top) {
        scroller.scrollTop = top;
        this._pendingScroll = scroller.scrollTop < top - 1 ? { scroller, top } : null;
    }

    _applyPendingScroll() {
        const pending = this._pendingScroll;
        if (!pending) return;
        pending.scroller.scrollTop = pending.top;
        if (pending.scroller.scrollTop >= pending.top - 1) this._pendingScroll = null;
    }

    renderGrid(items, container) {
        container.innerHTML = '';
        this._renderInChunks(items, container, (item, i) => {
            if (item.name === '.' || item.name === '..') return null;

            const isImg = this.isImageItem(item);
            const iconSrc = isImg ? convertFileSrc(item.path) : this.fileIconSrc(item);
            const imgClass = isImg ? 'file-thumb' : '';

            const el = document.createElement('div');
            el.className = 'file-item' + (this.selectedFiles.has(item.path) ? ' selected' : '');
            el.dataset.path = item.path;
            el.dataset.isDir = item.is_dir;
            el.dataset.name = item.name;
            this._appearStyle(el, i, 15, 150);

            el.innerHTML = `
                <div class="file-icon ${isImg ? 'has-thumb' : ''}">
                    <img src="${iconSrc}" class="${imgClass}" alt="" draggable="false" loading="lazy" decoding="async">
                </div>
                <div class="file-name" aria-label="${item.name}">${item.name}</div>
            `;
            this.installIconFallback(el.querySelector('img'), item);

            el.addEventListener('dblclick', () => this.openItem(item, el));

            el.addEventListener('click', (e) => {
                this.selectFile(item, e);
            });

            el.addEventListener('contextmenu', (e) => {
                e.preventDefault();
                e.stopPropagation();
                if (!this.selectedFiles.has(item.path)) {
                    this.selectFile(item, e);
                }
                this.contextMenu.show(e.clientX, e.clientY, 'file', item);
            });

            return el;
        });
    }

    renderList(items, container) {
        container.innerHTML = '';
        this._renderInChunks(items, container, (item, i) => {
            if (item.name === '.' || item.name === '..') return null;

            const isImg = this.isImageItem(item);
            const iconSrc = isImg ? convertFileSrc(item.path) : this.fileIconSrc(item);
            const imgClass = isImg ? 'row-icon row-thumb' : 'row-icon';

            const row = document.createElement('div');
            row.className = 'file-row' + (this.selectedFiles.has(item.path) ? ' selected' : '');
            row.dataset.path = item.path;
            row.dataset.isDir = item.is_dir;
            row.dataset.name = item.name;
            this._appearStyle(row, i, 10, 100);

            const size = item.is_dir ? '—' : this.formatSize(item.file_size || 0);
            const type = item.is_dir ? 'Folder' : this.getFileTypeName(item.path);
            const date = item.modified_time || '—';

            row.innerHTML = `
                <div class="col-name">
                    <img src="${iconSrc}" class="${imgClass}" alt="" draggable="false" loading="lazy" decoding="async">
                    <span class="row-name" aria-label="${item.name}">${item.name}</span>
                </div>
                <div class="col-size">${size}</div>
                <div class="col-type">${type}</div>
                <div class="col-modified">${date}</div>
            `;
            this.installIconFallback(row.querySelector('img'), item);

            row.addEventListener('dblclick', () => this.openItem(item, row));

            row.addEventListener('click', (e) => {
                this.selectFile(item, e);
            });

            row.addEventListener('contextmenu', (e) => {
                e.preventDefault();
                e.stopPropagation();
                if (!this.selectedFiles.has(item.path)) {
                    this.selectFile(item, e);
                }
                this.contextMenu.show(e.clientX, e.clientY, 'file', item);
            });

            return row;
        });
    }

    /** Open folder in-shell; open files with the system handler. Never send dirs to explorer.exe. */
    async openItem(item, originEl = null) {
        if (!item?.path) return;
        const path = String(item.path);
        const name = String(item.name || '').toLowerCase();

        if (item.is_dir || item.isDir) {
            await this.navigateTo(path, originEl);
            return;
        }

        // .lnk / special names that are "folders" to the user
        if (
            name === 'recycle bin' ||
            name === 'корзина' ||
            /\$recycle\.bin$/i.test(path.replace(/\\/g, '/'))
        ) {
            await this.navigateTo(path, originEl);
            return;
        }

        try {
            const isDir = await invoke('path_is_directory', { path });
            if (isDir) {
                await this.navigateTo(path, originEl);
                return;
            }
        } catch {
            try {
                const info = await invoke('get_file_info', { path });
                if (info?.is_dir) {
                    await this.navigateTo(path, originEl);
                    return;
                }
            } catch {}
        }

        await this.openFile(path);
    }

    selectFile(item, e) {
        if (!item || !item.path) return;
        const currentTab = this.tabManager?.getActiveTab();
        const activeItems = (currentTab?.path === '::MY_PC::')
            ? (currentTab.items || [])
            : (currentTab?.items || []);

        if (e && e.shiftKey && this.lastSelectedPath) {
            // Shift + Click: range selection from lastSelectedPath to item.path
            const paths = activeItems.map(i => i.path);
            const anchorIdx = paths.indexOf(this.lastSelectedPath);
            const targetIdx = paths.indexOf(item.path);

            if (anchorIdx !== -1 && targetIdx !== -1) {
                const start = Math.min(anchorIdx, targetIdx);
                const end = Math.max(anchorIdx, targetIdx);

                if (!e.ctrlKey && !e.metaKey) {
                    this.selectedFiles.clear();
                }

                for (let i = start; i <= end; i++) {
                    const it = activeItems[i];
                    if (it?.path) this.selectedFiles.add(it.path);
                }
            } else {
                if (!e.ctrlKey && !e.metaKey) this.selectedFiles.clear();
                this.selectedFiles.add(item.path);
                this.lastSelectedPath = item.path;
            }
        } else if (e && (e.ctrlKey || e.metaKey)) {
            // Ctrl + Click: toggle this file
            if (this.selectedFiles.has(item.path)) {
                this.selectedFiles.delete(item.path);
            } else {
                this.selectedFiles.add(item.path);
                this.lastSelectedPath = item.path;
            }
        } else {
            // Normal Click: single selection
            this.selectedFiles.clear();
            this.selectedFiles.add(item.path);
            this.lastSelectedPath = item.path;
        }

        // Update DOM selected classes
        document.querySelectorAll('.file-item, .file-row').forEach(el => {
            const path = el.dataset.path;
            if (path && this.selectedFiles.has(path)) {
                el.classList.add('selected');
            } else {
                el.classList.remove('selected');
            }
        });

        // Update preview panel
        if (this.selectedFiles.size === 1) {
            const singleItem = activeItems.find(i => i.path === item.path) || item;
            this.previewPanel.showFile(singleItem);
        } else if (this.selectedFiles.size > 1) {
            this.previewPanel.showFile({
                name: `${this.selectedFiles.size} items selected`,
                is_dir: true,
                isDir: true,
                path: '',
                size_formatted: `${this.selectedFiles.size} files`
            });
        } else {
            this.previewPanel.clear();
        }
    }

    deselectAll() {
        this.selectedFiles.clear();
        this.lastSelectedPath = null;
        document.querySelectorAll('.file-item.selected, .file-row.selected').forEach(el => el.classList.remove('selected'));
        this.previewPanel.clear();
    }

    selectAll() {
        const tab = this.tabManager?.getActiveTab();
        const items = (tab?.path === '::MY_PC::')
            ? (tab.items || [])
            : (tab?.items || []);
        this.selectedFiles.clear();
        items.forEach(it => {
            if (it?.path) this.selectedFiles.add(it.path);
        });
        document.querySelectorAll('.file-item, .file-row').forEach(el => el.classList.add('selected'));
        if (this.selectedFiles.size === 1) {
            this.previewPanel.showFile(items[0]);
        } else if (this.selectedFiles.size > 1) {
            this.previewPanel.showFile({
                name: `${this.selectedFiles.size} items selected`,
                is_dir: true,
                isDir: true,
                path: '',
                size_formatted: `${this.selectedFiles.size} files`
            });
        }
    }

    /** Full asset URL for a file/folder icon (folders live under icons/ui/). */
    fileIconSrc(item) {
        if (!item) return explorerAsset('icons/file-types/file-generic.svg');
        if (item.is_dir || item.isDir) {
            return explorerAsset('icons/ui/' + this.getFolderIcon(item));
        }
        return explorerAsset('icons/file-types/' + this.getFileIcon(item));
    }

    installIconFallback(img, item) {
        if (!img) return;
        img.addEventListener('error', () => {
            img.classList.remove('file-thumb', 'row-thumb');
            img.closest('.file-icon')?.classList.remove('has-thumb');
            const fallback = (item?.is_dir || item?.isDir)
                ? explorerAsset('icons/ui/folder.svg')
                : explorerAsset('icons/file-types/file-generic.svg');
            if (img.src !== fallback) img.src = fallback;
        }, { once: true });
    }

    getFolderIcon(item) {
        return getFolderIconName(item);
    }

    getFileIcon(item) {
        return getFileIconName(item);
    }

    getFileTypeName(path) {
        return getFileTypeName(path);
    }

    formatSize(bytes) {
        if (bytes === 0) return '0 B';
        const units = ['B', 'KB', 'MB', 'GB', 'TB'];
        const i = Math.floor(Math.log(bytes) / Math.log(1024));
        return (bytes / Math.pow(1024, i)).toFixed(i > 0 ? 1 : 0) + ' ' + units[i];
    }

    updatePathBar(path) {
        const pathBar = document.getElementById('pathBar');
        if (!pathBar) return;
        pathBar.innerHTML = '';
        if (path === '::MY_PC::') {
            const span = document.createElement('span');
            span.className = 'path-segment active';
            span.textContent = 'This PC';
            pathBar.appendChild(span);
            return;
        }

        const parts = path.split(/[\\/]/).filter(Boolean);
        let cumPath = '';

        parts.forEach((part, i) => {
            if (i === 0 && /^[A-Za-z]:?$/.test(part)) {
                const driveLetter = part.replace(':', '').toUpperCase();
                cumPath = driveLetter + ':\\';
                part = driveLetter + ':';
            } else {
                cumPath += (i === 0 ? '' : (cumPath.endsWith('\\') ? '' : '\\')) + part;
            }
            const isLast = i === parts.length - 1;
            const seg = document.createElement('span');
            seg.className = 'path-segment' + (isLast ? ' active' : '');
            seg.textContent = part;
            const targetPath = cumPath;
            seg.addEventListener('click', () => {
                this.navigateTo(targetPath, seg, 'up');
            });
            pathBar.appendChild(seg);

            if (!isLast) {
                const sep = document.createElement('span');
                sep.className = 'path-separator';
                sep.textContent = '›';
                pathBar.appendChild(sep);
            }
        });
    }

    updateNavButtons() {
        const tab = this.tabManager.getActiveTab();
        document.getElementById('btnBack').disabled = !tab || tab.historyIndex <= 0;
        document.getElementById('btnForward').disabled = !tab || tab.historyIndex >= tab.history.length - 1;
        document.getElementById('btnUp').disabled = !tab || !tab.path || tab.path === '::MY_PC::';
    }

    updateStatusBar(items) {
        const dirs = items.filter(i => i.is_dir).length;
        const files = items.length - dirs;
        let text = '';
        if (dirs > 0) text += `${dirs} folder${dirs !== 1 ? 's' : ''}`;
        if (dirs > 0 && files > 0) text += ', ';
        if (files > 0) text += `${files} file${files !== 1 ? 's' : ''}`;
        if (items.length === 0) text = 'Empty folder';
        document.getElementById('statusItems').textContent = text;

        const free = document.getElementById('statusFree');
        if (free) free.textContent = '';
    }

    showLoading(show) {
        if (this._loadingTimer) {
            clearTimeout(this._loadingTimer);
            this._loadingTimer = null;
        }
        if (show) {
            if (this._activeSurfaceEl || this._activeTabTransition) return;
            this._loadingTimer = setTimeout(() => {
                if (this._activeSurfaceEl || this._activeTabTransition) return;
                document.getElementById('loadingOverlay')?.classList.remove('hidden');
            }, 280);
        } else {
            document.getElementById('loadingOverlay')?.classList.add('hidden');
        }
    }

    showEmpty(show, text) {
        const empty = document.getElementById('emptyState');
        if (show) {
            empty.classList.remove('hidden');
            const txt = empty.querySelector('.empty-text');
            if (txt && text) txt.textContent = text;
        } else {
            empty.classList.add('hidden');
        }
    }

    async openFile(path) {
        // Final guard: never hand directories to the system opener.
        try {
            const isDir = await invoke('path_is_directory', { path });
            if (isDir) {
                await this.navigateTo(path);
                return;
            }
        } catch {}

        try {
            await invoke('open_path_default', { path });
        } catch (e2) {
            console.error('Open file failed:', path, e2);
        }
    }

    sidebarItemEl({ name, path, icon, active }) {
        const item = document.createElement('div');
        item.className = `sidebar-item${active ? ' active' : ''}`;
        item.dataset.path = path || '';
        item.innerHTML = `
            <img src="${explorerAsset('icons/ui/' + (icon || 'folder.svg'))}" class="item-icon" alt="">
            <span class="item-name">${name}</span>
        `;
        item.addEventListener('click', () => {
            if (path === '::MY_PC::' || path) this.navigateTo(path, item);
            this.highlightSidebar(path);
        });
        return item;
    }

    highlightSidebar(path) {
        const norm = (p) => String(p || '').replace(/[\\/]+$/, '').toLowerCase();
        const target = norm(path);
        document.querySelectorAll('.sidebar-item').forEach((el) => {
            el.classList.toggle('active', norm(el.dataset.path) === target);
        });
    }

    async loadPlaces() {
        const container = document.getElementById('placesList');
        if (!container) return;
        container.innerHTML = '';

        // Always-present roots
        container.appendChild(
            this.sidebarItemEl({
                name: 'This PC',
                path: '::MY_PC::',
                icon: 'drive.svg',
                active: true,
            }),
        );

        try {
            const places = await invoke('get_user_places');
            this._places = Array.isArray(places) ? places : [];
            for (const place of this._places) {
                if (place.kind === 'recycle') {
                    container.appendChild(
                        this.sidebarItemEl({
                            name: place.name,
                            path: place.path,
                            icon: 'folder-recycle.svg',
                        }),
                    );
                }
            }
        } catch (e) {
            console.error('loadPlaces failed:', e);
        }
    }

    async loadDrives() {
        const container = document.getElementById('drivesList');
        if (!container) return;
        container.innerHTML = '';

        try {
            let drives = await this.getDrives();
            if (!drives?.length) {
                // Fallback: raw letters only
                try {
                    const raw = await invoke('get_raw_drives');
                    drives = (raw || []).map((p) => ({
                        display: p,
                        path: p,
                        is_removable: false,
                        total_space: null,
                        free_space: null,
                    }));
                } catch {}
            }

            this._cachedDrives = (drives || []).map((d) => ({
                name: d.display,
                path: d.path,
                icon: 'drive.svg',
                type: d.is_removable ? 'Removable Drive' : 'Local Drive',
                free_space: d.free_space,
                total_space: d.total_space,
            }));

            for (const drive of drives || []) {
                container.appendChild(
                    this.sidebarItemEl({
                        name: drive.display,
                        path: drive.path,
                        icon: 'drive.svg',
                    }),
                );
            }
        } catch (e) {
            console.error('Failed to load drives:', e);
        }
    }

    async loadFavorites() {
        const container = document.getElementById('favoritesList');
        if (!container) return;

        const favorites = await this.resolveFavorites();
        container.innerHTML = '';

        favorites.forEach((fav) => {
            container.appendChild(
                this.sidebarItemEl({
                    name: fav.name,
                    path: fav.path,
                    icon: fav.icon || this.getFolderIcon(fav),
                }),
            );
        });
    }

    setView(view) {
        this.currentView = view;
        const viewGrid = document.getElementById('viewGrid');
        const viewList = document.getElementById('viewList');
        if (viewGrid) viewGrid.classList.toggle('active', view === 'grid');
        if (viewList) viewList.classList.toggle('active', view === 'list');

        const tab = this.tabManager.getActiveTab();
        if (!tab) return;
        if (tab.path === '::MY_PC::') {
            this.renderMyPc();
        } else {
            this.updateSortHeader();
            this.renderFiles(tab.items || []);
        }
    }

    handleContextMenuAction(action, item) {
        switch (action) {
            case 'open':
                if (item) this.openItem(item);
                break;

            case 'open-with':
                this.openFileWithSystem(item?.path);
                break;

            case 'copy':
                this.copySelected();
                break;

            case 'cut':
                this.cutSelected();
                break;

            case 'delete':
                this.deleteSelected();
                break;

            case 'rename':
                this.renameSelected();
                break;

            case 'send':
                this.sendViaTelegram(item?.path);
                break;

            case 'new-folder':
                this.createFolder();
                break;

            case 'new-file':
                this.createFile();
                break;

            case 'refresh':
                this.refreshCurrent();
                break;

            case 'paste':
                this.pasteFiles();
                break;

            case 'properties':
                this.showProperties(item);
                break;

            case 'copy-path':
                this.copyPathAction(item);
                break;

            case 'open-terminal':
                this.openTerminalAction(item);
                break;

            case 'select-all':
                this.selectAll();
                break;

            case 'sort-name':
                this.setSorting('name');
                break;

            case 'sort-date':
                this.setSorting('modified');
                break;

            case 'sort-size':
                this.setSorting('size');
                break;

            case 'sort-type':
                this.setSorting('type');
                break;

            default:
                console.log('Unknown context menu action:', action);
        }
    }

    setSorting(column) {
        if (this.sortColumn === column) {
            this.sortAscending = !this.sortAscending;
        } else {
            this.sortColumn = column;
            this.sortAscending = column === 'name' || column === 'type';
        }
        this.updateSortHeader();
        const tab = this.tabManager.getActiveTab();
        if (tab && Array.isArray(tab.items) && tab.path !== '::MY_PC::') {
            const fileArea = document.getElementById('fileArea');
            const scroll = fileArea ? fileArea.scrollTop : 0;
            tab.items = this.sortItems(tab.items, this.sortColumn, this.sortAscending);
            tab._sortCol = this.sortColumn;
            tab._sortAsc = this.sortAscending;
            this.renderFiles(tab.items);
            if (fileArea) fileArea.scrollTop = scroll;
        }

        clearTimeout(this._saveSortTimer);
        this._saveSortTimer = setTimeout(() => {
            invoke('explorer_sort_set', {
                column: this.sortColumn,
                ascending: this.sortAscending,
            }).catch((err) => {
                console.warn('explorer_sort_set failed:', err);
            });
        }, 300);
    }

    async copyPathAction(item) {
        const tab = this.tabManager.getActiveTab();
        let targetPath = null;
        // If the menu target is among the current selection, copy ALL selected
        // paths (the user intentionally multi-selected). Otherwise copy just the
        // single menu item or the active tab path.
        if (item?.path && this.selectedFiles.has(item.path) && this.selectedFiles.size > 1) {
            targetPath = Array.from(this.selectedFiles).join('\r\n');
        } else if (item?.path) {
            targetPath = item.path;
        } else if (this.selectedFiles.size > 0) {
            targetPath = Array.from(this.selectedFiles).join('\r\n');
        } else if (tab && tab.path && tab.path !== '::MY_PC::') {
            targetPath = tab.path;
        }
        if (!targetPath) return;

        try {
            await invoke('set_clipboard_text', { text: targetPath });
        } catch {
            try {
                if (navigator.clipboard) {
                    await navigator.clipboard.writeText(targetPath);
                }
            } catch (e) {
                console.error('Clipboard copy failed:', e);
            }
        }

        const statusHint = document.getElementById('statusHint');
        if (statusHint) {
            const origText = statusHint.textContent;
            statusHint.textContent = 'Path copied to clipboard';
            statusHint.style.color = 'rgba(var(--fg-rgb), 0.9)';
            // Guard against overlapping calls clobbering the hint mid-timeout.
            if (this._statusHintTimer) clearTimeout(this._statusHintTimer);
            this._statusHintTimer = setTimeout(() => {
                this._statusHintTimer = null;
                statusHint.textContent = origText;
                statusHint.style.color = '';
            }, 1800);
        }
    }

    async openTerminalAction(item) {
        const tab = this.tabManager.getActiveTab();
        let targetPath = item?.path;
        if (!targetPath) {
            if (tab && tab.path && tab.path !== '::MY_PC::') {
                targetPath = tab.path;
            } else {
                targetPath = '';
            }
        }
        try {
            await invoke('open_terminal', { path: targetPath });
        } catch (e) {
            console.error('Open terminal error:', e);
        }
    }

    async openFileWithSystem(path) {
        if (!path) return;
        try {
            await invoke('open_path_default', { path });
        } catch (e) {
            console.error('Open with system error:', e);
        }
    }

    sendViaTelegram(path) {
        if (!path) return;
        window.dispatchEvent(new CustomEvent('telegram-send-file', { detail: { path } }));
    }

    async createFolder() {
        const tab = this.tabManager.getActiveTab();
        if (!tab || !tab.path || tab.path === '::MY_PC::') return;

        let name = 'New Folder';
        let counter = 1;
        let newPath = tab.path + '\\' + name;

        while (true) {
            try {
                await invoke('create_directory', { path: newPath });
                break;
            } catch {
                counter++;
                name = `New Folder (${counter})`;
                newPath = tab.path + '\\' + name;
                if (counter > 100) return;
            }
        }

        this.refreshCurrent();
    }

    async createFile() {
        const tab = this.tabManager.getActiveTab();
        if (!tab || !tab.path || tab.path === '::MY_PC::') return;

        let name = 'New File.txt';
        let counter = 1;
        let newPath = tab.path + '\\' + name;

        while (true) {
            try {
                await invoke('create_empty_file', { path: newPath });
                break;
            } catch {
                counter++;
                name = `New File (${counter}).txt`;
                newPath = tab.path + '\\' + name;
                if (counter > 100) return;
            }
        }

        this.refreshCurrent();
    }

    refreshCurrent() {
        const tab = this.tabManager.getActiveTab();
        if (tab) this.navigateTo(tab.path);
    }

    async showProperties(item) {
        if (!item?.path) return;
        this.previewPanel.showProperties(item);
    }

    bindEvents() {
        const on = (id, ev, fn) => document.getElementById(id)?.addEventListener(ev, fn);
        on('btnBack', 'click', () => this.goBack());
        on('btnForward', 'click', () => this.goForward());
        on('btnUp', 'click', () => this.goUp());
        on('viewGrid', 'click', () => this.setView('grid'));
        on('viewList', 'click', () => this.setView('list'));
        on('btnPreview', 'click', () => {
            document.getElementById('previewPanel')?.classList.toggle('collapsed');
        });
        on('btnRefresh', 'click', () => this.refreshCurrent());
        on('tabAdd', 'click', () => {
            this.tabManager.addTab('::MY_PC::');
        });

        const fileHeader = document.getElementById('fileHeader');
        fileHeader?.addEventListener('click', (e) => {
            const colEl = e.target.closest('[data-sort]');
            if (!colEl) return;
            const col = colEl.dataset.sort;
            this.setSorting(col);
        });

        // Enter opens the selected item (folder stays in-shell).
        document.addEventListener('keydown', (e) => {
            const overlay = document.querySelector('.explorer-overlay');
            if (overlay && !overlay.classList.contains('visible')) return;
            if (e.key !== 'Enter') return;
            if (e.target?.closest('input, textarea, [contenteditable]')) return;
            const path = [...this.selectedFiles][0];
            if (!path) return;
            e.preventDefault();
            const tab = this.tabManager.getActiveTab();
            const fromTab = tab?.items?.find((i) => i.path === path);
            const originEl = document.querySelector(`.file-item[data-path="${CSS.escape(path)}"], .file-row[data-path="${CSS.escape(path)}"]`);
            this.openItem(fromTab || { path, is_dir: false, name: path.split(/[\\/]/).pop() }, originEl);
        });

        const searchInput = document.getElementById('searchInput');
        let searchTimeout;
        searchInput?.addEventListener('input', () => {
            clearTimeout(searchTimeout);
            searchTimeout = setTimeout(() => {
                this.handleSearch(searchInput.value);
            }, 300);
        });

        searchInput?.addEventListener('keydown', (e) => {
            if (e.key === 'Escape') {
                searchInput.value = '';
                document.getElementById('searchResults')?.classList.add('hidden');
            }
        });

        document.getElementById('fileArea')?.addEventListener('contextmenu', (e) => {
            if (!e.target.closest('.file-item, .file-row')) {
                e.preventDefault();
                e.stopPropagation();
                this.contextMenu.show(e.clientX, e.clientY, 'empty', null);
            }
        });

        document.addEventListener('pointerdown', (e) => {
            if (!e.target.closest('.context-menu')) {
                this.contextMenu.hide();
            }
        }, true);

        document.addEventListener('click', (e) => {
            if (!e.target.closest('.context-menu')) {
                this.contextMenu.hide();
            }
        });

        document.addEventListener('keydown', (e) => {
            if (e.target.tagName === 'INPUT' || e.target.tagName === 'TEXTAREA') return;

            if (e.key === 'Delete') this.deleteSelected();
            if (e.key === 'F2') this.renameSelected();
            if (e.ctrlKey && e.key === 'c') this.copySelected();
            if (e.ctrlKey && e.key === 'x') this.cutSelected();
            if (e.ctrlKey && e.key === 'v') this.pasteFiles();
            if (e.ctrlKey && e.key === 'a') this.selectAll();
            if (e.key === 'Backspace') this.goUp();
            if (e.altKey && e.key === 'ArrowLeft') this.goBack();
            if (e.altKey && e.key === 'ArrowRight') this.goForward();
            if (e.key === 'F5') this.refreshCurrent();
        });

        document.addEventListener('keydown', (e) => {
            if (e.key === 'Escape') {
                this.contextMenu.hide();
                this.deselectAll();
            }
        });

        this.setupMarqueeSelection();
    }

    setupMarqueeSelection() {
        const fileArea = document.getElementById('fileArea');
        if (!fileArea || fileArea._marqueeReady) return;
        fileArea._marqueeReady = true;

        let startClientX = 0;
        let startClientY = 0;
        let startContentX = 0;
        let startContentY = 0;
        let isDragging = false;
        let rectEl = null;
        let cachedItems = [];
        let startedWithSelectionModifier = false;

        const isInsideFileArea = (clientX, clientY) => {
            const rect = fileArea.getBoundingClientRect();
            const left = rect.left + fileArea.clientLeft;
            const top = rect.top + fileArea.clientTop;
            return clientX >= left && clientX < left + fileArea.clientWidth &&
                clientY >= top && clientY < top + fileArea.clientHeight;
        };

        const isEmptyAreaTarget = (target) => {
            if (!(target instanceof Element) || !fileArea.contains(target)) return false;
            // Only the actual blank surfaces count. Headers, section labels, rows,
            // and controls should keep their normal click behavior.
            if (target.closest('.file-item, .file-row, .file-header, .section-header, button, input, select, textarea, a, [role="button"], [data-sort], .context-menu, .tab, .sidebar, .toolbar, .path-bar')) {
                return false;
            }
            return target === fileArea || !!target.closest('.file-grid, .file-list-body, .empty-state');
        };

        const hasSelectionModifier = (e) => e.ctrlKey || e.metaKey || e.shiftKey;

        const onPointerDown = (e) => {
            if (e.button !== 0) return;
            // Start a marquee only from a blank surface inside the content box.
            // client coordinates are relative to the viewport; offsetX/Y are
            // relative to the event target and can reject valid clicks in gaps.
            if (!isInsideFileArea(e.clientX, e.clientY) || !isEmptyAreaTarget(e.target)) return;

            startClientX = e.clientX;
            startClientY = e.clientY;
            startedWithSelectionModifier = hasSelectionModifier(e);
            const areaRect = fileArea.getBoundingClientRect();
            startContentX = e.clientX - areaRect.left + fileArea.scrollLeft;
            startContentY = e.clientY - areaRect.top + fileArea.scrollTop;
            isDragging = false;

            window.addEventListener('pointermove', onPointerMove, { passive: true });
            window.addEventListener('pointerup', onPointerUp, { once: true });
        };

        const onPointerMove = (e) => {
            const dx = e.clientX - startClientX;
            const dy = e.clientY - startClientY;
            const distSq = dx * dx + dy * dy;

            if (!isDragging) {
                if (distSq < 16) return; // 4px hysteresis
                isDragging = true;
                if (!e.ctrlKey && !e.metaKey && !e.shiftKey) {
                    this.deselectAll();
                }

                // Cache item bounding boxes once at drag start (prevents layout thrashing, ensures 120 FPS)
                const domItems = fileArea.querySelectorAll('.file-item, .file-row');
                const areaRect = fileArea.getBoundingClientRect();
                cachedItems = Array.from(domItems).map(el => {
                    const r = el.getBoundingClientRect();
                    return {
                        el,
                        path: el.dataset.path,
                        left: r.left - areaRect.left + fileArea.scrollLeft,
                        top: r.top - areaRect.top + fileArea.scrollTop,
                        right: r.right - areaRect.left + fileArea.scrollLeft,
                        bottom: r.bottom - areaRect.top + fileArea.scrollTop,
                    };
                });

                if (!rectEl) {
                    rectEl = document.createElement('div');
                    rectEl.className = 'selection-rect';
                    fileArea.appendChild(rectEl);
                }
                rectEl.style.display = 'block';
                rectEl.style.opacity = '1';
            }

            const areaRect = fileArea.getBoundingClientRect();
            const currentContentX = e.clientX - areaRect.left + fileArea.scrollLeft;
            const currentContentY = e.clientY - areaRect.top + fileArea.scrollTop;

            const boxLeft = Math.min(startContentX, currentContentX);
            const boxTop = Math.min(startContentY, currentContentY);
            const boxWidth = Math.abs(currentContentX - startContentX);
            const boxHeight = Math.abs(currentContentY - startContentY);

            rectEl.style.left = boxLeft + 'px';
            rectEl.style.top = boxTop + 'px';
            rectEl.style.width = boxWidth + 'px';
            rectEl.style.height = boxHeight + 'px';

            // Hit test against cached bounding boxes
            const boxRight = boxLeft + boxWidth;
            const boxBottom = boxTop + boxHeight;

            for (const item of cachedItems) {
                const intersects = !(item.left > boxRight || item.right < boxLeft || item.top > boxBottom || item.bottom < boxTop);
                if (intersects) {
                    if (item.path) this.selectedFiles.add(item.path);
                    item.el.classList.add('selected');
                } else if (!e.ctrlKey && !e.metaKey) {
                    if (item.path) this.selectedFiles.delete(item.path);
                    item.el.classList.remove('selected');
                }
            }
            this.updateStatusBar(this.tabManager.getActiveTab()?.items || []);
        };

        const onPointerUp = (e) => {
            window.removeEventListener('pointermove', onPointerMove);
            if (!isDragging) {
                // Clear only when both ends of the gesture remain on blank space.
                // Releasing over a row/control or outside the viewport is not an
                // empty-space click.
                if (!startedWithSelectionModifier && !hasSelectionModifier(e) &&
                    isInsideFileArea(e.clientX, e.clientY) && isEmptyAreaTarget(e.target)) {
                    this.deselectAll();
                }
                startedWithSelectionModifier = false;
                return;
            }
            isDragging = false;
            startedWithSelectionModifier = false;
            cachedItems = [];
            if (rectEl) {
                rectEl.style.display = 'none';
            }
            // Update preview panel if single file is selected
            if (this.selectedFiles.size === 1) {
                const selectedPath = [...this.selectedFiles][0];
                const activeItems = this.tabManager.getActiveTab()?.items || [];
                const singleItem = activeItems.find(i => i.path === selectedPath);
                if (singleItem) {
                    this.previewPanel.update(singleItem);
                }
            }
        };

        fileArea.addEventListener('pointerdown', onPointerDown);
    }

    goBack() {
        const tab = this.tabManager.getActiveTab();
        if (tab && tab.historyIndex > 0) {
            tab.historyIndex--;
            this.navigateToSilent(tab.history[tab.historyIndex], 'back');
        }
    }

    goForward() {
        const tab = this.tabManager.getActiveTab();
        if (tab && tab.historyIndex < tab.history.length - 1) {
            tab.historyIndex++;
            this.navigateToSilent(tab.history[tab.historyIndex], 'forward');
        }
    }

    goUp() {
        const tab = this.tabManager.getActiveTab();
        if (!tab || tab.path === '::MY_PC::') return;

        const parts = tab.path.split(/[\\/]/).filter(Boolean);
        if (parts.length > 1) {
            parts.pop();
            let parentPath = parts.join('\\');
            if (/^[A-Za-z]:$/.test(parentPath)) {
                parentPath += '\\';
            }
            this.navigateTo(parentPath, null, 'up');
        } else {
            this.navigateTo('::MY_PC::', null, 'up');
        }
    }

    async navigateToSilent(path, transitionType = null, origin = null) {
        if (typeof path === 'string' && /^[A-Za-z]:$/.test(path.trim())) {
            path = path.trim() + '\\';
        }
        const fileArea = document.getElementById('fileArea');
        if (path === '::MY_PC::') {
            const tab = this.tabManager.getActiveTab();
            if (tab) {
                tab.path = path;
                tab.items = [];
                if (fileArea && transitionType) {
                    this.playTabTransition(transitionType, () => {
                        this.renderMyPc();
                    }, origin);
                } else {
                    this.renderMyPc();
                }
                this.updatePathBar(path);
                this.updateNavButtons();
                this.updateStatusBar([]);
            }
            return;
        }

        const tab = this.tabManager.getActiveTab();
        // Fast path: if tab already has cached items, transition immediately!
        if (tab && Array.isArray(tab.items) && tab.items.length > 0 && tab.path === path) {
            if (tab._sortCol !== this.sortColumn || tab._sortAsc !== this.sortAscending) {
                tab.items = this.sortItems(tab.items, this.sortColumn, this.sortAscending);
                tab._sortCol = this.sortColumn;
                tab._sortAsc = this.sortAscending;
            }
            this.updateSortHeader();
            if (fileArea && transitionType) {
                this.playTabTransition(transitionType, () => {
                    this.renderFiles(tab.items);
                    if (fileArea && typeof tab.scrollTop === 'number') this._restoreScroll(fileArea, tab.scrollTop);
                }, origin);
            } else {
                this.renderFiles(tab.items);
                if (fileArea && typeof tab.scrollTop === 'number') this._restoreScroll(fileArea, tab.scrollTop);
            }
            this.updatePathBar(path);
            this.updateNavButtons();
            this.updateStatusBar(tab.items);
            // Revalidate in background without blocking UI or showing loader
            this.scanDirectory(path).then(items => {
                const currentTab = this.tabManager.getActiveTab();
                if (currentTab && currentTab.path === path) {
                    const filtered = items.filter(i => String(i?.name || '').toLowerCase() !== 'desktop.ini');
                    currentTab.items = this.sortItems(filtered, this.sortColumn, this.sortAscending);
                    currentTab._sortCol = this.sortColumn;
                    currentTab._sortAsc = this.sortAscending;
                    this.renderFiles(currentTab.items);
                    this.updateStatusBar(currentTab.items);
                }
            }).catch(() => {});
            return;
        }

        this.showLoading(true);
        try {
            const items = await this.scanDirectory(path);
            const activeTab = this.tabManager.getActiveTab();
            if (activeTab) {
                const filtered = items.filter(i => String(i?.name || '').toLowerCase() !== 'desktop.ini');
                activeTab.path = path;
                activeTab.items = this.sortItems(filtered, this.sortColumn, this.sortAscending);
                activeTab._sortCol = this.sortColumn;
                activeTab._sortAsc = this.sortAscending;
                this.updateSortHeader();
                if (fileArea && transitionType) {
                    this.playTabTransition(transitionType, () => {
                        this.renderFiles(activeTab.items);
                        if (fileArea && typeof activeTab.scrollTop === 'number') fileArea.scrollTop = activeTab.scrollTop;
                    }, origin);
                } else {
                    this.renderFiles(activeTab.items);
                    if (fileArea && typeof activeTab.scrollTop === 'number') fileArea.scrollTop = activeTab.scrollTop;
                }
                this.updatePathBar(path);
                this.updateNavButtons();
                this.updateStatusBar(activeTab.items);
            }
        } catch (e) {
            console.error('Navigation error:', e);
        } finally {
            this.showLoading(false);
        }
    }

    async handleSearch(query) {
        const container = document.getElementById('searchResults');
        if (!query || query.length < 2) {
            container.classList.add('hidden');
            return;
        }

        // Sequence token — only the latest query may render its results.
        const searchToken = (this._searchToken || 0) + 1;
        this._searchToken = searchToken;
        container.classList.remove('hidden');

        try {
            const results = await this.searchFiles(query);
            if (this._searchToken !== searchToken) return;
            this.renderSearchResults(results);
        } catch (e) {
            console.error('Search error:', e);
        }
    }

    renderSearchResults(results) {
        const container = document.getElementById('searchResults');
        if (results.length === 0) {
            container.innerHTML = '<div class="search-no-results">No results found</div>';
        } else {
            container.innerHTML = results.slice(0, 30).map(r => `
                <div class="search-result-item" data-path="${r.path}">
                    <span class="search-result-name">${r.name}</span>
                    <span class="search-result-path">${r.path}</span>
                </div>
            `).join('');

            container.querySelectorAll('.search-result-item').forEach(el => {
                el.addEventListener('click', () => {
                    const path = el.dataset.path;
                    const parts = path.split(/[\\/]/).filter(Boolean);
                    parts.pop();
                    let parentPath = parts.join('\\');
                    if (/^[A-Za-z]:$/.test(parentPath)) {
                        parentPath += '\\';
                    }
                    if (parentPath) {
                        this.navigateTo(parentPath);
                    }
                    container.classList.add('hidden');
                    document.getElementById('searchInput').value = '';
                });
            });
        }
        container.classList.remove('hidden');
    }

    async deleteSelected() {
        const paths = [...this.selectedFiles];
        if (paths.length === 0) return;

        for (const p of paths) {
            try {
                await invoke('delete_to_recycle_bin', { path: p });
            } catch (e) {
                console.error('Delete error:', e);
            }
        }

        this.selectedFiles.clear();
        this.refreshCurrent();
    }

    async renameSelected() {
        if (this.selectedFiles.size !== 1) return;
        const path = [...this.selectedFiles][0];
        const oldName = path.split(/[\\/]/).pop();

        // Locate DOM element for this file
        const itemEl = document.querySelector(`.file-item[data-path="${CSS.escape(path)}"], .file-row[data-path="${CSS.escape(path)}"]`);
        const labelEl = itemEl ? itemEl.querySelector('.file-name, .row-name') : null;

        if (!labelEl) {
            return;
        }

        // Inline rename input
        const input = document.createElement('input');
        input.type = 'text';
        input.className = 'explorer-inline-rename';
        input.value = oldName;
        input.style.width = '100%';
        input.style.background = 'rgba(var(--bg-elev-rgb), 0.15)';
        input.style.color = 'rgb(var(--fg-rgb))';
        input.style.border = '1px solid rgba(var(--bg-elev-rgb), 0.4)';
        input.style.borderRadius = '4px';
        input.style.padding = '2px 4px';
        input.style.fontSize = 'inherit';
        input.style.fontFamily = 'inherit';
        input.style.outline = 'none';

        const originalDisplay = labelEl.style.display;
        labelEl.style.display = 'none';
        labelEl.parentNode.insertBefore(input, labelEl.nextSibling);

        // Select basename excluding extension
        const dotIdx = oldName.lastIndexOf('.');
        input.focus();
        if (dotIdx > 0) {
            input.setSelectionRange(0, dotIdx);
        } else {
            input.select();
        }

        let finished = false;
        const finish = async (commit) => {
            if (finished) return;
            finished = true;
            const newName = input.value.trim();
            input.remove();
            labelEl.style.display = originalDisplay;

            if (commit && newName && newName !== oldName) {
                const dir = path.split(/[\\/]/).slice(0, -1).join('\\');
                const newPath = dir + '\\' + newName;
                try {
                    await invoke('rename_path', { from: path, to: newPath });
                    this.selectedFiles.clear();
                    this.refreshCurrent();
                } catch (e) {
                    console.error('Rename error:', e);
                }
            }
        };

        input.addEventListener('keydown', (e) => {
            e.stopPropagation();
            if (e.key === 'Enter') {
                finish(true);
            } else if (e.key === 'Escape') {
                finish(false);
            }
        });

        input.addEventListener('blur', () => {
            finish(true);
        });
    }

    _getUniqueCopyPath(parentDir, originalName) {
        const dotIdx = originalName.lastIndexOf('.');
        let baseName = originalName;
        let ext = '';
        if (dotIdx > 0) {
            baseName = originalName.substring(0, dotIdx);
            ext = originalName.substring(dotIdx);
        }
        const cleanParent = parentDir.replace(/\\+$/, '');
        let candidate = `${cleanParent}\\${baseName} - Copy${ext}`;
        const existingNames = new Set(
            (this.tabManager.getActiveTab()?.items || []).map(i => (i.name || '').toLowerCase())
        );
        let counter = 2;
        while (existingNames.has((`${baseName} - Copy${counter > 2 ? ` (${counter})` : ''}${ext}`).toLowerCase())) {
            counter++;
            candidate = `${cleanParent}\\${baseName} - Copy (${counter})${ext}`;
        }
        return candidate;
    }

    copySelected() {
        this.clipboard = { files: [...this.selectedFiles], cut: false };
        if (this.selectedFiles.size > 0) {
            invoke('copy_to_clipboard', { paths: [...this.selectedFiles], cut: false }).catch(() => {});
        }
    }

    cutSelected() {
        this.clipboard = { files: [...this.selectedFiles], cut: true };
        if (this.selectedFiles.size > 0) {
            invoke('copy_to_clipboard', { paths: [...this.selectedFiles], cut: true }).catch(() => {});
        }
    }

    async pasteFiles() {
        if (this.clipboard.files.length === 0) {
            const sysFiles = await invoke('paste_from_clipboard').catch(() => []);
            if (Array.isArray(sysFiles) && sysFiles.length > 0) {
                this.clipboard = { files: sysFiles, cut: false };
            }
        }
        if (this.clipboard.files.length === 0) return;
        const tab = this.tabManager.getActiveTab();
        if (!tab) return;

        for (const src of this.clipboard.files) {
            const name = src.split(/[\\/]/).pop();
            const cleanParent = tab.path.replace(/\\+$/, '');
            let dest = cleanParent + '\\' + name;

            if (!this.clipboard.cut && src.toLowerCase() === dest.toLowerCase()) {
                dest = this._getUniqueCopyPath(tab.path, name);
            }

            try {
                if (this.clipboard.cut) {
                    await invoke('rename_path', { from: src, to: dest });
                } else {
                    await invoke('copy_path', { from: src, to: dest });
                }
            } catch (e) {
                console.error('Paste error:', e);
            }
        }

        if (this.clipboard.cut) {
            this.clipboard = { files: [], cut: false };
        }

        this.refreshCurrent();
    }

    selectAll() {
        const tab = this.tabManager.getActiveTab();
        if (!tab) return;

        this.selectedFiles.clear();
        document.querySelectorAll('.file-item, .file-row').forEach(el => {
            el.classList.add('selected');
            this.selectedFiles.add(el.dataset.path);
        });
    }
}

// Overlay injects this module after DOMContentLoaded has already fired,
// so boot immediately when the document is already interactive.
async function bootExplorer() {
    // Do not use window.explorer for application state. The element whose id
    // is "explorer" is exposed by WebView2 as window.explorer, so the previous
    // truthy guard mistook that HTMLElement for an initialized controller and
    // skipped the entire boot sequence.
    if (window.nothingExplorer) {
        try { await window.nothingExplorer.ready; } catch {}
        window.dispatchEvent(new CustomEvent('explorer-ready'));
        return;
    }
    const explorer = new NothingExplorer();
    window.nothingExplorer = explorer;
    try {
        await explorer.ready;
    } catch (err) {
        console.error('explorer init failed:', err);
        const empty = document.getElementById('emptyState');
        const txt = empty?.querySelector('.empty-text');
        if (empty && txt) {
            empty.classList.remove('hidden');
            txt.textContent = String(err?.message || err);
        }
    }
    window.dispatchEvent(new CustomEvent('explorer-ready'));
}

if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', () => { bootExplorer(); });
} else {
    bootExplorer();
}
