use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// Tauri needs a single `frontendDist` directory. The frontend is authored as
/// vertical slices (`core/ui`, `modules/<id>/ui`, `assets`), but the
/// browser resolves the core's dynamic `import("../modules/<id>/widget.js")`
/// relative to the module URL — so every UI file has to end up in one tree.
/// We materialise that tree in `toolchain/dist/` right here, before Tauri embeds
/// it. Mapping (see `tauri.conf.json > build.frontendDist`):
///
///   core/ui/*            -> dist/                 (index.html, unidtk-core.js, motion.js, styles.css, ...)
///   modules/<id>/ui/*    -> dist/modules/<id>/
///   assets/              -> dist/assets/
///
/// Because the core files land at the dist root, the existing relative paths keep
/// resolving unchanged: `../modules/<id>/widget.js` from `/unidtk-core.js` clamps
/// to `/modules/<id>/widget.js`, and `../styles.css` from `/index.html` clamps to
/// `/styles.css`.
const FRONTEND_CORE: &str = "../core/ui";
const FRONTEND_MODULES: &str = "../modules";
const FRONTEND_ASSETS: &str = "../assets";
const DIST_DIR: &str = "dist";

/// Recursively stage `src` into `dst`: copy only files that actually differ,
/// delete files `src` no longer has.
///
/// Deliberately NOT "wipe + recopy": Tauri embeds `dist/` with per-file
/// fingerprints, so a full wipe refreshed every mtime and forced a relink of
/// the 53 MB app even when a single JS file changed. Copy-if-different keeps
/// unchanged files' mtimes stable -> an unchanged frontend means no relink.
///
/// "Different" means content-different: bytes are folded through an FNV-1a 64
/// hash and compared (a same-length edit used to slip through a length-only
/// comparison, shipping stale CSS with a fresh timestamp).
///
/// `keep` names directories at this level that are staged from a different
/// source and must not be treated as ghosts (dist root keeps `modules`/`assets`).
fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// True when both files exist and their contents hash identically.
fn files_identical(src: &Path, dst: &Path) -> bool {
    match (std::fs::read(src), std::fs::read(dst)) {
        (Ok(a), Ok(b)) => a.len() == b.len() && fnv1a64(&a) == fnv1a64(&b),
        _ => false,
    }
}

fn sync_tree(src: &Path, dst: &Path, keep: &[&str]) {
    let Ok(entries) = std::fs::read_dir(src) else {
        return;
    };
    if std::fs::create_dir_all(dst).is_err() {
        return;
    }
    let mut seen: HashSet<std::ffi::OsString> = HashSet::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name() else {
            continue;
        };
        seen.insert(name.to_os_string());
        let target = dst.join(name);
        if path.is_dir() {
            // A stale file squatting where a directory must land would make
            // create_dir_all fail and keep dead entries alive.
            if target.is_file() {
                let _ = std::fs::remove_file(&target);
            }
            sync_tree(&path, &target, &[]);
        } else {
            let needs_copy = match (std::fs::metadata(&path), std::fs::metadata(&target)) {
                (Ok(a), Ok(b)) => a.is_file() != b.is_file() || !files_identical(&path, &target),
                _ => true,
            };
            if needs_copy {
                // Mirror of the dir branch: a stale directory under a file's
                // target path would silently swallow the copy.
                if target.is_dir() {
                    let _ = std::fs::remove_dir_all(&target);
                }
                if let Some(parent) = target.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                let _ = std::fs::copy(&path, &target);
            }
        }
    }
    // Ghosts: files/dirs the source no longer produces. A leftover dead module
    // folder renders a second, ghost widget card (discovery reads the listing).
    if let Ok(existing) = std::fs::read_dir(dst) {
        for entry in existing.flatten() {
            let name = entry.file_name();
            if seen.contains(&name) {
                continue;
            }
            if keep.iter().any(|k| *k == name.to_string_lossy()) {
                continue;
            }
            let path = entry.path();
            if path.is_dir() {
                let _ = std::fs::remove_dir_all(&path);
            } else {
                let _ = std::fs::remove_file(&path);
            }
        }
    }
}

