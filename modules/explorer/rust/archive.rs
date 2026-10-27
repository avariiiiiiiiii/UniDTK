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

pub fn extract_archive(archive_path: &Path, dest_dir: &Path) -> Result<Vec<String>, String> {
    let file = File::open(archive_path).map_err(|e| format!("Failed to open archive: {}", e))?;
    let mut archive = ZipArchive::new(file).map_err(|e| format!("Failed to read zip: {}", e))?;

    let mut extracted = Vec::new();

    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| format!("Failed to read entry: {}", e))?;
        let entry_path = entry.mangled_name().to_string_lossy().to_string();
        let out_path = dest_dir.join(&entry_path);

        if entry.is_dir() {
            std::fs::create_dir_all(&out_path).map_err(|e| format!("Failed to create dir: {}", e))?;
        } else {
            if let Some(parent) = out_path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| format!("Failed to create parent: {}", e))?;
            }
            let mut out_file = File::create(&out_path).map_err(|e| format!("Failed to create file: {}", e))?;
            std::io::copy(&mut entry, &mut out_file).map_err(|e| format!("Failed to write: {}", e))?;
        }

        extracted.push(out_path.to_string_lossy().to_string());
    }

    Ok(extracted)
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
