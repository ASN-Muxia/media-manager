//! 分类与标签：Series 的归类、打标签、筛选入口。
//!
//! ## 分类 vs 标签
//!
//! - **分类（category）**：左侧导航的「大栏目」。可以手动建，也可以由自动识别
//!   （目前是 TMDb 的类型字段）动态补充。全部存在 `categories` 表里，
//!   **没有任何硬编码枚举**——界面上看到什么完全是数据决定的。
//! - **标签（tag）**：比分类更自由的记号（「看过」「收藏」「4K」…），
//!   主要用于筛选，不进左侧导航。
//!
//! ## 与剧集的关联
//! 都是**多对多**：一部剧可以同时属于「剧情」和「悬疑」，也可以有多个标签。
//! 关联表里没有冗余字段，删剧集 / 删分类都靠外键级联，不留孤儿行。

use rusqlite::{params, Connection};
use serde::Serialize;
use std::sync::Mutex;
use tauri::State;

/// 分类。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Category {
    pub id: i64,
    pub name: String,
    /// `manual`（用户手动建） / `auto`（自动识别补出来的）。
    pub kind: String,
    /// 来源：`manual` / `tmdb` / `folder` / `rule`。
    pub origin: String,
    pub sort_order: i64,
    /// 该分类下有多少条影视（左侧导航上直接显示）。
    pub series_count: i64,
}

/// 标签。
#[derive(Debug, Clone, Serialize)]
pub struct Tag {
    pub id: i64,
    pub name: String,
    /// 有多少条影视用了这个标签。
    pub usage_count: i64,
}

/// 自动同步的结果（界面上给用户一句反馈）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncResult {
    pub created: i64,
    pub linked: i64,
    pub scanned: i64,
}

fn now() -> i64 {
    crate::db::now_epoch()
}

/* --------------------------- 分类 --------------------------- */

const CATEGORY_COLUMNS: &str = "c.id, c.name, c.kind, c.origin, c.sort_order, \
     (SELECT COUNT(*) FROM media_categories mc WHERE mc.category_id = c.id) AS series_count";

fn map_category(row: &rusqlite::Row<'_>) -> rusqlite::Result<Category> {
    Ok(Category {
        id: row.get(0)?,
        name: row.get(1)?,
        kind: row.get(2)?,
        origin: row.get(3)?,
        sort_order: row.get(4)?,
        series_count: row.get(5)?,
    })
}

/// 按名字取分类（大小写不敏感），不存在则新建。
///
/// `kind`/`origin` 只在**新建**时生效：已经存在的分类不会被改成别的来源，
/// 否则用户手动建的栏目会被自动同步抢走。
fn upsert_category(conn: &Connection, name: &str, kind: &str, origin: &str) -> Result<i64, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("分类名不能为空".to_string());
    }
    if let Some(id) = conn
        .query_row(
            "SELECT id FROM categories WHERE name = ?1 COLLATE NOCASE",
            params![name],
            |r| r.get::<_, i64>(0),
        )
        .optional_row()
    {
        return Ok(id);
    }
    conn.execute(
        "INSERT INTO categories (name, kind, origin, sort_order, created_at) \
         VALUES (?1, ?2, ?3, 0, ?4)",
        params![name, kind, origin, now()],
    )
    .map_err(|e| format!("新建分类失败：{e}"))?;
    Ok(conn.last_insert_rowid())
}

/// 小工具：把 `Option` 结果从 rusqlite 里取出来（避免到处写 match）。
trait OptionalRow<T> {
    fn optional_row(self) -> Option<T>;
}

impl<T> OptionalRow<T> for rusqlite::Result<T> {
    fn optional_row(self) -> Option<T> {
        self.ok()
    }
}

/// 列出全部分类（手动建的在前面，其次按名字）。
#[tauri::command]
pub fn list_categories(state: State<'_, Mutex<Connection>>) -> Result<Vec<Category>, String> {
    let conn = state.lock().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare(&format!(
            "SELECT {CATEGORY_COLUMNS} FROM categories c \
             ORDER BY CASE c.kind WHEN 'manual' THEN 0 ELSE 1 END, c.name COLLATE NOCASE ASC"
        ))
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], map_category)
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| e.to_string())?);
    }
    Ok(out)
}

