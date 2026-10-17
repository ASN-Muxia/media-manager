//! 元数据缓存（`provider_cache` 表）。
//!
//! 缓存放在 `store`、在 `commands` 层旁路调用，**不改动任何 provider 的函数签名
//! 与优先级顺序**（B5）。命中就省一次网络请求，未命中与现在完全一样。
//!
//! 两条纪律：
//! 1. **只缓存成功响应**——一次网络抖动不该把错误记住好几分钟；
//! 2. key 必须带 provider 名，否则 TMDb 和豆瓣的同名查询会互相串数据。

use rusqlite::{params, Connection};
use crate::util::now_epoch;

/// 默认的缓存有效期（秒）：TMDb 这类不常变的数据可以放久一点。
pub const DEFAULT_TTL: i64 = 7 * 24 * 3600;

/// 读缓存。过期或不存在都返回 `None`（当作 miss）。
pub fn get(conn: &Connection, provider: &str, key: &str) -> Option<String> {
    let now = now_epoch();
    conn.query_row(
        "SELECT payload FROM provider_cache \
         WHERE provider = ?1 AND cache_key = ?2 AND expires_at > ?3",
        params![provider, key, now],
        |r| r.get::<_, String>(0),
    )
    .ok()
}

/// 写缓存。同一 key 覆盖（upsert）。
pub fn put(
    conn: &Connection,
    provider: &str,
    key: &str,
    payload: &str,
    ttl_secs: i64,
) -> Result<(), String> {
    let now = now_epoch();
    conn.execute(
        "INSERT INTO provider_cache (provider, cache_key, payload, created_at, expires_at) \
         VALUES (?1, ?2, ?3, ?4, ?5) \
         ON CONFLICT(provider, cache_key) DO UPDATE SET \
             payload = excluded.payload, \
             created_at = excluded.created_at, \
             expires_at = excluded.expires_at",
        params![provider, key, payload, now, now + ttl_secs.max(0)],
    )
    .map_err(|e| format!("写入元数据缓存失败: {e}"))?;
    Ok(())
}

/// 清掉过期行（设置页「清理缓存」与启动时各调一次即可）。
pub fn purge_expired(conn: &Connection) -> Result<usize, String> {
    let n = conn
        .execute(
            "DELETE FROM provider_cache WHERE expires_at <= ?1",
            params![now_epoch()],
        )
        .map_err(|e| e.to_string())?;
    Ok(n)
}

/// 清空某个源（或全部）的缓存。
pub fn clear(conn: &Connection, provider: Option<&str>) -> Result<usize, String> {
    let n = match provider {
        Some(p) => conn
            .execute("DELETE FROM provider_cache WHERE provider = ?1", params![p])
            .map_err(|e| e.to_string())?,
        None => conn
            .execute("DELETE FROM provider_cache", [])
            .map_err(|e| e.to_string())?,
    };
    Ok(n)
}

/// 缓存行数（诊断/设置页展示用）。
pub fn count(conn: &Connection) -> Result<i64, String> {
    conn.query_row("SELECT COUNT(*) FROM provider_cache", [], |r| r.get(0))
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::schema::migrate;

    fn db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        conn
    }

    #[test]
    fn miss_then_hit() {
        let conn = db();
        assert_eq!(get(&conn, "tmdb", "k1"), None);
        put(&conn, "tmdb", "k1", "{\"a\":1}", 3600).unwrap();
        assert_eq!(get(&conn, "tmdb", "k1").as_deref(), Some("{\"a\":1}"));
    }

    /// key 带 provider 名：不同源的同一个 key 不能互相串。
    #[test]
    fn same_key_different_providers_do_not_collide() {
        let conn = db();
        put(&conn, "tmdb", "卧虎藏龙", "tmdb-payload", 3600).unwrap();
        put(&conn, "douban", "卧虎藏龙", "douban-payload", 3600).unwrap();
        assert_eq!(get(&conn, "tmdb", "卧虎藏龙").as_deref(), Some("tmdb-payload"));
        assert_eq!(get(&conn, "douban", "卧虎藏龙").as_deref(), Some("douban-payload"));
    }

    /// 过期就是 miss——缓存不能给出陈旧数据。
    #[test]
    fn expired_entry_is_a_miss() {
        let conn = db();
        put(&conn, "tmdb", "k", "old", -1).unwrap();
        assert_eq!(get(&conn, "tmdb", "k"), None);
    }

    #[test]
    fn put_twice_is_an_upsert_not_a_duplicate() {
        let conn = db();
        put(&conn, "tmdb", "k", "v1", 3600).unwrap();
        put(&conn, "tmdb", "k", "v2", 3600).unwrap();
        assert_eq!(get(&conn, "tmdb", "k").as_deref(), Some("v2"));
        assert_eq!(count(&conn).unwrap(), 1, "不应产生重复行");
    }

    #[test]
    fn purge_and_clear_work() {
        let conn = db();
        put(&conn, "tmdb", "a", "1", 3600).unwrap();
        put(&conn, "tmdb", "b", "2", -1).unwrap();
        put(&conn, "omdb", "c", "3", 3600).unwrap();
        assert_eq!(purge_expired(&conn).unwrap(), 1);
        assert_eq!(count(&conn).unwrap(), 2);
        assert_eq!(clear(&conn, Some("tmdb")).unwrap(), 1);
        assert_eq!(clear(&conn, None).unwrap(), 1);
        assert_eq!(count(&conn).unwrap(), 0);
    }
}
