//! 媒体库查询与扫描命令。
//!
//! ## 扫描为什么不能再同步执行
//! 和刮削同理（见 `scrape.rs` 模块头）：Tauri 的**同步**命令跑在主线程上，
//! 而遍历一个几 TB 的媒体目录要几十秒到几分钟——这段时间主线程收不到窗口
//! 消息，界面就是「卡死」的样子。
//!
//! 所以扫描改成和批量刮削一样的形式：
//! - `scan_library` 只负责**起任务**并立即返回，真正的遍历与入库在后台线程；
//! - 期间通过 `scan-progress` 事件推送「已发现多少个 / 正在入库第几个」；
//! - 遍历阶段可随时 `cancel_scan` 取消（入库阶段不中断，见 `scanner` 注释）。

use crate::collection;
use crate::commands::settings::load_media_dirs;
use crate::mediatype::{self, FolderEntry, FolderStat, MediaKind, Signals};
use crate::parser::{parse_file_name, ParsedName};
use crate::scanner::{self, ScanStats};
use rusqlite::{params, Connection, Row};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, State};

/// 扫描进度事件名（前端 `listen("scan-progress", ...)`）。
pub const PROGRESS_EVENT: &str = "scan-progress";

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
    /// `scan`（扫描归纳）/ `manual`（手动新建的 Series）。
    pub source: String,
    /// 手动锁定的分组不会被二次校正调整。
    pub locked: bool,
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

pub(crate) const ITEM_COLUMNS: &str = "m.id, m.type, m.title, m.year, m.tmdb_id, m.poster_path, m.backdrop_path, \
     m.overview, m.vote_average, m.genre, m.file_path, m.file_name, m.added_at, \
     COALESCE(m.source, 'scan') AS source, COALESCE(m.locked, 0) AS locked, \
     (SELECT COUNT(*) FROM episodes e WHERE e.media_id = m.id) AS episode_count, \
     CASE WHEN m.group_key IS NOT NULL \
            AND (SELECT COUNT(*) FROM media d WHERE d.type = 'tv' AND d.type = m.type \
                 AND d.title = m.title) > 1 \
          THEN m.title || ' · ' || m.group_key ELSE m.title END AS display_title";

pub(crate) fn map_item(row: &Row<'_>) -> rusqlite::Result<MediaListItem> {
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
        source: row.get(13)?,
        locked: row.get::<_, i64>(14)? != 0,
        episode_count: row.get(15)?,
        display_title: row.get(16)?,
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

/// 扫描进度（事件载荷）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanProgress {
    pub running: bool,
    /// 本次任务是否已结束（无论正常完成、被取消还是出错）。
    pub finished: bool,
    pub cancelled: bool,
    /// 阶段说明：`遍历目录` / `写入数据库` / `完成`。
    pub phase: String,
    /// 已发现的视频文件数（遍历阶段实时增长）。
    pub found: i64,
    /// 已入库条数（入库阶段实时增长）。
    pub imported: i64,
    /// 待入库总数（进入入库阶段后才确定）。
    pub total: i64,
    /// 当前目录 / 文件名。
    pub current: String,
    pub message: String,
    /// 结束时的统计；进行中为 null。
    pub stats: Option<ScanStats>,
}

impl ScanProgress {
    fn tick(phase: &str, found: i64, imported: i64, total: i64, current: String) -> Self {
        Self {
            running: true,
            finished: false,
            cancelled: false,
            phase: phase.to_string(),
            found,
            imported,
            total,
            current,
            message: String::new(),
            stats: None,
        }
    }

    fn settled(phase: &str, message: String, cancelled: bool, stats: Option<ScanStats>) -> Self {
        Self {
            running: false,
            finished: true,
            cancelled,
            phase: phase.to_string(),
            found: stats.as_ref().map(|s| s.scanned as i64).unwrap_or(0),
            imported: stats
                .as_ref()
                .map(|s| (s.added_movies + s.added_episodes) as i64)
                .unwrap_or(0),
            total: stats.as_ref().map(|s| s.scanned as i64).unwrap_or(0),
            current: String::new(),
            message,
            stats,
        }
    }
}

/// 启动扫描的返回值：命令不等结果，只说明「起没起来」。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanStart {
    pub started: bool,
    pub message: String,
}

/// 扫描的控制开关：取消标志 + 运行中标志（与 `ScrapeControl` 同构）。
#[derive(Debug, Default)]
pub struct ScanControl {
    cancel: AtomicBool,
    running: AtomicBool,
}

impl ScanControl {
    pub fn begin(&self) {
        self.cancel.store(false, Ordering::SeqCst);
        self.running.store(true, Ordering::SeqCst);
    }

    pub fn finish(&self) {
        self.running.store(false, Ordering::SeqCst);
    }

    pub fn request_cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }

    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }

    pub fn running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }
}

/// 扫描媒体库。`roots` 为空时回落到设置里的 `mediaDirs`。
///
/// 命令**立即返回**，实际工作交给后台线程；进度通过 `scan-progress` 事件推送。
#[tauri::command]
pub async fn scan_library(
    app: AppHandle,
    state: State<'_, Mutex<Connection>>,
    control: State<'_, Arc<ScanControl>>,
    roots: Option<Vec<String>>,
) -> Result<ScanStart, String> {
    let dirs = {
        let conn = state.lock().map_err(|e| e.to_string())?;
        match roots {
            Some(r) if !r.is_empty() => r,
            _ => load_media_dirs(&conn),
        }
    };
    if dirs.is_empty() {
        return Ok(ScanStart {
            started: false,
            message: "尚未配置媒体目录，请先在设置中添加".to_string(),
        });
    }
    if control.running() {
        return Ok(ScanStart {
            started: false,
            message: "已有扫描任务在进行中".to_string(),
        });
    }

    let root_count = dirs.len();
    let control = control.inner().clone();
    control.begin();
    std::thread::spawn(move || {
        run_scan(app, control, dirs);
    });

    Ok(ScanStart {
        started: true,
        message: format!("已开始扫描 {} 个目录", root_count),
    })
}

/// 后台扫描：先遍历目录（可取消），再入库（不可取消）。
fn run_scan(app: AppHandle, control: Arc<ScanControl>, roots: Vec<String>) {
    let emit = |p: ScanProgress| {
        let _ = app.emit(PROGRESS_EVENT, p);
    };

    // 后台线程用自己的连接：WAL 模式下多连接并发读写是安全的，
    // 而且扫描期间不该占着全局连接锁，否则界面上的列表查询会一起卡住。
    let mut conn = match crate::db::open(&app) {
        Ok(c) => c,
        Err(e) => {
            emit(ScanProgress::settled(
                "出错",
                format!("打开数据库失败: {e}"),
                false,
                None,
            ));
            control.finish();
            return;
        }
    };

    // --- 阶段一：遍历目录（可取消）---
    let mut cancelled = false;
    let files = {
        let control = &control;
        let mut tick = |_phase: &'static str, n: usize, dir: &str| -> bool {
            if control.cancelled() {
                cancelled = true;
                return false;
            }
            emit(ScanProgress::tick(
                "遍历目录",
                n as i64,
                0,
                0,
                dir.to_string(),
            ));
            true
        };
        scanner::collect_files_reported(&roots, &mut tick)
    };
    if cancelled {
        emit(ScanProgress::settled(
            "已取消",
            format!("已取消（已遍历 {} 个文件，未写入数据库）", files.len()),
            true,
            None,
        ));
        control.finish();
        return;
    }

    // --- 阶段二：入库 ---
    let total = files.len() as i64;
    let mut tick = |_phase: &'static str, n: usize, name: &str| -> bool {
        emit(ScanProgress::tick("写入数据库", total, n as i64, total, name.to_string()));
        true
    };
    match scanner::import_reported(&mut conn, &files, &roots, &mut tick) {
        Ok(stats) => emit(ScanProgress::settled(
            "完成",
            format!(
                "扫描完成：发现 {} 个视频（新增电影 {}、剧集 {}、单集 {}）",
                stats.scanned, stats.added_movies, stats.added_series, stats.added_episodes
            ),
            false,
            Some(stats),
        )),
        Err(e) => emit(ScanProgress::settled("出错", format!("扫描失败：{e}"), false, None)),
    }
    control.finish();
}

/// 取消正在进行的扫描（仅在遍历目录阶段生效，入库阶段不中断）。
#[tauri::command]
pub fn cancel_scan(control: State<'_, Arc<ScanControl>>) -> Result<bool, String> {
    control.request_cancel();
    Ok(control.running())
}

