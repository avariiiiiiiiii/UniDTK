import { convertFileSrc, invoke } from '@unidtk/bridge';
import { explorerAsset } from './asset.js';

export class PreviewPanel {
    constructor(explorer) {
        this.explorer = explorer;
        this.currentFile = null;
        this.container = document.getElementById('previewContent');
        this.placeholder = document.getElementById('previewPlaceholder');
    }

    showFile(item) {
        this.currentFile = item;
        this.placeholder.classList.add('hidden');
        this.container.classList.remove('hidden');

        document.getElementById('previewFileName').textContent = item.name;

        const imgContainer = document.getElementById('previewImageContainer');
        const ext = item.name.split('.').pop()?.toLowerCase();
        const imageExts = ['jpg', 'jpeg', 'png', 'gif', 'bmp', 'svg', 'webp', 'ico', 'avif'];

        if (imageExts.includes(ext) && !item.is_dir) {
            imgContainer.innerHTML = `<img src="${convertFileSrc(item.path)}" class="preview-image" alt="">`;
            imgContainer.classList.remove('hidden');
        } else {
            imgContainer.innerHTML = `<img src="${this.explorer.fileIconSrc(item)}" class="preview-type-icon" alt="">`;
            imgContainer.classList.remove('hidden');
        }

        document.getElementById('metaType').textContent = item.is_dir ? 'Folder' : this.explorer.getFileTypeName(item.path);
        document.getElementById('metaSize').textContent = item.is_dir ? '—' : this.explorer.formatSize(item.file_size || 0);
        document.getElementById('metaModified').textContent = item.modified_time || '—';
        document.getElementById('metaCreated').textContent = item.created_time || '—';

        const hashes = document.getElementById('previewHashes');
        if (hashes) {
            hashes.classList.toggle('hidden', !!item.is_dir);
            hashes.querySelector('.hash-result')?.classList.add('hidden');
        }

        document.getElementById('btnHashMd5').onclick = () => this.calculateHash('md5');
        document.getElementById('btnHashSha').onclick = () => this.calculateHash('sha256');
    }

    async calculateHash(type) {
        if (!this.currentFile || this.currentFile.is_dir) return;

        const resultEl = document.getElementById('hashResult');
        resultEl.classList.remove('hidden');
        resultEl.textContent = 'Calculating...';

        try {
            let hash;
            if (type === 'md5') {
                hash = await invoke('hash_file_md5', { path: this.currentFile.path });
            } else {
                hash = await invoke('hash_file_sha256', { path: this.currentFile.path });
            }
            resultEl.textContent = `${type.toUpperCase()}: ${hash}`;
        } catch (e) {
            resultEl.textContent = 'Error calculating hash';
        }
    }

    clear() {
        this.currentFile = null;
        this.container.classList.add('hidden');
        this.placeholder.classList.remove('hidden');
    }

    toggle() {
        const panel = document.getElementById('previewPanel');
        panel.classList.toggle('collapsed');
    }
}
