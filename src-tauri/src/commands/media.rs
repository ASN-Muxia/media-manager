//! 媒体库查询与扫描命令。

use crate::commands::settings::load_media_dirs;
use crate::scanner::{self, ScanStats};
use rusqlite::{params, Connection, Row};
use serde::Serialize;
use std::sync::Mutex;
use tauri::State;

/// 列表项（对应前端 `MediaItem`，保持 snake_case）。
#[derive(Debug, Clone, Serialize)]
pub struct MediaListItem {
    pub id: i64,
    #[serde(rename = "type")]
    pub media_type: String,
    pub title: String,
    pub year: Option<i32>,
    pub tmdb_id: Option<i64>,
    pub poster_path: Option<String>,
    pub backdrop_path: Option<String>,
    pub overview: Option<String>,
    pub vote_average: Option<f64>,
    pub genre: Option<String>,
    pub file_path: Option<String>,
    pub file_name: Option<String>,
    pub episode_count: i64,
    pub added_at: i64,
    /// 展示用标题：同名剧集会自动带上目录名以示区分（`剧名 · 目录名`）。
    pub display_title: String,
}

/// 单集。
#[derive(Debug, Clone, Serialize)]
pub struct Episode {
    pub id: i64,
    pub media_id: i64,
    pub season: i32,
    pub episode: i32,
    pub title: Option<String>,
    pub file_path: Option<String>,
    pub file_name: Option<String>,
    pub file_size: Option<i64>,
    pub added_at: i64,
    /// 以下字段由单集级刮削回填，未刮削时为 null。
    pub tmdb_id: Option<i64>,
    pub overview: Option<String>,
    pub still_path: Option<String>,
    pub air_date: Option<String>,
}

/// 详情（主记录 + 单集列表）。
#[derive(Debug, Clone, Serialize)]
pub struct MediaDetail {
    pub media: MediaListItem,
    pub episodes: Vec<Episode>,
}

/// 扫描历史。
#[derive(Debug, Clone, Serialize)]
pub struct ScanHistoryItem {
    pub id: i64,
    pub path: String,
    pub started_at: i64,
    pub finished_at: Option<i64>,
    pub status: String,
    pub found: i64,
    pub added: i64,
    pub message: Option<String>,
}

const ITEM_COLUMNS: &str = "m.id, m.type, m.title, m.year, m.tmdb_id, m.poster_path, m.backdrop_path, \
     m.overview, m.vote_average, m.genre, m.file_path, m.file_name, m.added_at, \
     (SELECT COUNT(*) FROM episodes e WHERE e.media_id = m.id) AS episode_count, \
     CASE WHEN m.group_key IS NOT NULL \
            AND (SELECT COUNT(*) FROM media d WHERE d.type = 'tv' AND d.type = m.type \
                 AND d.title = m.title) > 1 \
          THEN m.title || ' · ' || m.group_key ELSE m.title END AS display_title";

fn map_item(row: &Row<'_>) -> rusqlite::Result<MediaListItem> {
    Ok(MediaListItem {
        id: row.get(0)?,
        media_type: row.get(1)?,
        title: row.get(2)?,
        year: row.get(3)?,
        tmdb_id: row.get(4)?,
        poster_path: row.get(5)?,
        backdrop_path: row.get(6)?,
        overview: row.get(7)?,
        vote_average: row.get(8)?,
        genre: row.get(9)?,
        file_path: row.get(10)?,
        file_name: row.get(11)?,
        added_at: row.get(12)?,
        episode_count: row.get(13)?,
        display_title: row.get(14)?,
    })
}

fn map_episode(row: &Row<'_>) -> rusqlite::Result<Episode> {
    Ok(Episode {
        id: row.get(0)?,
        media_id: row.get(1)?,
        season: row.get(2)?,
        episode: row.get(3)?,
        title: row.get(4)?,
        file_path: row.get(5)?,
        file_name: row.get(6)?,
        file_size: row.get(7)?,
        added_at: row.get(8)?,
        tmdb_id: row.get(9)?,
        overview: row.get(10)?,
        still_path: row.get(11)?,
        air_date: row.get(12)?,
    })
}

/* --------------------------- 命令 --------------------------- */

