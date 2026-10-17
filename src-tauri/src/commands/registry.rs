//! 资源目录库的命令层：检测（sync）/ 查询（catalog）/ 对比（diff）。
//!
//! ## 与「媒体库扫描」的分工
//! `commands::media::scan_library` 回答的是「用户有哪些作品」——它会新建 / 归并 /
//! 改挂 `media`、`episodes` 行。本模块回答的是另一个问题：
//! **「盘上到底有哪些文件，和上次比有什么变化」**，只写 `resource_assets`。
//! 两者刻意不做成一个：作品行会被重新识别改写，而资源行以绝对路径为身份、只增不删，
//! 才能在文件消失之后仍然答出「它上次在哪、多大、什么时候不见的」。
//!
//! ## 分层
//! - 规则（该算 New / Modified / Restored、谁该标 missing）在 `core::registry`，
//!   纯函数、可单测；
//! - 这里只负责「取设置 → 遍历磁盘 → 套规则 → 写库」，写入分批放进事务。
//!
//! ## 遍历与入库是分开的两个函数
//! `walk_media_files` 碰磁盘，`apply_catalog_sync` 只认一份已经收集好的文件列表。
//! 拆开是为了单测：否则每个用例都得依赖用户真实的媒体目录。见 `tests/registry.rs`。

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Mutex;
use tauri::State;
use walkdir::WalkDir;

use crate::commands::settings::load_media_dirs;
use crate::core::registry::{self, AssetChange, AssetFingerprint};
use crate::store::search::escape_like;

/* ------------------------------ 数据结构 ------------------------------ */

/// 资源目录里的一行（LEFT JOIN `media` 补上作品信息）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogEntry {
    pub id: i64,
    pub path: String,
    pub file_name: String,
    pub rel_dir: String,
    pub root_path: String,
    pub size: i64,
    pub mtime: Option<i64>,
    /// `present` / `missing`（取值见 `core::registry` 的两个常量）。
    pub status: String,
    /// 关联的媒体条目 id；对不上时为 `None`（**不拦**：资源行本来就允许没有作品）。
    pub media_id: Option<i64>,
    pub display_title: Option<String>,
    pub media_type: Option<String>,
}

/// 一次检测的汇总（前端直接展示，也写进 `resource_snapshots` 供下次对比）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogSyncResult {
    pub started_at: i64,
    pub finished_at: i64,
    pub roots: usize,
    pub scanned: usize,
    pub added: usize,
    pub updated: usize,
    pub unchanged: usize,
    pub missing: usize,
    pub restored: usize,
    pub message: String,
}

/// 本次检测相对上次的差异清单。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogDiff {
    /// 上次检测之后新出现的文件。
    pub added: Vec<CatalogEntry>,
    /// 内容在上次检测之后被改写过的文件（判据见 `diff_resource_catalog_sync`）。
    pub changed: Vec<CatalogEntry>,
    /// 已消失的文件——**行还在**，这是「定位旧资源」的落点。
    pub missing: Vec<CatalogEntry>,
    /// 当前仍在盘上的资源总数。
    pub present: usize,
    /// 上一次检测的结束时间；从未检测过为 `None`。
    pub last_sync_at: Option<i64>,
}

/// 遍历到的一个视频文件（磁盘事实，还没进库）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScannedAsset {
    /// 命中的媒体根目录（原样存下来，便于按盘区分；根目录设置改了也不会错位）
    pub root_path: String,
    pub path: String,
    pub dir_path: String,
    /// 相对根目录的目录（`''` = 直接躺在根目录下）
    pub rel_dir: String,
    pub file_name: String,
    /// 小写扩展名（不含点）
    pub ext: String,
    pub size: u64,
    /// Unix 秒；取不到为 `None`（会被保守当成「变了」）
    pub mtime: Option<i64>,
    /// 归一化标题：让「同一部作品的不同文件」在库里能对上
    pub content_key: String,
}

/// 差异清单每类最多返回多少条。
///
/// 历史积压时可能有上万条「消失」，全量塞给前端既没人看、又会把 IPC 撑爆，
/// 所以封顶；汇总数字照样是全量的（来自 `resource_snapshots`）。
const DIFF_LIMIT: i64 = 500;

