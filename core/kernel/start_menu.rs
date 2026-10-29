//! Shortcut sources for desktop folders: the Start menu, the icon Windows shows
//! for a target, and what a pasted/picked/dropped path or link points at.
//!
//! COM (`IShellLinkW`, `IShellItemImageFactory`) runs on a dedicated worker
//! thread with its own apartment, never on the Tauri UI thread (architecture §3).
//! A shortcut stores the real program, file, folder or URL — `.lnk` files are
//! only read, never copied.

use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct ShortcutDraft {
    pub name: String,
    pub target: String,
    pub args: String,
    pub kind: String,
}

/// Pixels of an icon, BGRA top-down with straight alpha, base64.
#[derive(Serialize, Clone, Debug)]
pub struct IconPixels {
    pub width: u32,
    pub height: u32,
    pub bgra: String,
}

/// Run `job` on a fresh STA thread. COM objects never cross threads here.
fn with_com<T: Send + 'static>(job: impl FnOnce() -> T + Send + 'static) -> Result<T, String> {
    std::thread::Builder::new()
        .name("shell-com".into())
        .spawn(move || {
            use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED};
            let init = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
            let out = job();
            if init.is_ok() {
                unsafe { CoUninitialize() };
            }
            out
        })
        .map_err(|e| format!("shell worker: {e}"))?
        .join()
        .map_err(|_| "shell worker panicked".to_string())
}

/// URL-like: a scheme followed by `:` (`https://`, `steam://`, `mailto:`), but
/// not a drive letter (`C:\`).
pub fn is_url(target: &str) -> bool {
    let Some((scheme, _)) = target.split_once(':') else { return false };
    scheme.len() > 1
        && scheme.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
        && scheme.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
}

/// A link's name: its host without `www.` (the page title is out of reach —
/// the kernel has no HTTP client and the user renames when needed).
fn name_from_url(url: &str) -> String {
    let rest = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    let host = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    let host = host.rsplit('@').next().unwrap_or(host);
    let host = host.split(':').next().unwrap_or(host);
    let host = host.strip_prefix("www.").unwrap_or(host);
    if host.is_empty() { url.to_string() } else { host.to_string() }
}

fn file_stem(path: &Path) -> String {
    path.file_stem()
        .or_else(|| path.file_name())
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// `URL=` line of an Internet Shortcut (`.url`, an INI file).
fn read_url_file(path: &Path) -> Option<String> {
    let text = std::fs::read(path).ok()?;
    String::from_utf8_lossy(&text)
        .lines()
        .find_map(|line| line.trim().strip_prefix("URL=").map(|u| u.trim().to_string()))
        .filter(|u| !u.is_empty())
}

/// Target path and arguments of a `.lnk`. Must run inside [`with_com`].
/// Advertised installer shortcuts (Office and the like) have no target path;
/// for those the `.lnk` itself is what Windows launches, so it is kept.
fn resolve_lnk(path: &Path) -> Result<(String, String), String> {
    use windows::core::{Interface, HSTRING};
    use windows::Win32::System::Com::{CoCreateInstance, IPersistFile, CLSCTX_INPROC_SERVER, STGM_READ};
    use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};

    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)
            .map_err(|e| format!("ShellLink: {e}"))?;
        let file: IPersistFile = link.cast().map_err(|e| format!("IPersistFile: {e}"))?;
        file.Load(&HSTRING::from(path.as_os_str()), STGM_READ)
            .map_err(|e| format!("read {}: {e}", path.display()))?;
        let mut target = vec![0u16; 1024];
        let _ = link.GetPath(&mut target, std::ptr::null_mut(), 0);
        let mut args = vec![0u16; 2048];
        let _ = link.GetArguments(&mut args);
        let utf16 = |buf: &[u16]| {
            let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
            String::from_utf16_lossy(&buf[..end])
        };
        let target = utf16(&target);
        if target.is_empty() {
            return Ok((path.display().to_string(), String::new()));
        }
        Ok((target, utf16(&args)))
    }
}