/// 扫描媒体库。`roots` 为空时回落到设置里的 `mediaDirs`。
#[tauri::command]
pub fn scan_library(
    state: State<'_, Mutex<Connection>>,
    roots: Option<Vec<String>>,
) -> Result<ScanStats, String> {
    let mut conn = state.lock().map_err(|e| e.to_string())?;
    let dirs = match roots {
        Some(r) if !r.is_empty() => r,
        _ => load_media_dirs(&conn),
    };
    scanner::scan_and_import(&mut conn, &dirs)
}

/// 媒体列表，`media_type` 为 `all` / `movie` / `tv`。
#[tauri::command]
pub fn get_media_list(
    state: State<'_, Mutex<Connection>>,
    media_type: Option<String>,
) -> Result<Vec<MediaListItem>, String> {
    let conn = state.lock().map_err(|e| e.to_string())?;
    let filter = media_type.unwrap_or_else(|| "all".to_string());
    let sql = format!(
        "SELECT {ITEM_COLUMNS} FROM media m \
         WHERE COALESCE(?1, 'all') IN ('all', m.type) \
         ORDER BY m.title COLLATE NOCASE ASC, m.year ASC"
    );
    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![filter], map_item)
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| e.to_string())?);
    }
    Ok(out)
}

/// 媒体详情（含单集）。
#[tauri::command]
pub fn get_media_detail(
    state: State<'_, Mutex<Connection>>,
    id: i64,
) -> Result<MediaDetail, String> {
    let conn = state.lock().map_err(|e| e.to_string())?;
    let sql = format!("SELECT {ITEM_COLUMNS} FROM media m WHERE m.id = ?1");
    let media = conn
        .query_row(&sql, params![id], map_item)
        .map_err(|e| format!("未找到媒体 id={id}: {e}"))?;
    let episodes = query_episodes(&conn, id)?;
    Ok(MediaDetail { media, episodes })
}

fn query_episodes(conn: &Connection, media_id: i64) -> Result<Vec<Episode>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT id, media_id, season, episode, title, file_path, file_name, file_size, added_at, \
                    tmdb_id, overview, still_path, air_date \
             FROM episodes WHERE media_id = ?1 ORDER BY season ASC, episode ASC",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![media_id], map_episode)
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| e.to_string())?);
    }
    Ok(out)
}

/// 某个剧集下的单集列表。
#[tauri::command]
pub fn get_episodes(
    state: State<'_, Mutex<Connection>>,
    media_id: i64,
) -> Result<Vec<Episode>, String> {
    let conn = state.lock().map_err(|e| e.to_string())?;
    query_episodes(&conn, media_id)
}

