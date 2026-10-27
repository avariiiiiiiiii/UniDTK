use std::path::PathBuf;
use redb::{Database, ReadableTable, TableDefinition};
use serde::{Deserialize, Serialize};

const WIDGET_SETTINGS: TableDefinition<&str, &[u8]> = TableDefinition::new("widget_settings");
const FOLDERS: TableDefinition<&str, &[u8]> = TableDefinition::new("folders");

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct FolderItem {
    pub id: String,
    pub name: String,
    pub target: String,
    pub kind: String, // "url", "file", "folder"
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct FolderRecord {
    pub id: String,
    pub name: String,
    pub icon_svg: String,
    #[serde(default)]
    pub items: Vec<FolderItem>,
}

pub fn default_db_path() -> PathBuf {
    let base = dirs::data_local_dir().unwrap_or_else(|| PathBuf::from("."));
    base.join("UniDTK").join("unidtk.redb")
}

pub fn list_folders() -> Result<Vec<FolderRecord>, String> {
    let path = default_db_path();
    if !path.exists() {
        return Ok(Vec::new());
    }
    let db = Database::open(&path).map_err(|e| format!("redb open error: {e}"))?;
    let txn = db.begin_read().map_err(|e| format!("redb begin_read error: {e}"))?;
    let table = match txn.open_table(FOLDERS) {
        Ok(t) => t,
        Err(_) => return Ok(Vec::new()),
    };
    let mut list = Vec::new();
    let iter = table.iter().map_err(|e| format!("redb iter error: {e}"))?;
    for row in iter {
        let (_key, val) = row.map_err(|e| format!("redb row error: {e}"))?;
        if let Ok(rec) = serde_json::from_slice::<FolderRecord>(val.value()) {
            list.push(rec);
        }
    }
    Ok(list)
}

pub fn get_widget_config() -> Result<Option<serde_json::Value>, String> {
    let path = default_db_path();
    if !path.exists() {
        return Ok(None);
    }
    let db = Database::open(&path).map_err(|e| format!("redb open error: {e}"))?;
    let txn = db.begin_read().map_err(|e| format!("redb begin_read error: {e}"))?;
    let table = match txn.open_table(WIDGET_SETTINGS) {
        Ok(t) => t,
        Err(_) => return Ok(None),
    };
    match table.get("config").map_err(|e| format!("redb get error: {e}"))? {
        Some(val) => {
            let res = serde_json::from_slice(val.value()).map_err(|e| format!("json error: {e}"))?;
            Ok(Some(res))
        }
        None => Ok(None),
    }
}
