//! 媒体库扫描：递归遍历媒体目录 → 解析文件名 → 入库（media / episodes / scan_history）。

use crate::db::now_epoch;
use crate::parser::{is_video_file, parse_file_name, ParsedName};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

/// 扫描统计。
#[derive(Debug, Default, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanStats {
    pub roots: usize,
    pub scanned: usize,
    pub movies: usize,
    pub series: usize,
    pub episodes: usize,
    pub added_movies: usize,
    pub added_series: usize,
    pub added_episodes: usize,
    pub skipped: usize,
    pub failed: usize,
}

/// 一个待入库的视频文件。
#[derive(Debug, Clone)]
pub struct ScanFile {
    pub path: PathBuf,
    pub name: String,
    pub size: u64,
    pub parsed: ParsedName,
}

/// 需要跳过的系统/隐藏目录。
fn is_skippable_dir(name: &str) -> bool {
    matches!(
        name,
        "$RECYCLE.BIN" | "System Volume Information" | "RECYCLER" | "lost+found"
    ) || name.starts_with('.')
}

/// 递归收集并解析视频文件（不做任何数据库操作）。
pub fn collect_files(roots: &[String]) -> (Vec<ScanFile>, usize) {
    let mut files = Vec::new();
    let mut used_roots = 0usize;

    for root in roots {
        let p = Path::new(root);
        if !p.is_dir() {
            continue;
        }
        used_roots += 1;
        let walker = WalkDir::new(p)
            .follow_links(false)
            .into_iter()
            .filter_entry(|e| !(e.file_type().is_dir() && is_skippable_dir(&e.file_name().to_string_lossy())));
        for entry in walker.filter_map(Result::ok) {
            if !entry.file_type().is_file() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            if !is_video_file(&name) {
                continue;
            }
            let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
            let parsed = parse_file_name(&name);
            files.push(ScanFile {
                path: entry.path().to_path_buf(),
                name,
                size,
                parsed,
            });
        }
    }
    (files, used_roots)
}

/// 剧集分组查找：`title`（刮削后可能被 TMDB 标题覆盖）与 `original_title`
/// （扫描时的原始标题，永远稳定）都参与匹配，保证重复扫描不会产生重复剧集。
fn find_series_id(conn: &Connection, title: &str) -> Result<Option<i64>, String> {
    conn.query_row(
        "SELECT id FROM media WHERE type = 'tv' AND (title = ?1 OR original_title = ?1) \
         ORDER BY id LIMIT 1",
        params![title],
        |r| r.get::<_, i64>(0),
    )
    .optional()
    .map_err(|e| e.to_string())
}

fn insert_movie(conn: &Connection, f: &ScanFile, ts: i64) -> Result<bool, String> {
    let path = f.path.to_string_lossy().to_string();
    let title = if f.parsed.title.is_empty() {
        f.name.clone()
    } else {
        f.parsed.title.clone()
    };
    let n = conn
        .execute(
            "INSERT INTO media (type, title, original_title, year, tmdb_id, file_path, file_name, file_size, added_at, updated_at) \
             VALUES ('movie', ?1, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7) \
             ON CONFLICT(file_path) DO NOTHING",
            params![title, f.parsed.year, f.parsed.tmdb_id, path, f.name, f.size as i64, ts],
        )
        .map_err(|e| e.to_string())?;
    Ok(n > 0)
}

fn upsert_series(conn: &Connection, title: &str, year: Option<i32>, ts: i64) -> Result<(i64, bool), String> {
    if let Some(id) = find_series_id(conn, title)? {
        return Ok((id, false));
    }
    conn.execute(
        "INSERT INTO media (type, title, original_title, year, added_at, updated_at) \
         VALUES ('tv', ?1, ?1, ?2, ?3, ?3)",
        params![title, year, ts],
    )
    .map_err(|e| e.to_string())?;
    Ok((conn.last_insert_rowid(), true))
}

fn insert_episode(conn: &Connection, media_id: i64, f: &ScanFile, ts: i64) -> Result<bool, String> {
    let path = f.path.to_string_lossy().to_string();
    let title = if f.parsed.title.is_empty() {
        f.name.clone()
    } else {
        f.parsed.title.clone()
    };
    let n = conn
        .execute(
            "INSERT INTO episodes (media_id, season, episode, title, file_path, file_name, file_size, added_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) \
             ON CONFLICT(file_path) DO NOTHING",
            params![
                media_id,
                f.parsed.season.unwrap_or(1),
                f.parsed.episode.unwrap_or(1),
                title,
                path,
                f.name,
                f.size as i64,
                ts
            ],
        )
        .map_err(|e| e.to_string())?;
    Ok(n > 0)
}

