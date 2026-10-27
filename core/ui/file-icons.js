/**
 * Shared File & Folder Icon Resolution Module
 * Central repository for file type SVGs and folder icons.
 */

const FILE_ICON_MAP = {
    // Web & Scripting
    'js': 'js.svg', 'mjs': 'js.svg', 'cjs': 'js.svg', 'jsx': 'js.svg',
    'ts': 'ts.svg', 'tsx': 'ts.svg', 'mts': 'ts.svg', 'cts': 'ts.svg',
    'html': 'html.svg', 'htm': 'html.svg', 'vue': 'html.svg', 'svelte': 'html.svg', 'astro': 'html.svg',
    'css': 'css.svg', 'scss': 'css.svg', 'less': 'css.svg', 'sass': 'css.svg', 'styl': 'css.svg',
    'json': 'json.svg', 'jsonc': 'json.svg', 'json5': 'json.svg',
    'xml': 'xml.svg', 'xsl': 'xml.svg', 'xslt': 'xml.svg', 'plist': 'xml.svg',
    'yaml': 'yaml.svg', 'yml': 'yaml.svg', 'toml': 'yaml.svg',

    // Programming Languages
    'py': 'py.svg', 'pyw': 'py.svg',
    'rs': 'rs.svg',
    'go': 'go.svg',
    'cs': 'cs.svg', 'csx': 'cs.svg', 'csproj': 'cs.svg', 'sln': 'cs.svg',
    'c': 'code.svg', 'cc': 'code.svg', 'cpp': 'code.svg', 'cxx': 'code.svg',
    'h': 'code.svg', 'hh': 'code.svg', 'hpp': 'code.svg', 'hxx': 'code.svg',
    'java': 'code.svg', 'kt': 'code.svg', 'kts': 'code.svg',
    'swift': 'code.svg', 'dart': 'code.svg', 'lua': 'code.svg',
    'rb': 'code.svg', 'php': 'code.svg', 'pl': 'code.svg', 'r': 'code.svg',
    'sh': 'dot-bash.svg', 'bash': 'dot-bash.svg', 'zsh': 'dot-zsh.svg',
    'bat': 'txt.svg', 'cmd': 'txt.svg', 'ps1': 'txt.svg',
    'sql': 'json.svg', 'db': 'json.svg', 'sqlite': 'json.svg', 'sqlite3': 'json.svg',
    'gradle': 'code.svg', 'cmake': 'code.svg', 'wasm': 'code.svg',

    // Documents & Text
    'md': 'md.svg', 'mdx': 'md.svg', 'markdown': 'md.svg',
    'pdf': 'pdf.svg',
    'doc': 'doc.svg', 'docx': 'doc.svg', 'odt': 'doc.svg', 'rtf': 'doc.svg',
    'xls': 'xls.svg', 'xlsx': 'xls.svg', 'csv': 'xls.svg', 'tsv': 'xls.svg',
    'ppt': 'ppt.svg', 'pptx': 'ppt.svg', 'odp': 'ppt.svg',
    'txt': 'txt.svg', 'log': 'txt.svg', 'ini': 'txt.svg', 'cfg': 'txt.svg', 'conf': 'txt.svg', 'properties': 'txt.svg',
    'lock': 'txt.svg', 'ttf': 'txt.svg', 'otf': 'txt.svg', 'woff': 'txt.svg', 'woff2': 'txt.svg',

    // Media & Graphics
    'mp3': 'mp3.svg', 'wav': 'mp3.svg', 'flac': 'mp3.svg', 'ogg': 'mp3.svg', 'aac': 'mp3.svg', 'm4a': 'mp3.svg', 'wma': 'mp3.svg', 'opus': 'mp3.svg',
    'mp4': 'mp4.svg', 'avi': 'mp4.svg', 'mkv': 'mp4.svg', 'mov': 'mp4.svg', 'webm': 'mp4.svg', 'flv': 'mp4.svg', 'wmv': 'mp4.svg', 'm4v': 'mp4.svg',
    'jpg': 'jpg.svg', 'jpeg': 'jpg.svg', 'webp': 'jpg.svg', 'avif': 'jpg.svg', 'tiff': 'jpg.svg', 'psd': 'jpg.svg',
    'png': 'png.svg', 'bmp': 'png.svg', 'ico': 'png.svg',
    'svg': 'svg.svg',
    'gif': 'gif.svg', 'apng': 'gif.svg',

    // Archives & Executables
    'zip': 'zip.svg', 'tar': 'zip.svg', 'gz': 'zip.svg', 'bz2': 'zip.svg', 'xz': 'zip.svg', 'tgz': 'zip.svg', 'zst': 'zip.svg', 'iso': 'zip.svg',
    'rar': 'rar.svg', '7z': 'rar.svg',
    'exe': 'exe.svg', 'msi': 'exe.svg', 'dll': 'exe.svg', 'sys': 'exe.svg', 'com': 'exe.svg', 'apk': 'exe.svg',
};