/// 删除一条媒体记录（剧集单集级联删除）。
#[tauri::command]
pub fn delete_media(state: State<'_, Mutex<Connection>>, id: i64) -> Result<(), String> {
    let conn = state.lock().map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM media WHERE id = ?1", params![id])
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// 清空媒体库（保留设置）。
#[tauri::command]
pub fn clear_library(state: State<'_, Mutex<Connection>>) -> Result<(), String> {
    let conn = state.lock().map_err(|e| e.to_string())?;
    conn.execute_batch("DELETE FROM episodes; DELETE FROM media; DELETE FROM scan_history;")
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// 最近若干条扫描记录。
#[tauri::command]
pub fn get_scan_history(
    state: State<'_, Mutex<Connection>>,
    limit: Option<i64>,
) -> Result<Vec<ScanHistoryItem>, String> {
    let conn = state.lock().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare(
            "SELECT id, path, started_at, finished_at, status, found, added, message \
             FROM scan_history ORDER BY id DESC LIMIT ?1",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![limit.unwrap_or(20)], |row| {
            Ok(ScanHistoryItem {
                id: row.get(0)?,
                path: row.get(1)?,
                started_at: row.get(2)?,
                finished_at: row.get(3)?,
                status: row.get(4)?,
                found: row.get(5)?,
                added: row.get(6)?,
                message: row.get(7)?,
            })
        })
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
    use crate::db;

    fn memory_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        conn.execute_batch(db::SCHEMA).unwrap();
        conn
    }

    fn seed(conn: &Connection) {
        conn.execute(
            "INSERT INTO media (type,title,year,file_path,file_name,added_at,updated_at) \
             VALUES ('movie','Dune',2021,'D:\\\\Dune.mkv','Dune.mkv',1,1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO media (type,title,year,added_at,updated_at) VALUES ('tv','Breaking Bad',2008,1,1)",
            [],
        )
        .unwrap();
        let sid = conn.last_insert_rowid();
        for (s, e) in [(1, 1), (1, 2), (2, 1)] {
            conn.execute(
                "INSERT INTO episodes (media_id,season,episode,file_path,file_name,added_at) \
                 VALUES (?1,?2,?3,?4,?5,1)",
                params![
                    sid,
                    s,
                    e,
                    format!("D:\\\\BB.S{s:02}E{e:02}.mkv"),
                    format!("BB.S{s:02}E{e:02}.mkv")
                ],
            )
            .unwrap();
        }
    }

    fn query_items(conn: &Connection, filter: &str) -> Vec<MediaListItem> {
        let sql = format!(
            "SELECT {ITEM_COLUMNS} FROM media m \
             WHERE COALESCE(?1,'all') IN ('all', m.type) ORDER BY m.title COLLATE NOCASE ASC"
        );
        let mut stmt = conn.prepare(&sql).unwrap();
        let rows = stmt.query_map(params![filter], map_item).unwrap();
        rows.map(|r| r.unwrap()).collect()
    }

    #[test]
    fn list_filters_by_type() {
        let conn = memory_db();
        seed(&conn);
        assert_eq!(query_items(&conn, "all").len(), 2);
        let movies = query_items(&conn, "movie");
        assert_eq!(movies.len(), 1);
        assert_eq!(movies[0].media_type, "movie");
        let tv = query_items(&conn, "tv");
        assert_eq!(tv.len(), 1);
        assert_eq!(tv[0].media_type, "tv");
    }

    #[test]
    fn episode_count_is_joined_for_series() {
        let conn = memory_db();
        seed(&conn);
        let tv = query_items(&conn, "tv");
        assert_eq!(tv[0].episode_count, 3);
        let movie = query_items(&conn, "movie");
        assert_eq!(movie[0].episode_count, 0, "电影不应有单集");
    }

    #[test]
    fn episodes_are_ordered_by_season_then_episode() {
        let conn = memory_db();
        seed(&conn);
        let sid: i64 = conn
            .query_row("SELECT id FROM media WHERE type='tv'", [], |r| r.get(0))
            .unwrap();
        let eps = query_episodes(&conn, sid).unwrap();
        let pairs: Vec<(i32, i32)> = eps.iter().map(|e| (e.season, e.episode)).collect();
        assert_eq!(pairs, vec![(1, 1), (1, 2), (2, 1)]);
    }

    #[test]
    fn foreign_key_cascade_removes_episodes() {
        let conn = memory_db();
        seed(&conn);
        let sid: i64 = conn
            .query_row("SELECT id FROM media WHERE type='tv'", [], |r| r.get(0))
            .unwrap();
        conn.execute("DELETE FROM media WHERE id = ?1", params![sid]).unwrap();
        let left: i64 = conn
            .query_row("SELECT COUNT(*) FROM episodes", [], |r| r.get(0))
            .unwrap();
        assert_eq!(left, 0, "删除剧集主记录应级联删除单集");
    }

    #[test]
    fn item_serializes_type_as_type() {
        let conn = memory_db();
        seed(&conn);
        let items = query_items(&conn, "movie");
        let json = serde_json::to_value(&items[0]).unwrap();
        assert!(json.get("type").is_some());
        assert_eq!(json["type"], "movie");
        assert!(json.get("media_type").is_none());
    }

    /// 同名剧集要给用户看得见的区分，否则封面墙上是两个一模一样的名字。
    #[test]
    fn display_title_disambiguates_same_titled_series() {
        let conn = memory_db();
        for key in ["Show A", "Show B"] {
            conn.execute(
                "INSERT INTO media (type,title,original_title,group_key,added_at,updated_at) \
                 VALUES ('tv','Show','Show',?1,1,1)",
                params![key],
            )
            .unwrap();
        }
        let items = query_items(&conn, "tv");
        let titles: Vec<String> = items.iter().map(|i| i.display_title.clone()).collect();
        assert!(titles.contains(&"Show · Show A".to_string()), "{titles:?}");
        assert!(titles.contains(&"Show · Show B".to_string()), "{titles:?}");
    }

    #[test]
    fn display_title_equals_title_when_unique() {
        let conn = memory_db();
        seed(&conn);
        let tv = query_items(&conn, "tv");
        assert_eq!(tv[0].display_title, tv[0].title);
        let movie = query_items(&conn, "movie");
        assert_eq!(movie[0].display_title, movie[0].title);
    }
}