/// Build `toolchain/dist/` from the vertical slices — idempotent (see sync_tree):
/// a deleted module cannot linger, an untouched file keeps its mtime.
fn stage_dist() {
    let dist = Path::new(DIST_DIR);
    let _ = std::fs::create_dir_all(dist);

    // Frontend core -> dist root. `modules`/`assets` are staged below from
    // their own sources, so they are kept, not pruned as ghosts.
    sync_tree(Path::new(FRONTEND_CORE), dist, &["modules", "assets"]);

    // Every feature module that ships a UI -> dist/modules/<id>/.
    if let Ok(entries) = std::fs::read_dir(FRONTEND_MODULES) {
        let mut module_ui_dirs: Vec<(String, PathBuf)> = entries
            .flatten()
            .filter_map(|entry| {
                let ui = entry.path().join("ui");
                if ui.is_dir() {
                    entry.file_name().to_str().map(|id| (id.to_string(), ui))
                } else {
                    None
                }
            })
            .collect();
        module_ui_dirs.sort(); // deterministic staging order
        for (id, ui) in &module_ui_dirs {
            sync_tree(ui, &dist.join("modules").join(id), &[]);
        }
        // A module that lost its ui/ (or was removed) must not keep a stale folder.
        if let Ok(staged) = std::fs::read_dir(dist.join("modules")) {
            for entry in staged.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if !module_ui_dirs.iter().any(|(id, _)| *id == name) {
                    let _ = std::fs::remove_dir_all(entry.path());
                }
            }
        }
    }

    sync_tree(Path::new(FRONTEND_ASSETS), &dist.join("assets"), &[]);
}

fn watch_frontend_tree(path: &Path) {
    let Ok(entries) = std::fs::read_dir(path) else {
        return;
    };
    for entry in entries.flatten() {
        let child = entry.path();
        if child.is_dir() {
            let name = child.file_name().and_then(|n| n.to_str()).unwrap_or_default();
            if name == "node_modules" || name == ".git" {
                continue;
            }
            // `fonts` is staged too — an unstaged font change shipped a stale
            // ttf in the installer because nothing re-ran the build script.
            watch_frontend_tree(&child);
        } else {
            // Every file, not just a whitelist of extensions: assets the
            // embedder sees must trigger a rebuild when they change.
            println!("cargo:rerun-if-changed={}", child.display());
        }
    }
}

fn copy_native_binaries(target_dir: &Path) {
    let opt_dirs = [
        Path::new("../opt/tdlib-native/win32-x64"),
        Path::new("../opt/libmpv"),
        Path::new("../opt/singbox"),
        Path::new("../opt/byedpi"),
    ];
    for opt_dir in &opt_dirs {
        if let Ok(entries) = std::fs::read_dir(opt_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                    if ext.eq_ignore_ascii_case("dll") || ext.eq_ignore_ascii_case("exe") {
                        if let Some(file_name) = path.file_name() {
                            let dest = target_dir.join(file_name);
                            let need_copy = match (std::fs::metadata(&path), std::fs::metadata(&dest)) {
                                (Ok(src_meta), Ok(dst_meta)) => {
                                    src_meta.len() != dst_meta.len()
                                        || src_meta.modified().unwrap_or(std::time::SystemTime::UNIX_EPOCH)
                                            > dst_meta.modified().unwrap_or(std::time::SystemTime::UNIX_EPOCH)
                                }
                                (Ok(_), Err(_)) => true,
                                _ => false,
                            };
                            if need_copy {
                                let _ = std::fs::copy(&path, &dest);
                            }
                        }
                    }
                }
            }
        }
    }
}

fn lint_modules_integrity() {
    println!("cargo:rerun-if-changed=../core/kernel/registry.rs");
    let modules_dir = Path::new("../modules");
    let Ok(entries) = std::fs::read_dir(modules_dir) else {
        return;
    };

    let registry_content = std::fs::read_to_string("../core/kernel/registry.rs").unwrap_or_default();
    let toolchain_cargo = std::fs::read_to_string("Cargo.toml").unwrap_or_default();

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let rust_dir = path.join("rust");
            let cargo_toml = rust_dir.join("Cargo.toml");
            if cargo_toml.is_file() {
                let Some(id) = path.file_name().and_then(|n| n.to_str()) else {
                    continue;
                };
                if !registry_content.contains(&format!("\"{id}\"")) {
                    panic!(
                        "BUILD INTEGRITY ERROR: Rust module '{id}' found at {} is missing from core/kernel/registry.rs (ALL_MODULE_IDS)!",
                        path.display()
                    );
                }
                let expected_dep = format!("unidtk-mod-{}", id);
                if !toolchain_cargo.contains(&expected_dep) {
                    panic!(
                        "BUILD INTEGRITY ERROR: Rust module '{id}' ({expected_dep}) found at {} is missing from toolchain/Cargo.toml dependencies!",
                        path.display()
                    );
                }
            }
        }
    }
}

