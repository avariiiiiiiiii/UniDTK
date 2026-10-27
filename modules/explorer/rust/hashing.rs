use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;

use sha2::Digest;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FileHash {
    pub md5: String,
    pub sha256: String,
    pub crc32: String,
}

pub fn hash_file(path: &Path) -> Result<FileHash, String> {
    let file = File::open(path).map_err(|e| format!("Failed to open file: {}", e))?;
    let mut reader = BufReader::new(file);

    let mut md5_hasher = md5::Md5::new();
    let mut sha256_hasher = sha2::Sha256::new();
    let mut crc32_hasher = crc32fast::Hasher::new();

    let mut buffer = [0u8; 8192];

    loop {
        let bytes_read = reader.read(&mut buffer).map_err(|e| format!("Read error: {}", e))?;
        if bytes_read == 0 {
            break;
        }

        let data = &buffer[..bytes_read];
        Digest::update(&mut md5_hasher, data);
        Digest::update(&mut sha256_hasher, data);
        crc32_hasher.update(data);
    }

    Ok(FileHash {
        md5: format!("{:x}", md5_hasher.finalize()),
        sha256: format!("{:x}", sha256_hasher.finalize()),
        crc32: format!("{:08x}", crc32_hasher.finalize()),
    })
}

pub fn hash_file_md5(path: &Path) -> Result<String, String> {
    let file = File::open(path).map_err(|e| format!("Failed to open file: {}", e))?;
    let mut reader = BufReader::new(file);
    let mut hasher = md5::Md5::new();
    let mut buffer = [0u8; 8192];

    loop {
        let bytes_read = reader.read(&mut buffer).map_err(|e| format!("Read error: {}", e))?;
        if bytes_read == 0 {
            break;
        }
        Digest::update(&mut hasher, &buffer[..bytes_read]);
    }

    Ok(format!("{:x}", hasher.finalize()))
}

pub fn hash_file_sha256(path: &Path) -> Result<String, String> {
    let file = File::open(path).map_err(|e| format!("Failed to open file: {}", e))?;
    let mut reader = BufReader::new(file);
    let mut hasher = sha2::Sha256::new();
    let mut buffer = [0u8; 8192];

    loop {
        let bytes_read = reader.read(&mut buffer).map_err(|e| format!("Read error: {}", e))?;
        if bytes_read == 0 {
            break;
        }
        Digest::update(&mut hasher, &buffer[..bytes_read]);
    }

    Ok(format!("{:x}", hasher.finalize()))
}