/// 新建分类。同名（忽略大小写）已存在时直接返回已有的那条，不算错误。
#[tauri::command]
pub fn create_category(
    state: State<'_, Mutex<Connection>>,
    name: String,
    origin: Option<String>,
) -> Result<Category, String> {
    let conn = state.lock().map_err(|e| e.to_string())?;
    let origin = origin.unwrap_or_else(|| "manual".to_string());
    let id = upsert_category(&conn, &name, "manual", &origin)?;
    conn.query_row(
        &format!("SELECT {CATEGORY_COLUMNS} FROM categories c WHERE c.id = ?1"),
        params![id],
        map_category,
    )
    .map_err(|e| format!("读取新建分类失败：{e}"))
}

/// 重命名分类。
#[tauri::command]
pub fn rename_category(
    state: State<'_, Mutex<Connection>>,
    id: i64,
    name: String,
) -> Result<(), String> {
    let name = name.trim().to_string();
    if name.is_empty() {
        return Err("分类名不能为空".to_string());
    }
    let conn = state.lock().map_err(|e| e.to_string())?;
    conn.execute(
        "UPDATE categories SET name = ?2 WHERE id = ?1",
        params![id, name],
    )
    .map_err(|e| format!("重命名分类失败：{e}（可能与其他分类重名）"))?;
    Ok(())
}

/// 删除分类（关联行由外键级联清理）。
#[tauri::command]
pub fn delete_category(state: State<'_, Mutex<Connection>>, id: i64) -> Result<(), String> {
    let conn = state.lock().map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM categories WHERE id = ?1", params![id])
        .map_err(|e| e.to_string())?;
    Ok(())
}

/* --------------------------- 标签 --------------------------- */

const TAG_COLUMNS: &str = "t.id, t.name, \
     (SELECT COUNT(*) FROM media_tags mt WHERE mt.tag_id = t.id) AS usage_count";

fn map_tag(row: &rusqlite::Row<'_>) -> rusqlite::Result<Tag> {
    Ok(Tag {
        id: row.get(0)?,
        name: row.get(1)?,
        usage_count: row.get(2)?,
    })
}

/// 列出全部标签（用得多的在前）。
#[tauri::command]
pub fn list_tags(state: State<'_, Mutex<Connection>>) -> Result<Vec<Tag>, String> {
    let conn = state.lock().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare(&format!(
            "SELECT {TAG_COLUMNS} FROM tags t ORDER BY usage_count DESC, t.name COLLATE NOCASE ASC"
        ))
        .map_err(|e| e.to_string())?;
    let rows = stmt.query_map([], map_tag).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| e.to_string())?);
    }
    Ok(out)
}

