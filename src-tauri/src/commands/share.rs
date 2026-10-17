//! NFO 导出 / 导入、数据库备份恢复。
//!
//! ## 对视频文件零写入
//! - **导出**：只创建 `*.nfo` 新文件，默认不覆盖已存在的同名文件；
//! - **导入**：只读 NFO，把元数据写进数据库；类型只作为 `SourceHint` 候选过 gate；
//! - **备份**：用 SQLite `VACUUM INTO` 生成库文件副本，不动 `db::open` 的路径。
//! 没有任何一步会以写模式打开视频文件。

use crate::commands::job::{self, JobProgress};
use crate::core::nfo::{NfoInfo, NfoKind};
use crate::share::nfo;
use rusqlite::Connection;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tauri::{AppHandle, State};

/// 导出结果。
#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ExportResult {
    /// 实际写出的 NFO 文件数。
    pub written: i64,
    /// 因已存在而跳过的数量（默认不覆盖，这是刻意的）。
    pub skipped: i64,
    /// 失败数量。
    pub failed: i64,
    pub message: String,
}

/// 导入结果。
#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ImportResult {
    /// 读到的 NFO 数。
    pub scanned: i64,
    /// 回填了元数据的条数。
    pub filled: i64,
    /// 因数据源标识而改判类型的条数（过 gate 后的真实改判数）。
    pub reclassified: i64,
    /// 没有匹配到库内条目的 NFO 数。
    pub unmatched: i64,
    pub message: String,
}

/// 导出 NFO。
///
/// - `target_dir` 为 `None` 时写在**媒体文件旁边**（Kodi/tMM 的 sidecar 约定）；
///   传了目录则全部写到那个目录里。
/// - `overwrite = false`（默认）：遇到同名文件直接跳过。
///   这是「不改动磁盘上已有东西」的底线——用户或别的软件写的 NFO 不会被冲掉。
#[tauri::command]
pub fn export_nfo(
    app: AppHandle,
    state: State<'_, Mutex<Connection>>,
    media_ids: Vec<i64>,
    target_dir: Option<String>,
    overwrite: Option<bool>,
) -> Result<ExportResult, String> {
    let conn = state.lock().map_err(|e| e.to_string())?;
    let overwrite = overwrite.unwrap_or(false);
    let shared_target = target_dir
        .as_deref()
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty());
    if let Some(p) = &shared_target {
        std::fs::create_dir_all(p).map_err(|e| format!("创建导出目录失败：{e}"))?;
    }

    let mut out = ExportResult::default();
    // 不传 ids = 导出全库（界面上的「导出全部」）。显式传 ids 时只导所选。
    let ids: Vec<i64> = if media_ids.is_empty() {
        let mut st = conn
            .prepare("SELECT id FROM media ORDER BY id")
            .map_err(|e| e.to_string())?;
        let rows = st
            .query_map([], |r| r.get::<_, i64>(0))
            .map_err(|e| e.to_string())?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(|e| e.to_string())?
    } else {
        media_ids
    };
    let total = ids.len() as i64;
    for (i, id) in ids.into_iter().enumerate() {
        // 统一任务事件：导出可能是几千条，界面需要一个进度源
        job::emit(
            &app,
            &JobProgress::tick(job::KIND_EXPORT, "导出 NFO", i as i64, total, &format!("#{id}")),
        );
        match export_one(&conn, id, shared_target.as_deref(), overwrite) {
            Ok((w, s)) => {
                out.written += w;
                out.skipped += s;
            }
            Err(_) => out.failed += 1,
        }
    }
    let mut msg = format!("已导出 {} 个 NFO", out.written);
    if out.skipped > 0 {
        msg.push_str(&format!("，跳过 {} 个已存在的（默认不覆盖）", out.skipped));
    }
    if out.failed > 0 {
        msg.push_str(&format!("，失败 {} 个", out.failed));
    }
    out.message = msg;
    job::emit(
        &app,
        &if out.failed > 0 && out.written == 0 {
            JobProgress::failed(job::KIND_EXPORT, &out.message)
        } else {
            JobProgress::done(job::KIND_EXPORT, &out.message)
        },
    );
    Ok(out)
}