/// What a path or link points at, named and typed. Must run inside [`with_com`].
fn describe_in_com(raw: &str) -> Result<ShortcutDraft, String> {
    let text = raw.trim().trim_matches('"').trim();
    if text.is_empty() {
        return Err("пусто".into());
    }
    let url = if text.starts_with("www.") { format!("https://{text}") } else { text.to_string() };
    if is_url(&url) && !url.starts_with("file:") {
        return Ok(ShortcutDraft { name: name_from_url(&url), target: url, args: String::new(), kind: "url".into() });
    }
    let path = PathBuf::from(text);
    if !path.exists() {
        return Err(format!("не найдено: {text}"));
    }
    let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    if ext == "lnk" {
        let (target, args) = resolve_lnk(&path)?;
        let kind = if Path::new(&target).is_dir() { "folder" } else { "file" };
        return Ok(ShortcutDraft { name: file_stem(&path), target, args, kind: kind.into() });
    }
    if ext == "url" {
        if let Some(target) = read_url_file(&path) {
            return Ok(ShortcutDraft { name: file_stem(&path), target, args: String::new(), kind: "url".into() });
        }
    }
    let kind = if path.is_dir() { "folder" } else { "file" };
    // A drive root has no stem: keep "C:\" as its name.
    let name = if path.parent().is_none() { text.to_string() } else { file_stem(&path) };
    Ok(ShortcutDraft { name, target: path.display().to_string(), args: String::new(), kind: kind.into() })
}

pub fn describe(target: String) -> Result<ShortcutDraft, String> {
    with_com(move || describe_in_com(&target))?
}

/// Which targets are gone from disk. URLs are never "missing".
pub fn missing(targets: &[String]) -> Vec<bool> {
    targets
        .iter()
        .map(|t| !t.is_empty() && !is_url(t) && !Path::new(t).exists())
        .collect()
}

fn known_folder(id: &windows::core::GUID) -> Option<PathBuf> {
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::UI::Shell::{SHGetKnownFolderPath, KF_FLAG_DEFAULT};
    unsafe {
        let raw = SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None).ok()?;
        let path = raw.to_string().ok().map(PathBuf::from);
        CoTaskMemFree(Some(raw.0 as *const _));
        path
    }
}

fn collect_shortcuts(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_shortcuts(&path, out);
        } else if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("lnk") || e.eq_ignore_ascii_case("url"))
        {
            out.push(path);
        }
    }
}

/// Uninstallers, help and readme links are Start menu clutter, not programs.
fn is_clutter(name: &str) -> bool {
    let lower = name.to_lowercase();
    ["uninstall", "удал", "readme", "help", "справка", "documentation", "license", "лиценз"]
        .iter()
        .any(|w| lower.contains(w))
}

/// Store (packaged) apps have no `.lnk`: they live only in the shell's
/// `AppsFolder`, addressed by their AUMID (`Microsoft.WindowsCalculator_…!App`).
/// Desktop programs in that folder are skipped — the `.lnk` walk has them.
/// Must run inside [`with_com`].
fn packaged_apps() -> Vec<ShortcutDraft> {
    use windows::core::w;
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::UI::Shell::{
        IEnumShellItems, IShellItem, SHCreateItemFromParsingName, BHID_EnumItems, SIGDN_NORMALDISPLAY,
        SIGDN_PARENTRELATIVEPARSING,
    };

    let mut apps = Vec::new();
    unsafe {
        let Ok(folder) = SHCreateItemFromParsingName::<_, _, IShellItem>(w!("shell:AppsFolder"), None) else {
            return apps;
        };
        let Ok(items) = folder.BindToHandler::<_, IEnumShellItems>(None, &BHID_EnumItems) else {
            return apps;
        };
        let text = |item: &IShellItem, sigdn| -> Option<String> {
            let raw = item.GetDisplayName(sigdn).ok()?;
            let value = raw.to_string().ok();
            CoTaskMemFree(Some(raw.0 as *const _));
            value
        };
        loop {
            let mut slot = [None];
            let mut fetched = 0u32;
            if items.Next(&mut slot, Some(&mut fetched)).is_err() || fetched == 0 {
                break;
            }
            let Some(item) = slot[0].take() else { break };
            let (Some(name), Some(aumid)) = (text(&item, SIGDN_NORMALDISPLAY), text(&item, SIGDN_PARENTRELATIVEPARSING)) else {
                continue;
            };
            if is_packaged_aumid(&aumid) {
                apps.push(ShortcutDraft { name, target: format!("{APPS_FOLDER}{aumid}"), args: String::new(), kind: "app".into() });
            }
        }
    }
    apps
}

