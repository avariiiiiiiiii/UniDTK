import { explorerAsset } from './asset.js';
import { surfaceManager } from '../../surface-manager.js';

export class ContextMenu {
    constructor(explorer) {
        this.explorer = explorer;
        this.element = document.getElementById('contextMenu');
        this.currentTarget = null;
        this.currentItem = null;
        this.hideOnClick = true;
    }

    show(x, y, targetType, item) {
        this.currentTarget = targetType;
        this.currentItem = item;
        this.render(targetType, item);

        if (!this.element) {
            this.element = document.getElementById('contextMenu');
            if (!this.element) return;
        }

        // Cancel any pending fade-out before re-showing.
        if (this._hideTimer) clearTimeout(this._hideTimer);
        this.element.classList.remove('closing', 'hidden');
        this.element.style.visibility = 'hidden';
        this.element.style.display = 'block';

        const menuRect = this.element.getBoundingClientRect();
        const menuW = menuRect.width || 240;
        const menuH = menuRect.height || 180;

        const container = document.getElementById('explorer') || document.getElementById('explorerContainer') || document.body;
        const contRect = container.getBoundingClientRect();

        // Convert viewport (x, y) to container-relative coordinates
        const localX = x - contRect.left;
        const localY = y - contRect.top;

        // Container boundary padding
        const minX = 8;
        const maxX = Math.max(minX, contRect.width - 8);
        const minY = 8;
        const maxY = Math.max(minY, contRect.height - 8);

        const spaceRight = maxX - localX;
        const spaceLeft = localX - minX;
        const spaceBelow = maxY - localY;
        const spaceAbove = localY - minY;

        let posX = localX;
        let posY = localY;
        let originX = 'left';
        let originY = 'top';

        // Horizontal: open right if space available, otherwise open left
        if (spaceRight >= menuW) {
            posX = localX;
            originX = 'left';
        } else if (spaceLeft >= menuW || spaceLeft > spaceRight) {
            posX = localX - menuW;
            originX = 'right';
        } else {
            posX = maxX - menuW;
            originX = 'right';
        }

        // Vertical: open downwards if space available, otherwise open upwards
        if (spaceBelow >= menuH) {
            posY = localY;
            originY = 'top';
        } else if (spaceAbove >= menuH || spaceAbove > spaceBelow) {
            posY = localY - menuH;
            originY = 'bottom';
        } else {
            posY = maxY - menuH;
            originY = 'bottom';
        }

        // Ensure clamped within visible bounds
        posX = Math.max(minX, Math.min(posX, Math.max(minX, maxX - menuW)));
        posY = Math.max(minY, Math.min(posY, Math.max(minY, maxY - menuH)));

        this.element.style.left = Math.round(posX) + 'px';
        this.element.style.top = Math.round(posY) + 'px';
        this.element.style.transformOrigin = `${originY} ${originX}`;
        this.element.style.visibility = '';

        surfaceManager.openSurface(this.element, {
            origin: { left: x, top: y, width: 4, height: 4 },
            originRadius: "8px",
            elevation: "var(--z-surface-menu, 4000)",
            display: "block",
            onClosed: () => {
                if (this.element) {
                    this.element.classList.add('hidden');
                    this.element.style.display = 'none';
                }
            }
        });
    }

    hide() {
        if (!this.element || this.element.classList.contains('hidden')) return;
        if (surfaceManager.isOpen(this.element)) {
            surfaceManager.closeSurface(this.element);
            return;
        }
        this.element.classList.add('hidden');
        this.element.style.display = 'none';
    }