/* --------------------------- 从文件夹手动导入 --------------------------- */

/// 从文件夹导入的结果（界面直接展示这几个数字）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportFolderResult {
    /// 本次真正入库的文件数（电影 + 单集）。
    pub added: i64,
    /// 库里已经有了、被跳过的文件数。
    pub skipped: i64,
    /// 入库失败的文件数。
    pub failed: i64,
    /// 挂到目标栏目下的影视条数。
    pub linked: i64,
    pub message: String,
}

/// 导入的核心逻辑（同步，供命令与单测共用）。
///
/// 复用扫描那套「遍历 → 解析 → 分组 → 入库」代码，所以**重复项自动跳过**是
/// 天然成立的（入库语句是 `ON CONFLICT(file_path) DO NOTHING`）：同一个文件
/// 第二次导入只会进 `skipped`，不会产生第二条记录。
pub fn import_folder_sync(
    conn: &mut Connection,
    path: &str,
    category_id: Option<i64>,
) -> Result<ImportFolderResult, String> {
    if !std::path::Path::new(path).is_dir() {
        return Err(format!("目录不存在或不可访问：{path}"));
    }
    let roots = vec![path.to_string()];
    let files = scanner::collect_files_reported(&roots, &mut |_, _, _| true);
    if files.is_empty() {
        return Ok(ImportFolderResult {
            added: 0,
            skipped: 0,
            failed: 0,
            linked: 0,
            message: "该目录下没有找到视频文件".to_string(),
        });
    }

    let stats = scanner::import_reported(conn, &files, &roots, &mut |_, _, _| true)?;
    let linked = match category_id {
        Some(cid) => link_imported_to_category(conn, &files, cid)?,
        None => 0,
    };
    // 「待确认」也是入库了的一条，必须算进 `added`——否则导入一个判定不出来的
    // 文件时界面会报「新增 0 个」，而库里其实多了一条。
    let added = (stats.added_movies + stats.added_episodes + stats.added_unknown) as i64;
    let skipped = stats.skipped as i64;
    let failed = stats.failed as i64;

    let mut parts = vec![format!("新增 {added} 个文件")];
    if skipped > 0 {
        parts.push(format!("跳过 {skipped} 个已存在"));
    }
    if failed > 0 {
        parts.push(format!("{failed} 个失败"));
    }
    if stats.added_unknown > 0 {
        parts.push(format!("{0} 个待确认", stats.added_unknown));
    }
    if linked > 0 {
        parts.push(format!("归入栏目 {linked} 条"));
    }
    Ok(ImportFolderResult {
        added,
        skipped,
        failed,
        linked,
        message: parts.join("，"),
    })
}

/// 把刚导入的文件对应的影视条目挂到指定栏目下。
///
/// 电影按 `media.file_path` 直接命中；剧集的单集在 `episodes` 表里，要绕一层
/// `media_id`。被跳过的重复文件同样会被挂上——它们本来就属于这个栏目。
fn link_imported_to_category(
    conn: &Connection,
    files: &[scanner::ScanFile],
    category_id: i64,
) -> Result<i64, String> {
    let ts = crate::db::now_epoch();
    let mut ids: BTreeSet<i64> = BTreeSet::new();
    for f in files {
        let path = f.path.to_string_lossy().to_string();
        if let Ok(id) = conn.query_row(
            "SELECT id FROM media WHERE file_path = ?1",
            params![path],
            |r| r.get::<_, i64>(0),
        ) {
            ids.insert(id);
        }
        if let Ok(id) = conn.query_row(
            "SELECT media_id FROM episodes WHERE file_path = ?1",
            params![path],
            |r| r.get::<_, i64>(0),
        ) {
            ids.insert(id);
        }
    }
    let mut linked = 0i64;
    for id in ids {
        let n = conn
            .execute(
                "INSERT OR IGNORE INTO media_categories (media_id, category_id, created_at) \
                 VALUES (?1, ?2, ?3)",
                params![id, category_id, ts],
            )
            .map_err(|e| e.to_string())?;
        linked += n as i64;
    }
    Ok(linked)
}

/// 从本地文件夹导入影视（界面上的「从文件夹添加」）。
///
/// 与扫描同理：遍历 + 入库放到后台线程（`spawn_blocking`）并用自己的连接，
/// 否则一个大目录会把主线程（界面）锁住几十秒。
#[tauri::command]
pub async fn import_folder(
    app: AppHandle,
    path: String,
    category_id: Option<i64>,
) -> Result<ImportFolderResult, String> {
    let path = path.trim().to_string();
    if path.is_empty() {
        return Err("请先选择要导入的文件夹".to_string());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let mut conn = crate::db::open(&app).map_err(|e| format!("打开数据库失败：{e}"))?;
        import_folder_sync(&mut conn, &path, category_id)
    })
    .await
    .map_err(|e| format!("导入任务异常：{e}"))?
}

/// 列表筛选条件（全部可选，不传就是「不过滤」）。
const LIST_WHERE: &str = "
         WHERE COALESCE(?1, 'all') IN ('all', m.type)
           AND (?2 IS NULL OR EXISTS (SELECT 1 FROM media_categories mc
                                      WHERE mc.media_id = m.id AND mc.category_id = ?2))
           AND (?3 IS NULL OR EXISTS (SELECT 1 FROM media_tags mt
                                      WHERE mt.media_id = m.id AND mt.tag_id = ?3))
           AND (?4 IS NULL OR ?4 = ''
                OR m.title LIKE '%' || ?4 || '%'
                OR m.original_title LIKE '%' || ?4 || '%'
                OR m.genre LIKE '%' || ?4 || '%'
                OR COALESCE(m.group_key, '') LIKE '%' || ?4 || '%'
                OR CAST(m.year AS TEXT) LIKE '%' || ?4 || '%')
           AND (?5 IS NULL
                OR (CASE WHEN EXISTS (SELECT 1 FROM media_categories mc2
                                      WHERE mc2.media_id = m.id) THEN 0 ELSE 1 END) = ?5)";

/// `unclassified` 参数归一化：只有 `Some(true)` 才过滤（只看未归类），
/// `None` 与 `Some(false)` 一律不过滤。
fn normalize_unclassified(unclassified: Option<bool>) -> Option<i64> {
    unclassified.filter(|b| *b).map(|_| 1i64)
}

/// 媒体列表。
///
/// 过滤条件全部是**可选**的：`media_type`（all/movie/tv）、`category_id`、
/// `tag_id`、`keyword`（标题 / 原名 / 类型 / 年份 / 目录名）、
/// `unclassified`（1 = 只看还没归类的）。
#[tauri::command]
pub fn get_media_list(
    state: State<'_, Mutex<Connection>>,
    media_type: Option<String>,
    category_id: Option<i64>,
    tag_id: Option<i64>,
    keyword: Option<String>,
    unclassified: Option<bool>,
) -> Result<Vec<MediaListItem>, String> {
    let conn = state.lock().map_err(|e| e.to_string())?;
    let filter = media_type.unwrap_or_else(|| "all".to_string());
    let kw = keyword.unwrap_or_default().trim().to_string();
    let kw = if kw.is_empty() { None } else { Some(kw) };
    // 关键：`None` 与 `Some(false)` 都必须是「不过滤」。
    // 前端默认会传 `unclassified: false`（而不是不传）；若把它当成 0 传进 SQL，
    // 条件 `(CASE WHEN 有分类 THEN 0 ELSE 1 END) = ?5` 就只剩「已归类」的条目——
    // 历史数据没有分类关联时，「全部 / 电影 / 剧集」三个栏目会整体空白。
    let unclassified = normalize_unclassified(unclassified);

    let sql = format!(
        "SELECT {ITEM_COLUMNS} FROM media m {LIST_WHERE} \
         ORDER BY m.title COLLATE NOCASE ASC, m.year ASC"
    );
    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![filter, category_id, tag_id, kw, unclassified], map_item)
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| e.to_string())?);
    }
    Ok(out)
}