const APPS_FOLDER: &str = "shell:AppsFolder\\";

/// `Family_publisherid!AppId` — not a path, not a URL.
fn is_packaged_aumid(parsing_name: &str) -> bool {
    parsing_name.contains('!') && !parsing_name.contains('\\') && !parsing_name.contains("://")
}

/// A packaged app shortcut (`shell:AppsFolder\<AUMID>`): launched and drawn
/// by the shell itself, never "missing" on disk.
pub fn is_packaged_app(target: &str) -> bool {
    target.len() > APPS_FOLDER.len() && target[..APPS_FOLDER.len()].eq_ignore_ascii_case(APPS_FOLDER)
}

/// Programs of the Start menu (all users and the current one) and Store apps, by name.
pub fn start_menu() -> Result<Vec<ShortcutDraft>, String> {
    with_com(|| {
        use windows::Win32::UI::Shell::{FOLDERID_CommonPrograms, FOLDERID_Programs};
        let mut files = Vec::new();
        for id in [&FOLDERID_CommonPrograms, &FOLDERID_Programs] {
            if let Some(dir) = known_folder(id) {
                collect_shortcuts(&dir, &mut files);
            }
        }
        let mut seen = std::collections::HashSet::new();
        let mut apps: Vec<ShortcutDraft> = files
            .iter()
            .filter(|p| !is_clutter(&file_stem(p)))
            .filter_map(|p| describe_in_com(&p.display().to_string()).ok())
            .chain(packaged_apps())
            .filter(|d| seen.insert(d.name.to_lowercase()))
            .collect();
        apps.sort_by_key(|d| d.name.to_lowercase());
        apps
    })
}

/// Launch a shortcut the way Explorer does: files, programs with arguments,
/// `.lnk`, and any registered URL scheme (`steam://`, `https://`).
pub fn launch(target: &str, args: &str) -> Result<(), String> {
    use windows::core::{w, HSTRING, PCWSTR};
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    let file = HSTRING::from(target);
    let params = HSTRING::from(args);
    let dir = Path::new(target)
        .parent()
        .filter(|_| !is_url(target))
        .map(|p| HSTRING::from(p.as_os_str()));
    let rc = unsafe {
        ShellExecuteW(
            None,
            w!("open"),
            &file,
            if args.is_empty() { PCWSTR::null() } else { PCWSTR(params.as_ptr()) },
            dir.as_ref().map(|d| PCWSTR(d.as_ptr())).unwrap_or(PCWSTR::null()),
            SW_SHOWNORMAL,
        )
    };
    if rc.0 as isize <= 32 {
        return Err(format!("не удалось открыть {target} (код {})", rc.0 as isize));
    }
    Ok(())
}

// ─── Icons ──────────────────────────────────────────────────────────────────

