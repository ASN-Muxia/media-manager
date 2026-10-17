//! 观看状态、续播与连播。
//!
//! ## 对视频文件零写入
//! 本模块做的全部事情只有两类：读写 `playback_state` 表、把**路径**交给外部播放器。
//! 没有任何一步会打开视频文件做写入——不转码、不重编码、不裁剪、不覆盖。
//! 连播也不生成临时播放列表文件（直接把多个路径作为参数传给播放器），
//! 避免在用户的媒体目录里留下任何东西。

use crate::commands::play::resolve_player;
use crate::commands::settings::load_potplayer_path;
use crate::store::playback;
use crate::util::no_console;
use rusqlite::Connection;
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Mutex;
use tauri::State;

/// 一个条目的观看摘要（与 `store::playback::MediaPlayback` 同名，直接透出）。
pub use crate::store::playback::MediaPlayback;

/// 读一个条目的观看状态。
#[tauri::command]
pub fn get_playback(
    state: State<'_, Mutex<Connection>>,
    media_id: i64,
) -> Result<MediaPlayback, String> {
    let conn = state.lock().map_err(|e| e.to_string())?;
    playback::summarize(&conn, media_id)
}

/// 批量读（列表页一次拿全，避免 N+1）。
#[tauri::command]
pub fn get_playback_many(
    state: State<'_, Mutex<Connection>>,
    media_ids: Vec<i64>,
) -> Result<BTreeMap<i64, MediaPlayback>, String> {
    let conn = state.lock().map_err(|e| e.to_string())?;
    playback::summarize_many(&conn, &media_ids)
}

/// 最近播放（「继续观看」用）。
#[tauri::command]
pub fn recent_playback(
    state: State<'_, Mutex<Connection>>,
    limit: Option<i64>,
) -> Result<Vec<MediaPlayback>, String> {
    let conn = state.lock().map_err(|e| e.to_string())?;
    playback::recent(&conn, limit.unwrap_or(12).clamp(1, 100))
}

/// 标记已看 / 未看。`episode_id` 为空表示整部（电影，或剧集整体标记）。
#[tauri::command]
pub fn mark_watched(
    state: State<'_, Mutex<Connection>>,
    media_id: i64,
    episode_id: Option<i64>,
    watched: bool,
) -> Result<MediaPlayback, String> {
    let conn = state.lock().map_err(|e| e.to_string())?;
    playback::set_watched(&conn, media_id, episode_id, watched)?;
    playback::summarize(&conn, media_id)
}

/// 批量标记（多选后一次搞定）。
#[tauri::command]
pub fn mark_watched_batch(
    state: State<'_, Mutex<Connection>>,
    media_ids: Vec<i64>,
    watched: bool,
) -> Result<i64, String> {
    let conn = state.lock().map_err(|e| e.to_string())?;
    let mut n = 0;
    for id in media_ids {
        playback::set_watched(&conn, id, None, watched)?;
        n += 1;
    }
    Ok(n)
}

/// 手动修正播放位置（秒）。本期不自动采集，但允许用户自己填。
#[tauri::command]
pub fn set_playback_position(
    state: State<'_, Mutex<Connection>>,
    media_id: i64,
    episode_id: Option<i64>,
    position_secs: Option<i64>,
    duration_secs: Option<i64>,
) -> Result<MediaPlayback, String> {
    let conn = state.lock().map_err(|e| e.to_string())?;
    playback::set_position(&conn, media_id, episode_id, position_secs, duration_secs)?;
    playback::summarize(&conn, media_id)
}

/// 清掉某条目的全部观看记录。
#[tauri::command]
pub fn clear_playback(
    state: State<'_, Mutex<Connection>>,
    media_id: i64,
) -> Result<MediaPlayback, String> {
    let conn = state.lock().map_err(|e| e.to_string())?;
    playback::clear(&conn, media_id)?;
    playback::summarize(&conn, media_id)
}

/// 连播 / 播放单集的结果。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayResult {
    /// 实际使用的播放器（`"system"` 表示系统默认）。
    pub player: String,
    /// 本次交给播放器的文件数（连播时 > 1）。
    pub files: i64,
    pub media_id: i64,
    pub episode_id: Option<i64>,
    /// 这一集的季 / 集号（界面提示「已播放 S01E03」用）；电影为 `None`。
    pub season: Option<i64>,
    pub episode: Option<i64>,
}