const DOTFILE_ICON_MAP = [
    { prefix: '.env', icon: 'dot-env.svg' },
    { names: ['.gitignore', '.gitattributes', '.gitmodules', '.gitkeep', '.gitconfig'], icon: 'dot-git.svg' },
    { names: ['.editorconfig'], icon: 'dot-editorconfig.svg' },
    { names: ['.npmrc', '.npmignore', '.yarnrc', '.yarnrc.yml', '.pnpmfile.cjs'], icon: 'dot-npm.svg' },
    { prefix: '.prettier', icon: 'dot-prettier.svg' },
    { prefix: '.eslint', icon: 'dot-eslint.svg' },
    { prefix: '.babel', icon: 'dot-babel.svg' },
    { prefix: '.bash', icon: 'dot-bash.svg' },
    { prefix: '.zsh', icon: 'dot-zsh.svg' },
    { prefix: '.docker', names: ['dockerfile'], icon: 'dot-docker.svg' },
    { prefix: '.tauri', names: ['tauri.conf.json'], icon: 'dot-tauri.svg' },
    { prefix: '.claude', names: ['claude.json'], icon: 'dot-claude.svg' },
    { prefix: '.cargo', names: ['cargo.lock', 'cargo.toml'], icon: 'dot-cargo.svg' },
    { prefix: '.cookie', names: ['cookies.txt'], icon: 'dot-cookie.svg' },
    { prefix: '.archive', names: ['archive.txt'], icon: 'dot-archive.svg' },
    { prefix: '.secret', names: ['credentials.json', 'server_pass'], icon: 'dot-secret.svg' },
    { prefix: '.cleanup', icon: 'dot-cleanup.svg' },
    { prefix: '.ai', icon: 'dot-ai.svg' },
];

/**
 * Returns the icon filename for a given file item or filename.
 * @param {string|object} item - Filename string or object with `.name` property.
 * @returns {string} Icon filename, e.g. 'pdf.svg'
 */
export function getFileIconName(item) {
    if (!item) return 'file-generic.svg';
    const name = String(typeof item === 'string' ? item : item.name || '').toLowerCase().trim();
    if (!name) return 'file-generic.svg';

    // Dotfiles and special names
    for (const entry of DOTFILE_ICON_MAP) {
        if (entry.names && entry.names.includes(name)) return entry.icon;
        if (entry.prefix && name.startsWith(entry.prefix)) return entry.icon;
    }

    // Extension match
    const ext = name.split('.').pop() || '';
    return FILE_ICON_MAP[ext] || 'file-generic.svg';
}

/**
 * Returns the icon filename for a folder item or folder name.
 * @param {string|object} item - Folder name string or object with `.name` / `.path`.
 * @returns {string} Folder icon filename, e.g. 'folder-downloads.svg'
 */