const BASE64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn base64(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = (chunk[0] as u32) << 16
            | (*chunk.get(1).unwrap_or(&0) as u32) << 8
            | *chunk.get(2).unwrap_or(&0) as u32;
        out.push(BASE64[(n >> 18) as usize & 63] as char);
        out.push(BASE64[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { BASE64[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { BASE64[n as usize & 63] as char } else { '=' });
    }
    out
}

/// Cache file of one icon: the key covers the target, its last change and the
/// size, so an updated program gets a fresh icon.
fn icon_cache_path(target: &str, size: u32) -> PathBuf {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    target.to_lowercase().hash(&mut hasher);
    size.hash(&mut hasher);
    if let Ok(modified) = std::fs::metadata(target).and_then(|m| m.modified()) {
        modified.hash(&mut hasher);
    }
    unidtk_api::paths::cache_dir().join("icons").join(format!("{:016x}.bgra", hasher.finish()))
}

/// Cache layout: width and height as u32 LE, then the BGRA pixels.
fn read_cached(path: &Path) -> Option<(u32, u32, Vec<u8>)> {
    let bytes = std::fs::read(path).ok()?;
    let w = u32::from_le_bytes(bytes.get(0..4)?.try_into().ok()?);
    let h = u32::from_le_bytes(bytes.get(4..8)?.try_into().ok()?);
    let pixels = bytes.get(8..)?.to_vec();
    (pixels.len() == (w * h * 4) as usize).then_some((w, h, pixels))
}

fn write_cached(path: &Path, w: u32, h: u32, pixels: &[u8]) {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let mut bytes = Vec::with_capacity(8 + pixels.len());
    bytes.extend_from_slice(&w.to_le_bytes());
    bytes.extend_from_slice(&h.to_le_bytes());
    bytes.extend_from_slice(pixels);
    let _ = std::fs::write(path, bytes);
}

/// The icon Explorer draws for `target`. Must run inside [`with_com`].
fn extract_icon(target: &str, size: u32) -> Result<(u32, u32, Vec<u8>), String> {
    use windows::core::HSTRING;
    use windows::Win32::Foundation::SIZE;
    use windows::Win32::Graphics::Gdi::{
        CreateCompatibleDC, DeleteDC, DeleteObject, GetDIBits, GetObjectW, BITMAP, BITMAPINFO,
        BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS,
    };
    use windows::Win32::UI::Shell::{
        IShellItemImageFactory, SHCreateItemFromParsingName, SIIGBF_BIGGERSIZEOK, SIIGBF_ICONONLY,
    };

    unsafe {
        let factory: IShellItemImageFactory = SHCreateItemFromParsingName(&HSTRING::from(target), None)
            .map_err(|e| format!("shell item {target}: {e}"))?;
        let side = size as i32;
        let bitmap = factory
            .GetImage(SIZE { cx: side, cy: side }, SIIGBF_ICONONLY | SIIGBF_BIGGERSIZEOK)
            .map_err(|e| format!("icon {target}: {e}"))?;
        let mut info = BITMAP::default();
        GetObjectW(bitmap, std::mem::size_of::<BITMAP>() as i32, Some(&mut info as *mut _ as *mut _));
        let (w, h) = (info.bmWidth.max(0) as u32, info.bmHeight.unsigned_abs());
        let mut header = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w as i32,
                // Negative height: rows top-down, as canvas expects.
                biHeight: -(h as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut pixels = vec![0u8; (w * h * 4) as usize];
        let dc = CreateCompatibleDC(None);
        let lines = GetDIBits(dc, bitmap, 0, h, Some(pixels.as_mut_ptr() as *mut _), &mut header, DIB_RGB_COLORS);
        let _ = DeleteDC(dc);
        let _ = DeleteObject(bitmap);
        if lines == 0 || w == 0 {
            return Err(format!("icon {target}: no pixels"));
        }
        // The shell hands out premultiplied alpha; ImageData wants straight.
        for px in pixels.chunks_exact_mut(4) {
            let a = px[3] as u32;
            if a > 0 && a < 255 {
                for c in &mut px[..3] {
                    *c = ((*c as u32 * 255 + a / 2) / a).min(255) as u8;
                }
            }
        }
        Ok((w, h, pixels))
    }
}

pub fn icon(target: String, size: u32) -> Result<Option<IconPixels>, String> {
    let on_disk = !is_url(&target) && Path::new(&target).exists();
    if target.is_empty() || !(on_disk || is_packaged_app(&target)) {
        return Ok(None);
    }
    let size = size.clamp(16, 256);
    let cache = icon_cache_path(&target, size);
    let (w, h, pixels) = match read_cached(&cache) {
        Some(hit) => hit,
        None => {
            let fresh = with_com(move || extract_icon(&target, size))??;
            write_cached(&cache, fresh.0, fresh.1, &fresh.2);
            fresh
        }
    };
    Ok(Some(IconPixels { width: w, height: h, bgra: base64(&pixels) }))
}

// ─── One-time move off copied `.lnk` files ──────────────────────────────────

/// Folders once stored copies of `.lnk`/`.url` files under
/// `%LOCALAPPDATA%\UniDTK\folders\<id>\shortcuts`. Point each such item at
/// what the copy points at; a copy whose target is gone stays as it is. The
/// copies are deleted only after the records holding the new targets are
/// saved. Returns how many items moved.
pub fn migrate_copied_shortcuts(db: &crate::unidtk_db::UniDtkDatabase) -> Result<usize, String> {
    let root = unidtk_api::paths::data_dir().join("folders");
    let prefix = format!("{}{}", root.display(), std::path::MAIN_SEPARATOR).to_lowercase();
    let folders = db.list_folders()?;
    let copies: Vec<String> = folders
        .iter()
        .flat_map(|f| f.items.iter())
        .map(|it| it.target.clone())
        .filter(|t| t.to_lowercase().starts_with(&prefix))
        .filter(|t| {
            let lower = t.to_lowercase();
            lower.ends_with(".lnk") || lower.ends_with(".url")
        })
        .collect();
    if copies.is_empty() {
        return Ok(0);
    }
    let resolved: Vec<(String, ShortcutDraft)> = with_com(move || {
        copies
            .into_iter()
            .filter_map(|copy| {
                let draft = describe_in_com(&copy).ok()?;
                // A copy that resolves to itself (advertised or broken) has no
                // better target; one whose target is gone is left alone.
                let usable = draft.target != copy && (draft.kind == "url" || Path::new(&draft.target).exists());
                usable.then_some((copy, draft))
            })
            .collect()
    })?;
    let mut moved = Vec::new();
    for mut folder in folders {
        let mut changed = false;
        for item in &mut folder.items {
            if let Some((copy, draft)) = resolved.iter().find(|(copy, _)| copy == &item.target) {
                item.target = draft.target.clone();
                item.args = draft.args.clone();
                item.kind = draft.kind.clone();
                moved.push(copy.clone());
                changed = true;
            }
        }
        if changed {
            db.save_folder(&folder)?;
        }
    }
    for copy in &moved {
        let _ = std::fs::remove_file(copy);
    }
    Ok(moved.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_detection_skips_drive_letters() {
        assert!(is_url("https://example.com"));
        assert!(is_url("steam://rungameid/1"));
        assert!(is_url("mailto:a@b.c"));
        assert!(!is_url(r"C:\Windows\notepad.exe"));
        assert!(!is_url("notes.txt"));
    }

    #[test]
    fn packaged_apps_are_recognised_by_aumid() {
        assert!(is_packaged_aumid("Microsoft.WindowsCalculator_8wekyb3d8bbwe!App"));
        assert!(!is_packaged_aumid(r"C:\Program Files\App\app.exe"));
        assert!(!is_packaged_aumid("{6D809377-6AF0-444B-8957-A3773F02200E}\\app.exe"));
        assert!(is_packaged_app(r"shell:AppsFolder\Microsoft.WindowsCalculator_8wekyb3d8bbwe!App"));
        assert!(!is_packaged_app("shell:AppsFolder\\"));
        assert_eq!(missing(&[r"shell:AppsFolder\X_1!App".to_string()]), [false]);
    }

    #[test]
    fn link_names_come_from_the_host() {
        assert_eq!(name_from_url("https://www.youtube.com/watch?v=1"), "youtube.com");
        assert_eq!(name_from_url("https://user@git.example.org:8443/x"), "git.example.org");
        assert_eq!(name_from_url("steam://rungameid/1"), "rungameid");
    }

    #[test]
    fn www_text_becomes_an_https_link() {
        let draft = describe_in_com("www.example.com/page").unwrap();
        assert_eq!(draft.kind, "url");
        assert_eq!(draft.target, "https://www.example.com/page");
        assert_eq!(draft.name, "example.com");
    }

    #[test]
    fn plain_paths_are_typed_by_what_is_on_disk() {
        let dir = std::env::temp_dir().join(format!("unidtk_describe_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("notes.txt");
        std::fs::write(&file, "x").unwrap();
        let url_file = dir.join("Fall Guys.url");
        std::fs::write(&url_file, "[InternetShortcut]\r\nURL=com.epicgames.launcher://apps/x\r\n").unwrap();

        let as_file = describe_in_com(&format!("\"{}\"", file.display())).unwrap();
        assert_eq!((as_file.name.as_str(), as_file.kind.as_str()), ("notes", "file"));
        let as_dir = describe_in_com(&dir.display().to_string()).unwrap();
        assert_eq!(as_dir.kind, "folder");
        let as_url = describe_in_com(&url_file.display().to_string()).unwrap();
        assert_eq!((as_url.name.as_str(), as_url.kind.as_str()), ("Fall Guys", "url"));
        assert_eq!(as_url.target, "com.epicgames.launcher://apps/x");
        assert!(describe_in_com(&dir.join("gone.exe").display().to_string()).is_err());

        assert_eq!(missing(&[file.display().to_string(), dir.join("gone").display().to_string(), "https://x".into()]), [false, true, false]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn base64_matches_the_standard_alphabet() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(&[0xff, 0xfe, 0xfd, 0x00]), "//79AA==");
    }
}