/// 每批写多少行。
///
/// 为什么不「整批一个事务」：几万个文件的大事务会让 WAL 涨到几百 MB，
/// 中途失败还要把已写好的全部回滚、白跑一遍遍历。分批之后每批各自原子，
/// 重跑幂等（靠 `UNIQUE(path)`），中途崩最多重写最后一批——代价可控得多。
const UPSERT_CHUNK: usize = 500;

/* ------------------------------ 遍历 ------------------------------ */

/// 路径分隔符（两种都认：Windows 上混用是常态）。
fn is_sep(c: char) -> bool {
    c == '\\' || c == '/'
}

/// 去掉目录末尾的分隔符（`D:/media/` 与 `D:/media` 必须视为同一个根）。
fn trim_root(s: &str) -> &str {
    let t = s.trim_end_matches(is_sep);
    if t.is_empty() {
        s
    } else {
        t
    }
}

/// 需要跳过的系统 / 隐藏目录（与 `core::scanner` 里的同名判断同口径；
/// 那边是模块私有函数，不为了它去改 `core` 的可见性）。
fn is_skippable_dir(name: &str) -> bool {
    matches!(
        name,
        "$RECYCLE.BIN" | "System Volume Information" | "RECYCLER" | "lost+found"
    ) || name.starts_with('.')
}

/// 文件修改时间（Unix 秒）；取不到返回 `None`。
///
/// 取不到**不报错**：某些网络盘 / 特殊文件系统给不出 mtime，而这不该让整次检测
/// 失败——`core::registry` 会把 `None` 保守当成「变了」，只是多写一次而已。
fn mtime_of(meta: &std::fs::Metadata) -> Option<i64> {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
}

/// 扩展名（小写，不含点）；没有扩展名时为空串。
fn ext_of(name: &str) -> String {
    Path::new(name)
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default()
}

/// 归一化标题：抹掉季集号 / 年份 / 噪声词之后剩下的部分。
///
/// 直接复用 `core::grouping::normalize_title`——「同一部作品」的判定口径整个
/// 项目只能有一份，否则资源库认出来的「同一部」和分组认出来的会不一致。
fn content_key_of(file_name: &str) -> String {
    let stem = Path::new(file_name)
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| file_name.to_string());
    crate::core::grouping::normalize_title(&stem)
}

/// 递归收集媒体目录下的视频文件（**只碰磁盘，不碰库**）。
///
/// 扩展名表复用 `core::parser`（`is_video_file` / `looks_like_video_file`），
/// 不在这里另写一套——两套扩展名表迟早会漂移。
///
/// 返回的第二个值是**真正走通的根目录下标**：根目录不存在（盘没挂上）时它不进这个
/// 集合，调用方据此只对走通的根做 reconcile——否则拔掉一块移动硬盘，
/// 那上面的几百条资源就会被整片误判成「消失」，而这正是最怕的误伤。
pub fn walk_media_files(roots: &[String]) -> (Vec<ScannedAsset>, BTreeSet<usize>) {
    let mut files: Vec<ScannedAsset> = Vec::new();
    let mut walked: BTreeSet<usize> = BTreeSet::new();

    for (index, root) in roots.iter().enumerate() {
        let root_path = Path::new(root);
        if !root_path.is_dir() {
            continue; // 盘没挂上 / 目录被删：不算走通，也不参与 missing 判定
        }
        walked.insert(index);
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
            let file_name = entry.file_name().to_string_lossy().to_string();
            let meta = match entry.metadata() {
                Ok(m) => m,
                Err(_) => continue, // 读不到元数据就当没看见，不猜
            };
            let size = meta.len();
            // 认得出扩展名就直接过；认不出来（无扩展名 / 被改名）才读文件头兜底，
            // 与 `core::scanner::collect_files_reported` 完全同一判据。
            if !crate::core::parser::is_video_file(&file_name)
                && !crate::core::parser::looks_like_video_file(entry.path(), size)
            {
                continue;
            }
            let dir = entry.path().parent().unwrap_or(root_path);
            files.push(ScannedAsset {
                root_path: trim_root(root).to_string(),
                path: entry.path().to_string_lossy().to_string(),
                dir_path: dir.to_string_lossy().to_string(),
                rel_dir: dir
                    .strip_prefix(root_path)
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_default(),
                file_name: file_name.clone(),
                ext: ext_of(&file_name),
                size,
                mtime: mtime_of(&meta),
                content_key: content_key_of(&file_name),
            });
        }
    }

    (files, walked)
}

/* ------------------------------ 入库 ------------------------------ */