/// 导出一条（含其单集）。返回 `(写入数, 跳过数)`。
fn export_one(
    conn: &Connection,
    media_id: i64,
    shared_target: Option<&Path>,
    overwrite: bool,
) -> Result<(i64, i64), String> {
    let (kind, title, original, year, tmdb, tvdb, file_path): (
        String,
        String,
        Option<String>,
        Option<i32>,
        Option<i64>,
        Option<i64>,
        Option<String>,
    ) = conn
        .query_row(
            "SELECT type, title, original_title, year, tmdb_id, tvdb_id, file_path \
             FROM media WHERE id = ?1",
            rusqlite::params![media_id],
            |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                ))
            },
        )
        .map_err(|e| e.to_string())?;

    // 目标目录：优先导出目录；否则 sidecar 模式用媒体文件所在目录；
    // 剧集主记录没有 file_path（它是容器）→ 退回第一个单集的目录。
    let dir: Option<PathBuf> = match shared_target {
        Some(p) => Some(p.to_path_buf()),
        None => file_path
            .as_deref()
            .map(Path::new)
            .and_then(|p| p.parent().map(|x| x.to_path_buf()))
            .or_else(|| first_episode_dir(conn, media_id)),
    };
    let Some(dir) = dir else {
        return Err("找不到导出目录（该条目没有关联文件）".to_string());
    };

    let nfo_kind = if kind == "tv" {
        NfoKind::TvShow
    } else {
        NfoKind::Movie
    };
    let info = NfoInfo {
        title: Some(title),
        original_title: original,
        year,
        tmdb_id: tmdb,
        tvdb_id: tvdb,
        season: None,
        episode: None,
        kind: Some(nfo_kind),
    };
    let base = if kind == "tv" { "tvshow" } else { "movie" };
    let mut written = 0i64;
    let mut skipped = 0i64;
    match nfo::write_nfo(&dir, base, nfo_kind, &info, overwrite) {
        Ok(Some(_)) => written += 1,
        Ok(None) => skipped += 1,
        Err(e) => return Err(e),
    }

    // 剧集：再给每集写一份单集 NFO
    if kind == "tv" {
        let mut stmt = conn
            .prepare(
                "SELECT id, season, episode, title, file_path, tmdb_id, tvdb_id \
                 FROM episodes WHERE media_id = ?1 ORDER BY season, episode",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(rusqlite::params![media_id], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, Option<String>>(3)?,
                    r.get::<_, Option<String>>(4)?,
                    r.get::<_, Option<i64>>(5)?,
                    r.get::<_, Option<i64>>(6)?,
                ))
            })
            .map_err(|e| e.to_string())?;
        for r in rows {
            let (_id, season, episode, etitle, epath, etmdb, etvdb) = r.map_err(|e| e.to_string())?;
            let ep_dir = match shared_target {
                Some(p) => p.to_path_buf(),
                None => match epath.as_deref().map(Path::new).and_then(|p| p.parent()) {
                    Some(p) => p.to_path_buf(),
                    None => continue,
                },
            };
            let file_base = epath
                .as_deref()
                .map(Path::new)
                .and_then(|p| p.file_stem())
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| format!("S{season:02}E{episode:02}"));
            let ep_info = NfoInfo {
                title: etitle,
                original_title: None,
                year: None,
                tmdb_id: etmdb,
                tvdb_id: etvdb,
                season: Some(season as i32),
                episode: Some(episode as i32),
                kind: Some(NfoKind::Episode),
            };
            match nfo::write_nfo(&ep_dir, &file_base, NfoKind::Episode, &ep_info, overwrite) {
                Ok(Some(_)) => written += 1,
                Ok(None) => skipped += 1,
                Err(_) => {}
            }
        }
    }
    Ok((written, skipped))
}

fn first_episode_dir(conn: &Connection, media_id: i64) -> Option<PathBuf> {
    let p: Option<String> = conn
        .query_row(
            "SELECT file_path FROM episodes WHERE media_id = ?1 AND file_path IS NOT NULL \
             ORDER BY season, episode LIMIT 1",
            rusqlite::params![media_id],
            |r| r.get(0),
        )
        .ok()
        .flatten();
    p.as_deref().map(Path::new).and_then(|x| x.parent()).map(|x| x.to_path_buf())
}

/// 从一个目录导入 NFO。
///
/// 只做两件事：① 把 NFO 里的标题/年份/TMDB id 回填到**已存在的**条目
/// （按文件名匹配，匹配不上就跳过，绝不凭空建条目）；
/// ② 把 NFO 的类型当成 `SourceHint` 候选，过 `commands::media` 的 gate 后改判。
#[tauri::command]
pub fn import_nfo(
    app: AppHandle,
    state: State<'_, Mutex<Connection>>,
    dir: String,
) -> Result<ImportResult, String> {
    let root = PathBuf::from(&dir);
    if !root.is_dir() {
        return Err(format!("目录不存在：{dir}"));
    }
    let mut out = ImportResult::default();

    // 收集 NFO 文件（只递归一层足够：NFO 惯例就是放在媒体旁边）
    let mut files: Vec<PathBuf> = Vec::new();
    let mut stack = vec![root.clone()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().and_then(|x| x.to_str()).map(|x| x.eq_ignore_ascii_case(nfo::NFO_EXT)) == Some(true) {
                files.push(p);
            }
        }
    }

    let conn = state.lock().map_err(|e| e.to_string())?;
    let total = files.len() as i64;
    for (i, f) in files.into_iter().enumerate() {
        job::emit(
            &app,
            &JobProgress::tick(job::KIND_IMPORT, "导入 NFO", i as i64, total, &f.to_string_lossy()),
        );
        let Ok(text) = std::fs::read_to_string(&f) else {
            continue;
        };
        out.scanned += 1;
        let info = nfo::parse(&text);
        // 用 NFO 的文件名（去掉 .nfo）去库里找同名的媒体文件
        let stem = f
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        let Some(media_id) = find_media_by_stem(&conn, &stem) else {
            out.unmatched += 1;
            continue;
        };
        let filled = fill_metadata(&conn, media_id, &info)?;
        if filled {
            out.filled += 1;
        }
        // 类型：只作为候选，交给 gate（locked / 已有单集 / 方向限制）
        if let Some(hint) = crate::core::nfo::source_hint_of(&info) {
            let allowed = crate::commands::media::source_hint_allowed(&conn, media_id, hint)?;
            if allowed && crate::commands::media::apply_source_hint(&conn, media_id, hint)? {
                out.reclassified += 1;
            }
        }
    }
    out.message = format!(
        "读取 {} 个 NFO：回填 {} 条，改判 {} 条，未匹配 {} 个",
        out.scanned, out.filled, out.reclassified, out.unmatched
    );
    job::emit(&app, &JobProgress::done(job::KIND_IMPORT, &out.message));
    Ok(out)
}