/// 新建标签（同名已存在时返回已有的）。
#[tauri::command]
pub fn create_tag(state: State<'_, Mutex<Connection>>, name: String) -> Result<Tag, String> {
    let name = name.trim().to_string();
    if name.is_empty() {
        return Err("标签名不能为空".to_string());
    }
    let conn = state.lock().map_err(|e| e.to_string())?;
    if let Some(id) = conn
        .query_row(
            "SELECT id FROM tags WHERE name = ?1 COLLATE NOCASE",
            params![name],
            |r| r.get::<_, i64>(0),
        )
        .optional_row()
    {
        return conn
            .query_row(
                &format!("SELECT {TAG_COLUMNS} FROM tags t WHERE t.id = ?1"),
                params![id],
                map_tag,
            )
            .map_err(|e| e.to_string());
    }
    conn.execute(
        "INSERT INTO tags (name, created_at) VALUES (?1, ?2)",
        params![name, now()],
    )
    .map_err(|e| e.to_string())?;
    let id = conn.last_insert_rowid();
    conn.query_row(
        &format!("SELECT {TAG_COLUMNS} FROM tags t WHERE t.id = ?1"),
        params![id],
        map_tag,
    )
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn rename_tag(state: State<'_, Mutex<Connection>>, id: i64, name: String) -> Result<(), String> {
    let name = name.trim().to_string();
    if name.is_empty() {
        return Err("标签名不能为空".to_string());
    }
    let conn = state.lock().map_err(|e| e.to_string())?;
    conn.execute("UPDATE tags SET name = ?2 WHERE id = ?1", params![id, name])
        .map_err(|e| format!("重命名标签失败：{e}（可能与其他标签重名）"))?;
    Ok(())
}

#[tauri::command]
pub fn delete_tag(state: State<'_, Mutex<Connection>>, id: i64) -> Result<(), String> {
    let conn = state.lock().map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM tags WHERE id = ?1", params![id])
        .map_err(|e| e.to_string())?;
    Ok(())
}

/* --------------------- 剧集 ↔ 分类 / 标签 --------------------- */

/// 某条影视所属的分类。
#[tauri::command]
pub fn get_media_categories(
    state: State<'_, Mutex<Connection>>,
    media_id: i64,
) -> Result<Vec<Category>, String> {
    let conn = state.lock().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare(&format!(
            "SELECT {CATEGORY_COLUMNS} FROM categories c \
             JOIN media_categories mc ON mc.category_id = c.id \
             WHERE mc.media_id = ?1 ORDER BY c.name COLLATE NOCASE ASC"
        ))
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![media_id], map_category)
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| e.to_string())?);
    }
    Ok(out)
}

/// 设置某条影视的分类（全量覆盖：传什么就是什么）。
#[tauri::command]
pub fn set_media_categories(
    state: State<'_, Mutex<Connection>>,
    media_id: i64,
    category_ids: Vec<i64>,
) -> Result<Vec<Category>, String> {
    let conn = state.lock().map_err(|e| e.to_string())?;
    conn.execute(
        "DELETE FROM media_categories WHERE media_id = ?1",
        params![media_id],
    )
    .map_err(|e| e.to_string())?;
    for id in &category_ids {
        conn.execute(
            "INSERT OR IGNORE INTO media_categories (media_id, category_id, created_at) \
             VALUES (?1, ?2, ?3)",
            params![media_id, id, now()],
        )
        .map_err(|e| e.to_string())?;
    }
    drop(conn);
    get_media_categories(state, media_id)
}

/// 某条影视的标签。
#[tauri::command]
pub fn get_media_tags(
    state: State<'_, Mutex<Connection>>,
    media_id: i64,
) -> Result<Vec<Tag>, String> {
    let conn = state.lock().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare(&format!(
            "SELECT {TAG_COLUMNS} FROM tags t \
             JOIN media_tags mt ON mt.tag_id = t.id \
             WHERE mt.media_id = ?1 ORDER BY t.name COLLATE NOCASE ASC"
        ))
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![media_id], map_tag)
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| e.to_string())?);
    }
    Ok(out)
}

/// 用**名字**设置标签（全量覆盖）。不存在的名字会顺手建出来——
/// 用户打字时不应该先被要求「去建个标签」。
#[tauri::command]
pub fn set_media_tags(
    state: State<'_, Mutex<Connection>>,
    media_id: i64,
    names: Vec<String>,
) -> Result<Vec<Tag>, String> {
    let conn = state.lock().map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM media_tags WHERE media_id = ?1", params![media_id])
        .map_err(|e| e.to_string())?;

    for raw in names {
        let name = raw.trim().to_string();
        if name.is_empty() {
            continue;
        }
        let tag_id = match conn
            .query_row(
                "SELECT id FROM tags WHERE name = ?1 COLLATE NOCASE",
                params![name],
                |r| r.get::<_, i64>(0),
            )
            .optional_row()
        {
            Some(id) => id,
            None => {
                conn.execute(
                    "INSERT INTO tags (name, created_at) VALUES (?1, ?2)",
                    params![name, now()],
                )
                .map_err(|e| e.to_string())?;
                conn.last_insert_rowid()
            }
        };
        conn.execute(
            "INSERT OR IGNORE INTO media_tags (media_id, tag_id, created_at) VALUES (?1, ?2, ?3)",
            params![media_id, tag_id, now()],
        )
        .map_err(|e| e.to_string())?;
    }
    drop(conn);
    get_media_tags(state, media_id)
}

