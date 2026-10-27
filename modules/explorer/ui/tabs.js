import { invoke } from '@unidtk/bridge';

export class TabManager {
    constructor(explorer) {
        this.explorer = explorer;
        this.tabs = [];
        this.activeTabId = null;
        this.tabCounter = 0;
        this._restored = false;
    }

    /**
     * Persist current tabs + active tab to the unified redb (EXPLORER_TABS),
     * so open tabs survive app restarts and are CLI-queryable.
     */
    async _saveTabs() {
        // Don't persist before restore has finished: a startup addTab (from
        // openExplorer) would otherwise overwrite the saved tabs with the
        // bootstrap state before restoreTabs() had a chance to read them.
        if (!this._restored) return;
        try {
            const tabs = this.tabs.map(t => [t.path, t.title]);
            await invoke('explorer_tabs_set', { tabs });
            try { await invoke('frontend_log', { message: `explorer save tabs (${tabs.length}): ` + tabs.map(t=>t[0]).join(' | ') }); } catch {}
        } catch (e) {
            // Fail silently if the command is unavailable.
        }
    }

    /**
     * Restore tabs from redb. Returns true if tabs were restored.
     * Call once after the explorer is ready.
     */
    async restoreTabs() {
        if (this._restored) return false;
        this._restored = true;

        let saved;
        try {
            saved = await invoke('explorer_tabs_get');
        } catch (e) {
            return false;
        }
        // Backend returns [[path, title], ...]
        if (!Array.isArray(saved) || saved.length === 0) return false;

        // Clear the default "This PC" tab that addTab() already created.
        this.tabs = [];
        this.activeTabId = null;
        this.tabCounter = 0;

        for (const entry of saved) {
            const path = Array.isArray(entry) ? entry[0] : entry?.path;
            const title = Array.isArray(entry) ? entry[1] : entry?.title;
            if (!path) continue;
            this.tabCounter++;
            const tab = {
                id: this.tabCounter,
                path: String(path),
                title: title || (String(path) === '::MY_PC::' ? 'This PC' : this.getFolderName(String(path))),
                history: [String(path)],
                historyIndex: 0,
                items: [],
            };
            this.tabs.push(tab);
        }

        if (this.tabs.length === 0) return false;

        // If no active tab matched, activate the last one.
        if (this.activeTabId === null) {
            this.activeTabId = this.tabs[this.tabs.length - 1].id;
        }

        this.renderTabs();
        return true;
    }

    get container() {
        return document.getElementById('tabsContainer');
    }

    getActiveTab() {
        return this.tabs.find(t => t.id === this.activeTabId) || null;
    }

    async addTab(path) {
        let target = !path || !String(path).trim() ? '::MY_PC::' : String(path).trim();
        if (/^[A-Za-z]:$/.test(target)) {
            target += '\\';
        }
        this.tabCounter++;
        const tab = {
            id: this.tabCounter,
            path: target,
            title: target === '::MY_PC::' ? 'This PC' : this.getFolderName(target),
            history: [target],
            historyIndex: 0,
            items: [],
        };

        this.tabs.push(tab);
        this.renderTabs(tab.id);
        // Persist the newly added tab immediately.
        this._saveTabs();
        // switchTab re-renders tabs; re-tag the new tab so its enter animation
        // survives that re-render.
        await this.switchTab(tab.id);
        this.retagEnter(tab.id);
    }

    async switchTab(tabId, originTabEl = null) {
        const prevIdx = this.tabs.findIndex(t => t.id === this.activeTabId);
        if (prevIdx >= 0) {
            const prevTab = this.tabs[prevIdx];
            const fileArea = document.getElementById('fileArea');
            if (fileArea && prevTab) {
                prevTab.scrollTop = fileArea.scrollTop;
            }
        }
        const nextIdx = this.tabs.findIndex(t => t.id === tabId);
        const transitionType = prevIdx >= 0 && nextIdx >= 0 && prevIdx !== nextIdx
            ? (nextIdx > prevIdx ? 'tab-forward' : 'tab-back')
            : null;

        // Captured before renderTabs() replaces the clicked element: the file area
        // morphs out of this tab's box.
        const origin = transitionType && originTabEl instanceof HTMLElement
            ? {
                rect: originTabEl.getBoundingClientRect(),
                radius: getComputedStyle(originTabEl).borderRadius,
                node: originTabEl.cloneNode(true),
            }
            : null;

        this.activeTabId = tabId;
        this.renderTabs();

        const tab = this.getActiveTab();
        if (!tab) return;

        if (tab.path === '::MY_PC::') {
            await this.showThisPC(transitionType, origin);
        } else {
            await this.explorer.navigateToSilent(tab.path, transitionType, origin);
        }
        await this._saveTabs();
    }