/// 入库计划里的一行：磁盘事实 + 判定结论 + 库里那一行的 id（0 = 还没有）。
struct Planned<'a> {
    asset: &'a ScannedAsset,
    change: AssetChange,
    id: i64,
}

/// 把一批磁盘文件写进资源目录库，并标出消失的行。
///
/// 这是整个模块的核心，也是**单测直接调用的入口**——它不碰磁盘，
/// 只认传进来的 `files`。`roots` 与 `walked` 界定 reconcile 的范围：
/// 只有「本次真正走通的根目录下」且当前是 `present` 的行才可能被判为消失。
pub fn apply_catalog_sync(
    conn: &mut Connection,
    roots: &[String],
    files: &[ScannedAsset],
    walked: &BTreeSet<usize>,
    started_at: i64,
) -> Result<CatalogSyncResult, String> {
    // 1) 本次磁盘所见：路径 → 事实。重复路径（junction / 硬链接指向同一文件）
    //    只留第一份，否则同一路径会在一次检测里被写两遍，计数看着像翻倍。
    let mut seen: BTreeMap<&str, &ScannedAsset> = BTreeMap::new();
    for f in files {
        seen.entry(f.path.as_str()).or_insert(f);
    }

    // 2) 路径 → media.id（整表读一次，避免每个文件查一遍）
    let links = load_media_links(conn)?;

    // 3) 逐条判定。取旧行与判定都在写事务**之外**做，事务里只剩纯写入。
    let mut added = 0usize;
    let mut updated = 0usize;
    let mut unchanged = 0usize;
    let mut restored = 0usize;
    let mut plan: Vec<Planned> = Vec::with_capacity(seen.len());
    for (path, asset) in &seen {
        let prev = load_prev(conn, path)?;
        let change = registry::classify(
            prev.as_ref().map(|p| (p.size, p.mtime, p.status.as_str())),
            &AssetFingerprint {
                size: asset.size,
                mtime: asset.mtime,
            },
        );
        match change {
            AssetChange::New => added += 1,
            AssetChange::Modified => updated += 1,
            AssetChange::Unchanged => unchanged += 1,
            AssetChange::Restored => restored += 1,
        }
        plan.push(Planned {
            asset,
            change,
            id: prev.map(|p| p.id).unwrap_or(0),
        });
    }
    for chunk in plan.chunks(UPSERT_CHUNK) {
        flush_batch(conn, chunk, &links, started_at)?;
    }

    // 4) reconcile：走通的根目录下、当前 present、但这次没见到的行 → 标 missing。
    //    行**不删**——保留才有「它上次在哪、多大、什么时候不见的」。
    let stale = load_present_rows(conn, roots, walked)?;
    let seen_paths: BTreeSet<String> = seen.keys().map(|p| p.to_string()).collect();
    let to_miss = registry::reconcile(&stale, &seen_paths);
    if !to_miss.is_empty() {
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        for id in &to_miss {
            // `missing_at` 只在第一次消失时落定（COALESCE）：每次检测都刷新它，
            // 「什么时候消失的」这条最有价值的线索就废了。
            tx.execute(
                "UPDATE resource_assets \
                    SET status = ?2, missing_at = COALESCE(missing_at, ?3) \
                  WHERE id = ?1 AND status = ?4",
                params![
                    id,
                    registry::STATUS_MISSING,
                    started_at,
                    registry::STATUS_PRESENT
                ],
            )
            .map_err(|e| e.to_string())?;
        }
        tx.commit().map_err(|e| e.to_string())?;
    }

    let result = CatalogSyncResult {
        started_at,
        finished_at: crate::store::db::now_epoch(),
        roots: roots.len(),
        scanned: seen.len(),
        added,
        updated,
        unchanged,
        missing: to_miss.len(),
        restored,
        message: summarize(added, updated, unchanged, to_miss.len(), restored),
    };
    write_snapshot(conn, roots, &result)?;
    Ok(result)
}

/// 库里已有的那一行（判定与更新都要用）。
struct PrevRow {
    size: u64,
    mtime: Option<i64>,
    status: String,
    id: i64,
}