export function getFolderIconName(item) {
    const name = String(typeof item === 'string' ? item : item?.name || '').toLowerCase().trim();
    const rawPath = String(typeof item === 'string' ? item : item?.path || '').toLowerCase().trim().replace(/\\/g, '/');

    if (name === 'desktop' || name === 'рабочий стол' || rawPath.endsWith('/desktop')) return 'folder-desktop.svg';
    if (name === 'downloads' || name === 'загрузки' || rawPath.endsWith('/downloads')) return 'folder-downloads.svg';
    if (name === 'documents' || name === 'документы' || name === 'my documents' || rawPath.endsWith('/documents')) return 'folder-documents.svg';
    if (name === 'pictures' || name === 'photos' || name === 'images' || name === 'изображения' || name === 'картинки' || name === 'фото' || name === 'фотографии' || rawPath.endsWith('/pictures') || rawPath.endsWith('/photos')) return 'folder-pictures.svg';
    if (name === 'music' || name === 'audio' || name === 'музыка' || name === 'аудио' || rawPath.endsWith('/music')) return 'folder-music.svg';
    if (name === 'videos' || name === 'movies' || name === 'видео' || name === 'фильмы' || rawPath.endsWith('/videos') || rawPath.endsWith('/movies')) return 'folder-videos.svg';
    if (name === 'recycle bin' || name === 'корзина' || rawPath.endsWith('$recycle.bin') || rawPath.includes('$recycle.bin') || (typeof item === 'object' && item?.kind === 'recycle')) return 'folder-recycle.svg';

    if (name === '.minecraft' || name === 'minecraft' || rawPath.includes('.minecraft')) return 'folder-minecraft.svg';
    if (name === '.tlauncher' || name === 'tlauncher') return 'folder-tlauncher.svg';
    if (name === '.obsidian' || name === 'obsidian' || rawPath.includes('.obsidian')) return 'folder-obsidian.svg';
    if (name.startsWith('.stfolder') || name.startsWith('.syncthing') || name === 'syncthing') return 'folder-syncthing.svg';
    if (name === '.gallery' || name === 'gallery') return 'folder-gallery.svg';
    if (name === 'onedrive' || rawPath.includes('/onedrive')) return 'folder-onedrive.svg';
    if (name === 'saved games' || name === 'сохраненные игры' || name === 'games' || name === 'игры') return 'folder-games.svg';
    if (name === 'searches' || name === 'поиски') return 'folder-searches.svg';
    if (name === 'contacts' || name === 'контакты') return 'folder-contacts.svg';
    if (name === 'favorites' || name === 'избранное') return 'folder-favorites.svg';
    if (name === 'links' || name === 'ссылки') return 'folder-links.svg';
    if (name === 'mcps' || name === '.mcps') return 'folder-mcps.svg';
    if (name === 'appdata' || name === '.appdata' || rawPath.includes('/appdata')) return 'folder-appdata.svg';
    if (name === '_apphome' || name === '_devhome' || name === 'devhome') return 'folder-devhome.svg';
    if (name === '_profile_config' || name === 'profile_config') return 'folder-profile.svg';
    if (name.startsWith('.sandbox') || name.startsWith('sandbox')) return 'folder-sandbox.svg';
    if (name === '.secret' || name === '.secrets' || name === 'secrets') return 'folder-secret.svg';
    if (name === '.tmp' || name === 'temp' || name === 'tmp') return 'folder-temp.svg';
    if (name === '.thumbnails' || name === 'thumbnails') return 'folder-thumbnails.svg';
    if (name === '.trash' || name === 'trash') return 'folder-trash.svg';
    if (name === '.dotnet' || name === 'dotnet') return 'folder-dotnet.svg';
    if (name === '.cache' || name === 'cache') return 'folder-cache.svg';

    if (name === '.git') return 'folder-git.svg';
    if (name === '.github') return 'folder-github.svg';
    if (name === '.vscode' || name === '.code') return 'folder-vscode.svg';
    if (name === '.idea') return 'folder-idea.svg';
    if (name === '.cargo') return 'folder-cargo.svg';
    if (name === '.rustup') return 'folder-rust.svg';
    if (name === '.config' || name === '.cfg') return 'folder-config.svg';
    if (name === '.gemini' || name === '.agents' || name === '.copilot' || name === '.claude' || name === '.grok' || name === '.kimi' || name === '.antigravity-ide') return 'folder-ai.svg';
    if (name === '.ssh') return 'folder-ssh.svg';
    if (name === '.local') return 'folder-local.svg';
    if (name === '.android') return 'folder-android.svg';
    if (name === '.tauri') return 'folder-tauri.svg';
    if (name === '.docker' || name === '.devcontainer') return 'folder-docker.svg';
    if (name === 'node_modules' || name === '.npm' || name === '.yarn' || name === '.pnpm') return 'folder-node.svg';
    if (name === '.venv' || name === 'venv' || name === '__pycache__' || name === 'env') return 'folder-python.svg';
    if (name === 'src' || name === 'source' || name === 'sources' || name === 'lib') return 'folder-code.svg';
    if (name === 'build' || name === 'dist' || name === 'target' || name === 'out' || name === 'bin' || name === 'release' || name === 'debug') return 'folder-build.svg';
    if (name === 'assets' || name === 'public' || name === 'static' || name === 'res' || name === 'resources' || name === 'icons' || name === 'images' || name === 'img') return 'folder-assets.svg';

    return 'folder.svg';
}

