use std::fs::File;
use std::io::Read;
use std::path::Path;

use zip::ZipArchive;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ArchiveEntry {
    pub name: String,
    pub path: String,
    pub size: u64,
    pub is_dir: bool,
    pub compressed_size: u64,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ArchiveInfo {
    pub path: String,
    pub comment: Option<String>,
    pub entries: Vec<ArchiveEntry>,
    pub total_size: u64,
    pub compressed_size: u64,
    pub file_count: u32,
    pub dir_count: u32,
}

pub fn open_archive(path: &Path) -> Result<ArchiveInfo, String> {
    let file = File::open(path).map_err(|e| format!("Failed to open archive: {}", e))?;
    let mut archive = ZipArchive::new(file).map_err(|e| format!("Failed to read zip: {}", e))?;

    let mut entries = Vec::new();
    let mut total_size: u64 = 0;
    let mut compressed_size: u64 = 0;
    let mut file_count: u32 = 0;
    let mut dir_count: u32 = 0;

    for i in 0..archive.len() {
        let entry = archive.by_index(i).map_err(|e| format!("Failed to read entry: {}", e))?;
        let name = entry.name().to_string();
        let entry_path = entry.mangled_name().to_string_lossy().to_string();
        let size = entry.size();
        let comp_size = entry.compressed_size();
        let is_dir = entry.is_dir();

        if is_dir {
            dir_count += 1;
        } else {
            file_count += 1;
        }

        total_size += size;
        compressed_size += comp_size;

        entries.push(ArchiveEntry {
            name,
            path: entry_path,
            size,
            is_dir,
            compressed_size: comp_size,
        });
    }

    Ok(ArchiveInfo {
        path: path.to_string_lossy().to_string(),
        comment: None,
        entries,
        total_size,
        compressed_size,
        file_count,
        dir_count,
    })
}

/// Total uncompressed bytes one extraction may write. Counted from the bytes actually
/// produced, never from the archive headers, which a hostile archive can understate.
const MAX_EXTRACT_TOTAL_BYTES: u64 = 4 * 1024 * 1024 * 1024;

pub fn extract_archive(archive_path: &Path, dest_dir: &Path) -> Result<Vec<String>, String> {
    extract_archive_with_limit(archive_path, dest_dir, MAX_EXTRACT_TOTAL_BYTES)
}

fn extract_archive_with_limit(archive_path: &Path, dest_dir: &Path, limit: u64) -> Result<Vec<String>, String> {
    let file = File::open(archive_path).map_err(|e| format!("Failed to open archive: {}", e))?;
    let mut archive = ZipArchive::new(file).map_err(|e| format!("Failed to read zip: {}", e))?;

    let mut extracted = Vec::new();
    let mut written_total: u64 = 0;

    // Everything is checked before anything is written: an archive that would replace
    // files must not replace some of them and then stop.
    let mut conflicts = Vec::new();
    for i in 0..archive.len() {
        let entry = archive.by_index(i).map_err(|e| format!("Failed to read entry: {}", e))?;
        let out_path = entry_out_path(&entry, dest_dir)?;
        if !entry.is_dir() && std::fs::symlink_metadata(&out_path).is_ok() {
            conflicts.push(out_path);
        }
    }
    if let Some(first) = conflicts.first() {
        return Err(format!(
            "{} file(s) already exist and would be overwritten, for example {}; nothing was extracted",
            conflicts.len(),
            first.display()
        ));
    }

    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| format!("Failed to read entry: {}", e))?;
        let out_path = entry_out_path(&entry, dest_dir)?;

        if entry.is_dir() {
            std::fs::create_dir_all(&out_path).map_err(|e| format!("Failed to create dir: {}", e))?;
        } else {
            if let Some(parent) = out_path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| format!("Failed to create parent: {}", e))?;
            }
            let mut out_file = create_new_file(&out_path)?;
            let budget = limit.saturating_sub(written_total);
            match copy_within_budget(&mut entry, &mut out_file, budget) {
                Ok(written) => written_total += written,
                Err(error) => {
                    drop(out_file);
                    let _ = std::fs::remove_file(&out_path);
                    return Err(error);
                }
            }
        }

        extracted.push(out_path.to_string_lossy().to_string());
    }

    Ok(extracted)
}

/// Where an entry goes, refusing paths that leave the destination. `enclosed_name`
/// rejects absolute paths and `..` components; the `starts_with` check is a second
/// guard on the joined path.
fn entry_out_path(entry: &zip::read::ZipFile, dest_dir: &Path) -> Result<std::path::PathBuf, String> {
    let relative = entry
        .enclosed_name()
        .map(Path::to_path_buf)
        .ok_or_else(|| format!("Unsafe path in archive: {}", entry.name()))?;
    let out_path = dest_dir.join(&relative);
    if !out_path.starts_with(dest_dir) {
        return Err(format!("Unsafe path in archive: {}", entry.name()));
    }
    Ok(out_path)
}

/// A new file that must not exist yet: `File::create` would truncate one that does.
fn create_new_file(path: &Path) -> Result<File, String> {
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::AlreadyExists {
                format!("Already exists: {}", path.display())
            } else {
                format!("Failed to create file: {}", e)
            }
        })
}