/// 给一条媒体挂分类与标签（手动新建时用；不存在的标签名会自动建出来）。
fn link_categories_and_tags(
    conn: &Connection,
    media_id: i64,
    category_ids: Option<Vec<i64>>,
    tag_names: Option<Vec<String>>,
    ts: i64,
) -> Result<(), String> {
    if let Some(ids) = category_ids {
        for cid in ids {
            conn.execute(
                "INSERT OR IGNORE INTO media_categories (media_id, category_id, created_at) \
                 VALUES (?1, ?2, ?3)",
                params![media_id, cid, ts],
            )
            .map_err(|e| e.to_string())?;
        }
    }
    if let Some(names) = tag_names {
        for raw in names {
            let name = raw.trim().to_string();
            if name.is_empty() {
                continue;
            }
            let tag_id: i64 = match conn
                .query_row(
                    "SELECT id FROM tags WHERE name = ?1 COLLATE NOCASE",
                    params![name],
                    |r| r.get(0),
                )
                .ok()
            {
                Some(v) => v,
                None => {
                    conn.execute(
                        "INSERT INTO tags (name, created_at) VALUES (?1, ?2)",
                        params![name, ts],
                    )
                    .map_err(|e| e.to_string())?;
                    conn.last_insert_rowid()
                }
            };
            conn.execute(
                "INSERT OR IGNORE INTO media_tags (media_id, tag_id, created_at) VALUES (?1,?2,?3)",
                params![media_id, tag_id, ts],
            )
            .map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

/// 手动新建一条资源（**电影或剧集**）。
///
/// 与 `create_series` 共用同一张 `media` 表（`source = 'manual'`），区别只在
/// `type` 可以是 `'movie'`——手动建的电影没有文件关联，但详情、刮削、归类、
/// 打标签走同一条链路，不是另一套数据。
#[tauri::command]
pub fn create_media(
    state: State<'_, Mutex<Connection>>,
    title: String,
    media_type: String,
    year: Option<i32>,
    genre: Option<String>,
    overview: Option<String>,
    category_ids: Option<Vec<i64>>,
    tag_names: Option<Vec<String>>,
) -> Result<MediaListItem, String> {
    let conn = state.lock().map_err(|e| e.to_string())?;
    let ts = crate::db::now_epoch();
    let id = insert_manual_media(&conn, &title, &media_type, year, genre, overview, ts)?;
    link_categories_and_tags(&conn, id, category_ids, tag_names, ts)?;
    drop(conn);
    get_media_detail(state, id).map(|d| d.media)
}

/// 更新一条资源的基本信息（名称 / 类型 / 年份 / 流派 / 简介）。
///
/// 几个有意为之的行为（见 `update_media_fields`）：
/// - `original_title` 没被刮削改写过才跟着改名；
/// - 手动编辑会把 `locked` 置 1，二次校正不再动这条；
/// - 剧集改成电影**不会**删除已有单集（不做破坏性操作）。
#[tauri::command]
pub fn update_media(
    state: State<'_, Mutex<Connection>>,
    id: i64,
    title: String,
    media_type: String,
    year: Option<i32>,
    genre: Option<String>,
    overview: Option<String>,
) -> Result<MediaListItem, String> {
    let conn = state.lock().map_err(|e| e.to_string())?;
    update_media_fields(&conn, id, &title, &media_type, year, genre, overview)?;
    drop(conn);
    get_media_detail(state, id).map(|d| d.media)
}

/// 清洗一个可空的文本字段：trim 后为空串按 None 处理（前端留空 = 清空）。
fn clean_optional(raw: Option<String>) -> Option<String> {
    raw.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// 手动新建资源的入库逻辑（校验 + INSERT），返回新行 id。
///
/// 校验规则：名称去空格后非空；类型只能是 `movie` / `tv`；
/// 流派 / 简介留空按 NULL 存。`source = 'manual'`、`group_key` 为空——
/// 重扫不会把它并掉，也不会给它凭空造单集。
fn insert_manual_media(
    conn: &Connection,
    title: &str,
    media_type: &str,
    year: Option<i32>,
    genre: Option<String>,
    overview: Option<String>,
    ts: i64,
) -> Result<i64, String> {
    let title = title.trim();
    if title.is_empty() {
        return Err("名称不能为空".to_string());
    }
    if media_type != "movie" && media_type != "tv" {
        return Err(format!("不支持的类型：{media_type}（只能是 movie 或 tv）"));
    }
    let genre = clean_optional(genre);
    let overview = clean_optional(overview);
    conn.execute(
        "INSERT INTO media (type, title, original_title, year, genre, overview, source, \
                            added_at, updated_at) \
         VALUES (?1, ?2, ?2, ?3, ?4, ?5, 'manual', ?6, ?6)",
        params![media_type, title, year, genre, overview, ts],
    )
    .map_err(|e| format!("新建资源失败：{e}"))?;
    Ok(conn.last_insert_rowid())
}

/// 更新基本字段的入库逻辑（校验 + UPDATE）。
///
/// 三个有意为之的行为：
/// - `original_title` 只有在「本来就和 `title` 相同」（即没被刮削改写过）时才跟着
///   改名——它是扫描重分组的兜底键，手动改名不该破坏它；
/// - 手动编辑会把 `locked` 置 1：用户已经亲手整理过，二次校正不再动这条；
/// - 类型从剧集改成电影**不会**删除已有单集（不做破坏性操作），只是卡片上
///   不再显示「N 集」；详情页的单集列表仍然保留、可以播放。
fn update_media_fields(
    conn: &Connection,
    id: i64,
    title: &str,
    media_type: &str,
    year: Option<i32>,
    genre: Option<String>,
    overview: Option<String>,
) -> Result<(), String> {
    let title = title.trim();
    if title.is_empty() {
        return Err("名称不能为空".to_string());
    }
    // 编辑允许保留 `unknown`：用户可能只是改个名字，还没弄清楚它到底是电影还是剧集。
    // 新建不允许——那是人亲手录的，没理由录一条自己都说不清是什么的条目。
    if !matches!(media_type, "movie" | "tv" | "unknown") {
        return Err(format!(
            "不支持的类型：{media_type}（只能是 movie / tv / unknown）"
        ));
    }
    let genre = clean_optional(genre);
    let overview = clean_optional(overview);
    let ts = crate::db::now_epoch();
    let changed = conn
        .execute(
            "UPDATE media SET title = ?2, type = ?3, year = ?4, genre = ?5, overview = ?6, \
                original_title = CASE WHEN original_title IS NULL OR original_title = title \
                                      THEN ?2 ELSE original_title END, \
                locked = 1, updated_at = ?7 \
             WHERE id = ?1",
            params![id, title, media_type, year, genre, overview, ts],
        )
        .map_err(|e| format!("更新失败：{e}"))?;
    if changed == 0 {
        return Err(format!("未找到媒体 id={id}"));
    }
    Ok(())
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

/// 按 id 批量删除某一类记录（单事务），返回实际删除的条数。
///
/// `table` 由调用方以字面量传入（不是用户输入），所以这里拼 SQL 是安全的；
/// id 则逐个绑定占位符，绝不拼接。
fn delete_rows(conn: &Connection, table: &str, ids: &[i64]) -> Result<i64, String> {
    if ids.is_empty() {
        return Ok(0);
    }
    let placeholders: Vec<String> = (1..=ids.len()).map(|i| format!("?{i}")).collect();
    let sql = format!("DELETE FROM {table} WHERE id IN ({})", placeholders.join(","));
    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let n = stmt
        .execute(rusqlite::params_from_iter(ids.iter()))
        .map_err(|e| e.to_string())?;
    Ok(n as i64)
}

/// 批量删除媒体（界面上的「多选 → 删除所选」）。
///
/// 只删 `ids` 里的条目——不在列表里的一律不动，这是批量操作的底线。
/// 返回实际删除条数，供界面提示「已删除 N 条」。
#[tauri::command]
pub fn delete_media_batch(
    state: State<'_, Mutex<Connection>>,
    ids: Vec<i64>,
) -> Result<i64, String> {
    let conn = state.lock().map_err(|e| e.to_string())?;
    delete_rows(&conn, "media", &ids)
}

/// 批量删除单集（聚合剧目详情页的「删减资源」）。
///
/// 只删传入的单集 id；同一剧目剩下的单集、以及**其他剧目的资源都不受影响**。
#[tauri::command]
pub fn delete_episodes(
    state: State<'_, Mutex<Connection>>,
    ids: Vec<i64>,
) -> Result<i64, String> {
    let conn = state.lock().map_err(|e| e.to_string())?;
    delete_rows(&conn, "episodes", &ids)
}

/* --------------------------- 存量数据的重新识别 --------------------------- */

/// 重新识别的结果（界面直接展示这几个数字）。
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReclassifyResult {
    /// 参与重新判定的条目数（媒体行 + 单集行）。
    pub scanned: i64,
    pub changed: i64,
    /// 转成剧集（原来被误判成电影 / 待确认的）。
    pub to_tv: i64,
    /// 转成电影（待确认 → 电影，以及「整部剧只有一集」的单集 → 电影）。
    pub to_movie: i64,
    /// 转成待确认（证据不足，不再静默归成电影）。
    pub to_unknown: i64,
    /// 用户手动确认过（`locked`）而跳过的条目数。
    pub skipped_locked: i64,
    pub message: String,
}

/// 一条「单文件」媒体行（重新识别的输入）。
struct FileRow {
    id: i64,
    kind: MediaKind,
    title: String,
    file_name: String,
    file_path: String,
    file_size: i64,
    duration: Option<i64>,
    year: Option<i32>,
    group_key: Option<String>,
    locked: bool,
}

/// 文件名：库里 `file_name` 为空时从路径末尾兜一个出来。
fn row_name(file_name: &str, file_path: &str) -> String {
    let n = file_name.trim();
    if !n.is_empty() {
        return n.to_string();
    }
    Path::new(file_path)
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default()
}

fn parent_dir_of(file_path: &str) -> String {
    Path::new(file_path)
        .parent()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default()
}

fn dir_name_of(file_path: &str) -> String {
    Path::new(file_path)
        .parent()
        .and_then(|p| p.file_name())
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default()
}

/// `IN (?,?,...)` 占位符；空列表返回 `IN (NULL)`（永远不匹配，但 SQL 合法）。
fn placeholders(n: usize) -> String {
    if n == 0 {
        return "NULL".to_string();
    }
    (1..=n).map(|i| format!("?{i}")).collect::<Vec<_>>().join(",")
}

/// 把一条被误判的单文件行转成「某一部剧的单集」。
///
/// 老数据里大量剧集被判成了电影，修正就是把这条记录**搬进 episodes**，
/// 并挂到一个剧集主记录上（`upsert_series` 会复用已有的同名剧集）。
/// 原来那条记录会被删除——电影的 TMDB 元数据对单集没有意义，留着只会误导。
fn convert_to_episode(
    conn: &Connection,
    row: &FileRow,
    mut parsed: ParsedName,
    ts: i64,
) -> Result<(), String> {
    let name = row_name(&row.file_name, &row.file_path);
    mediatype::fill_episode_numbers(&mut parsed, &name, Some(&dir_name_of(&row.file_path)));
    let parsed_title = if parsed.title.trim().is_empty() {
        row.title.clone()
    } else {
        parsed.title.clone()
    };
    let dir_name = dir_name_of(&row.file_path);
    let known_group = row
        .group_key
        .clone()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    // 分组键：扫描时记过的就照用；否则退回**所在目录名**——同一目录的文件
    // 多半就是同一部剧，比拿「01」这种纯编号当剧名靠谱得多；都没有才用标题。
    let (group_key, series_title) = match (known_group, dir_name.is_empty()) {
        (Some(g), _) => (g, parsed_title.clone()),
        (None, false) => (dir_name.clone(), dir_name),
        (None, true) => (parsed_title.clone(), parsed_title.clone()),
    };

    let (series_id, _) = scanner::upsert_series(conn, &group_key, &series_title, parsed.year, ts)?;
    let f = scanner::ScanFile {
        path: PathBuf::from(&row.file_path),
        name,
        size: row.file_size.max(0) as u64,
        parsed,
        kind: MediaKind::Tv,
        root_index: 0,
        folder: None,
        group_key: Some(group_key),
        resolution: None,
        release_source: None,
    };
    scanner::insert_episode(conn, series_id, &f, ts)?;
    conn.execute("DELETE FROM media WHERE id = ?1", params![row.id])
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// 重新识别库里已有的条目（**向后兼容的关键一步**）。
///
/// 老版本只有一条判定规则，把大量不带季集标记的剧集判成了电影。升级后不用
/// 清空重扫，跑一次这个就能把存量错误分类修正过来：
/// - 电影 / 待确认 → 剧集：搬进 `episodes`，挂到同名剧集主记录下；
/// - 待确认 → 电影：补齐判定后直接改 `type`；
/// - 明确判定 → 待确认：证据不足的不再硬塞进「电影」；
/// - 「整部剧只有一集」且那一条判成电影 → 反方向修正成电影行。
///
/// 用户手动确认过（`locked = 1`）的条目**一律不动**，只统计进 `skipped_locked`。
///
/// `only` 传 id 列表时只重判这些条目（界面上的「重新识别所选」）。
pub fn reclassify_library(
    conn: &mut Connection,
    only: Option<&[i64]>,
) -> Result<ReclassifyResult, String> {
    let cfg = mediatype::config();
    let mut out = ReclassifyResult::default();

    // 1) 取出所有带文件的媒体行
    let rows: Vec<FileRow> = {
        let filter = match only {
            Some(ids) => format!(" AND id IN ({})", placeholders(ids.len())),
            None => String::new(),
        };
        let sql = format!(
            "SELECT id, type, title, COALESCE(file_name,''), file_path, COALESCE(file_size,0), \
                    duration, year, group_key, COALESCE(locked,0) \
             FROM media WHERE file_path IS NOT NULL AND file_path <> ''{filter} ORDER BY id"
        );
        let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
        let mapped = stmt
            .query_map(rusqlite::params_from_iter(only.unwrap_or(&[]).iter()), |r| {
                Ok(FileRow {
                    id: r.get(0)?,
                    kind: MediaKind::from_str(&r.get::<_, String>(1)?),
                    title: r.get(2)?,
                    file_name: r.get(3)?,
                    file_path: r.get(4)?,
                    file_size: r.get(5)?,
                    duration: r.get(6)?,
                    year: r.get(7)?,
                    group_key: r.get(8)?,
                    locked: r.get::<_, i64>(9)? != 0,
                })
            })
            .map_err(|e| e.to_string())?;
        let mut v = Vec::new();
        for r in mapped {
            v.push(r.map_err(|e| e.to_string())?);
        }
        v
    };

    // 2) 单集行（只处理「整部剧只剩一集」的那种——它多半本来就是部电影）
    let eps: Vec<(i64, i64, String, String, i64, i64)> = {
        let filter = match only {
            Some(ids) => format!(" AND media_id IN ({})", placeholders(ids.len())),
            None => String::new(),
        };
        let sql = format!(
            "SELECT id, media_id, COALESCE(file_name,''), file_path, COALESCE(file_size,0), added_at \
             FROM episodes WHERE file_path IS NOT NULL AND file_path <> ''{filter} ORDER BY id"
        );
        let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
        let mapped = stmt
            .query_map(rusqlite::params_from_iter(only.unwrap_or(&[]).iter()), |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, i64>(4)?,
                    r.get::<_, i64>(5)?,
                ))
            })
            .map_err(|e| e.to_string())?;
        let mut v = Vec::new();
        for r in mapped {
            v.push(r.map_err(|e| e.to_string())?);
        }
        v
    };

    // 每个剧集主记录的单集数与锁定状态
    let mut ep_counts: BTreeMap<i64, i64> = BTreeMap::new();
    {
        let mut stmt = conn
            .prepare("SELECT media_id, COUNT(*) FROM episodes GROUP BY media_id")
            .map_err(|e| e.to_string())?;
        let mapped = stmt
            .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))
            .map_err(|e| e.to_string())?;
        for r in mapped {
            let (k, v) = r.map_err(|e| e.to_string())?;
            ep_counts.insert(k, v);
        }
    }
    let mut series_locked: BTreeMap<i64, bool> = BTreeMap::new();
    {
        let mut stmt = conn
            .prepare("SELECT id, COALESCE(locked,0) FROM media WHERE type = 'tv'")
            .map_err(|e| e.to_string())?;
        let mapped = stmt
            .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)? != 0)))
            .map_err(|e| e.to_string())?;
        for r in mapped {
            let (k, v) = r.map_err(|e| e.to_string())?;
            series_locked.insert(k, v);
        }
    }

    // 3) 目录统计（媒体行与单集行可能在同一目录，一起算）
    let mut by_dir: BTreeMap<String, Vec<(String, ParsedName)>> = BTreeMap::new();
    let mut parsed_of_media: BTreeMap<i64, ParsedName> = BTreeMap::new();
    for r in &rows {
        let name = row_name(&r.file_name, &r.file_path);
        let parsed = parse_file_name(&name);
        by_dir
            .entry(parent_dir_of(&r.file_path))
            .or_default()
            .push((name, parsed.clone()));
        parsed_of_media.insert(r.id, parsed);
    }
    let mut parsed_of_ep: BTreeMap<i64, ParsedName> = BTreeMap::new();
    for e in &eps {
        let name = row_name(&e.2, &e.3);
        let parsed = parse_file_name(&name);
        by_dir
            .entry(parent_dir_of(&e.3))
            .or_default()
            .push((name, parsed.clone()));
        parsed_of_ep.insert(e.0, parsed);
    }
    let mut stats: BTreeMap<String, FolderStat> = BTreeMap::new();
    for (dir, list) in &by_dir {
        let entries: Vec<FolderEntry> = list
            .iter()
            .map(|(n, p)| FolderEntry {
                file_name: n,
                parsed: p,
            })
            .collect();
        stats.insert(dir.clone(), mediatype::build_folder_stats(&entries));
    }

    let ts = crate::db::now_epoch();
    let tx = conn.transaction().map_err(|e| e.to_string())?;

    // 4) 媒体行：重新判定 → 修正
    for r in &rows {
        if r.locked {
            out.skipped_locked += 1;
            continue;
        }
        let parsed = parsed_of_media.get(&r.id).cloned().unwrap_or_default();
        let name = row_name(&r.file_name, &r.file_path);
        let dir = parent_dir_of(&r.file_path);
        let dname = dir_name_of(&r.file_path);
        let st = stats.get(&dir).cloned().unwrap_or_default();
        let mut sig = Signals::from_parsed(&parsed, &name);
        sig.duration_secs = r.duration;
        // 库里已存的年份（多半是刮削写的）只当「这是部电影」的旁证，
        // 不参与编号共识的排除——否则一整季都翻不回剧集（见 `stored_year` 注释）。
        sig.stored_year = r.year;
        sig.folder = r.group_key.as_deref().or(Some(dname.as_str()));
        sig.parent = Some(dname.as_str());
        sig.folder_total = st.total;
        sig.folder_tv_count = st.tv_count;
        sig.folder_numbered_count = st.numbered_count;
        sig.folder_consensus_title = st.consensus_title.as_deref();
        sig.folder_number_prefix = st.numbered_prefix.as_deref();
        let kind = mediatype::classify(&sig, &cfg).kind;
        if kind == r.kind {
            continue;
        }
        match kind {
            MediaKind::Tv => {
                if r.kind == MediaKind::Tv {
                    continue;
                }
                convert_to_episode(&tx, r, parsed, ts)?;
                out.to_tv += 1;
            }
            MediaKind::Movie => {
                if r.kind == MediaKind::Tv {
                    continue; // 剧集主记录不往回拆，避免破坏已有单集
                }
                tx.execute(
                    "UPDATE media SET type = 'movie', updated_at = ?2 WHERE id = ?1",
                    params![r.id, ts],
                )
                .map_err(|e| e.to_string())?;
                out.to_movie += 1;
            }
            MediaKind::Unknown => {
                if r.kind == MediaKind::Tv {
                    continue;
                }
                tx.execute(
                    "UPDATE media SET type = 'unknown', updated_at = ?2 WHERE id = ?1",
                    params![r.id, ts],
                )
                .map_err(|e| e.to_string())?;
                out.to_unknown += 1;
            }
        }
    }

    // 5) 单集行：只有一集的「剧集」若判成电影，反方向修正
    for e in &eps {
        if series_locked.get(&e.1).copied().unwrap_or(false) {
            out.skipped_locked += 1;
            continue;
        }
        if ep_counts.get(&e.1).copied().unwrap_or(0) != 1 {
            continue;
        }
        let parsed = parsed_of_ep.get(&e.0).cloned().unwrap_or_default();
        let name = row_name(&e.2, &e.3);
        let dir = parent_dir_of(&e.3);
        let dname = dir_name_of(&e.3);
        let st = stats.get(&dir).cloned().unwrap_or_default();
        let mut sig = Signals::from_parsed(&parsed, &name);
        sig.folder = Some(dname.as_str());
        sig.parent = Some(dname.as_str());
        sig.folder_total = st.total;
        sig.folder_tv_count = st.tv_count;
        sig.folder_numbered_count = st.numbered_count;
        sig.folder_consensus_title = st.consensus_title.as_deref();
        sig.folder_number_prefix = st.numbered_prefix.as_deref();
        if mediatype::classify(&sig, &cfg).kind != MediaKind::Movie {
            continue;
        }
        let title = if parsed.title.is_empty() {
            e.2.clone()
        } else {
            parsed.title.clone()
        };
        tx.execute(
            "INSERT INTO media (type, title, original_title, year, tmdb_id, file_path, file_name, \
                    file_size, added_at, updated_at) \
             VALUES ('movie', ?1, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7) \
             ON CONFLICT(file_path) DO NOTHING",
            params![title, parsed.year, parsed.tmdb_id, e.3, name, e.4, e.5],
        )
        .map_err(|e| e.to_string())?;
        tx.execute("DELETE FROM episodes WHERE id = ?1", params![e.0])
            .map_err(|e| e.to_string())?;
        tx.execute("DELETE FROM media WHERE id = ?1", params![e.1])
            .map_err(|e| e.to_string())?;
        out.to_movie += 1;
    }

    tx.commit().map_err(|e| e.to_string())?;

    out.scanned = (rows.len() + eps.len()) as i64;
    out.changed = out.to_tv + out.to_movie + out.to_unknown;
    let mut parts = vec![format!("已重新识别 {} 条", out.scanned)];
    if out.to_tv > 0 {
        parts.push(format!("{} 条修正为剧集", out.to_tv));
    }
    if out.to_movie > 0 {
        parts.push(format!("{} 条修正为电影", out.to_movie));
    }
    if out.to_unknown > 0 {
        parts.push(format!("{} 条标为待确认", out.to_unknown));
    }
    if out.skipped_locked > 0 {
        parts.push(format!("跳过 {} 条已锁定", out.skipped_locked));
    }
    if out.changed == 0 {
        parts.push("没有发现需要修正的类型".to_string());
    }
    out.message = parts.join("，");
    Ok(out)
}