/**
 * Returns the resolved asset URL for a file icon.
 * @param {string|object} item - Filename string or item object.
 * @param {string} [base='assets/icons/file-types/'] - Custom base path.
 * @returns {string}
 */
export function getFileIconUrl(item, base = 'assets/icons/file-types/') {
    const iconName = getFileIconName(item);
    return base + iconName;
}

/**
 * Returns the resolved asset URL for a folder icon.
 * @param {string|object} item - Folder name or item object.
 * @param {string} [base='assets/icons/ui/'] - Custom base path.
 * @returns {string}
 */
export function getFolderIconUrl(item, base = 'assets/icons/ui/') {
    const iconName = getFolderIconName(item);
    return base + iconName;
}

/**
 * Returns human-readable file type description.
 * @param {string} path - File path or filename.
 * @returns {string}
 */
export function getFileTypeName(path) {
    const name = String(path || '').split(/[\\/]/).pop()?.toLowerCase() || '';

    if (name.startsWith('.env')) return 'Environment Config';
    if (name === '.gitignore') return 'Git Ignore';
    if (name === '.gitattributes') return 'Git Attributes';
    if (name === '.gitmodules') return 'Git Modules';
    if (name === '.editorconfig') return 'EditorConfig';
    if (name === '.npmrc' || name === '.npmignore') return 'npm Config';
    if (name.startsWith('.prettierrc')) return 'Prettier Config';
    if (name.startsWith('.eslintrc')) return 'ESLint Config';
    if (name.startsWith('.babelrc')) return 'Babel Config';
    if (name.startsWith('dockerfile')) return 'Docker Image Specification';
    if (name === 'cargo.toml') return 'Rust Package Manifest';
    if (name === 'package.json') return 'Node.js Package Manifest';

    const ext = name.split('.').pop() || '';
    const typeNames = {
        'js': 'JavaScript Source', 'mjs': 'JavaScript Module', 'jsx': 'React JavaScript',
        'ts': 'TypeScript Source', 'tsx': 'React TypeScript',
        'py': 'Python Script',
        'rs': 'Rust Source',
        'go': 'Go Source',
        'cs': 'C# Source',
        'html': 'HTML Document', 'htm': 'HTML Document', 'vue': 'Vue Component', 'svelte': 'Svelte Component',
        'css': 'Cascading Style Sheet', 'scss': 'Sass Stylesheet',
        'json': 'JSON Document',
        'xml': 'XML Document',
        'yaml': 'YAML Document', 'yml': 'YAML Document', 'toml': 'TOML Document',
        'md': 'Markdown Document',
        'pdf': 'PDF Document',
        'doc': 'Word Document', 'docx': 'Word Document',
        'xls': 'Excel Spreadsheet', 'xlsx': 'Excel Spreadsheet', 'csv': 'CSV Table',
        'ppt': 'PowerPoint Presentation', 'pptx': 'PowerPoint Presentation',
        'mp3': 'MP3 Audio', 'wav': 'WAV Audio', 'flac': 'FLAC Audio', 'ogg': 'OGG Audio',
        'mp4': 'MP4 Video', 'mkv': 'MKV Video', 'mov': 'QuickTime Video', 'webm': 'WebM Video',
        'jpg': 'JPEG Image', 'jpeg': 'JPEG Image', 'png': 'PNG Image', 'webp': 'WebP Image', 'gif': 'GIF Animation', 'svg': 'SVG Vector',
        'zip': 'ZIP Archive', 'rar': 'RAR Archive', '7z': '7-Zip Archive', 'tar': 'TAR Archive', 'gz': 'GZip Archive',
        'exe': 'Executable Application', 'msi': 'Windows Installer', 'dll': 'Dynamic Link Library',
        'txt': 'Text Document', 'log': 'Log File', 'ini': 'Configuration File',
    };

    return typeNames[ext] || (ext ? `${ext.toUpperCase()} File` : 'File');
}