/// 取出某个路径在库里的旧行。
///
/// 一次一条 SELECT 看着笨，但它跑在写事务**之外**，而 `path` 上有唯一索引，
/// 单条就是索引点查；反过来先把整表读进内存，在大库上会白读几万行。
fn load_prev(conn: &Connection, path: &str) -> Result<Option<PrevRow>, String> {
    conn.query_row(
        "SELECT size, mtime, status, id FROM resource_assets WHERE path = ?1",
        params![path],
        |r| {
            Ok(PrevRow {
                // 负数不该出现（size 恒 >= 0）；真出现了也按 0 处理，
                // 免得 u64 转换把它变成一个天文数字而永远判「变了」。
                size: r.get::<_, i64>(0)?.max(0) as u64,
                mtime: r.get(1)?,
                status: r.get(2)?,
                id: r.get(3)?,
            })
        },
    )
    .optional()
    .map_err(|e| e.to_string())
}

/// 文件路径 → media.id（电影取 `media.file_path`，单集取 `episodes.file_path`）。
///
/// 对不上就留空：资源行必须允许没有作品——「扫到了但还没入库」正是它的常态。
fn load_media_links(conn: &Connection) -> Result<BTreeMap<String, i64>, String> {
    let mut out: BTreeMap<String, i64> = BTreeMap::new();
    let mut stmt = conn
        .prepare("SELECT file_path, id FROM media WHERE file_path IS NOT NULL")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))
        .map_err(|e| e.to_string())?;
    for r in rows {
        let (path, id) = r.map_err(|e| e.to_string())?;
        out.entry(path).or_insert(id);
    }
    // 单集覆盖媒体行：同一个文件同时有 media 行和 episodes 行时，
    // 「这片文件属于哪一集」是更精确的答案。
    let mut stmt = conn
        .prepare("SELECT file_path, media_id FROM episodes WHERE file_path IS NOT NULL")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))
        .map_err(|e| e.to_string())?;
    for r in rows {
        let (path, id) = r.map_err(|e| e.to_string())?;
        out.insert(path, id);
    }
    Ok(out)
}

/// 写一批行（幂等：同一个路径写第二遍只会刷新 last_seen）。
///
/// 四种变化分开写，是**故意**的：
/// - `New`：第一次见到，`first_seen` 与 `last_seen` 都从这次开始；
/// - `Modified`：内容变了，全字段刷新，但 `first_seen` **绝不覆盖**；
/// - `Restored`：行还在（上次标了 missing），置回 present 并清掉 `missing_at`；
/// - `Unchanged`：只刷新 `last_seen`，一个字节的内容字段都不重写。
fn flush_batch(
    conn: &mut Connection,
    batch: &[Planned<'_>],
    links: &BTreeMap<String, i64>,
    ts: i64,
) -> Result<(), String> {
    if batch.is_empty() {
        return Ok(());
    }
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    for p in batch {
        let a = p.asset;
        let media_id = links.get(&a.path).copied();
        if p.id == 0 || p.change == AssetChange::New {
            tx.execute(
                "INSERT INTO resource_assets
                     (root_path, dir_path, rel_dir, file_name, path, size, mtime, ext,
                      status, first_seen, last_seen, missing_at, media_id, content_key)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10, NULL, ?11, ?12)
                 ON CONFLICT(path) DO UPDATE SET last_seen = excluded.last_seen",
                params![
                    a.root_path,
                    a.dir_path,
                    a.rel_dir,
                    a.file_name,
                    a.path,
                    a.size as i64,
                    a.mtime,
                    a.ext,
                    registry::STATUS_PRESENT,
                    ts,
                    media_id,
                    a.content_key,
                ],
            )
            .map_err(|e| format!("写入资源目录失败: {e}"))?;
        } else if p.change == AssetChange::Unchanged {
            tx.execute(
                "UPDATE resource_assets SET status = ?2, last_seen = ?3 WHERE id = ?1",
                params![p.id, registry::STATUS_PRESENT, ts],
            )
            .map_err(|e| e.to_string())?;
        } else {
            tx.execute(
                "UPDATE resource_assets
                    SET root_path = ?2, dir_path = ?3, rel_dir = ?4, file_name = ?5, size = ?6,
                        mtime = ?7, ext = ?8, status = ?9, last_seen = ?10, missing_at = NULL,
                        media_id = ?11, content_key = ?12
                  WHERE id = ?1",
                params![
                    p.id,
                    a.root_path,
                    a.dir_path,
                    a.rel_dir,
                    a.file_name,
                    a.size as i64,
                    a.mtime,
                    a.ext,
                    registry::STATUS_PRESENT,
                    ts,
                    media_id,
                    a.content_key,
                ],
            )
            .map_err(|e| e.to_string())?;
        }
    }
    tx.commit().map_err(|e| e.to_string())
}