    render(type, item) {
        const isDir = item?.is_dir || item?.isDir;

        if (type === 'file') {
            if (isDir) {
                this.element.innerHTML = `
                    <div class="cm-row">
                        <div class="cm-item" data-action="open">
                            <img src="${explorerAsset('icons/ui/cm-open.svg')}" class="cm-icon" alt="">
                            <span class="cm-label">Open</span>
                        </div>
                        <div class="cm-item" data-action="open-terminal">
                            <img src="${explorerAsset('icons/ui/cm-terminal.svg')}" class="cm-icon" alt="">
                            <span class="cm-label">Terminal</span>
                        </div>
                        <div class="cm-item" data-action="copy-path">
                            <img src="${explorerAsset('icons/ui/cm-copy-path.svg')}" class="cm-icon" alt="">
                            <span class="cm-label">Copy Path</span>
                        </div>
                    </div>
                    <div class="cm-separator"></div>
                    <div class="cm-row">
                        <div class="cm-item" data-action="copy">
                            <img src="${explorerAsset('icons/ui/cm-copy.svg')}" class="cm-icon" alt="">
                            <span class="cm-label">Copy</span>
                        </div>
                        <div class="cm-item" data-action="cut">
                            <img src="${explorerAsset('icons/ui/cm-cut.svg')}" class="cm-icon" alt="">
                            <span class="cm-label">Cut</span>
                        </div>
                        <div class="cm-item" data-action="delete">
                            <img src="${explorerAsset('icons/ui/cm-delete.svg')}" class="cm-icon" alt="">
                            <span class="cm-label">Delete</span>
                        </div>
                        <div class="cm-item" data-action="rename">
                            <img src="${explorerAsset('icons/ui/cm-rename.svg')}" class="cm-icon" alt="">
                            <span class="cm-label">Rename</span>
                        </div>
                    </div>
                `;
            } else {
                this.element.innerHTML = `
                    <div class="cm-row">
                        <div class="cm-item" data-action="open">
                            <img src="${explorerAsset('icons/ui/cm-open.svg')}" class="cm-icon" alt="">
                            <span class="cm-label">Open</span>
                        </div>
                        <div class="cm-item" data-action="open-with">
                            <img src="${explorerAsset('icons/ui/cm-open-with.svg')}" class="cm-icon" alt="">
                            <span class="cm-label">Open with</span>
                        </div>
                        <div class="cm-item" data-action="copy-path">
                            <img src="${explorerAsset('icons/ui/cm-copy-path.svg')}" class="cm-icon" alt="">
                            <span class="cm-label">Copy Path</span>
                        </div>
                    </div>
                    <div class="cm-separator"></div>
                    <div class="cm-row">
                        <div class="cm-item" data-action="copy">
                            <img src="${explorerAsset('icons/ui/cm-copy.svg')}" class="cm-icon" alt="">
                            <span class="cm-label">Copy</span>
                        </div>
                        <div class="cm-item" data-action="cut">
                            <img src="${explorerAsset('icons/ui/cm-cut.svg')}" class="cm-icon" alt="">
                            <span class="cm-label">Cut</span>
                        </div>
                        <div class="cm-item" data-action="delete">
                            <img src="${explorerAsset('icons/ui/cm-delete.svg')}" class="cm-icon" alt="">
                            <span class="cm-label">Delete</span>
                        </div>
                        <div class="cm-item" data-action="rename">
                            <img src="${explorerAsset('icons/ui/cm-rename.svg')}" class="cm-icon" alt="">
                            <span class="cm-label">Rename</span>
                        </div>
                    </div>
                    <div class="cm-separator"></div>
                    <div class="cm-row">
                        <div class="cm-item" data-action="send">
                            <img src="${explorerAsset('icons/ui/cm-send.svg')}" class="cm-icon" alt="">
                            <span class="cm-label">Send</span>
                        </div>
                    </div>
                `;
            }
        } else {
            this.element.innerHTML = `
                <div class="cm-row">
                    <div class="cm-item" data-action="open-terminal">
                        <img src="${explorerAsset('icons/ui/cm-terminal.svg')}" class="cm-icon" alt="">
                        <span class="cm-label">Terminal</span>
                    </div>
                    <div class="cm-item" data-action="copy-path">
                        <img src="${explorerAsset('icons/ui/cm-copy-path.svg')}" class="cm-icon" alt="">
                        <span class="cm-label">Copy Path</span>
                    </div>
                    <div class="cm-item" data-action="paste">
                        <img src="${explorerAsset('icons/ui/cm-paste.svg')}" class="cm-icon" alt="">
                        <span class="cm-label">Paste</span>
                    </div>
                    <div class="cm-item" data-action="refresh">
                        <img src="${explorerAsset('icons/ui/cm-refresh.svg')}" class="cm-icon" alt="">
                        <span class="cm-label">Refresh</span>
                    </div>
                </div>
                <div class="cm-separator"></div>
                <div class="cm-row">
                    <div class="cm-item" data-action="new-folder">
                        <img src="${explorerAsset('icons/ui/cm-new-folder.svg')}" class="cm-icon" alt="">
                        <span class="cm-label">Folder</span>
                    </div>
                    <div class="cm-item" data-action="new-file">
                        <img src="${explorerAsset('icons/ui/cm-new-file.svg')}" class="cm-icon" alt="">
                        <span class="cm-label">File</span>
                    </div>
                    <div class="cm-item" data-action="select-all">
                        <img src="${explorerAsset('icons/ui/search.svg')}" class="cm-icon" alt="">
                        <span class="cm-label">Select all</span>
                    </div>
                </div>
                <div class="cm-separator"></div>
                <div class="cm-row">
                    <div class="cm-item" data-action="sort-name">
                        <span class="cm-label">Sort: Name</span>
                    </div>
                    <div class="cm-item" data-action="sort-date">
                        <span class="cm-label">Sort: Date</span>
                    </div>
                    <div class="cm-item" data-action="sort-size">
                        <span class="cm-label">Sort: Size</span>
                    </div>
                    <div class="cm-item" data-action="sort-type">
                        <span class="cm-label">Sort: Type</span>
                    </div>
                </div>
            `;
        }

        this.element.querySelectorAll('.cm-item').forEach(itemEl => {
            itemEl.addEventListener('click', (e) => {
                e.stopPropagation();
                const action = itemEl.dataset.action;
                this.explorer.handleContextMenuAction(action, this.currentItem);
                this.hide();
            });
        });
    }
}
