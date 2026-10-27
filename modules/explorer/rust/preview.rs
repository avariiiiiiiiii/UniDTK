use std::path::Path;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FilePreview {
    pub path: String,
    pub name: String,
    pub extension: String,
    pub mime_type: String,
    pub category: FileCategory,
    pub size: Option<u64>,
    pub is_text: bool,
    pub is_image: bool,
    pub is_code: bool,
    pub is_media: bool,
    pub is_archive: bool,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub enum FileCategory {
    Image,
    Video,
    Audio,
    Text,
    Code,
    Archive,
    Document,
    Font,
    Executable,
    Unknown,
}

pub fn get_preview_info(path: &Path) -> FilePreview {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();

    let extension = path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();

    let category = categorize_file(&extension);
    let mime_type = get_mime_type(&extension);
    let size = std::fs::metadata(path).ok().map(|m| m.len());

    FilePreview {
        path: path.to_string_lossy().to_string(),
        name,
        extension,
        mime_type,
        category: category.clone(),
        size,
        is_text: category == FileCategory::Text || category == FileCategory::Code,
        is_image: category == FileCategory::Image,
        is_code: category == FileCategory::Code,
        is_media: category == FileCategory::Video || category == FileCategory::Audio,
        is_archive: category == FileCategory::Archive,
    }
}

pub fn get_text_preview(path: &Path, max_bytes: usize) -> Option<String> {
    let mut file = std::fs::File::open(path).ok()?;
    let mut buffer = vec![0u8; max_bytes.min(1024 * 1024)];
    let bytes_read = std::io::Read::read(&mut file, &mut buffer).ok()?;
    buffer.truncate(bytes_read);

    Some(String::from_utf8_lossy(&buffer).to_string())
}

pub fn get_file_icon(extension: &str) -> &'static str {
    match extension {
        "jpg" | "jpeg" | "png" | "gif" | "bmp" | "svg" | "webp" | "ico" => "image",
        "mp4" | "avi" | "mkv" | "mov" | "wmv" | "flv" | "webm" => "video",
        "mp3" | "wav" | "flac" | "ogg" | "aac" | "wma" | "m4a" => "audio",
        "txt" | "md" | "log" | "csv" | "json" | "xml" | "yaml" | "yml" => "text",
        "rs" | "py" | "js" | "ts" | "jsx" | "tsx" | "java" | "c" | "cpp" | "h" | "go" | "rb" => "code",
        "zip" | "rar" | "7z" | "tar" | "gz" | "bz2" | "xz" => "archive",
        "pdf" | "doc" | "docx" | "xls" | "xlsx" | "ppt" | "pptx" => "document",
        "ttf" | "otf" | "woff" | "woff2" | "eot" => "font",
        "exe" | "msi" | "dll" | "app" | "deb" | "rpm" => "executable",
        _ => "file",
    }
}

fn categorize_file(extension: &str) -> FileCategory {
    match extension {
        "jpg" | "jpeg" | "png" | "gif" | "bmp" | "svg" | "webp" | "ico" | "tiff" | "raw" => {
            FileCategory::Image
        }
        "mp4" | "avi" | "mkv" | "mov" | "wmv" | "flv" | "webm" | "m4v" | "3gp" => {
            FileCategory::Video
        }
        "mp3" | "wav" | "flac" | "ogg" | "aac" | "wma" | "m4a" | "opus" | "aiff" => {
            FileCategory::Audio
        }
        "txt" | "md" | "log" | "csv" | "json" | "xml" | "yaml" | "yml" | "toml" | "ini"
        | "cfg" | "conf" | "env" | "rst" | "tex" | "rtf" | "odt" => FileCategory::Text,
        "rs" | "py" | "js" | "ts" | "jsx" | "tsx" | "java" | "c" | "cpp" | "h" | "hpp"
        | "go" | "rb" | "php" | "swift" | "kt" | "scala" | "clj" | "ex" | "exs"
        | "hs" | "ml" | "r" | "lua" | "pl" | "sh" | "bash" | "zsh" | "fish"
        | "ps1" | "bat" | "cmd" | "vbs" | "sql" | "graphql" | "proto" => FileCategory::Code,
        "zip" | "rar" | "7z" | "tar" | "gz" | "bz2" | "xz" | "zst" | "lz4" => {
            FileCategory::Archive
        }
        "pdf" | "doc" | "docx" | "xls" | "xlsx" | "ppt" | "pptx" | "epub" => {
            FileCategory::Document
        }
        "ttf" | "otf" | "woff" | "woff2" | "eot" => FileCategory::Font,
        "exe" | "msi" | "dll" | "app" | "deb" | "rpm" | "apk" | "appimage" => {
            FileCategory::Executable
        }
        _ => FileCategory::Unknown,
    }
}

fn get_mime_type(extension: &str) -> String {
    match extension {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "webp" => "image/webp",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "flac" => "audio/flac",
        "ogg" => "audio/ogg",
        "pdf" => "application/pdf",
        "json" => "application/json",
        "xml" => "application/xml",
        "zip" => "application/zip",
        "txt" | "md" | "log" => "text/plain",
        "html" | "htm" => "text/html",
        "css" => "text/css",
        "js" => "application/javascript",
        "py" => "text/x-python",
        "rs" => "text/x-rust",
        _ => "application/octet-stream",
    }
    .to_string()
}