/* --------------------- 与 TMDb 识别结果同步 --------------------- */

/// 把一条影视的 `genre` 字段（`"剧情 / 悬疑"`）落成分类并关联。
///
/// 刮削时顺手调用，这样「TMDb 认出来的类型」和「左侧栏目」始终一致。
pub fn link_genre_categories(conn: &Connection, media_id: i64, genre: &str) -> Result<i64, String> {
    let mut linked = 0i64;
    for part in genre.split(['/', '、', ',', '，', '|']) {
        let name = part.trim();
        if name.is_empty() {
            continue;
        }
        let id = upsert_category(conn, name, "auto", "tmdb")?;
        let n = conn
            .execute(
                "INSERT OR IGNORE INTO media_categories (media_id, category_id, created_at) \
                 VALUES (?1, ?2, ?3)",
                params![media_id, id, now()],
            )
            .map_err(|e| e.to_string())?;
        linked += n as i64;
    }
    Ok(linked)
}

/// 全量同步：把库里所有条目的 `genre` 补成分类。
///
/// 老数据（升级前刮过、当时还没有分类表）靠它一次性补齐。
#[tauri::command]
pub fn sync_categories_from_tmdb(state: State<'_, Mutex<Connection>>) -> Result<SyncResult, String> {
    let conn = state.lock().map_err(|e| e.to_string())?;
    let before: i64 = conn
        .query_row("SELECT COUNT(*) FROM categories", [], |r| r.get(0))
        .map_err(|e| e.to_string())?;

    let mut stmt = conn
        .prepare("SELECT id, genre FROM media WHERE genre IS NOT NULL AND genre <> ''")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))
        .map_err(|e| e.to_string())?;
    let mut pairs = Vec::new();
    for r in rows {
        pairs.push(r.map_err(|e| e.to_string())?);
    }

    let mut linked = 0i64;
    for (id, genre) in pairs {
        linked += link_genre_categories(&conn, id, &genre)?;
    }

    let after: i64 = conn
        .query_row("SELECT COUNT(*) FROM categories", [], |r| r.get(0))
        .map_err(|e| e.to_string())?;

    Ok(SyncResult {
        created: after - before,
        linked,
        scanned: pairs_count(&conn)?,
    })
}

