//! 设置项持久化：基于 `settings` 表（key/value）。
//!
//! 前端契约（`src/stores/settingsStore.ts`）：`{ tmdbKey, mediaDirs, potPlayerPath }`。

use crate::db;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use tauri::State;

const KEY_TMDB_KEY: &str = "tmdbKey";
const KEY_MEDIA_DIRS: &str = "mediaDirs";
const KEY_POTPLAYER: &str = "potPlayerPath";

/// 设置结构体，字段与前端 settingsStore 一一对应。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    #[serde(default)]
    pub tmdb_key: String,
    #[serde(default)]
    pub media_dirs: Vec<String>,
    #[serde(default)]
    pub pot_player_path: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            tmdb_key: String::new(),
            media_dirs: Vec::new(),
            pot_player_path: String::new(),
        }
    }
}

/// 数据库健康信息。
#[derive(Debug, Serialize)]
pub struct DbInfo {
    pub tables: Vec<String>,
}

fn read_value(conn: &Connection, key: &str) -> Option<String> {
    conn.query_row(
        "SELECT value FROM settings WHERE key = ?1",
        params![key],
        |r| r.get::<_, String>(0),
    )
    .ok()
}

/// 读取媒体目录列表（供扫描命令复用）。
pub fn load_media_dirs(conn: &Connection) -> Vec<String> {
    read_value(conn, KEY_MEDIA_DIRS)
        .and_then(|raw| serde_json::from_str::<Vec<String>>(&raw).ok())
        .unwrap_or_default()
}

/// 读取 PotPlayer 路径（供播放命令复用）。
pub fn load_potplayer_path(conn: &Connection) -> String {
    read_value(conn, KEY_POTPLAYER).unwrap_or_default()
}

/// 读取 TMDB API Key（供刮削命令复用）。
pub fn load_tmdb_key(conn: &Connection) -> String {
    read_value(conn, KEY_TMDB_KEY).unwrap_or_default()
}

fn write_value(conn: &Connection, key: &str, value: &str) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO settings (key, value) VALUES (?1, ?2) \
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

/// 读取全部设置（缺失项回落默认值）。
#[tauri::command]
pub fn get_settings(state: State<'_, Mutex<Connection>>) -> Result<Settings, String> {
    let conn = state.lock().map_err(|e| e.to_string())?;
    let media_dirs = read_value(&conn, KEY_MEDIA_DIRS)
        .and_then(|raw| serde_json::from_str::<Vec<String>>(&raw).ok())
        .unwrap_or_default();
    Ok(Settings {
        tmdb_key: read_value(&conn, KEY_TMDB_KEY).unwrap_or_default(),
        media_dirs,
        pot_player_path: read_value(&conn, KEY_POTPLAYER).unwrap_or_default(),
    })
}

/// 写入全部设置（upsert）。
#[tauri::command]
pub fn set_settings(
    state: State<'_, Mutex<Connection>>,
    settings: Settings,
) -> Result<(), String> {
    let conn = state.lock().map_err(|e| e.to_string())?;
    let dirs = serde_json::to_string(&settings.media_dirs).map_err(|e| e.to_string())?;
    write_value(&conn, KEY_TMDB_KEY, &settings.tmdb_key).map_err(|e| e.to_string())?;
    write_value(&conn, KEY_MEDIA_DIRS, &dirs).map_err(|e| e.to_string())?;
    write_value(&conn, KEY_POTPLAYER, &settings.pot_player_path).map_err(|e| e.to_string())?;
    Ok(())
}

/// 数据层自检：返回已建的表名。
#[tauri::command]
pub fn db_info(state: State<'_, Mutex<Connection>>) -> Result<DbInfo, String> {
    let conn = state.lock().map_err(|e| e.to_string())?;
    Ok(DbInfo {
        tables: db::tables(&conn)?,
    })
}

/// 在资源管理器中打开应用数据目录（数据库所在位置，便于排查问题）。
#[tauri::command]
pub fn open_data_dir(app: tauri::AppHandle) -> Result<String, String> {
    let dir = db::data_dir(&app)?;
    let path = dir.to_string_lossy().to_string();
    std::process::Command::new("explorer")
        .arg(&path)
        .spawn()
        .map_err(|e| format!("打开数据目录失败: {e}"))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn memory_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);",
        )
        .unwrap();
        conn
    }

    #[test]
    fn write_then_read_roundtrip() {
        let conn = memory_db();
        write_value(&conn, KEY_TMDB_KEY, "abc123").unwrap();
        assert_eq!(read_value(&conn, KEY_TMDB_KEY).as_deref(), Some("abc123"));
    }

    #[test]
    fn upsert_overwrites_same_key() {
        let conn = memory_db();
        write_value(&conn, KEY_POTPLAYER, "old").unwrap();
        write_value(&conn, KEY_POTPLAYER, "new").unwrap();
        assert_eq!(read_value(&conn, KEY_POTPLAYER).as_deref(), Some("new"));
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM settings", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1, "upsert 不应产生重复行");
    }

    #[test]
    fn missing_key_returns_none() {
        let conn = memory_db();
        assert!(read_value(&conn, KEY_MEDIA_DIRS).is_none());
    }

    #[test]
    fn media_dirs_json_roundtrip() {
        let dirs = vec!["D:\\Movies".to_string(), "E:\\TV".to_string()];
        let raw = serde_json::to_string(&dirs).unwrap();
        let back: Vec<String> = serde_json::from_str(&raw).unwrap();
        assert_eq!(dirs, back);
    }
}