/// 把收集到的文件写入数据库（单事务），并记录 scan_history。
pub fn import(conn: &mut Connection, files: &[ScanFile], roots: usize) -> Result<ScanStats, String> {
    let mut stats = ScanStats {
        roots,
        scanned: files.len(),
        ..Default::default()
    };
    let ts = now_epoch();

    // 先建立扫描记录
    let history_id = {
        let path_label = format!("{roots} 个目录");
        conn.execute(
            "INSERT INTO scan_history (path, started_at, status) VALUES (?1, ?2, 'running')",
            params![path_label, ts],
        )
        .map_err(|e| e.to_string())?;
        conn.last_insert_rowid()
    };

    let tx = conn.transaction().map_err(|e| e.to_string())?;

    // 剧集按（标题）聚合：BTreeMap 保证处理顺序稳定
    let mut series_cache: BTreeMap<String, i64> = BTreeMap::new();

    for f in files {
        let is_tv = f.parsed.media_type() == "tv";
        if is_tv {
            let title = if f.parsed.title.is_empty() {
                f.name.clone()
            } else {
                f.parsed.title.clone()
            };
            let media_id = match series_cache.get(&title) {
                Some(id) => *id,
                None => {
                    let (id, created) = upsert_series(&tx, &title, f.parsed.year, ts)?;
                    if created {
                        stats.added_series += 1;
                    }
                    series_cache.insert(title.clone(), id);
                    id
                }
            };
            stats.series = series_cache.len();
            match insert_episode(&tx, media_id, f, ts) {
                Ok(true) => {
                    stats.episodes += 1;
                    stats.added_episodes += 1;
                }
                Ok(false) => {
                    stats.episodes += 1;
                    stats.skipped += 1;
                }
                Err(_) => stats.failed += 1,
            }
        } else {
            match insert_movie(&tx, f, ts) {
                Ok(true) => {
                    stats.movies += 1;
                    stats.added_movies += 1;
                }
                Ok(false) => {
                    stats.movies += 1;
                    stats.skipped += 1;
                }
                Err(_) => stats.failed += 1,
            }
        }
    }

    tx.commit().map_err(|e| e.to_string())?;

    conn.execute(
        "UPDATE scan_history SET finished_at = ?1, status = 'done', found = ?2, added = ?3 WHERE id = ?4",
        params![
            now_epoch(),
            stats.scanned as i64,
            (stats.added_movies + stats.added_episodes) as i64,
            history_id
        ],
    )
    .map_err(|e| e.to_string())?;

    Ok(stats)
}

