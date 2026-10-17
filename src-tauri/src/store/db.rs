//! SQLite 数据层：**连接入口**。
//!
//! 这里只回答三件事：数据库文件在哪、怎么打开、有哪些表。
//! 建表语句与增量迁移住在同级的 `schema.rs`（本模块 re-export，老调用方
//! 写 `db::migrate` / `db::SCHEMA` / `db::ensure_column` 照样能用）。
//!
//! 核心表：
//! - `settings`      键值设置（tmdbKey / mediaDirs / potPlayerPath …）
//! - `media`         影视条目（电影 & 剧集主记录；剧集即 Series 实体）
//! - `episodes`      剧集单集（外键 -> media）
//! - `scan_history`  媒体库扫描记录
//! - `categories` / `media_categories`  分类（多对多）
//! - `tags` / `media_tags`              标签（多对多）
//! v6 起另有 `playback_state`（观看状态）与 `provider_cache`（元数据缓存）。

use rusqlite::Connection;
use std::path::PathBuf;
use tauri::{AppHandle, Manager};

// 迁移与建表住在 schema.rs；re-export 让 `db::migrate` 这种老写法继续成立。
// `store` 是私有模块，rustc 判断不出这些符号会被上层用到，故显式放行。
#[allow(unused_imports)]
pub use crate::store::schema::{
    column_exists, ensure_column, migrate, SCHEMA, SCHEMA_VERSION,
};
// `now_epoch` 已下沉到 `util`（时间戳不属于数据层，且 `core` 用它时会反向依赖）。
#[allow(unused_imports)]
pub use crate::util::now_epoch;

/// 数据库文件名。
const DB_FILE: &str = "media.db";

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
///
/// 刮削后台线程会**另开一条连接**同时写库（WAL 模式下允许），
/// 因此这里设一个忙等超时，避免并发写时直接抛 `SQLITE_BUSY`。
pub fn open(app: &AppHandle) -> Result<Connection, String> {
    let path = db_path(app)?;
    let conn = Connection::open(&path).map_err(|e| format!("打开数据库失败: {e}"))?;
    conn.busy_timeout(std::time::Duration::from_secs(10))
        .map_err(|e| format!("设置数据库忙等超时失败: {e}"))?;
    migrate(&conn)?;
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

    /// `db` 必须把 `schema` 的东西原样透出去，否则几百处老调用会编译失败。
    #[test]
    fn reexports_schema_symbols() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        let v: i32 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, SCHEMA_VERSION);
        assert!(!SCHEMA.is_empty());
        ensure_column(&conn, "media", "probe_col", "TEXT").unwrap();
        assert!(column_exists(&conn, "media", "probe_col").unwrap());
    }

    /// 时间戳来自 util，但 `db::now_epoch()` 必须还能用（调用方零改动）。
    #[test]
    fn now_epoch_is_reexported_from_util() {
        assert!(now_epoch() > 1_609_459_200);
    }
}
