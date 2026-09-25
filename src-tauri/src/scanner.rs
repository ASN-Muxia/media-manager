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
    /// 剧集分组键（电影为 None）。取媒体根目录下的第一层目录名。
    pub group_key: Option<String>,
}

/// 需要跳过的系统/隐藏目录。
fn is_skippable_dir(name: &str) -> bool {
    matches!(
        name,
        "$RECYCLE.BIN" | "System Volume Information" | "RECYCLER" | "lost+found"
    ) || name.starts_with('.')
}

/// 取「相对根目录的第一层目录名」，用作剧集分组键。
///
/// `D:\TV\Breaking Bad\Season 1\S01E01.mkv` → `Breaking Bad`。
/// 这样即使两部不同剧集**同名**，只要各自在自己的目录里就不会被错误合并。
fn top_folder_of(root: &Path, file: &Path) -> Option<String> {
    let rel = file.strip_prefix(root).ok()?;
    let mut comps = rel.components();
    let first = comps.next()?;
    if comps.next().is_none() {
        return None; // 文件就在根目录下，没有中间层
    }
    Some(first.as_os_str().to_string_lossy().to_string())
}

/// 递归收集并解析视频文件（不做任何数据库操作）。
pub fn collect_files(roots: &[String]) -> Vec<ScanFile> {
    let mut files = Vec::new();

    for root in roots {
        let root_path = Path::new(root);
        if !root_path.is_dir() {
            continue;
        }
        let walker = WalkDir::new(root_path)
            .follow_links(false)
            .into_iter()
            .filter_entry(|e| {
                !(e.file_type().is_dir() && is_skippable_dir(&e.file_name().to_string_lossy()))
            });
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
            let group_key = if parsed.media_type() == "tv" {
                Some(top_folder_of(root_path, entry.path()).unwrap_or_else(|| {
                    if parsed.title.is_empty() {
                        name.clone()
                    } else {
                        parsed.title.clone()
                    }
                }))
            } else {
                None
            };
            files.push(ScanFile {
                path: entry.path().to_path_buf(),
                name,
                size,
                parsed,
                group_key,
            });
        }
    }
    files
}

/// 扫描范围的可读标签（过长时截断，避免塞爆 scan_history）。
fn summarize_roots(roots: &[String]) -> String {
    let joined = roots.join(" ; ");
    if joined.chars().count() <= 180 {
        joined
    } else {
        let head: String = joined.chars().take(177).collect();
        format!("{head}...")
    }
}