fn pairs_count(conn: &Connection) -> Result<i64, String> {
    conn.query_row("SELECT COUNT(*) FROM media", [], |r| r.get(0))
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    fn memory_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        db::migrate(&conn).unwrap();
        conn
    }

    fn add_media(conn: &Connection, title: &str, genre: Option<&str>) -> i64 {
        conn.execute(
            "INSERT INTO media (type, title, genre, added_at, updated_at) \
             VALUES ('tv', ?1, ?2, 0, 0)",
            params![title, genre],
        )
        .unwrap();
        conn.last_insert_rowid()
    }

    #[test]
    fn upsert_is_case_insensitive_and_idempotent() {
        let conn = memory_db();
        let a = upsert_category(&conn, "科幻", "manual", "manual").unwrap();
        let b = upsert_category(&conn, "科幻", "auto", "tmdb").unwrap();
        assert_eq!(a, b, "同名分类不应建两条");
        let kind: String = conn
            .query_row("SELECT kind FROM categories WHERE id = ?1", params![a], |r| r.get(0))
            .unwrap();
        assert_eq!(kind, "manual", "已存在的分类不应被改成自动来源");
    }

    #[test]
    fn empty_name_is_rejected() {
        let conn = memory_db();
        assert!(upsert_category(&conn, "   ", "manual", "manual").is_err());
    }

    #[test]
    fn genre_is_split_into_categories_and_linked() {
        let conn = memory_db();
        let id = add_media(&conn, "剧", Some("剧情 / 悬疑"));
        let linked = link_genre_categories(&conn, id, "剧情 / 悬疑").unwrap();
        assert_eq!(linked, 2);

        let names: Vec<String> = {
            let mut stmt = conn
                .prepare(
                    "SELECT c.name FROM categories c JOIN media_categories mc \
                     ON mc.category_id = c.id WHERE mc.media_id = ?1 ORDER BY c.name",
                )
                .unwrap();
            stmt.query_map(params![id], |r| r.get(0))
                .unwrap()
                .map(|r| r.unwrap())
                .collect()
        };
        assert_eq!(names, vec!["剧情".to_string(), "悬疑".to_string()]);

        // 再跑一次不应重复关联（INSERT OR IGNORE）
        assert_eq!(link_genre_categories(&conn, id, "剧情 / 悬疑").unwrap(), 0);
    }

    /// 中文顿号 / 逗号分隔也要能切开
    #[test]
    fn genre_separators_are_all_supported() {
        let conn = memory_db();
        let id = add_media(&conn, "剧", Some("动画、奇幻，冒险"));
        link_genre_categories(&conn, id, "动画、奇幻，冒险").unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM media_categories WHERE media_id = ?1",
                params![id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 3);
    }

    #[test]
    fn deleting_category_cascades_links() {
        let conn = memory_db();
        let id = add_media(&conn, "剧", Some("剧情"));
        link_genre_categories(&conn, id, "剧情").unwrap();
        conn.execute("DELETE FROM categories WHERE name = '剧情'", [])
            .unwrap();
        let left: i64 = conn
            .query_row("SELECT COUNT(*) FROM media_categories", [], |r| r.get(0))
            .unwrap();
        assert_eq!(left, 0, "删分类应级联清掉关联，不留孤儿行");
        let alive: i64 = conn
            .query_row("SELECT COUNT(*) FROM media", [], |r| r.get(0))
            .unwrap();
        assert_eq!(alive, 1, "删分类不应删掉影视本身");
    }

    #[test]
    fn deleting_media_cascades_links() {
        let conn = memory_db();
        let id = add_media(&conn, "剧", Some("剧情"));
        link_genre_categories(&conn, id, "剧情").unwrap();
        conn.execute("DELETE FROM media WHERE id = ?1", params![id])
            .unwrap();
        let left: i64 = conn
            .query_row("SELECT COUNT(*) FROM media_categories", [], |r| r.get(0))
            .unwrap();
        assert_eq!(left, 0);
        // 分类本身保留：它是栏目，不是某部剧的私有物
        let cats: i64 = conn
            .query_row("SELECT COUNT(*) FROM categories", [], |r| r.get(0))
            .unwrap();
        assert_eq!(cats, 1);
    }

    #[test]
    fn categories_are_listed_manual_first() {
        let conn = memory_db();
        upsert_category(&conn, "自动栏目", "auto", "tmdb").unwrap();
        upsert_category(&conn, "我的栏目", "manual", "manual").unwrap();
        let mut stmt = conn
            .prepare(&format!(
                "SELECT {CATEGORY_COLUMNS} FROM categories c \
                 ORDER BY CASE c.kind WHEN 'manual' THEN 0 ELSE 1 END, c.name COLLATE NOCASE ASC"
            ))
            .unwrap();
        let out: Vec<Category> = stmt
            .query_map([], map_category)
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert_eq!(out[0].name, "我的栏目");
        assert_eq!(out[0].kind, "manual");
        assert_eq!(out[1].origin, "tmdb");
    }

    #[test]
    fn series_count_is_reported() {
        let conn = memory_db();
        let cid = upsert_category(&conn, "剧情", "auto", "tmdb").unwrap();
        for t in ["A", "B"] {
            let id = add_media(&conn, t, None);
            conn.execute(
                "INSERT INTO media_categories (media_id, category_id, created_at) VALUES (?1,?2,0)",
                params![id, cid],
            )
            .unwrap();
        }
        let c: Category = conn
            .query_row(
                &format!("SELECT {CATEGORY_COLUMNS} FROM categories c WHERE c.id = ?1"),
                params![cid],
                map_category,
            )
            .unwrap();
        assert_eq!(c.series_count, 2);
    }
}
