//! TMDB 刮削命令：搜索候选、抓取元数据并回填 media 表。
//!
//! 注意：网络请求**不持锁**进行，避免阻塞其他命令。

use crate::commands::settings::load_tmdb_key;
use crate::db::now_epoch;
use crate::tmdb::{TmdbClient, TmdbSearchItem};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use std::sync::Mutex;
use tauri::State;

/// 单条刮削结果。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScrapeOutcome {
    pub ok: bool,
    pub media_id: i64,
    pub tmdb_id: Option<i64>,
    pub title: Option<String>,
    pub message: String,
}

/// 批量刮削汇总。
#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScrapeSummary {
    pub total: i64,
    pub ok: i64,
    pub failed: i64,
    pub no_key: bool,
}

/// 待刮削条目的最小信息。
struct PendingRow {
    media_type: String,
    title: String,
    year: Option<i32>,
}

fn read_pending(conn: &Connection, id: i64) -> Result<PendingRow, String> {
    conn.query_row(
        "SELECT id, type, title, year FROM media WHERE id = ?1",
        params![id],
        |r| {
            Ok(PendingRow {
                media_type: r.get(1)?,
                title: r.get(2)?,
                year: r.get(3)?,
            })
        },
    )
    .map_err(|e| format!("未找到媒体 id={id}: {e}"))
}

