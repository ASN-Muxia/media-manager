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
pub const SCHEMA: &str = r#"
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
    group_key      TEXT,
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
    added_at  INTEGER NOT NULL,
    tmdb_id    INTEGER,
    overview   TEXT,
    still_path TEXT,
    air_date   TEXT
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
-- file_path 用普通唯一索引：SQLite 中 NULL 互不冲突，因此剧集主记录（file_path 为 NULL）可多条共存，
-- 同时让 `ON CONFLICT(file_path)` upsert 生效。
CREATE UNIQUE INDEX IF NOT EXISTS idx_media_path ON media(file_path);
CREATE INDEX IF NOT EXISTS idx_episodes_media    ON episodes(media_id);
CREATE UNIQUE INDEX IF NOT EXISTS idx_episodes_path ON episodes(file_path);
CREATE INDEX IF NOT EXISTS idx_scan_path         ON scan_history(path);
"#;

/// 当前 Unix 时间戳（秒）。
pub fn now_epoch() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

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

/// 打开数据库并确保表结构就绪（含增量迁移）。
pub fn open(app: &AppHandle) -> Result<Connection, String> {
    let path = db_path(app)?;
    let conn = Connection::open(&path).map_err(|e| format!("打开数据库失败: {e}"))?;
    migrate(&conn)?;
    Ok(conn)
}

/// 当前结构版本。
/// v1 = 初始 4 表；v2 = episodes 增加 tmdb_id/overview/still_path/air_date；
/// v3 = media 增加 group_key（剧集分组键，同名剧集按目录区分）。
pub const SCHEMA_VERSION: i32 = 3;

/// 建表 + 增量迁移。对**已存在的老库**用 `ALTER TABLE` 补列，保证升级不丢数据。
pub fn migrate(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(SCHEMA)
        .map_err(|e| format!("初始化表结构失败: {e}"))?;

    // v1 -> v2：为单集补充 TMDB 元数据列
    ensure_column(conn, "episodes", "tmdb_id", "INTEGER")?;
    ensure_column(conn, "episodes", "overview", "TEXT")?;
    ensure_column(conn, "episodes", "still_path", "TEXT")?;
    ensure_column(conn, "episodes", "air_date", "TEXT")?;

    // v2 -> v3：剧集分组键（同名剧集按所在目录区分，避免被错误合并）
    ensure_column(conn, "media", "group_key", "TEXT")?;
    // 索引必须在补列之后建立，否则老库会因列不存在而失败
    conn.execute_batch("CREATE INDEX IF NOT EXISTS idx_media_group ON media(type, group_key);")
        .map_err(|e| format!("创建分组索引失败: {e}"))?;

    conn.pragma_update(None, "user_version", SCHEMA_VERSION)
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// 若列不存在则添加（幂等）。
pub fn ensure_column(
    conn: &Connection,
    table: &str,
    column: &str,
    decl: &str,
) -> Result<(), String> {
    if column_exists(conn, table, column)? {
        return Ok(());
    }
    conn.execute(&format!("ALTER TABLE {table} ADD COLUMN {column} {decl}"), [])
        .map_err(|e| format!("为 {table} 添加列 {column} 失败: {e}"))?;
    Ok(())
}

fn column_exists(conn: &Connection, table: &str, column: &str) -> Result<bool, String> {
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info({table})"))
        .map_err(|e| e.to_string())?;
    let names = stmt
        .query_map([], |r| r.get::<_, String>(1))
        .map_err(|e| e.to_string())?;
    for n in names {
        if n.map_err(|e| e.to_string())? == column {
            return Ok(true);
        }
    }
    Ok(false)
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

    #[test]
    fn fresh_schema_has_episode_metadata_columns() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        for col in ["tmdb_id", "overview", "still_path", "air_date"] {
            assert!(column_exists(&conn, "episodes", col).unwrap(), "缺少列 {col}");
        }
        assert!(column_exists(&conn, "media", "group_key").unwrap());
        let v: i32 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, SCHEMA_VERSION);
    }

    /// 老库（media 无 group_key）升级：补列 + 建索引都不能失败。
    #[test]
    fn migrate_adds_group_key_and_index_to_legacy_db() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE media (
                 id INTEGER PRIMARY KEY AUTOINCREMENT, type TEXT NOT NULL DEFAULT 'movie',
                 title TEXT NOT NULL, original_title TEXT, year INTEGER, tmdb_id INTEGER,
                 poster_path TEXT, backdrop_path TEXT, overview TEXT, vote_average REAL,
                 genre TEXT, file_path TEXT, file_name TEXT, file_size INTEGER,
                 duration INTEGER, added_at INTEGER NOT NULL, updated_at INTEGER NOT NULL);
             INSERT INTO media (type, title, added_at, updated_at) VALUES ('tv','老剧',0,0);",
        )
        .unwrap();

        migrate(&conn).unwrap();

        assert!(column_exists(&conn, "media", "group_key").unwrap());
        let title: String = conn
            .query_row("SELECT title FROM media WHERE id = 1", [], |r| r.get(0))
            .unwrap();
        assert_eq!(title, "老剧", "迁移不应丢失原有数据");
        // 再跑一次不应报错（幂等）
        migrate(&conn).unwrap();
        let idx: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name='idx_media_group'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(idx, 1);
    }

    /// 关键：老库升级不能丢数据，也不能因为列已存在而报错。
    #[test]
    fn migrate_upgrades_legacy_episodes_table_without_data_loss() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE episodes (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 media_id INTEGER NOT NULL,
                 season INTEGER NOT NULL DEFAULT 1,
                 episode INTEGER NOT NULL DEFAULT 1,
                 title TEXT, file_path TEXT, file_name TEXT,
                 file_size INTEGER, duration INTEGER, added_at INTEGER NOT NULL);
             INSERT INTO episodes (media_id, season, episode, title, added_at)
             VALUES (1, 1, 1, '老数据', 0);",
        )
        .unwrap();
        assert!(!column_exists(&conn, "episodes", "still_path").unwrap());

        migrate(&conn).unwrap();

        for col in ["tmdb_id", "overview", "still_path", "air_date"] {
            assert!(column_exists(&conn, "episodes", col).unwrap(), "缺少列 {col}");
        }
        let title: String = conn
            .query_row("SELECT title FROM episodes WHERE id = 1", [], |r| r.get(0))
            .unwrap();
        assert_eq!(title, "老数据", "迁移不应丢失原有数据");
    }

    #[test]
    fn ensure_column_is_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE t (a INTEGER);").unwrap();
        ensure_column(&conn, "t", "b", "TEXT").unwrap();
        ensure_column(&conn, "t", "b", "TEXT").unwrap();
        ensure_column(&conn, "t", "b", "INTEGER").unwrap(); // 已存在时忽略新声明
        assert!(column_exists(&conn, "t", "b").unwrap());
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM pragma_table_info('t')", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 2, "不应重复加列");
    }
}