/// 剧集分组查找。
///
/// 匹配优先级：
/// 1. `group_key` 精确命中（同一目录下的同一部剧）；
/// 2. 退回 `title`/`original_title`，**仅匹配 `group_key` 仍为空的老数据**——
///    这样既能兼容升级前建的行，又不会把两部同名但不同目录的剧集并成一条。
///
/// `title` 之所以还要参与匹配：刮削会把显示标题改成 TMDB 标题，`original_title`
/// 保留扫描时的原始标题，是稳定的兜底键。
fn find_series_id(
    conn: &Connection,
    group_key: &str,
    title: &str,
) -> Result<Option<(i64, bool)>, String> {
    conn.query_row(
        "SELECT id, group_key IS NULL FROM media WHERE type = 'tv' AND ( \
             group_key = ?1 \
             OR (group_key IS NULL AND (title = ?2 OR original_title = ?2)) \
         ) ORDER BY CASE WHEN group_key = ?1 THEN 0 ELSE 1 END, id LIMIT 1",
        params![group_key, title],
        |r| Ok((r.get::<_, i64>(0)?, r.get::<_, bool>(1)?)),
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

fn upsert_series(
    conn: &Connection,
    group_key: &str,
    title: &str,
    year: Option<i32>,
    ts: i64,
) -> Result<(i64, bool), String> {
    if let Some((id, needs_heal)) = find_series_id(conn, group_key, title)? {
        if needs_heal {
            // 老库升级上来的行没有分组键，顺手补上（自愈）
            conn.execute(
                "UPDATE media SET group_key = ?2 WHERE id = ?1",
                params![id, group_key],
            )
            .map_err(|e| e.to_string())?;
        }
        return Ok((id, false));
    }
    conn.execute(
        "INSERT INTO media (type, title, original_title, group_key, year, added_at, updated_at) \
         VALUES ('tv', ?2, ?2, ?1, ?3, ?4, ?4)",
        params![group_key, title, year, ts],
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
pub fn import(conn: &mut Connection, files: &[ScanFile], roots: &[String]) -> Result<ScanStats, String> {
    let mut stats = ScanStats {
        roots: roots.len(),
        scanned: files.len(),
        ..Default::default()
    };
    let ts = now_epoch();

    // 先建立扫描记录（写入真实目录列表，便于回溯）
    let history_id = {
        conn.execute(
            "INSERT INTO scan_history (path, started_at, status) VALUES (?1, ?2, 'running')",
            params![summarize_roots(roots), ts],
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
            let group_key = f.group_key.clone().unwrap_or_else(|| title.clone());
            let media_id = match series_cache.get(&group_key) {
                Some(id) => *id,
                None => {
                    let (id, created) =
                        upsert_series(&tx, &group_key, &title, f.parsed.year, ts)?;
                    if created {
                        stats.added_series += 1;
                    }
                    series_cache.insert(group_key.clone(), id);
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
    let files = collect_files(roots);
    import(conn, &files, roots)
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

        let files = collect_files(&[d.to_string_lossy().to_string()]);
        let names: Vec<_> = files.iter().map(|f| f.name.clone()).collect();
        assert_eq!(files.len(), 3, "实际收集到 {names:?}");
        assert!(names.iter().all(|n| !n.ends_with(".txt")));
        assert!(names.iter().all(|n| !n.ends_with(".srt")));
        assert!(!names.contains(&"secret.mkv".to_string()), "隐藏目录应被跳过");

        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn nonexistent_root_is_ignored() {
        let files = collect_files(&["Z:\\definitely\\not\\here".to_string()]);
        assert!(files.is_empty());
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
        let roots = vec![d.to_string_lossy().to_string()];
        let files = collect_files(&roots);
        let stats = import(&mut conn, &files, &roots).expect("导入应成功");

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
        let roots = vec![d.to_string_lossy().to_string()];

        let files = collect_files(&roots);
        let first = import(&mut conn, &files, &roots).expect("首次导入应成功");
        assert_eq!(first.added_movies, 1);
        assert_eq!(first.added_episodes, 1);

        let files2 = collect_files(&roots);
        let second = import(&mut conn, &files2, &roots).expect("重复导入应成功");
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
        let roots = vec![d.to_string_lossy().to_string()];

        let files = collect_files(&roots);
        import(&mut conn, &files, &roots).expect("首次导入");
        conn.execute("UPDATE media SET title = '绝命毒师' WHERE type = 'tv'", [])
            .unwrap();

        let files2 = collect_files(&roots);
        let second = import(&mut conn, &files2, &roots).expect("二次导入");
        assert_eq!(second.added_series, 0, "标题被改写后重扫不应新建剧集");

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM media WHERE type='tv'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);

        std::fs::remove_dir_all(&d).ok();
    }

    /// 同名剧集放在不同目录里，必须各自成条，不能并成一条。
    #[test]
    fn same_title_series_in_different_folders_are_not_merged() {
        let d = temp_dir("dup");
        touch(&d, "Show A/Show.S01E01.mkv");
        touch(&d, "Show A/Show.S01E02.mkv");
        touch(&d, "Show B/Show.S01E01.mkv");

        let mut conn = memory_db();
        let roots = vec![d.to_string_lossy().to_string()];
        let files = collect_files(&roots);
        let stats = import(&mut conn, &files, &roots).expect("导入应成功");

        assert_eq!(stats.added_series, 2, "同名不同目录的两部剧不应合并");
        let tv: i64 = conn
            .query_row("SELECT COUNT(*) FROM media WHERE type='tv'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(tv, 2);

        // 单集各自挂到各自的主记录上：按分组键校验，不依赖插入顺序
        let mut stmt = conn
            .prepare(
                "SELECT m.group_key, COUNT(e.id) FROM media m \
                 LEFT JOIN episodes e ON e.media_id = m.id \
                 WHERE m.type = 'tv' GROUP BY m.id ORDER BY m.group_key",
            )
            .unwrap();
        let per: Vec<(String, i64)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert_eq!(
            per,
            vec![("Show A".to_string(), 2), ("Show B".to_string(), 1)],
            "Show A 两集、Show B 一集，且各自挂在自己的主记录上"
        );

        std::fs::remove_dir_all(&d).ok();
    }

    /// 分组键取「根目录下的第一层目录名」；文件直接在根目录时退回解析标题。
    #[test]
    fn group_key_comes_from_first_folder_under_root() {
        let d = temp_dir("gk");
        touch(&d, "Breaking Bad/Season 1/Breaking.Bad.S01E01.mkv");
        touch(&d, "Breaking Bad/Season 2/Breaking.Bad.S02E01.mkv");
        touch(&d, "Loose.Show.S01E01.mkv");

        let roots = vec![d.to_string_lossy().to_string()];
        let files = collect_files(&roots);
        let key_of = |n: &str| {
            files
                .iter()
                .find(|f| f.name == n)
                .unwrap_or_else(|| panic!("未收集到 {n}"))
                .group_key
                .clone()
        };

        assert_eq!(
            key_of("Breaking.Bad.S01E01.mkv").as_deref(),
            Some("Breaking Bad")
        );
        assert_eq!(
            key_of("Breaking.Bad.S02E01.mkv").as_deref(),
            Some("Breaking Bad"),
            "同一部剧的不同季应落到同一个分组键"
        );
        assert_eq!(
            key_of("Loose.Show.S01E01.mkv").as_deref(),
            Some("Loose Show"),
            "根目录下的文件应退回解析出的标题"
        );

        std::fs::remove_dir_all(&d).ok();
    }

    /// 升级前的老行没有 group_key，重扫时应复用并自愈回填，而不是新建重复条目。
    #[test]
    fn legacy_series_row_is_healed_with_group_key_on_rescan() {
        let d = temp_dir("heal");
        touch(&d, "Breaking.Bad.S01E01.mkv");
        let roots = vec![d.to_string_lossy().to_string()];

        let mut conn = memory_db();
        conn.execute(
            "INSERT INTO media (type, title, year, added_at, updated_at) \
             VALUES ('tv','Breaking Bad',2008,1,1)",
            [],
        )
        .unwrap();
        let legacy_id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO episodes (media_id, season, episode, file_path, file_name, added_at) \
             VALUES (?1, 1, 1, 'D:\\\\legacy.mkv', 'Breaking.Bad.S01E01.mkv', 1)",
            params![legacy_id],
        )
        .unwrap();

        let files = collect_files(&roots);
        let stats = import(&mut conn, &files, &roots).expect("导入应成功");

        assert_eq!(stats.added_series, 0, "应复用老行而不是新建");
        let key: Option<String> = conn
            .query_row(
                "SELECT group_key FROM media WHERE id = ?1",
                params![legacy_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(key.as_deref(), Some("Breaking Bad"), "老行应被回填分组键");

        std::fs::remove_dir_all(&d).ok();
    }

    /// 扫描历史应记录真实目录，而不是「N 个目录」这种无信息量的标签。
    #[test]
    fn scan_history_stores_real_root_paths() {
        let d = temp_dir("hist");
        touch(&d, "Movie.2020.1080p.mkv");
        let roots = vec![d.to_string_lossy().to_string()];

        let mut conn = memory_db();
        let files = collect_files(&roots);
        import(&mut conn, &files, &roots).expect("导入应成功");

        let label: String = conn
            .query_row(
                "SELECT path FROM scan_history ORDER BY id DESC LIMIT 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            label.contains("mm-scan-hist"),
            "扫描历史应记录真实目录（实际为 {label}）"
        );

        std::fs::remove_dir_all(&d).ok();
    }
}