/// 一步到位：扫描 + 入库。
pub fn scan_and_import(conn: &mut Connection, roots: &[String]) -> Result<ScanStats, String> {
    if roots.is_empty() {
        return Err("尚未配置媒体目录，请先在设置中添加".to_string());
    }
    let (files, used_roots) = collect_files(roots);
    import(conn, &files, used_roots)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    fn temp_dir(tag: &str) -> PathBuf {
        let mut d = std::env::temp_dir();
        let uniq = format!(
            "mm-scan-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        d.push(uniq);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn touch(dir: &Path, rel: &str) {
        let p = dir.join(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&p, b"dummy").unwrap();
    }

    fn memory_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        conn.execute_batch(db::SCHEMA).unwrap();
        conn
    }

    #[test]
    fn collects_only_video_files_and_skips_hidden_dirs() {
        let d = temp_dir("collect");
        touch(&d, "The.Matrix.1999.1080p.mkv");
        touch(&d, "notes.txt");
        touch(&d, "cover.jpg");
        touch(&d, "Season 1/Show.S01E01.mkv");
        touch(&d, "Season 1/Show.S01E02.mkv");
        touch(&d, ".hidden/secret.mkv");
        touch(&d, "subs/Show.S01E01.srt");

        let (files, roots) = collect_files(&[d.to_string_lossy().to_string()]);
        let names: Vec<_> = files.iter().map(|f| f.name.clone()).collect();
        assert_eq!(roots, 1);
        assert_eq!(files.len(), 3, "实际收集到 {names:?}");
        assert!(names.iter().all(|n| !n.ends_with(".txt")));
        assert!(names.iter().all(|n| !n.ends_with(".srt")));
        assert!(!names.contains(&"secret.mkv".to_string()), "隐藏目录应被跳过");

        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn nonexistent_root_is_ignored() {
        let (files, roots) = collect_files(&["Z:\\definitely\\not\\here".to_string()]);
        assert!(files.is_empty());
        assert_eq!(roots, 0);
    }

    #[test]
    fn import_classifies_movies_and_groups_series() {
        let d = temp_dir("import");
        touch(&d, "The.Matrix.1999.1080p.BluRay.x264.mkv");
        touch(&d, "Inception.2010.1080p.mkv");
        touch(&d, "Breaking.Bad.S01E01.720p.mkv");
        touch(&d, "Breaking.Bad.S01E02.720p.mkv");
        touch(&d, "Breaking.Bad.S02E01.720p.mkv");

        let mut conn = memory_db();
        let (files, roots) = collect_files(&[d.to_string_lossy().to_string()]);
        let stats = import(&mut conn, &files, roots).expect("导入应成功");

        assert_eq!(stats.scanned, 5);
        assert_eq!(stats.added_movies, 2, "两部电影");
        assert_eq!(stats.added_series, 1, "一个剧集标题");
        assert_eq!(stats.added_episodes, 3, "三集");

        let tv_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM media WHERE type='tv'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(tv_count, 1, "剧集只应有一条 media 主记录（分组）");

        let ep_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM episodes", [], |r| r.get(0))
            .unwrap();
        assert_eq!(ep_count, 3);

        let seasons: Vec<i64> = {
            let mut stmt = conn
                .prepare("SELECT DISTINCT season FROM episodes ORDER BY season")
                .unwrap();
            let rows = stmt.query_map([], |r| r.get(0)).unwrap();
            rows.map(|r| r.unwrap()).collect()
        };
        assert_eq!(seasons, vec![1, 2], "应识别出两季");

        let history: i64 = conn
            .query_row("SELECT COUNT(*) FROM scan_history WHERE status='done'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(history, 1, "应写入一条完成的扫描记录");

        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn rescan_is_idempotent() {
        let d = temp_dir("rescan");
        touch(&d, "The.Matrix.1999.1080p.mkv");
        touch(&d, "Show.S01E01.mkv");

        let mut conn = memory_db();
        let root = d.to_string_lossy().to_string();

        let (files, roots) = collect_files(&[root.clone()]);
        let first = import(&mut conn, &files, roots).expect("首次导入应成功");
        assert_eq!(first.added_movies, 1);
        assert_eq!(first.added_episodes, 1);

        let (files2, roots2) = collect_files(&[root]);
        let second = import(&mut conn, &files2, roots2).expect("重复导入应成功");
        assert_eq!(second.added_movies, 0, "重复扫描不应重复入库");
        assert_eq!(second.added_episodes, 0);
        assert_eq!(second.skipped, 2, "两个文件都应被计为已存在");

        let total: i64 = conn
            .query_row("SELECT COUNT(*) FROM media", [], |r| r.get(0))
            .unwrap();
        assert_eq!(total, 2);

        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn empty_roots_returns_error() {
        let mut conn = memory_db();
        assert!(scan_and_import(&mut conn, &[]).is_err());
    }

    /// 回归：刮削会把显示标题改成 TMDB 标题，此时重扫不能新建重复剧集
    /// （分组匹配同时认 original_title）。
    #[test]
    fn rescan_after_title_change_does_not_duplicate_series() {
        let d = temp_dir("rename");
        touch(&d, "Breaking.Bad.S01E01.mkv");
        let mut conn = memory_db();
        let root = d.to_string_lossy().to_string();

        let (files, roots) = collect_files(&[root.clone()]);
        import(&mut conn, &files, roots).expect("首次导入");
        conn.execute("UPDATE media SET title = '绝命毒师' WHERE type = 'tv'", [])
            .unwrap();

        let (files2, roots2) = collect_files(&[root]);
        let second = import(&mut conn, &files2, roots2).expect("二次导入");
        assert_eq!(second.added_series, 0, "标题被改写后重扫不应新建剧集");

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM media WHERE type='tv'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);

        std::fs::remove_dir_all(&d).ok();
    }
}