/// 重新识别媒体类型（存量错误分类的修正入口）。
///
/// 库大时可能有几千条，所以放到后台线程；不传 `ids` 就是全库。
#[tauri::command]
pub async fn reclassify_media(
    app: AppHandle,
    ids: Option<Vec<i64>>,
) -> Result<ReclassifyResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let mut conn = crate::db::open(&app).map_err(|e| format!("打开数据库失败：{e}"))?;
        reclassify_library(&mut conn, ids.as_deref())
    })
    .await
    .map_err(|e| format!("重新识别任务异常：{e}"))?
}

/* --------------------------- 系列电影合集 --------------------------- */

/// 合集里的一部电影（给前端渲染用，字段刻意少，只带展示必需的）。
#[derive(Debug, Clone, Serialize)]
pub struct CollectionMovie {
    pub id: i64,
    pub title: String,
    pub year: Option<i32>,
    pub poster_path: Option<String>,
    pub file_path: Option<String>,
}

/// 一个系列合集：同一系列 / IP 下的电影，按上映时间排序。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MovieCollection {
    /// 聚合键（系列基名归一化结果）。
    pub key: String,
    /// 展示名。
    pub name: String,
    /// 成员（已按上映时间升序，同年按片名）。
    pub movies: Vec<CollectionMovie>,
    /// 被折叠掉的「同片多版本」条数。
    pub duplicate_count: usize,
    pub start_year: Option<i32>,
    pub end_year: Option<i32>,
}

