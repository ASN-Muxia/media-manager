//! SQLite 数据层：连接管理与表结构初始化。
//!
//! 4 张核心表：
//! - `settings`      键值设置（tmdbKey / mediaDirs / potPlayerPath …）
//! - `media`         影视条目（电影 & 剧集主记录）
//! - `episodes`      剧集单集（外键 -> media）
//! - `scan_history`  媒体库扫描记录

use rusqlite::Connection;
use std::path::PathBuf;
use tauri::{AppHandle, Manager};

/// 数据库文件名。
const DB_FILE: &str = "media.db";

/// 建表语句（幂等，可重复执行）。
const SCHEMA: &str = r#"
PRAGMA journal_mode = WAL;
PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS settings (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS media (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    type           TEXT    NOT NULL DEFAULT 'movie',
    title          TEXT    NOT NULL,
    original_title TEXT,
    year           INTEGER,
    tmdb_id        INTEGER,
    poster_path    TEXT,
    backdrop_path  TEXT,
    overview       TEXT,
    vote_average   REAL,
    genre          TEXT,
    file_path      TEXT,
    file_name      TEXT,
    file_size      INTEGER,
    duration       INTEGER,
    added_at       INTEGER NOT NULL,
    updated_at     INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS episodes (
    id        INTEGER PRIMARY KEY AUTOINCREMENT,
    media_id  INTEGER NOT NULL REFERENCES media(id) ON DELETE CASCADE,
    season    INTEGER NOT NULL DEFAULT 1,
    episode   INTEGER NOT NULL DEFAULT 1,
    title     TEXT,
    file_path TEXT,
    file_name TEXT,
    file_size INTEGER,
    duration  INTEGER,
    added_at  INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS scan_history (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    path        TEXT    NOT NULL,
    started_at  INTEGER NOT NULL,
    finished_at INTEGER,
    status      TEXT    NOT NULL DEFAULT 'running',
    found       INTEGER NOT NULL DEFAULT 0,
    added       INTEGER NOT NULL DEFAULT 0,
    message     TEXT
);

CREATE INDEX IF NOT EXISTS idx_media_type        ON media(type);
CREATE INDEX IF NOT EXISTS idx_media_tmdb        ON media(tmdb_id);
CREATE UNIQUE INDEX IF NOT EXISTS idx_media_path ON media(file_path) WHERE file_path IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_episodes_media    ON episodes(media_id);
CREATE UNIQUE INDEX IF NOT EXISTS idx_episodes_path ON episodes(file_path) WHERE file_path IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_scan_path         ON scan_history(path);
"#;

/// 应用数据目录（不存在则创建）。
pub fn data_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("获取应用数据目录失败: {e}"))?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建应用数据目录失败: {e}"))?;
    Ok(dir)
}

/// 数据库文件完整路径。
pub fn db_path(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(data_dir(app)?.join(DB_FILE))
}

/// 打开数据库并确保表结构就绪。
pub fn open(app: &AppHandle) -> Result<Connection, String> {
    let path = db_path(app)?;
    let conn = Connection::open(&path).map_err(|e| format!("打开数据库失败: {e}"))?;
    conn.execute_batch(SCHEMA)
        .map_err(|e| format!("初始化表结构失败: {e}"))?;
    Ok(conn)
}

/// 列出用户表名（健康检查用）。
pub fn tables(conn: &Connection) -> Result<Vec<String>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT name FROM sqlite_master \
             WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| r.get::<_, String>(0))
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| e.to_string())?);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_creates_exactly_four_tables() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(SCHEMA).unwrap();
        let names = tables(&conn).unwrap();
        for t in ["episodes", "media", "scan_history", "settings"] {
            assert!(names.contains(&t.to_string()), "缺少表 {t}，实际为 {names:?}");
        }
        assert_eq!(names.len(), 4, "期望 4 张表，实际为 {names:?}");
    }

    #[test]
    fn schema_is_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(SCHEMA).unwrap();
        conn.execute_batch(SCHEMA).unwrap();
        assert_eq!(tables(&conn).unwrap().len(), 4);
    }
}