/// Copies at most `budget` bytes. One byte past the budget is read so an over-limit
/// entry is detected instead of silently truncated.
fn copy_within_budget(reader: &mut impl Read, writer: &mut impl std::io::Write, budget: u64) -> Result<u64, String> {
    let copied = std::io::copy(&mut reader.take(budget.saturating_add(1)), writer)
        .map_err(|e| format!("Failed to write: {}", e))?;
    if copied > budget {
        return Err("Archive expands beyond the extraction size limit".to_string());
    }
    Ok(copied)
}

pub fn extract_single(archive_path: &Path, entry_name: &str, dest_dir: &Path) -> Result<std::path::PathBuf, String> {
    let file = File::open(archive_path).map_err(|e| format!("Failed to open archive: {}", e))?;
    let mut archive = ZipArchive::new(file).map_err(|e| format!("Failed to read zip: {}", e))?;

    let mut entry = archive.by_name(entry_name).map_err(|e| format!("Entry not found: {}", e))?;
    let safe_name = entry
        .enclosed_name()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| std::path::PathBuf::from(entry.mangled_name()));
    let out_path = dest_dir.join(safe_name);

    if entry.is_dir() {
        std::fs::create_dir_all(&out_path).map_err(|e| format!("Failed to create dir: {}", e))?;
    } else {
        if let Some(parent) = out_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("Failed to create parent: {}", e))?;
        }
        let mut out_file = create_new_file(&out_path)?;
        std::io::copy(&mut entry, &mut out_file).map_err(|e| format!("Failed to write: {}", e))?;
    }

    Ok(out_path)
}

/// One thing to put into an archive: where it is, what it is called inside, and whether
/// it is a folder.
struct ArchiveItem {
    source: std::path::PathBuf,
    name: String,
    is_dir: bool,
}

/// The items of `path` and, for a folder, everything below it (links are left out, as
/// in a copy: following one can leave the selection or loop).
fn collect_archive_items(path: &Path, name: &str, out: &mut Vec<ArchiveItem>) -> Result<(), String> {
    let meta = std::fs::symlink_metadata(path).map_err(|e| format!("Failed to read {}: {}", path.display(), e))?;
    if crate::files::is_reparse_point(&meta) {
        return Err(format!("cannot archive a link or junction: {}", path.display()));
    }
    if meta.is_dir() {
        out.push(ArchiveItem { source: path.to_path_buf(), name: format!("{name}/"), is_dir: true });
        let mut children: Vec<_> = std::fs::read_dir(path)
            .map_err(|e| format!("Failed to read {}: {}", path.display(), e))?
            .collect::<Result<_, _>>()
            .map_err(|e| format!("Failed to read {}: {}", path.display(), e))?;
        children.sort_by_key(|entry| entry.file_name());
        for child in children {
            let child_meta = std::fs::symlink_metadata(child.path()).map_err(|e| format!("Failed to read entry: {}", e))?;
            if crate::files::is_reparse_point(&child_meta) {
                continue;
            }
            let child_name = format!("{name}/{}", child.file_name().to_string_lossy());
            collect_archive_items(&child.path(), &child_name, out)?;
        }
    } else {
        out.push(ArchiveItem { source: path.to_path_buf(), name: name.to_string(), is_dir: false });
    }
    Ok(())
}

pub fn create_archive(files: &[std::path::PathBuf], archive_path: &Path) -> Result<(), String> {
    // Plan first: a name used twice or a file that cannot be read is reported before
    // anything is written, and an existing archive is never truncated.
    let mut items = Vec::new();
    for file_path in files {
        let name = file_path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| file_path.to_string_lossy().to_string());
        collect_archive_items(file_path, &name, &mut items)?;
    }
    let mut seen = std::collections::HashSet::new();
    for item in &items {
        if !seen.insert(item.name.to_lowercase()) {
            return Err(format!("two items would have the same name in the archive: {}", item.name.trim_end_matches('/')));
        }
    }

    let file = create_new_file(archive_path).map_err(|e| format!("Failed to create archive: {}", e))?;
    let written = write_archive(file, &items);
    if written.is_err() {
        // A half-written archive is worse than none.
        let _ = std::fs::remove_file(archive_path);
    }
    written
}

fn write_archive(file: File, items: &[ArchiveItem]) -> Result<(), String> {
    let mut writer = zip::ZipWriter::new(file);
    let options = zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    for item in items {
        if item.is_dir {
            writer.add_directory(item.name.clone(), options).map_err(|e| format!("Failed to add dir: {}", e))?;
        } else {
            writer.start_file(&item.name, options).map_err(|e| format!("Failed to start file: {}", e))?;
            let mut f = File::open(&item.source).map_err(|e| format!("Failed to open file: {}", e))?;
            std::io::copy(&mut f, &mut writer).map_err(|e| format!("Failed to write: {}", e))?;
        }
    }
    writer.finish().map_err(|e| format!("Failed to finish archive: {}", e))?;
    Ok(())
}