/// 连播命令：`from_episode_id` 起播，后续集依次排队。
///
/// - 不传 `from_episode_id`：**先取「上次真正播放过的那一集」**（双击封面直接播
///   上次那一集），没有播放记录时退回「下一个未看集」；
/// - `continuous = false`：只播这一集。
///
/// 播放成功后才记入观看状态（播放失败不算看过）。
#[tauri::command]
pub fn play_episode(
    state: State<'_, Mutex<Connection>>,
    media_id: i64,
    episode_id: Option<i64>,
    continuous: Option<bool>,
) -> Result<PlayResult, String> {
    let (paths, season, episode) = {
        let conn = state.lock().map_err(|e| e.to_string())?;
        let target = match episode_id {
            Some(id) => Some(id),
            None => {
                // 顺序有意义：**上次播放 > 第一个未看集**。
                // 反过来（未看集优先）会让「看了第 5 集但没标记已看」的用户
                // 每次双击都从第 1 集重来 —— 用户报的正是这条。
                let s = playback::summarize(&conn, media_id)?;
                s.last_played_episode_id.or(s.resume_episode_id)
            }
        };
        match target {
            Some(start) => {
                let se = episode_season_episode(&conn, start)?;
                let paths = if continuous.unwrap_or(false) {
                    episode_paths_from(&conn, media_id, start)?
                } else {
                    vec![episode_path(&conn, start)?]
                };
                (paths, Some(se.0), Some(se.1))
            }
            // 没有单集、也没有传 episode_id = **电影（或「待确认」的多文件条目）**。
            // 这种情况必须能播 `media.file_path`，不能报「还没有可播放的单集」：
            // 封面上双击一张电影卡走的就是这条路。
            None => {
                let p = movie_path(&conn, media_id)?;
                (vec![p], None, None)
            }
        }
    };
    if paths.is_empty() {
        return Err("没有可播放的文件".to_string());
    }
    if !Path::new(&paths[0]).is_file() {
        return Err(format!("文件不存在：{}", paths[0]));
    }

    let player = {
        let conn = state.lock().map_err(|e| e.to_string())?;
        load_potplayer_path(&conn)
    };
    let first = paths[0].clone();
    let used = match resolve_player(&player, &|p| Path::new(p).is_file()) {
        Some(exe) => {
            // 多文件直传：PotPlayer 会把它们按顺序排进播放列表。
            // 刻意**不写临时 .m3u8**——不往磁盘（尤其是媒体目录）里放任何东西。
            std::process::Command::new(&exe)
                .args(&paths)
                .spawn()
                .map_err(|e| format!("启动播放器失败：{e}"))?;
            exe
        }
        None => {
            // 系统默认播放器只认一个文件：连播降级为单集播放。
            // 同 `play.rs`：给 `cmd.exe` 套 CREATE_NO_WINDOW，避免弹出控制台黑框。
            no_console(
                std::process::Command::new("cmd").args(["/C", "start", "", first.as_str()]),
            )
            .spawn()
            .map_err(|e| format!("调用系统默认播放器失败：{e}"))?;
            "system".to_string()
        }
    };

    // 播放已成功启动 → 记录
    let conn = state.lock().map_err(|e| e.to_string())?;
    let ep = episode_id.or_else(|| first_episode_id_by_path(&conn, media_id, &first));
    playback::record_play(&conn, media_id, ep)?;
    Ok(PlayResult {
        player: used,
        files: paths.len() as i64,
        media_id,
        episode_id: ep,
        season,
        episode,
    })
}

/// 取某一集的（季, 集号）。送进播放器前先拿好，用来给界面一句「已播放 S01E03」。
fn episode_season_episode(conn: &Connection, episode_id: i64) -> Result<(i64, i64), String> {
    conn.query_row(
        "SELECT season, episode FROM episodes WHERE id = ?1",
        rusqlite::params![episode_id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .map_err(|e| e.to_string())
}

/// 电影（或「待确认」的条目）自己那一个文件。
///
/// 与 `episode_path` 分开：那张表是 `episodes.file_path`，这张是 `media.file_path`。
/// 单文件条目的「播放」一直是 `commands/play.rs::play_media` 直接拿路径调播放器，
/// 绕开了观看记录；双击播放统一走这个命令，才能顺带把「看过」与最近播放记下来。
fn movie_path(conn: &Connection, media_id: i64) -> Result<String, String> {
    let p: Option<String> = conn
        .query_row(
            "SELECT file_path FROM media WHERE id = ?1",
            rusqlite::params![media_id],
            |r| r.get(0),
        )
        .map_err(|_| format!("找不到 id 为 {media_id} 的条目（可能已被删除）"))?;
    p.filter(|s| !s.is_empty()).ok_or_else(|| {
        // 同一部片的多份文件（同片多版本）也存在 `episodes(season=0)` 里，
        // 那次要由调用方带上具体的 episode_id；这里只负责「你压根没给我目标」。
        "这个条目没有关联的本地文件（多版本条目请指定具体那一份）".to_string()
    })
}

fn episode_path(conn: &Connection, episode_id: i64) -> Result<String, String> {
    let path: Option<String> = conn
        .query_row(
            "SELECT file_path FROM episodes WHERE id = ?1",
            rusqlite::params![episode_id],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    path.filter(|s| !s.is_empty())
        .ok_or_else(|| "该单集没有关联的文件路径".to_string())
}

/// 从某一集开始，按（季, 集）顺序取出的全部文件路径。
fn episode_paths_from(
    conn: &Connection,
    media_id: i64,
    start_id: i64,
) -> Result<Vec<String>, String> {
    let (s, e): (i64, i64) = conn
        .query_row(
            "SELECT season, episode FROM episodes WHERE id = ?1",
            rusqlite::params![start_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare(
            "SELECT file_path FROM episodes
             WHERE media_id = ?1 AND (season > ?2 OR (season = ?2 AND episode >= ?3))
             ORDER BY season, episode, id",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(rusqlite::params![media_id, s, e], |r| {
            r.get::<_, Option<String>>(0)
        })
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for r in rows {
        if let Some(p) = r.map_err(|e| e.to_string())? {
            if !p.is_empty() {
                out.push(p);
            }
        }
    }
    Ok(out)
}

fn first_episode_id_by_path(
    conn: &Connection,
    media_id: i64,
    path: &str,
) -> Option<i64> {
    conn.query_row(
        "SELECT id FROM episodes WHERE media_id = ?1 AND file_path = ?2 LIMIT 1",
        rusqlite::params![media_id, path],
        |r| r.get(0),
    )
    .ok()
}

#[cfg(test)]
#[path = "tests/playback.rs"]
mod tests;