/// `path` 是否就在目录 `dir` 之下（按分隔符边界比较）。
///
/// 逐字符比较而不是折叠大小写：入库存的是原样路径，唯一索引也是逐字符的，
/// 判定口径必须跟它一致，否则会出现「库里有这一行、reconcile 却认不出来」。
fn path_under(path: &str, dir: &str) -> bool {
    let dir = trim_root(dir);
    match path.strip_prefix(dir) {
        Some(rest) => rest.starts_with(is_sep),
        None => false,
    }
}

/// 本次真正走通的那些根目录下、当前仍是 `present` 的 `(id, path)`。
///
/// 只取走通的根：盘没挂上时那一片资源不该被判成消失。
/// 只取 `present`：已经 missing 的行**不重复标记**——重复标记会把 `missing_at`
/// 冲成「现在」，而「什么时候消失的」正是最有价值的那条线索。
///
/// 用**路径前缀**而不是库里存的 `root_path` 来界定范围：用户随时可能把媒体目录
/// 从 `D:/media` 改成 `D:/media/movies`，只比字符串的话，老行就再也不会被
/// reconcile 到，会永远停在 `present`——那等于「定位旧资源」这条线断了。
/// 按前缀匹配则不受设置改动影响，只认「这次到底翻过哪些目录」。
fn load_present_rows(
    conn: &Connection,
    roots: &[String],
    walked: &BTreeSet<usize>,
) -> Result<Vec<(i64, String)>, String> {
    let live: Vec<&str> = walked
        .iter()
        .filter_map(|i| roots.get(*i).map(|r| trim_root(r.as_str())))
        .collect();
    let mut stmt = conn
        .prepare("SELECT id, path FROM resource_assets WHERE status = ?1")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![registry::STATUS_PRESENT], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
        })
        .map_err(|e| e.to_string())?;
    let mut out: Vec<(i64, String)> = Vec::new();
    for r in rows {
        let (id, path) = r.map_err(|e| e.to_string())?;
        if live.iter().any(|root| path_under(&path, root)) {
            out.push((id, path));
        }
    }
    Ok(out)
}

/// 写一条 `resource_snapshots`：每次检测都留一行，供「每次检测方便对比」。
fn write_snapshot(
    conn: &Connection,
    roots: &[String],
    r: &CatalogSyncResult,
) -> Result<(), String> {
    conn.execute(
        "INSERT INTO resource_snapshots
             (started_at, finished_at, roots, scanned, added, updated, unchanged, missing,
              restored, message)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            r.started_at,
            r.finished_at,
            summarize_roots(roots),
            r.scanned as i64,
            r.added as i64,
            r.updated as i64,
            r.unchanged as i64,
            r.missing as i64,
            r.restored as i64,
            r.message,
        ],
    )
    .map_err(|e| format!("写入资源快照失败: {e}"))?;
    Ok(())
}

/// 根目录列表的可读文本（过长时截断，别把整行撑爆）。
fn summarize_roots(roots: &[String]) -> String {
    let joined = roots.join(" ; ");
    if joined.chars().count() <= 180 {
        joined
    } else {
        let head: String = joined.chars().take(177).collect();
        format!("{head}...")
    }
}

/// 一句话汇总（面向用户的中文提示；数字本身就是结论，不堆形容词）。
fn summarize(
    added: usize,
    updated: usize,
    unchanged: usize,
    missing: usize,
    restored: usize,
) -> String {
    if added + updated + missing + restored == 0 {
        return format!("检测完成：{unchanged} 个文件与上次一致，没有变化");
    }
    let mut parts: Vec<String> = Vec::new();
    if added > 0 {
        parts.push(format!("新增 {added}"));
    }
    if updated > 0 {
        parts.push(format!("变动 {updated}"));
    }
    if restored > 0 {
        parts.push(format!("重新出现 {restored}"));
    }
    if missing > 0 {
        parts.push(format!("消失 {missing}"));
    }
    parts.push(format!("未变 {unchanged}"));
    format!("检测完成：{}", parts.join("，"))
}

/* ------------------------------ 命令 ------------------------------ */