/// Frontend imports must go through the import map, and Tauri must be reached only
/// through the bridge.
///
/// Two regressions are prevented here. First, `modules/explorer/ui/*.js` used to import
/// Tauri by relative path, bypassing the import map entirely. Second, after the facade
/// (`core/ui/unidtk-bridge.js`) became the single host entry point, any UI file that
/// still imports `@tauri-apps/...` directly would silently keep the old transport and
/// make the migration a half-truth. The bridge itself is the only exception.
fn lint_ui_imports() {
    const BRIDGE: &str = "unidtk-bridge.js";
    // Every UI surface: the shell core plus each module's `ui/` — a module that keeps
    // importing Tauri directly would silently stay on the old transport.
    let mut roots: Vec<String> = vec!["../core/ui".to_string()];
    if let Ok(entries) = std::fs::read_dir("../modules") {
        for entry in entries.flatten() {
            roots.push(entry.path().join("ui").to_string_lossy().to_string());
        }
    }
    for root in roots {
        println!("cargo:rerun-if-changed={root}");
        let Ok(entries) = std::fs::read_dir(&root) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("js") {
                continue;
            }
            let Ok(content) = std::fs::read_to_string(&path) else {
                continue;
            };
            if content.contains("assets/vendor/@tauri-apps") {
                panic!(
                    "UI IMPORT ERROR: {} imports Tauri through the vendor path, bypassing the \
                     import map; use the bare specifier `@tauri-apps/api/...` instead",
                    path.display()
                );
            }
            let is_bridge = path
                .file_name()
                .and_then(|name| name.to_str())
                .map(|name| name == BRIDGE)
                .unwrap_or(false);
            if !is_bridge && content.contains("@tauri-apps/") {
                panic!(
                    "UI IMPORT ERROR: {} imports Tauri directly; the interface must go through \
                     the bridge (`@unidtk/bridge`), which is the single place that knows the \
                     transport",
                    path.display()
                );
            }
        }
    }
}