/// 按文件名（不含扩展名）找库里的条目：先找 media.file_name，再找 episodes。
fn find_media_by_stem(conn: &Connection, stem: &str) -> Option<i64> {
    if stem.is_empty() {
        return None;
    }
    let like = format!("%{stem}%");
    let a: Option<i64> = conn
        .query_row(
            "SELECT id FROM media WHERE file_name LIKE ?1 OR title LIKE ?1 LIMIT 1",
            rusqlite::params![like],
            |r| r.get(0),
        )
        .ok();
    if a.is_some() {
        return a;
    }
    conn.query_row(
        "SELECT media_id FROM episodes WHERE file_name LIKE ?1 LIMIT 1",
        rusqlite::params![like],
        |r| r.get(0),
    )
    .ok()
}

/// 回填缺失字段（**只补空，绝不覆盖**）。返回是否真的写了。
fn fill_metadata(conn: &Connection, media_id: i64, info: &NfoInfo) -> Result<bool, String> {
    let ts = crate::util::now_epoch();
    let mut changed = false;
    if let Some(t) = &info.title {
        // 只在标题为空时补
        changed |= conn
            .execute(
                "UPDATE media SET title = ?2, updated_at = ?3 \
                 WHERE id = ?1 AND (title IS NULL OR title = '')",
                rusqlite::params![media_id, t, ts],
            )
            .map_err(|e| e.to_string())?
            > 0;
    }
    if let Some(y) = info.year {
        changed |= conn
            .execute(
                "UPDATE media SET year = ?2, updated_at = ?3 WHERE id = ?1 AND year IS NULL",
                rusqlite::params![media_id, y, ts],
            )
            .map_err(|e| e.to_string())?
            > 0;
    }
    if let Some(id) = info.tmdb_id {
        changed |= conn
            .execute(
                "UPDATE media SET tmdb_id = ?2, updated_at = ?3 WHERE id = ?1 AND tmdb_id IS NULL",
                rusqlite::params![media_id, id, ts],
            )
            .map_err(|e| e.to_string())?
            > 0;
    }
    if let Some(id) = info.tvdb_id {
        changed |= conn
            .execute(
                "UPDATE media SET tvdb_id = ?2, updated_at = ?3 WHERE id = ?1 AND tvdb_id IS NULL",
                rusqlite::params![media_id, id, ts],
            )
            .map_err(|e| e.to_string())?
            > 0;
    }
    if let Some(t) = &info.original_title {
        changed |= conn
            .execute(
                "UPDATE media SET original_title = ?2, updated_at = ?3 \
                 WHERE id = ?1 AND (original_title IS NULL OR original_title = '')",
                rusqlite::params![media_id, t, ts],
            )
            .map_err(|e| e.to_string())?
            > 0;
    }
    Ok(changed)
}

/// 备份数据库到指定路径（SQLite `VACUUM INTO`，生成一份一致快照）。
///
/// 目标文件若已存在会失败——不覆盖用户的文件（与导出 NFO 同一个原则）。
#[tauri::command]
pub fn backup_database(app: tauri::AppHandle, dest: String) -> Result<String, String> {
    let path = PathBuf::from(&dest);
    if path.exists() {
        return Err(format!("目标文件已存在，拒绝覆盖：{dest}"));
    }
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|e| format!("创建备份目录失败：{e}"))?;
        }
    }
    let src = crate::store::db::db_path(&app).map_err(|e| e)?;
    let conn = rusqlite::Connection::open(&src).map_err(|e| format!("打开数据库失败：{e}"))?;
    let dest_sql = dest.replace('\'', "''");
    conn.execute(&format!("VACUUM INTO '{dest_sql}'"), [])
        .map_err(|e| format!("备份失败（可能 SQLite 版本过低）：{e}"))?;
    Ok(dest)
}

#[cfg(test)]
#[path = "tests/share.rs"]
mod tests;