/// 同步资源目录：遍历设置里的媒体目录 → upsert → 标 missing → 写快照。
///
/// 媒体目录的取法与 `scan_library` 完全一致（`settings.mediaDirs`），
/// 不引入第二套「媒体目录在哪」的口径。
#[tauri::command]
pub fn sync_resource_catalog(
    state: State<'_, Mutex<Connection>>,
) -> Result<CatalogSyncResult, String> {
    let mut conn = state.lock().map_err(|e| e.to_string())?;
    let roots = load_media_dirs(&conn);
    if roots.is_empty() {
        return Err("尚未配置媒体目录，请先在设置中添加".to_string());
    }
    let started_at = crate::store::db::now_epoch();
    let (files, walked) = walk_media_files(&roots);
    apply_catalog_sync(&mut conn, &roots, &files, &walked, started_at)
}

/// 资源目录列表（可按状态过滤、按关键词搜）。
///
/// `keyword` 同时匹配文件名与相对目录——用户记得的往往是「在哪个文件夹里」。
/// LIKE 的元字符必须先转义（`store::search::escape_like`，全项目只此一份），
/// 否则关键词里的 `%` 会变成通配符，把整库都搜出来。
#[tauri::command]
pub fn get_resource_catalog(
    state: State<'_, Mutex<Connection>>,
    limit: Option<i64>,
    status: Option<String>,
    keyword: Option<String>,
) -> Result<Vec<CatalogEntry>, String> {
    let conn = state.lock().map_err(|e| e.to_string())?;
    get_resource_catalog_sync(
        &conn,
        limit.unwrap_or(200),
        status.as_deref(),
        keyword.as_deref(),
    )
}

/// `get_resource_catalog` 的同步实现（命令只是加锁转调，方便单测）。
pub fn get_resource_catalog_sync(
    conn: &Connection,
    limit: i64,
    status: Option<&str>,
    keyword: Option<&str>,
) -> Result<Vec<CatalogEntry>, String> {
    // 上限兜底：前端传 0 / 负数时给一个安全值，避免「一条都不要」这种谜之空列表。
    let limit = limit.clamp(1, 5000);
    let status = status.unwrap_or("").trim().to_string();
    let keyword = keyword.unwrap_or("").trim().to_string();
    let pattern = if keyword.is_empty() {
        String::new()
    } else {
        escape_like(&keyword)
    };

    // LEFT JOIN：对不上作品条目也要显示——「盘上有、库里没有」正是要找的东西。
    let sql = format!(
        "SELECT {CATALOG_COLUMNS} FROM resource_assets r \
           LEFT JOIN media m ON m.id = r.media_id \
          WHERE (?1 = '' OR r.status = ?1) \
            AND (?2 = '' \
                 OR r.file_name LIKE '%' || ?2 || '%' ESCAPE '\\' \
                 OR r.rel_dir   LIKE '%' || ?2 || '%' ESCAPE '\\') \
          ORDER BY r.status ASC, r.dir_path COLLATE NOCASE ASC, \
                   r.file_name COLLATE NOCASE ASC \
          LIMIT ?3"
    );
    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![status, pattern, limit], read_entry)
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| e.to_string())?);
    }
    Ok(out)
}

/// 查询列的唯一定义（列表与差异清单共用，避免两处口径漂移）。
const CATALOG_COLUMNS: &str = "r.id, r.path, r.file_name, r.rel_dir, r.root_path, r.size, \
     r.mtime, r.status, r.media_id, m.title, m.type, m.group_key";

/// 从一行结果读出 `CatalogEntry`。
///
/// `display_title` 与媒体库列表同口径：同名剧集带目录名后缀（`剧名 · 分组键`），
/// 否则前端会对着满屏同名卡片分不清谁是谁。
fn read_entry(r: &rusqlite::Row<'_>) -> rusqlite::Result<CatalogEntry> {
    let title: Option<String> = r.get(9)?;
    let group_key: Option<String> = r.get(11)?;
    let display_title = match (&title, &group_key) {
        (Some(t), Some(g)) if !g.is_empty() && g != t => Some(format!("{t} · {g}")),
        (Some(t), _) => Some(t.clone()),
        (None, _) => None,
    };
    Ok(CatalogEntry {
        id: r.get(0)?,
        path: r.get(1)?,
        file_name: r.get(2)?,
        rel_dir: r.get(3)?,
        root_path: r.get(4)?,
        size: r.get(5)?,
        mtime: r.get(6)?,
        status: r.get(7)?,
        media_id: r.get(8)?,
        display_title,
        media_type: r.get(10)?,
    })
}