pub fn is_zip_file(path: &Path) -> bool {
    if let Ok(mut file) = File::open(path) {
        let mut magic = [0u8; 4];
        if file.read_exact(&mut magic).is_ok() {
            return magic == [0x50, 0x4B, 0x03, 0x04];
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Scratch directory under the system temp dir, removed on drop (also on panic).
    struct Scratch(std::path::PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let dir = std::env::temp_dir().join(format!("unidtk-archive-{name}-{}-{nanos}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            Scratch(dir)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn write_zip(path: &Path, entries: &[(&str, &[u8])]) {
        let file = File::create(path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        let options = zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Stored);
        for (name, data) in entries {
            writer.start_file(*name, options).unwrap();
            writer.write_all(data).unwrap();
        }
        writer.finish().unwrap();
    }

    #[test]
    fn extract_never_overwrites_and_writes_nothing_when_a_file_would_be_replaced() {
        let root = Scratch::new("zip-clash");
        let archive_path = root.0.join("a.zip");
        let dest = root.0.join("out");
        std::fs::create_dir_all(&dest).unwrap();
        std::fs::write(dest.join("b.txt"), "keep me").unwrap();
        write_zip(&archive_path, &[("a.txt", b"new"), ("b.txt", b"replacement")]);

        let result = extract_archive(&archive_path, &dest);

        assert!(result.unwrap_err().contains("nothing was extracted"));
        assert_eq!(std::fs::read_to_string(dest.join("b.txt")).unwrap(), "keep me");
        assert!(!dest.join("a.txt").exists(), "no entry was written before the conflict was found");
        let single = extract_single(&archive_path, "b.txt", &dest);
        assert!(single.is_err());
        assert_eq!(std::fs::read_to_string(dest.join("b.txt")).unwrap(), "keep me");
    }

    #[test]
    fn create_archive_keeps_the_tree_and_refuses_an_existing_target_or_a_name_clash() {
        let root = Scratch::new("zip-create");
        let folder = root.0.join("docs");
        std::fs::create_dir_all(folder.join("inner")).unwrap();
        std::fs::write(folder.join("a.txt"), "a").unwrap();
        std::fs::write(folder.join("inner").join("b.txt"), "b").unwrap();
        let target = root.0.join("out.zip");

        create_archive(&[folder.clone()], &target).unwrap();
        let info = open_archive(&target).unwrap();
        let names: Vec<String> = info.entries.iter().map(|e| e.name.clone()).collect();
        assert!(names.contains(&"docs/a.txt".to_string()), "{names:?}");
        assert!(names.contains(&"docs/inner/b.txt".to_string()), "{names:?}");

        // An existing archive is not truncated.
        let before = std::fs::read(&target).unwrap();
        assert!(create_archive(&[folder.clone()], &target).is_err());
        assert_eq!(std::fs::read(&target).unwrap(), before);

        // Two items with one name collide before anything is written.
        let other = root.0.join("elsewhere");
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(other.join("a.txt"), "x").unwrap();
        std::fs::write(root.0.join("a.txt"), "y").unwrap();
        let clash = root.0.join("clash.zip");
        let result = create_archive(&[other.join("a.txt"), root.0.join("a.txt")], &clash);
        assert!(result.unwrap_err().contains("same name"));
        assert!(!clash.exists());
    }

    #[test]
    fn extract_rejects_entries_that_escape_the_destination() {
        let root = Scratch::new("zip-slip");
        let archive_path = root.0.join("evil.zip");
        let dest = root.0.join("out");
        std::fs::create_dir_all(&dest).unwrap();
        write_zip(&archive_path, &[("../escaped.txt", b"x")]);

        let result = extract_archive(&archive_path, &dest);

        assert!(result.is_err(), "a traversal entry must abort extraction");
        assert!(!root.0.join("escaped.txt").exists());
    }

    #[test]
    fn extract_stops_at_the_total_size_limit_and_removes_the_partial_file() {
        let root = Scratch::new("zip-limit");
        let archive_path = root.0.join("big.zip");
        let dest = root.0.join("out");
        std::fs::create_dir_all(&dest).unwrap();
        write_zip(&archive_path, &[("a.bin", &[7u8; 40]), ("b.bin", &[7u8; 40])]);

        // 40 bytes fit, the second entry would bring the total to 80 > 60.
        let result = extract_archive_with_limit(&archive_path, &dest, 60);

        assert!(result.is_err());
        assert!(!dest.join("b.bin").exists(), "partial file must be removed on limit");
    }

    #[test]
    fn extract_within_the_size_limit_succeeds() {
        let root = Scratch::new("zip-ok");
        let archive_path = root.0.join("ok.zip");
        let dest = root.0.join("out");
        std::fs::create_dir_all(&dest).unwrap();
        write_zip(&archive_path, &[("a.bin", &[7u8; 40]), ("b.bin", &[7u8; 40])]);

        let extracted = extract_archive_with_limit(&archive_path, &dest, 80).unwrap();

        assert_eq!(extracted.len(), 2);
        assert_eq!(std::fs::read(dest.join("b.bin")).unwrap().len(), 40);
    }
}