/// Doctor's `simulate` probes must come from data, not from literals.
///
/// Two sources are checked. First, every `metadata.json` under `modules/*/ui`
/// that carries a `simulate` section must name a real JS file and function;
/// the CLI generates its CDP probe from these, so a stale entry means a probe
/// that can never pass. Second, the CLI must not name render entry points
/// itself — any literal `renderClock` / `initVpnPanel` / … in `opt/cli/main.rs`
/// is a second list that will drift, exactly the disease the inventory cured.
fn lint_simulate_metadata() {
    const ENTRY_POINTS: &[&str] = &[
        "renderClock",
        "initVpnPanel",
        "renderMusicPlayer",
        "renderGlyphBar",
        "renderTelegramWidget",
        "renderMicrographics",
    ];
    let mut declared = 0usize;
    let modules_dir = Path::new("../modules");
    let Ok(entries) = std::fs::read_dir(modules_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let meta = entry.path().join("ui").join("metadata.json");
        let Ok(text) = std::fs::read_to_string(&meta) else {
            continue;
        };
        // No serde here (build deps are tauri-build only): the schema is tiny,
        // so string fields are pulled out by hand.
        fn field(text: &str, key: &str) -> Option<String> {
            let pat = format!("\"{key}\"");
            let mut rest = &text[..];
            loop {
                let i = rest.find(&pat)?;
                let mut after = rest[i + pat.len()..].trim_start();
                if !after.starts_with(':') {
                    rest = &rest[i + pat.len()..];
                    continue;
                }
                after = after[1..].trim_start();
                if !after.starts_with('"') {
                    rest = &rest[i + pat.len()..];
                    continue;
                }
                let mut out = String::new();
                let mut chars = after[1..].chars();
                loop {
                    let c = chars.next()?;
                    match c {
                        '\\' => {
                            if let Some(e) = chars.next() {
                                out.push(e);
                            }
                        }
                        '"' => return Some(out),
                        _ => out.push(c),
                    }
                }
            }
        }
        // `simulate` lives one level down (`"simulate": {"module": …}`), so
        // slice the object out before reading its fields.
        let sim_body = text.find("\"simulate\"").and_then(|i| {
            let open = text[i..].find('{')?;
            let mut depth = 0usize;
            let bytes = text[i + open..].as_bytes();
            for (n, b) in bytes.iter().enumerate() {
                match b {
                    b'{' => depth += 1,
                    b'}' => {
                        depth -= 1;
                        if depth == 0 {
                            return Some(&text[i + open..i + open + n + 1]);
                        }
                    }
                    _ => {}
                }
            }
            None
        });
        let Some(sim_body) = sim_body else {
            continue;
        };
        let Some(module) = field(sim_body, "module").filter(|s| !s.is_empty()) else {
            panic!(
                "SIMULATE METADATA ERROR: {}: `simulate.module` is missing",
                meta.display()
            );
        };
        let Some(function) = field(sim_body, "function").filter(|s| !s.is_empty()) else {
            panic!(
                "SIMULATE METADATA ERROR: {}: `simulate.function` is missing",
                meta.display()
            );
        };
        // The probe imports `../modules/<id>/<module>` relative to the page,
        // so the file must exist where the entry point says it is.
        let ui_dir = entry.path().join("ui");
        if !ui_dir.join(&module).is_file() {
            panic!(
                "SIMULATE METADATA ERROR: {} names module file `{module}`, which does not exist in {}",
                meta.display(),
                ui_dir.display()
            );
        }
        let ui_js = ui_dir.join(&module);
        let js = std::fs::read_to_string(&ui_js).unwrap_or_default();
        if !js.contains(function.as_str()) {
            panic!(
                "SIMULATE METADATA ERROR: {} names function `{function}`, which `{module}` does not define",
                meta.display()
            );
        }
        declared += 1;
    }
    if declared == 0 {
        panic!(
            "SIMULATE METADATA ERROR: no module declares `simulate` in metadata.json — \
             `udtk doctor -simulate` would probe nothing"
        );
    }

    let cli_full = std::fs::read_to_string("../opt/cli/main.rs").unwrap_or_default();
    // Test assertions name the entry points on purpose (they prove discovery
    // finds them); only production code must stay free of literals.
    let cli = cli_full
        .find("#[cfg(test)]")
        .map(|i| &cli_full[..i])
        .unwrap_or(&cli_full);
    for name in ENTRY_POINTS {
        if cli.contains(name) {
            panic!(
                "SIMULATE HARDCODE ERROR: opt/cli/main.rs names render entry point `{name}` directly; \
                 entry points live in modules/*/ui/metadata.json (`simulate`), the CLI only reads them"
            );
        }
    }
    println!("cargo:rerun-if-changed=../opt/cli/main.rs");
    println!("cargo:rerun-if-changed=../modules");
}

/// Bake the repo `.env` (tdlib keys) into every binary of this package as
/// compile-time env vars.
///
/// The installed copy has no `.env` next to it — it does not ship — and these
/// keys are identical for every user, so there is no secret to protect, only a
/// value that must not depend on a file that does not exist after install.
/// `core/kernel/main.rs` reads them back with `option_env!` and seeds the
/// process env, which is where `modules/telegram` looks for them at runtime.
fn embed_dotenv() {
    println!("cargo:rerun-if-changed=../.env");
    let Ok(text) = std::fs::read_to_string("../.env") else {
        return;
    };
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim().trim_matches('"');
        if key.is_empty() || !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            continue;
        }
        println!("cargo:rustc-env={key}={value}");
    }
}