/// 本次检测相对上次的差异。
///
/// ## `changed` 的口径
/// 用 `mtime`：**上次检测结束之后被改写过的文件**。判据是
/// `first_seen <= 上次快照时间 AND mtime > 上次快照时间`
/// ——「上次检测时就见过它」+「它在那之后被改过」，两条同时成立才算变动。
///
/// 为什么不用 `last_seen`：每次检测都会把本次见到的每一行都写一遍
/// （新建或刷新 `last_seen`），所以 `last_seen > 上次快照` 对「每次都见到的
/// 老文件」同样成立，会退化成「上次检测之后盘上有的全部文件」——对不上「变动」二字。
/// `mtime` 是文件系统给的改动时刻，不受本模块写库策略影响，才真正能区分。
///
/// 已知取舍：`mtime` 取不到的文件（极少数网络盘）不会出现在 `changed` 里
/// ——这里宁可**少报**也不假报「变了」。`added` 用 `first_seen`，两个清单
/// 互不重叠（一个要求 `first_seen > 门槛`，另一个要求 `<=`）。
#[tauri::command]
pub fn diff_resource_catalog(
    state: State<'_, Mutex<Connection>>,
) -> Result<CatalogDiff, String> {
    let conn = state.lock().map_err(|e| e.to_string())?;
    diff_resource_catalog_sync(&conn)
}

/// `diff_resource_catalog` 的同步实现。
pub fn diff_resource_catalog_sync(conn: &Connection) -> Result<CatalogDiff, String> {
    let last = last_snapshot_at(conn)?;
    let present = count_present(conn)?;
    // 从未检测过时用 i64::MAX 当门槛：`x > MAX` 恒不成立，两个清单自然为空，
    // SQL 里不必再为「没有上次」拼一个分支。
    let gate = last.unwrap_or(i64::MAX);

    let added = query_entries(
        conn,
        "SELECT {cols} FROM resource_assets r LEFT JOIN media m ON m.id = r.media_id \
          WHERE r.status = ?1 AND r.first_seen > ?2 \
          ORDER BY r.first_seen DESC, r.id DESC LIMIT ?3",
        registry::STATUS_PRESENT,
        gate,
    )?;
    let changed = query_entries(
        conn,
        "SELECT {cols} FROM resource_assets r LEFT JOIN media m ON m.id = r.media_id \
          WHERE r.status = ?1 AND r.first_seen <= ?2 \
            AND r.mtime IS NOT NULL AND r.mtime > ?2 \
          ORDER BY r.mtime DESC, r.id DESC LIMIT ?3",
        registry::STATUS_PRESENT,
        gate,
    )?;
    let missing = query_entries(
        conn,
        "SELECT {cols} FROM resource_assets r LEFT JOIN media m ON m.id = r.media_id \
          WHERE r.status = ?1 \
          ORDER BY COALESCE(r.missing_at, 0) DESC, r.id DESC LIMIT ?3",
        registry::STATUS_MISSING,
        gate,
    )?;

    Ok(CatalogDiff {
        added,
        changed,
        missing,
        present,
        last_sync_at: last,
    })
}

/// 按给定模板取一批条目（`?1` = 状态，`?2` = 上次检测的时间门槛，`?3` = 上限）。
fn query_entries(
    conn: &Connection,
    sql: &str,
    status: &str,
    gate: i64,
) -> Result<Vec<CatalogEntry>, String> {
    let sql = sql.replace("{cols}", CATALOG_COLUMNS);
    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![status, gate, DIFF_LIMIT], read_entry)
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| e.to_string())?);
    }
    Ok(out)
}

/// 上一次检测的结束时间（没有快照时为 `None`）。
fn last_snapshot_at(conn: &Connection) -> Result<Option<i64>, String> {
    conn.query_row(
        "SELECT finished_at FROM resource_snapshots \
          WHERE finished_at IS NOT NULL ORDER BY finished_at DESC, id DESC LIMIT 1",
        [],
        |r| r.get::<_, Option<i64>>(0),
    )
    .optional()
    .map(|v| v.flatten())
    .map_err(|e| e.to_string())
}

/// 当前仍在盘上的资源数（`missing` 的不计）。
fn count_present(conn: &Connection) -> Result<usize, String> {
    let n: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM resource_assets WHERE status = ?1",
            params![registry::STATUS_PRESENT],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    Ok(n.max(0) as usize)
}

#[cfg(test)]
#[path = "tests/registry.rs"]
mod tests;