fn update_meta(conn: &Connection, id: i64, tmdb_id: i64, meta: &crate::tmdb::TmdbMeta) -> Result<(), String> {
    conn.execute(
        "UPDATE media SET tmdb_id = ?2, \
                title = CASE WHEN ?3 = '' THEN title ELSE ?3 END, \
                year = COALESCE(year, ?4), \
                poster_path = ?5, backdrop_path = ?6, overview = ?7, \
                vote_average = ?8, genre = ?9, updated_at = ?10 \
         WHERE id = ?1",
        params![
            id,
            tmdb_id,
            meta.title,
            meta.year,
            meta.poster_path,
            meta.backdrop_path,
            meta.overview,
            meta.vote_average,
            meta.genre,
            now_epoch()
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// 搜索 TMDB 候选（供详情页手动匹配）。
#[tauri::command]
pub fn search_tmdb(
    state: State<'_, Mutex<Connection>>,
    media_type: String,
    query: String,
    year: Option<i32>,
) -> Result<Vec<TmdbSearchItem>, String> {
    let key = {
        let conn = state.lock().map_err(|e| e.to_string())?;
        load_tmdb_key(&conn)
    };
    if key.trim().is_empty() {
        return Err("尚未配置 TMDB API Key，请先到「设置」填写".to_string());
    }
    let client = TmdbClient::new(key);
    client.search(&media_type, &query, year)
}

/// 刮削单条：给了 `tmdb_id` 就直接取详情，否则按标题+年份自动匹配第一条。
#[tauri::command]
pub fn scrape_media(
    state: State<'_, Mutex<Connection>>,
    id: i64,
    tmdb_id: Option<i64>,
) -> Result<ScrapeOutcome, String> {
    let (key, pending) = {
        let conn = state.lock().map_err(|e| e.to_string())?;
        (load_tmdb_key(&conn), read_pending(&conn, id)?)
    };
    if key.trim().is_empty() {
        return Err("尚未配置 TMDB API Key，请先到「设置」填写".to_string());
    }
    let client = TmdbClient::new(key);

    // 1) 确定 TMDB id
    let resolved = match tmdb_id {
        Some(t) => Some(t),
        None => {
            if pending.title.trim().is_empty() {
                None
            } else {
                client
                    .search(&pending.media_type, &pending.title, pending.year)
                    .ok()
                    .and_then(|items| {
                        // 优先同年份，其次第一条
                        items
                            .iter()
                            .find(|it| {
                                pending.year.is_some()
                                    && crate::tmdb::year_from_date(it.date().as_deref())
                                        == pending.year
                            })
                            .or_else(|| items.first())
                            .map(|it| it.id)
                    })
            }
        }
    };

    let Some(tmdb_id) = resolved else {
        return Ok(ScrapeOutcome {
            ok: false,
            media_id: id,
            tmdb_id: None,
            title: None,
            message: "未在 TMDB 找到匹配条目".to_string(),
        });
    };

    // 2) 取详情并回填
    let meta = client.detail(&pending.media_type, tmdb_id)?;
    {
        let conn = state.lock().map_err(|e| e.to_string())?;
        update_meta(&conn, id, tmdb_id, &meta)?;
    }

    Ok(ScrapeOutcome {
        ok: true,
        media_id: id,
        tmdb_id: Some(tmdb_id),
        title: Some(meta.title.clone()),
        message: "刮削成功".to_string(),
    })
}

/// 批量刮削尚未获取到海报的条目（最多 `limit` 条）。
#[tauri::command]
pub fn scrape_all(
    state: State<'_, Mutex<Connection>>,
    limit: Option<i64>,
) -> Result<ScrapeSummary, String> {
    let (key, targets) = {
        let conn = state.lock().map_err(|e| e.to_string())?;
        let key = load_tmdb_key(&conn);
        let mut stmt = conn
            .prepare(
                "SELECT id FROM media WHERE poster_path IS NULL OR poster_path = '' \
                 ORDER BY id ASC LIMIT ?1",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![limit.unwrap_or(50)], |r| r.get::<_, i64>(0))
            .map_err(|e| e.to_string())?;
        let ids: Vec<i64> = rows.filter_map(Result::ok).collect();
        (key, ids)
    };

    if key.trim().is_empty() {
        return Ok(ScrapeSummary {
            total: targets.len() as i64,
            no_key: true,
            ..Default::default()
        });
    }

    let client = TmdbClient::new(key);
    let mut summary = ScrapeSummary {
        total: targets.len() as i64,
        ..Default::default()
    };

    for id in targets {
        let pending = {
            let conn = state.lock().map_err(|e| e.to_string())?;
            read_pending(&conn, id)
        };
        let Ok(pending) = pending else {
            summary.failed += 1;
            continue;
        };
        let found = if pending.title.trim().is_empty() {
            None
        } else {
            client
                .search(&pending.media_type, &pending.title, pending.year)
                .ok()
                .and_then(|items| {
                    items
                        .iter()
                        .find(|it| {
                            pending.year.is_some()
                                && crate::tmdb::year_from_date(it.date().as_deref()) == pending.year
                        })
                        .or_else(|| items.first())
                        .map(|it| it.id)
                })
        };
        match found {
            Some(tmdb_id) => match client.detail(&pending.media_type, tmdb_id) {
                Ok(meta) => {
                    let conn = state.lock().map_err(|e| e.to_string())?;
                    if update_meta(&conn, id, tmdb_id, &meta).is_ok() {
                        summary.ok += 1;
                    } else {
                        summary.failed += 1;
                    }
                }
                Err(_) => summary.failed += 1,
            },
            None => summary.failed += 1,
        }
    }

    Ok(summary)
}

/// 手动绑定某个 TMDB 条目（详情页「应用」按钮）。
#[tauri::command]
pub fn apply_tmdb_match(
    state: State<'_, Mutex<Connection>>,
    id: i64,
    tmdb_id: i64,
) -> Result<ScrapeOutcome, String> {
    scrape_media(state, id, Some(tmdb_id))
}

/// 判断某条媒体是否已有 TMDB 元数据。
#[tauri::command]
pub fn has_metadata(state: State<'_, Mutex<Connection>>, id: i64) -> Result<bool, String> {
    let conn = state.lock().map_err(|e| e.to_string())?;
    let v: Option<Option<String>> = conn
        .query_row(
            "SELECT poster_path FROM media WHERE id = ?1",
            params![id],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    Ok(matches!(v, Some(Some(p)) if !p.is_empty()))
}