    async closeTab(tabId) {
        const index = this.tabs.findIndex(t => t.id === tabId);
        if (index === -1) return;

        this.tabs.splice(index, 1);

        if (this.tabs.length === 0) {
            await this.addTab('::MY_PC::');
            return;
        }

        if (this.activeTabId === tabId) {
            const newIndex = Math.min(index, this.tabs.length - 1);
            await this.switchTab(this.tabs[newIndex].id);
        } else {
            this.renderTabs();
            await this._saveTabs();
        }
    }

    renderTabs(newTabId = null) {
        const container = this.container;
        if (!container) return;
        container.innerHTML = '';

        this.tabs.forEach(tab => {
            const el = document.createElement('div');
            el.className = `tab${tab.id === this.activeTabId ? ' active' : ''}`;
            el.dataset.tabId = tab.id;

            el.innerHTML = `
                <svg class="tab-icon" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5">
                    <path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"/>
                </svg>
                <span class="tab-title" aria-label="${tab.path}">${tab.title}</span>
                <button class="tab-close" aria-label="Close tab">
                    <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                        <line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/>
                    </svg>
                </button>
            `;

            el.addEventListener('click', (e) => {
                if (!e.target.closest('.tab-close')) {
                    this.switchTab(tab.id, el);
                }
            });

            el.querySelector('.tab-close').addEventListener('click', (e) => {
                e.stopPropagation();
                this.closeTab(tab.id);
            });

            container.appendChild(el);
        });

        // Enter animation — applied AFTER the tabs are in the DOM so the
        // keyframe plays once for the newly added tab, and survives the
        // renderTabs() call inside switchTab().
        if (newTabId != null) {
            const el = container.querySelector(`[data-tab-id="${newTabId}"]`);
            if (el) {
                el.classList.add('tab-enter');
                el.addEventListener('animationend', () => {
                    el.classList.remove('tab-enter');
                }, { once: true });
            }
        }
    }

    // Re-add the enter animation after a re-render (e.g. switchTab's renderTabs
    // wiped the freshly added tab's animation class).
    retagEnter(tabId) {
        const container = this.container;
        if (!container) return;
        const el = container.querySelector(`[data-tab-id="${tabId}"]`);
        if (el) {
            el.classList.add('tab-enter');
            el.addEventListener('animationend', () => {
                el.classList.remove('tab-enter');
            }, { once: true });
        }
    }

    async showThisPC(transitionType = null, origin = null) {
        const tab = this.getActiveTab();
        if (tab) {
            tab.path = '::MY_PC::';
            tab.items = [];
        }
        await this.explorer.getDrives().catch(() => {});
        const fileArea = document.getElementById('fileArea');
        if (fileArea && transitionType && typeof this.explorer.playTabTransition === 'function') {
            this.explorer.playTabTransition(transitionType, () => {
                this.explorer.renderMyPc();
            }, origin);
        } else {
            this.explorer.renderMyPc();
            if (fileArea && transitionType && typeof this.explorer._playTransition === 'function') {
                this.explorer._playTransition(fileArea, transitionType);
            }
        }
        this.explorer.updatePathBar('::MY_PC::');
        this.explorer.updateNavButtons();
        this.explorer.updateStatusBar([]);
        this.explorer.previewPanel?.clear();
    }

    getFolderName(path) {
        if (path === '::MY_PC::') return 'This PC';
        const parts = path.split(/[\\/]/).filter(Boolean);
        if (parts.length === 1 && /^[A-Za-z]:$/.test(parts[0])) {
            return `${parts[0]}\\`;
        }
        return parts[parts.length - 1] || path;
    }

    updateActiveTabTitle(title) {
        const tab = this.getActiveTab();
        if (tab) {
            tab.title = title;
            this.renderTabs();
        }
    }
}