fn main() {
    lint_modules_integrity();
    lint_ui_imports();
    lint_simulate_metadata();
    embed_dotenv();
    println!("cargo:rerun-if-changed=../opt/tdlib-native/win32-x64");
    println!("cargo:rerun-if-changed=../opt/libmpv");
    println!("cargo:rerun-if-changed=../opt/singbox");
    println!("cargo:rerun-if-changed=../opt/byedpi");

    if let Ok(out_dir) = std::env::var("OUT_DIR") {
        let out_path = Path::new(&out_dir);
        if let Some(target_dir) = out_path.ancestors().nth(3) {
            copy_native_binaries(target_dir);
        }
    }

    // Materialise the single frontend tree Tauri will embed.
    stage_dist();

    // Tauri's directory-only marker is insufficient on Windows: editing an
    // existing JS file does not update the parent directory timestamp, which
    // can leave stale embedded assets in an otherwise freshly linked .exe.
    watch_frontend_tree(Path::new(FRONTEND_CORE));
    watch_frontend_tree(Path::new(FRONTEND_MODULES));
    watch_frontend_tree(Path::new(FRONTEND_ASSETS));

    // NOTE: the wallpaper window must stay unelevated. Attaching it to the
    // desktop's WorkerW (see wallpaper::attach) reparents it under explorer.exe,
    // which runs at medium integrity — Windows UIPI silently
    // blocks that reparent if our process is elevated, leaving a normal
    // top-level window (with full OS decorations) floating over the desktop
    // instead of an embedded wallpaper. VPN/TUN privilege is handled
    // separately via dedicated elevated helper scheduled tasks (see vpn.rs),
    // not by elevating this whole process.
    let windows = tauri_build::WindowsAttributes::new().app_manifest(
        r#"
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0"
          xmlns:asmv3="urn:schemas-microsoft-com:asm.v3">
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3">
    <security>
      <requestedPrivileges>
        <requestedExecutionLevel level="asInvoker" uiAccess="false" />
      </requestedPrivileges>
    </security>
  </trustInfo>
  <asmv3:application>
    <asmv3:windowsSettings>
      <dpiAware xmlns="http://schemas.microsoft.com/SMI/2005/WindowsSettings">true/PM</dpiAware>
      <dpiAwareness xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">PerMonitorV2</dpiAwareness>
    </asmv3:windowsSettings>
  </asmv3:application>
  <compatibility xmlns="urn:schemas-microsoft-com:compatibility.v1">
    <application>
      <supportedOS Id="{e2011457-1546-43c5-a5fe-008deee3d3f0}" />
      <supportedOS Id="{35138b9a-5d96-4fbd-8e2d-a2440225f93a}" />
      <supportedOS Id="{4a2f28e3-53b9-4441-ba9c-d69d4a4a6e38}" />
      <supportedOS Id="{1f676c76-80e1-4239-95bb-83d0f6d0da78}" />
      <supportedOS Id="{8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a}" />
    </application>
  </compatibility>
  <dependency>
    <dependentAssembly>
      <assemblyIdentity
        type="win32"
        name="Microsoft.Windows.Common-Controls"
        version="6.0.0.0"
        processorArchitecture="*"
        publicKeyToken="6595b64144ccf1df"
        language="*"
      />
    </dependentAssembly>
  </dependency>
</assembly>
"#,
    );

    // tauri_build копирует bundle.resources в каталог профиля безусловно, а
    // назначение в этом окне часто занято: сам запущенный `udtk` (он же
    // запускает и эту сборку) замапил libmpv-2.dll/DirectML.dll как свои
    // импорты, ort держит свой symlink DirectML, крупные файлы сканирует
    // антивирус — везде os error 32. Наполнение профиля уже даёт наша
    // стадинг (copy_native_binaries/stage_dist) плюс сами бинарники cargo;
    // ресурсы для установщика читает отдельный процесс бандлера прямо из
    // tauri.conf.json. Копию ресурсов отключаем: не-объект в RFC 7386
    // (json_patch::merge) заменяет значение, поэтому пустой массив обнуляет
    // список, не трогая остальной конфиг.
    let mut cfg: serde_json::Value = std::env::var("TAURI_CONFIG")
        .ok()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
        .filter(|v| v.is_object())
        .unwrap_or_else(|| serde_json::json!({}));
    cfg["bundle"]["resources"] = serde_json::json!([]);
    std::env::set_var("TAURI_CONFIG", cfg.to_string());

    let attributes = tauri_build::Attributes::new().windows_attributes(windows);

    tauri_build::try_build(attributes).expect("tauri build script failed");
}