/// 系列电影合集（派生视图，不改数据）。
///
/// 从 `type = 'movie'` 的条目里算出系列基名（见 [`crate::collection`]），
/// 同基名的归到一起，按上映时间排序；同片多版本折叠成一个位次。
#[tauri::command]
pub fn get_movie_collections(
    state: State<'_, Mutex<Connection>>,
) -> Result<Vec<MovieCollection>, String> {
    let conn = state.lock().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare(
            "SELECT m.id, m.title, m.year, m.poster_path, m.file_path, m.file_size, m.tmdb_id \
             FROM media m WHERE m.type = 'movie'",
        )
        .map_err(|e| e.to_string())?;
    // `MovieEntry` 只带判定需要的字段，`file_path` 单独存一张表——它不参与
    // 聚合与去重，混进结构体只会让测试构造变啰嗦。
    let rows = stmt
        .query_map([], |row| {
            Ok((
                collection::MovieEntry {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    year: row.get(2)?,
                    poster_path: row.get(3)?,
                    tmdb_id: row.get(6)?,
                    file_size: row.get(5)?,
                },
                row.get::<_, Option<String>>(4)?,
            ))
        })
        .map_err(|e| e.to_string())?;

    let mut entries = Vec::new();
    let mut paths: BTreeMap<i64, Option<String>> = BTreeMap::new();
    for r in rows {
        let (e, path) = r.map_err(|e| e.to_string())?;
        paths.insert(e.id, path);
        entries.push(e);
    }

    let plan = collection::plan_collections(&entries, &collection::CollectionConfig::default());
    let by_id: BTreeMap<i64, &collection::MovieEntry> =
        entries.iter().map(|e| (e.id, e)).collect();

    Ok(plan
        .collections
        .into_iter()
        .map(|c| {
            let duplicate_count = c.duplicates.len();
            MovieCollection {
                key: c.key,
                name: c.name,
                movies: c
                    .members
                    .iter()
                    .filter_map(|id| {
                        by_id.get(id).map(|e| CollectionMovie {
                            id: e.id,
                            title: e.title.clone(),
                            year: e.year,
                            poster_path: e.poster_path.clone(),
                            file_path: paths.get(&e.id).cloned().flatten(),
                        })
                    })
                    .collect(),
                duplicate_count,
                start_year: c.start_year,
                end_year: c.end_year,
            }
        })
        .collect())
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
        // 走 migrate 而不是裸 SCHEMA：分类 / 标签表和实体扩展字段都要在
        db::migrate(&conn).unwrap();
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
            "SELECT {ITEM_COLUMNS} FROM media m {LIST_WHERE} \
             ORDER BY m.title COLLATE NOCASE ASC, m.year ASC"
        );
        let mut stmt = conn.prepare(&sql).unwrap();
        let rows = stmt
            .query_map(
                params![filter, None::<i64>, None::<i64>, None::<String>, None::<i64>],
                map_item,
            )
            .unwrap();
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

    /// 回归：前端默认给 `unclassified` 传 `false`，它必须是「不过滤」而不是
    /// 「只看已归类」——否则「全部 / 电影 / 剧集」三个栏目会整体空白。
    #[test]
    fn unclassified_false_means_no_filter() {
        assert_eq!(normalize_unclassified(None), None);
        assert_eq!(normalize_unclassified(Some(false)), None);
        assert_eq!(normalize_unclassified(Some(true)), Some(1));

        // 种子数据没有任何分类关联；不过滤时应全部可见。
        let conn = memory_db();
        seed(&conn);
        assert_eq!(query_items(&conn, "all").len(), 2);
    }

    /// 批量删除只应删掉名单里的条目，其他条目一动不动。
    #[test]
    fn delete_media_batch_removes_only_requested_rows() {
        let conn = memory_db();
        seed(&conn);
        let ids: Vec<i64> = conn
            .prepare("SELECT id FROM media ORDER BY id")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert_eq!(ids.len(), 2);

        let n = delete_rows(&conn, "media", &[ids[0]]).unwrap();
        assert_eq!(n, 1);
        let left: i64 = conn
            .query_row("SELECT COUNT(*) FROM media", [], |r| r.get(0))
            .unwrap();
        assert_eq!(left, 1);
        let alive: i64 = conn
            .query_row("SELECT COUNT(*) FROM media WHERE id = ?1", params![ids[1]], |r| r.get(0))
            .unwrap();
        assert_eq!(alive, 1, "没被选中的条目必须还在");

        assert_eq!(delete_rows(&conn, "media", &[]).unwrap(), 0, "空名单是空操作");
        assert_eq!(delete_rows(&conn, "media", &[9999]).unwrap(), 0);
    }

    /// 删单集：只动指定的单集，同剧目其余单集和其他剧目都不受影响。
    #[test]
    fn delete_episodes_touches_only_requested_rows() {
        let conn = memory_db();
        seed(&conn);
        let sid: i64 = conn
            .query_row("SELECT id FROM media WHERE type='tv'", [], |r| r.get(0))
            .unwrap();
        // 再来一部剧，确保删 A 不影响 B
        conn.execute(
            "INSERT INTO media (type,title,added_at,updated_at) VALUES ('tv','Other',1,1)",
            [],
        )
        .unwrap();
        let other = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO episodes (media_id,season,episode,file_path,file_name,added_at) \
             VALUES (?1,1,1,'D:\\\\o.mkv','o.mkv',1)",
            params![other],
        )
        .unwrap();

        let ep: i64 = conn
            .query_row(
                "SELECT id FROM episodes WHERE media_id = ?1 ORDER BY episode LIMIT 1",
                params![sid],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(delete_rows(&conn, "episodes", &[ep]).unwrap(), 1);

        let mine: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM episodes WHERE media_id = ?1",
                params![sid],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(mine, 2, "同一剧目剩下的单集应保留");
        let theirs: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM episodes WHERE media_id = ?1",
                params![other],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(theirs, 1, "其他剧目的资源不能被牵连");
    }

    /// 从文件夹导入：新文件入库、重复文件自动跳过、并挂到指定栏目。
    #[test]
    fn folder_import_dedupes_and_links_category() {
        let dir = std::env::temp_dir().join(format!("mm-import-{}-{}", std::process::id(), now_ns()));
        std::fs::create_dir_all(&dir).unwrap();
        let touch = |rel: &str| {
            let p = dir.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, b"dummy").unwrap();
        };
        touch("Movie.2020.1080p.mkv");
        touch("Show/Show.S01E01.mkv");

        let mut conn = memory_db();
        conn.execute(
            "INSERT INTO categories (name, kind, origin, sort_order, created_at) \
             VALUES ('我的栏目','manual','manual',0,1)",
            [],
        )
        .unwrap();
        let cid: i64 = conn
            .query_row("SELECT id FROM categories LIMIT 1", [], |r| r.get(0))
            .unwrap();

        let root = dir.to_string_lossy().to_string();
        let first = import_folder_sync(&mut conn, &root, Some(cid)).unwrap();
        assert_eq!(first.added, 2, "首次导入应新增两个文件：{first:?}");
        assert_eq!(first.skipped, 0);
        assert_eq!(first.linked, 2, "电影与剧集都应挂到栏目下：{first:?}");
        assert!(first.message.contains("新增 2"), "{}", first.message);

        let second = import_folder_sync(&mut conn, &root, Some(cid)).unwrap();
        assert_eq!(second.added, 0, "重复导入不应重复入库：{second:?}");
        assert_eq!(second.skipped, 2, "两个文件都应被识别为已存在：{second:?}");
        assert!(second.message.contains("跳过 2"), "{}", second.message);

        let media_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM media", [], |r| r.get(0))
            .unwrap();
        assert_eq!(media_count, 2, "重复项不应产生第二条记录");

        assert!(import_folder_sync(&mut conn, "Z:\\not\\here", None).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }

    /**
     * 判定不出来的文件同样要算「导入成功」。
     *
     * 回归：`added` 只累加 `added_movies + added_episodes` 时，导入一个既不像
     * 电影也不像剧集的文件会报「新增 0 个」，而库里其实多了一条待确认记录。
     */
    #[test]
    fn folder_import_counts_unknown_entries() {
        let dir = std::env::temp_dir().join(format!("mm-unk-{}-{}", std::process::id(), now_ns()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("一团乱麻.mkv"), b"dummy").unwrap();

        let mut conn = memory_db();
        let r = import_folder_sync(&mut conn, &dir.to_string_lossy(), None).unwrap();
        assert_eq!(r.added, 1, "待确认的条目也是入库了：{r:?}");
        assert!(r.message.contains("待确认"), "{}", r.message);
        assert_eq!(
            count(&conn, "SELECT COUNT(*) FROM media WHERE type = 'unknown'"),
            1
        );

        // 二次导入同样按重复跳过
        let again = import_folder_sync(&mut conn, &dir.to_string_lossy(), None).unwrap();
        assert_eq!(again.added, 0);
        assert_eq!(again.skipped, 1);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// 目录下没有视频时的反馈要明确，不能静默成功。
    #[test]
    fn folder_import_reports_empty_folder() {
        let dir = std::env::temp_dir().join(format!("mm-empty-{}-{}", std::process::id(), now_ns()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("readme.txt"), b"hi").unwrap();
        let mut conn = memory_db();
        let r = import_folder_sync(&mut conn, &dir.to_string_lossy(), None).unwrap();
        assert_eq!(r.added, 0);
        assert!(r.message.contains("没有找到视频文件"), "{}", r.message);
        std::fs::remove_dir_all(&dir).ok();
    }

    fn now_ns() -> u128 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    }

    #[test]
    fn create_media_supports_both_types_and_validates() {
        let conn = memory_db();
        let mid = insert_manual_media(&conn, "  沙丘  ", "movie", Some(2021), None, None, 1).unwrap();
        let tid = insert_manual_media(&conn, "绝命毒师", "tv", Some(2008), None, None, 1).unwrap();
        assert!(mid > 0 && tid > 0);
        assert_eq!(query_items(&conn, "movie").len(), 1);
        assert_eq!(query_items(&conn, "tv").len(), 1);
        // trim 生效
        let t: String = conn
            .query_row("SELECT title FROM media WHERE id = ?1", params![mid], |r| r.get(0))
            .unwrap();
        assert_eq!(t, "沙丘");
        // 手动建的条目 source 是 manual，且与扫描条目同表
        let src: String = conn
            .query_row("SELECT source FROM media WHERE id = ?1", params![mid], |r| r.get(0))
            .unwrap();
        assert_eq!(src, "manual");

        assert!(insert_manual_media(&conn, "   ", "movie", None, None, None, 1).is_err());
        assert!(insert_manual_media(&conn, "x", "anime", None, None, None, 1).is_err());
    }

    #[test]
    fn update_media_renames_locks_and_mirrors_original_title() {
        let conn = memory_db();
        // 未被刮削改写的条目：original_title == title，改名时跟着走
        let id = insert_manual_media(&conn, "旧名", "movie", Some(2000), None, None, 1).unwrap();
        update_media_fields(
            &conn, id, "新名", "tv", Some(2001),
            Some("剧情".to_string()), Some("简介".to_string()),
        )
        .unwrap();
        let row: (String, String, String, i32, String, i64) = conn
            .query_row(
                "SELECT title, original_title, type, year, genre, locked FROM media WHERE id = ?1",
                params![id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
            )
            .unwrap();
        assert_eq!(row.0, "新名");
        assert_eq!(row.1, "新名", "没被刮削改写的条目 original_title 应跟随改名");
        assert_eq!(row.2, "tv");
        assert_eq!(row.4, "剧情");
        assert_eq!(row.5, 1, "手动编辑后应锁定，防止二次校正改动");

        // 被刮削改写过（original_title != title）的条目：改名不破坏原名
        conn.execute(
            "UPDATE media SET title = 'Scraped', original_title = 'Raw.Name.2020' WHERE id = ?1",
            params![id],
        )
        .unwrap();
        update_media_fields(&conn, id, "用户改的名", "tv", None, None, None).unwrap();
        let orig: String = conn
            .query_row("SELECT original_title FROM media WHERE id = ?1", params![id], |r| r.get(0))
            .unwrap();
        assert_eq!(orig, "Raw.Name.2020", "刮削改写过的 original_title 必须保留");

        assert!(update_media_fields(&conn, id, " ", "tv", None, None, None).is_err());
        assert!(update_media_fields(&conn, id, "x", "anime", None, None, None).is_err());
        assert!(update_media_fields(&conn, 9999, "x", "tv", None, None, None).is_err());
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
    fn scan_control_begin_resets_cancel_flag() {
        let c = ScanControl::default();
        c.request_cancel();
        assert!(c.cancelled());
        c.begin();
        assert!(!c.cancelled(), "新一轮开始时应复位取消标志");
        assert!(c.running());
        c.finish();
        assert!(!c.running());
    }

    /// 遍历阶段的取消标志必须能被后台线程看到（这是「可取消」的前提）。
    #[test]
    fn scan_cancel_is_visible_to_worker_thread() {
        let c = std::sync::Arc::new(ScanControl::default());
        c.begin();
        let worker = c.clone();
        assert!(!worker.cancelled());
        c.request_cancel();
        assert!(worker.cancelled());
    }

    #[test]
    fn scan_progress_tick_reports_phase_and_counts() {
        let p = ScanProgress::tick("遍历目录", 120, 0, 0, "D:\\TV\\Breaking Bad".to_string());
        assert!(p.running && !p.finished);
        assert_eq!(p.phase, "遍历目录");
        assert_eq!(p.found, 120);
        assert_eq!(p.current, "D:\\TV\\Breaking Bad");
        assert!(p.stats.is_none(), "进行中不应该带统计");
    }

    #[test]
    fn scan_progress_settled_carries_stats_and_message() {
        let stats = ScanStats {
            scanned: 10,
            added_movies: 2,
            added_series: 1,
            added_episodes: 7,
            ..Default::default()
        };
        let p = ScanProgress::settled("完成", "扫描完成：发现 10 个视频".to_string(), false, Some(stats));
        assert!(p.finished && !p.running && !p.cancelled);
        assert_eq!(p.found, 10);
        assert_eq!(p.imported, 9, "新增电影 + 新增单集");
        assert!(p.stats.is_some());

        let cancelled = ScanProgress::settled("已取消", "已取消".to_string(), true, None);
        assert!(cancelled.cancelled);
        assert!(cancelled.stats.is_none());
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

    /* ------------------- 存量数据的重新识别（向后兼容） ------------------- */

    fn insert_media(
        conn: &Connection,
        media_type: &str,
        title: &str,
        path: &str,
        extra: &[(&str, &str)],
    ) -> i64 {
        let mut sql = String::from(
            "INSERT INTO media (type, title, file_path, file_name, added_at, updated_at",
        );
        let mut values = vec![
            format!("'{media_type}'"),
            format!("'{title}'"),
            format!("'{path}'"),
            format!("'{}'", file_name_of(path)),
            "1".to_string(),
            "1".to_string(),
        ];
        for (col, val) in extra {
            sql.push_str(&format!(", {col}"));
            values.push(format!("'{val}'"));
        }
        sql.push_str(&format!(") VALUES ({})", values.join(",")));
        conn.execute(&sql, []).unwrap();
        conn.last_insert_rowid()
    }

    fn file_name_of(path: &str) -> String {
        Path::new(path)
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default()
    }

    fn count(conn: &Connection, sql: &str) -> i64 {
        conn.query_row(sql, [], |r| r.get(0)).unwrap()
    }

    /// 老版本把带集号的剧集判成了电影：重跑之后它们应该搬进单集表，
    /// 并挂到**同一个**剧集主记录下（不能一集一部剧）。
    #[test]
    fn reclassify_moves_misclassified_episodes_into_one_series() {
        let mut conn = memory_db();
        for n in ["S01E01", "S01E02"] {
            insert_media(
                &conn,
                "movie",
                "Breaking Bad",
                &format!("D:\\TV\\Breaking Bad\\Breaking.Bad.{n}.mkv"),
                &[],
            );
        }

        let r = reclassify_library(&mut conn, None).unwrap();
        assert_eq!(r.to_tv, 2, "两条都该修正成剧集");
        assert_eq!(r.changed, 2);
        assert_eq!(
            count(&conn, "SELECT COUNT(*) FROM media WHERE type = 'movie'"),
            0,
            "原来的电影行应该被搬走"
        );
        assert_eq!(count(&conn, "SELECT COUNT(*) FROM media WHERE type = 'tv'"), 1);
        assert_eq!(count(&conn, "SELECT COUNT(*) FROM episodes"), 2);

        let mut stmt = conn
            .prepare("SELECT season, episode FROM episodes ORDER BY episode")
            .unwrap();
        let got: Vec<(i32, i32)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert_eq!(got, vec![(1, 1), (1, 2)], "季集号必须按文件名还原");
    }

    /// 目录里全是编号文件（`01.mkv` / `02.mkv`）：靠编号共识认出这是剧集，
    /// 分组键退回目录名——否则会拆成「01 剧」「02 剧」两部。
    #[test]
    fn reclassify_groups_numbered_files_by_folder() {
        let mut conn = memory_db();
        for n in ["01", "02"] {
            insert_media(&conn, "movie", n, &format!("D:\\TV\\大明王朝\\{n}.mkv"), &[]);
        }

        let r = reclassify_library(&mut conn, None).unwrap();
        assert_eq!(r.to_tv, 2);
        assert_eq!(count(&conn, "SELECT COUNT(*) FROM media WHERE type = 'tv'"), 1);

        let title: String = conn
            .query_row("SELECT title FROM media WHERE type = 'tv'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(title, "大明王朝", "剧名应取目录名，而不是「01」");
        let eps: Vec<i32> = conn
            .prepare("SELECT episode FROM episodes ORDER BY episode")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert_eq!(eps, vec![1, 2]);
    }

    /// 证据不足时不能再静默归成电影，要标成「待确认」。
    #[test]
    fn reclassify_marks_undecidable_rows_as_unknown() {
        let mut conn = memory_db();
        insert_media(&conn, "movie", "一团乱麻", "D:\\影视\\一团乱麻.mkv", &[]);

        let r = reclassify_library(&mut conn, None).unwrap();
        assert_eq!(r.to_unknown, 1);
        assert_eq!(
            count(&conn, "SELECT COUNT(*) FROM media WHERE type = 'unknown'"),
            1
        );
        assert!(r.message.contains("待确认"));
    }

    /// 库里已存着年份（刮削写入的）时，条目仍然算「有年份的电影」，
    /// 不会因为有年份就被翻成剧集。
    #[test]
    fn reclassify_keeps_movies_with_stored_year() {
        let mut conn = memory_db();
        insert_media(
            &conn,
            "unknown",
            "沙丘",
            "D:\\影视\\Dune.mkv",
            &[("year", "2021")],
        );

        let r = reclassify_library(&mut conn, None).unwrap();
        assert_eq!(r.to_movie, 1);
        assert_eq!(
            count(&conn, "SELECT COUNT(*) FROM media WHERE type = 'movie'"),
            1
        );
    }

    /// 用户锁定过的条目一律不动——手动整理过的分组不能被自动流程改写。
    #[test]
    fn reclassify_skips_locked_rows() {
        let mut conn = memory_db();
        insert_media(
            &conn,
            "movie",
            "Breaking Bad",
            "D:\\TV\\Breaking Bad\\Breaking.Bad.S01E01.mkv",
            &[("locked", "1")],
        );

        let r = reclassify_library(&mut conn, None).unwrap();
        assert_eq!(r.skipped_locked, 1);
        assert_eq!(r.changed, 0);
        assert_eq!(
            count(&conn, "SELECT COUNT(*) FROM media WHERE type = 'movie'"),
            1,
            "锁定的条目必须原样保留"
        );
    }

    /// 反向修正：整部「剧」只剩一集、而那一集判出来是电影时，搬回电影行。
    #[test]
    fn reclassify_turns_single_episode_series_back_into_movie() {
        let mut conn = memory_db();
        let sid = insert_media(&conn, "tv", "The Matrix", "", &[("year", "1999")]);
        conn.execute(
            "INSERT INTO episodes (media_id, season, episode, file_path, file_name, added_at) \
             VALUES (?1, 1, 1, 'D:\\影视\\The.Matrix.1999.mkv', 'The.Matrix.1999.mkv', 1)",
            params![sid],
        )
        .unwrap();

        let r = reclassify_library(&mut conn, None).unwrap();
        assert_eq!(r.to_movie, 1);
        // 空壳剧集与其下唯一的单集都被清掉，换成一条电影行
        assert_eq!(count(&conn, "SELECT COUNT(*) FROM media WHERE type = 'tv'"), 0);
        assert_eq!(count(&conn, "SELECT COUNT(*) FROM episodes"), 0);
        assert_eq!(
            count(&conn, "SELECT COUNT(*) FROM media WHERE type = 'movie'"),
            1
        );
    }

    /// 只重判指定的 id（界面上的「重新识别所选」）。
    #[test]
    fn reclassify_only_touches_requested_ids() {
        let mut conn = memory_db();
        let keep = insert_media(&conn, "movie", "沙丘", "D:\\影视\\Dune.2021.mkv", &[]);
        let fix = insert_media(
            &conn,
            "movie",
            "Breaking Bad",
            "D:\\TV\\Breaking Bad\\Breaking.Bad.S01E01.mkv",
            &[],
        );

        let r = reclassify_library(&mut conn, Some(&[fix])).unwrap();
        assert_eq!(r.scanned, 1, "只扫传入的这一条");
        assert_eq!(r.to_tv, 1);

        let alive: i64 = conn
            .query_row("SELECT COUNT(*) FROM media WHERE id = ?1", params![keep], |r| r.get(0))
            .unwrap();
        assert_eq!(alive, 1, "没被选中的条目不受影响");
        let moved: i64 = conn
            .query_row("SELECT COUNT(*) FROM media WHERE id = ?1", params![fix], |r| r.get(0))
            .unwrap();
        assert_eq!(moved, 0, "被修正的那条已搬进单集表");
        assert_eq!(count(&conn, "SELECT COUNT(*) FROM episodes"), 1);
    }

    /// 幂等：修完再跑一次不应该再动任何东西（否则重扫会反复搬家）。
    #[test]
    fn reclassify_is_idempotent() {
        let mut conn = memory_db();
        for n in ["S01E01", "S01E02"] {
            insert_media(
                &conn,
                "movie",
                "Breaking Bad",
                &format!("D:\\TV\\Breaking Bad\\Breaking.Bad.{n}.mkv"),
                &[],
            );
        }
        reclassify_library(&mut conn, None).unwrap();
        let again = reclassify_library(&mut conn, None).unwrap();
        assert_eq!(again.changed, 0, "第二次不应该再有变化");
        assert_eq!(count(&conn, "SELECT COUNT(*) FROM episodes"), 2);
    }

    /* ------------------------- 系列电影合集 ------------------------- */

    fn plan_from_rows(conn: &Connection) -> Vec<MovieCollection> {
        let mut stmt = conn
            .prepare("SELECT m.id, m.title, m.year, m.poster_path, m.file_path, m.file_size, m.tmdb_id \
                      FROM media m WHERE m.type = 'movie'")
            .unwrap();
        let rows = stmt
            .query_map([], |row| {
                Ok(collection::MovieEntry {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    year: row.get(2)?,
                    poster_path: row.get(3)?,
                    tmdb_id: row.get(6)?,
                    file_size: row.get(5)?,
                })
            })
            .unwrap();
        let mut entries = Vec::new();
        for r in rows {
            entries.push(r.unwrap());
        }
        let plan = collection::plan_collections(&entries, &collection::CollectionConfig::default());
        let by_id: BTreeMap<i64, &collection::MovieEntry> =
            entries.iter().map(|e| (e.id, e)).collect();
        plan.collections
            .into_iter()
            .map(|c| {
                let dup = c.duplicates.len();
                MovieCollection {
                    key: c.key,
                    name: c.name,
                    movies: c
                        .members
                        .iter()
                        .filter_map(|id| {
                            by_id.get(id).map(|e| CollectionMovie {
                                id: e.id,
                                title: e.title.clone(),
                                year: e.year,
                                poster_path: e.poster_path.clone(),
                                file_path: None,
                            })
                        })
                        .collect(),
                    duplicate_count: dup,
                    start_year: c.start_year,
                    end_year: c.end_year,
                }
            })
            .collect()
    }

    /// 同一个系列（含续作标识）的电影归到一起，且按上映时间排序。
    #[test]
    fn movie_collections_group_a_franchise_by_release_year() {
        let conn = memory_db();
        insert_media(&conn, "movie", "沙丘 2", "D:\\影视\\Dune2.mkv", &[("year", "2024")]);
        insert_media(&conn, "movie", "沙丘", "D:\\影视\\Dune.mkv", &[("year", "2021")]);

        let got = plan_from_rows(&conn);
        assert_eq!(got.len(), 1, "两部沙丘应聚成一个合集");
        assert_eq!(got[0].movies.iter().map(|m| m.year).collect::<Vec<_>>(), vec![Some(2021), Some(2024)]);
        assert_eq!(got[0].start_year, Some(2021));
        assert_eq!(got[0].end_year, Some(2024));
    }

    /// 毫不相干的电影不能被并进同一个合集。
    #[test]
    fn movie_collections_keep_unrelated_movies_apart() {
        let conn = memory_db();
        insert_media(&conn, "movie", "沙丘", "D:\\影视\\Dune.mkv", &[("year", "2021")]);
        insert_media(&conn, "movie", "沙丘 2", "D:\\影视\\Dune2.mkv", &[("year", "2024")]);
        insert_media(&conn, "movie", "1917", "D:\\影视\\1917.mkv", &[("year", "2019")]);

        let got = plan_from_rows(&conn);
        assert_eq!(got.len(), 1, "1917 不该自成合集，也不该并进沙丘");
        assert_eq!(got[0].movies.len(), 2);
    }
}
