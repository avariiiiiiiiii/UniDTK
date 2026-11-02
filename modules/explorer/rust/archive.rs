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

    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| format!("Failed to read entry: {}", e))?;
        // enclosed_name rejects absolute paths and `..` components; the starts_with
        // check is a second guard on the joined path.
        let relative = entry
            .enclosed_name()
            .map(Path::to_path_buf)
            .ok_or_else(|| format!("Unsafe path in archive: {}", entry.name()))?;
        let out_path = dest_dir.join(&relative);
        if !out_path.starts_with(dest_dir) {
            return Err(format!("Unsafe path in archive: {}", entry.name()));
        }

        if entry.is_dir() {
            std::fs::create_dir_all(&out_path).map_err(|e| format!("Failed to create dir: {}", e))?;
        } else {
            if let Some(parent) = out_path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| format!("Failed to create parent: {}", e))?;
            }
            let mut out_file = File::create(&out_path).map_err(|e| format!("Failed to create file: {}", e))?;
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
        let mut out_file = File::create(&out_path).map_err(|e| format!("Failed to create file: {}", e))?;
        std::io::copy(&mut entry, &mut out_file).map_err(|e| format!("Failed to write: {}", e))?;
    }

    Ok(out_path)
}

pub fn create_archive(files: &[std::path::PathBuf], archive_path: &Path) -> Result<(), String> {
    let file = File::create(archive_path).map_err(|e| format!("Failed to create archive: {}", e))?;
    let mut writer = zip::ZipWriter::new(file);
    let options = zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);

    for file_path in files {
        let name = file_path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| file_path.to_string_lossy().to_string());

        if file_path.is_dir() {
            writer.add_directory(name, options).map_err(|e| format!("Failed to add dir: {}", e))?;
        } else {
            writer.start_file(&name, options).map_err(|e| format!("Failed to start file: {}", e))?;
            let mut f = File::open(file_path).map_err(|e| format!("Failed to open file: {}", e))?;
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
