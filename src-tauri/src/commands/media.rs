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

use crate::commands::settings::load_media_dirs;
use super::control::{ScanControl, ScrapeControl};
use crate::commands::tree::load_edges;
use crate::core::mediatype::{self, FolderEntry, FolderStat, MediaKind, Signals, SourceHint};
use crate::core::parser::{parse_file_name, ParsedName};
use crate::core::scanner::{self, ScanStats};
use crate::core::tree;
use crate::commands::job::{self, JobProgress};
use crate::store::search;
use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, State};

/// 扫描进度事件名（前端 `listen("scan-progress", ...)`）。
pub const PROGRESS_EVENT: &str = "scan-progress";

/* -------------------- 大模型增强层的注入桥接 -------------------- */

/// 把「设置里的大模型配置」包装成 `core::scanner::NameEnhancer` 交给扫描层。
///
/// 为什么需要这一层：`core` 是纯领域层，既不该出现 `crate::provider`
/// （不知道元数据从哪来），也不该出现 `crate::commands::settings`（不知道配置
/// 存在哪）。以前 `core/scanner.rs` 直接调这两个模块，把分层破了个洞。
/// 现在改为由 `commands` 造好增强器注入进去——依赖方向重新变成单向。
pub struct LlmEnhancer {
    cfg: crate::provider::llm::LlmConfig,
    hosts: Vec<String>,
}

impl LlmEnhancer {
    /// 从设置表读配置。配置不可用时返回 `None`，扫描层会跳过增强（行为不变）。
    pub fn load_enabled(conn: &Connection) -> Option<Self> {
        let cfg = crate::commands::settings::load_llm_config(conn);
        if !cfg.usable() {
            return None;
        }
        let hosts = crate::commands::settings::load_hosts(conn);
        Some(Self { cfg, hosts })
    }
}

impl scanner::NameEnhancer for LlmEnhancer {
    fn needs(&self, parsed: &ParsedName, file_name: &str) -> bool {
        crate::provider::llm::needs_enhancement(parsed, file_name)
    }

    fn enhance(
        &self,
        parsed: ParsedName,
        file_name: &str,
    ) -> (ParsedName, Option<scanner::EnhancedFields>) {
        let (p, fields) =
            crate::provider::llm::enhance(parsed, file_name, &self.cfg, &self.hosts);
        let mapped = fields.map(|x| scanner::EnhancedFields {
            resolution: x.resolution,
            release_source: x.release_source,
        });
        (p, mapped)
    }
}

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
    /// 原始文件名派生的标题（刮削前的名字），检索要用它匹配英文原名等写法。
    pub original_title: Option<String>,
    /// 分组键（通常是所在目录名），检索要能按目录名命中。
    pub group_key: Option<String>,
    /// 父节点 id（`NULL` = 顶层）。封面墙的「拖拽聚合分组」要看到它，
    /// 否则一行数据摆在墙上根本分不出谁是父、谁是子。
    pub parent_id: Option<i64>,
    /// 直接子级条数。封面墙靠它决定「这张卡要不要给展开 / 折叠控件」。
    pub child_count: i64,
    /// 同一父节点内的次序（数据库列，取不到时按 0）。
    ///
    /// ⚠️ 它和列表**先后**不是一回事：列表先后是用户选的**展示排序**
    /// （默认按拼音名），`sort_order` 才是同级内用户手工调出来的次序。
    /// 详情页「聚合成员」的 ↑ / ↓ 必须拿它算下标，否则按名字排完再按下标
    /// 移动，后端算出的新序与旧序完全相同 → 点了看不出任何变化。
    pub sort_order: i64,
    /// 该条目在**全表**层级里的深度（顶层 = 0）。
    ///
    /// 值由 `attach_depth()` 用全表边表 + `core::tree::depth_of` 算出来
    /// （见 `ITEM_COLUMNS` 里那一列的说明）。封面墙的拖拽放行判定必须用它：
    /// 前端只看得见当前筛选到的条目，父节点被筛掉时它自己数出来的深度会
    /// **偏小**，于是"界面画可放置、请求也发出去了，后端回层级超限"。
    pub depth: i64,
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

/// 列表项 / 详情主记录的公共列。
///
/// ⚠️ 列序与 `map_item` 的 `row.get(N)` 下标**一一对应**：新列一律追加在末尾，
/// 或同时改两边——只在中间插一列会让后面所有字段整体错位（`child_count`
/// 会去读 `sort_order`），而且是静默错位：类型恰好都能转就不报错。
///
/// ⚠️ 末尾那一列 `depth` 是**占位**（恒 0）：层级要沿父链走到**全表的**根，
/// SQL 侧只能上递归 CTE，那等于把 `core::tree::depth_of` 的规则再写一遍，
/// 两套迟早对不上。真实值由 `attach_depth()` 用全表边表算出来覆盖，
/// 所以两个取列表的入口（`get_media_list` / `get_media_detail`）都必须过它。
pub(crate) const ITEM_COLUMNS: &str = "m.id, m.type, m.title, m.year, m.tmdb_id, m.poster_path, m.backdrop_path, \
     m.overview, m.vote_average, m.genre, m.file_path, m.file_name, m.added_at, \
     COALESCE(m.source, 'scan') AS source, COALESCE(m.locked, 0) AS locked, \
     (SELECT COUNT(*) FROM episodes e WHERE e.media_id = m.id) AS episode_count, \
     CASE WHEN m.group_key IS NOT NULL \
            AND (SELECT COUNT(*) FROM media d WHERE d.type = 'tv' AND d.type = m.type \
                 AND d.title = m.title) > 1 \
          THEN m.title || ' · ' || m.group_key ELSE m.title END AS display_title, \
     m.original_title, m.group_key, \
     m.parent_id, \
     (SELECT COUNT(*) FROM media c WHERE c.parent_id = m.id) AS child_count, \
     COALESCE(m.sort_order, 0) AS sort_order, \
     0 AS depth";

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
        original_title: row.get(17)?,
        group_key: row.get(18)?,
        parent_id: row.get(19)?,
        child_count: row.get(20)?,
        // 与 `ITEM_COLUMNS` 的列序一一对应：新列只能往后追加，
        // 中间插一列就会把后面所有下标顶偏（`child_count` 会去读 sort_order）。
        sort_order: row.get(21)?,
        // SQL 里那列恒 0（占位），真实层级由 `attach_depth` 覆盖，见 `ITEM_COLUMNS`
        depth: row.get(22)?,
    })
}

/// 给一批列表项补上**真实层级**（顶层 = 0）。
///
/// 数据来源是**全表** `(id, parent_id)` 边表 + `core::tree::depth_of`，与层级
/// 视图、放置校验（`commands/tree.rs`）用的是**同一套规则**——所以这里绝不能
/// 换成 SQL 递归 CTE，那是第二份实现。
///
/// 为什么必须由后端给：前端只拿得到**当前筛选**的条目，父节点被筛掉时它自己
/// 沿 `props.items` 数深度会提前停在"根"，算出的层级偏小 → `canDrop` 放行
/// 后端必拒的放置。`depth_of` 对成环老数据是防环的（就地停下），取不到才兜 0。
pub(crate) fn attach_depth(conn: &Connection, items: &mut [MediaListItem]) -> Result<(), String> {
    if items.is_empty() {
        return Ok(());
    }
    let edges = load_edges(conn)?;
    for it in items.iter_mut() {
        it.depth = tree::depth_of(&edges, it.id).unwrap_or(0) as i64;
    }
    Ok(())
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
                .map(|s| {
                    (s.added_movies + s.added_episodes + s.added_unknown + s.added_versions) as i64
                })
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

/// 扫描媒体库。`roots` 为空时回落到设置里的 `mediaDirs`。
///
/// 命令**立即返回**，实际工作交给后台线程；进度通过 `scan-progress` 事件推送。
#[tauri::command]
pub async fn scan_library(
    app: AppHandle,
    state: State<'_, Mutex<Connection>>,
    control: State<'_, Arc<ScanControl>>,
    scrape: State<'_, Arc<ScrapeControl>>,
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
    let scrape = scrape.inner().clone();
    std::thread::spawn(move || {
        run_scan(app, control, scrape, dirs);
    });

    Ok(ScanStart {
        started: true,
        message: format!("已开始扫描 {} 个目录", root_count),
    })
}

/// 后台扫描：先遍历目录（可取消），再入库（不可取消）。
///
/// 入库完成后，如果用户在设置里开了「扫描后自动刮削」，会顺手把新条目
/// 交给刮削链路（见 `scrape::maybe_auto_scrape`）；开关默认关闭。
fn run_scan(app: AppHandle, control: Arc<ScanControl>, scrape: Arc<ScrapeControl>, roots: Vec<String>) {
    let emit = |p: ScanProgress| {
        // 统一任务事件与旧的 scan-progress **并行**发出（B8：旧事件不删不改）。
        // 映射是忠实的：结束态从 ScanProgress 自己的 finished/cancelled/phase 推出来
        // （`phase == "出错"` 是既有的错误阶段名），不引入新的状态判断。
        let job = if p.finished {
            if p.cancelled {
                JobProgress::cancelled(job::KIND_SCAN, &p.message)
            } else if p.phase == "出错" {
                JobProgress::failed(job::KIND_SCAN, &p.message)
            } else {
                JobProgress::done(job::KIND_SCAN, &p.message)
            }
        } else {
            JobProgress::tick(job::KIND_SCAN, &p.phase, p.imported, p.total, &p.current)
        };
        let _ = app.emit(PROGRESS_EVENT, p);
        job::emit(&app, &job);
    };

    // 后台线程用自己的连接：WAL 模式下多连接并发读写是安全的，
    // 而且扫描期间不该占着全局连接锁，否则界面上的列表查询会一起卡住。
    let mut conn = match crate::store::db::open(&app) {
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
    // 增强层必须**在开事务之前**把配置读出来并注入（网络请求不能拿着写锁等）。
    let enhancer = LlmEnhancer::load_enabled(&conn);
    match scanner::import_reported(
        &mut conn,
        &files,
        &roots,
        enhancer.as_ref().map(|e| e as &dyn scanner::NameEnhancer),
        &mut tick,
    ) {
        Ok(stats) => {
            let mut msg = format!(
                "扫描完成：发现 {} 个视频（新增电影 {}、剧集 {}、单集 {}）",
                stats.scanned, stats.added_movies, stats.added_series, stats.added_episodes
            );
            // 存量修复的解释要带上，否则用户只看到"跳过 N 个已存在"，以为什么都没做
            if stats.repointed > 0 {
                msg.push_str(&format!("，{} 集改挂到已存在的剧集", stats.repointed));
            }
            if stats.dropped_shells > 0 {
                msg.push_str(&format!("，清理 {} 个空剧集条目", stats.dropped_shells));
            }
            if stats.added_versions > 0 {
                msg.push_str(&format!("，{} 个同片文件并入同一条目", stats.added_versions));
            }
            if stats.merged_entries > 0 {
                msg.push_str(&format!("，合并 {} 个重复条目", stats.merged_entries));
            }
            emit(ScanProgress::settled("完成", msg, false, Some(stats)))
        }
        Err(e) => emit(ScanProgress::settled("出错", format!("扫描失败：{e}"), false, None)),
    }
    control.finish();
    // 扫描收尾后接自动刮削（开关关闭 / 没配源 / 已有任务时都是空操作）
    crate::commands::scrape::maybe_auto_scrape(&app, &scrape);
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
    /// 从别的剧集改挂到已存在剧集下的单集数（老数据自愈）。
    pub repointed: i64,
    /// 清理掉的空剧集条目数。
    pub dropped_shells: i64,
    /// 散落的独立条目（电影 / 待确认）被收编成单集的条数。
    pub adopted: i64,
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
            repointed: 0,
            dropped_shells: 0,
            adopted: 0,
            message: "该目录下没有找到视频文件".to_string(),
        });
    }

    let enhancer = LlmEnhancer::load_enabled(conn);
    let stats = scanner::import_reported(
        conn,
        &files,
        &roots,
        enhancer.as_ref().map(|e| e as &dyn scanner::NameEnhancer),
        &mut |_, _, _| true,
    )?;
    let linked = match category_id {
        Some(cid) => link_imported_to_category(conn, &files, cid)?,
        None => 0,
    };
    // 「待确认」也是入库了的一条，必须算进 `added`——否则导入一个判定不出来的
    // 文件时界面会报「新增 0 个」，而库里其实多了一条。
    // 同目录同片归并出来的**版本**同样算：它们进了库，只是挂在已有条目下。
    let added =
        (stats.added_movies + stats.added_episodes + stats.added_unknown + stats.added_versions)
            as i64;
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
    // 存量修复要**明说**：用户看到的是"跳过 25 个已存在"，如果不解释，
    // 他会以为什么都没发生，而实际上单集刚刚被归到了正确的剧集下。
    if stats.repointed > 0 {
        parts.push(format!("{} 集改挂到已存在的剧集", stats.repointed));
    }
    if stats.dropped_shells > 0 {
        parts.push(format!("清理 {} 个空剧集条目", stats.dropped_shells));
    }
    // 归并必须**说清楚**：用户看到的是「新增 2 个」，如果不解释，他会以为
    // 另外两个文件丢了，而实际上它们已经并进同一条条目里当版本了。
    if stats.added_versions > 0 {
        parts.push(format!("{} 个同片文件并入同一条目的版本", stats.added_versions));
    }
    if stats.merged_entries > 0 {
        parts.push(format!("合并 {} 个重复的电影条目", stats.merged_entries));
    }
    if linked > 0 {
        parts.push(format!("归入栏目 {linked} 条"));
    }
    Ok(ImportFolderResult {
        added,
        skipped,
        failed,
        linked,
        repointed: stats.repointed as i64,
        dropped_shells: stats.dropped_shells as i64,
        adopted: 0,
        message: parts.join("，"),
    })
}

/// 把一个文件夹里的视频**指定**归到某条剧集下，不靠文件名 / 目录名推断。
///
/// 存在的理由：`86版西游记.01.mkv`、`XYJ.01.mkv` 这类名字里既没有剧名、
/// 所在目录名也不是剧名时，系统**无从推断归属**，导入后只会另建一条剧集，
/// 手建的那条永远接不到内容。推断做不到的，就让用户显式指定 ——
/// 他正站在这部剧的详情页上，说"这批文件是它的"，这个信息是确定的。
///
/// 做法分两步，刻意复用已有能力而不是改扫描器的分组逻辑：
/// 1. 先照常 `import_folder_sync`（该建什么建什么），这样新文件能正常入库、
///    已有的文件不会因为 `ON CONFLICT` 冲突而报错；
/// 2. 再把这批文件路径对应的单集**改挂**到目标剧集，并清掉被腾空的空壳。
pub fn import_folder_into_sync(
    conn: &mut Connection,
    media_id: i64,
    path: &str,
    category_id: Option<i64>,
) -> Result<ImportFolderResult, String> {
    let (mtype, title): (String, String) = conn
        .query_row(
            "SELECT type, title FROM media WHERE id = ?1",
            params![media_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(|_| format!("找不到 id 为 {media_id} 的条目"))?;
    if mtype != "tv" {
        return Err(format!("「{title}」不是剧集，只有剧集能添加单集"));
    }
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
            repointed: 0,
            dropped_shells: 0,
            adopted: 0,
            message: "该目录下没有找到视频文件".to_string(),
        });
    }

    let mut res = import_folder_sync(conn, path, category_id)?;

    // 记下这批单集**原来**挂在哪些剧集下，腾空后好清壳（清壳条件很严，宁可不删）
    let mut old_groups: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for f in &files {
        let p = f.path.to_string_lossy().to_string();
        if let Ok(Some(g)) = conn.query_row(
            "SELECT m.group_key FROM episodes e JOIN media m ON m.id = e.media_id \
             WHERE e.file_path = ?1 AND e.media_id <> ?2",
            params![p, media_id],
            |r| r.get::<_, Option<String>>(0),
        ) {
            old_groups.insert(g);
        }
    }

    let mut moved = 0usize;
    let mut adopted = 0usize;
    for f in &files {
        let p = f.path.to_string_lossy().to_string();
        if scanner::repoint_episode(conn, media_id, &p).unwrap_or(false) {
            moved += 1;
        } else if scanner::adopt_media_as_episode(conn, media_id, &p).unwrap_or(false) {
            // 没有 episodes 行可改挂（文件名没解析出集号 → 判成电影 / 待确认）；
            // 用户已经明确指定归属，直接把那条独立条目收编成单集。
            adopted += 1;
        }
    }

    let mut shells = 0usize;
    for g in &old_groups {
        shells += scanner::drop_empty_series_shells(conn, g).unwrap_or(0);
    }

    // 没归进去的要说清楚：静默吞掉会让用户以为加进去了。
    let total = moved + adopted;
    if total > 0 {
        res.message = format!("{}，{} 集归入「{}」", res.message, total, title);
    } else {
        res.message = format!(
            "{}，但没有可归入「{}」的单集（目录下没有视频文件，或它们已经属于别的剧集）",
            res.message, title
        );
    }
    res.repointed += moved as i64;
    res.adopted += adopted as i64;
    res.dropped_shells += shells as i64;
    if shells > 0 {
        res.message = format!("{}，清理 {} 个空剧集条目", res.message, shells);
    }
    Ok(res)
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
    let ts = crate::store::db::now_epoch();
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
///
/// 入库完成后，如果开了「扫描 / 导入后自动刮削」，新条目会自动交给刮削链路。
#[tauri::command]
pub async fn import_folder(
    app: AppHandle,
    scrape: State<'_, Arc<ScrapeControl>>,
    path: String,
    category_id: Option<i64>,
) -> Result<ImportFolderResult, String> {
    let path = path.trim().to_string();
    if path.is_empty() {
        return Err("请先选择要导入的文件夹".to_string());
    }
    let app_for_import = app.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        let mut conn =
            crate::store::db::open(&app_for_import).map_err(|e| format!("打开数据库失败：{e}"))?;
        // 连接在闭包里 drop，自动刮削那边再自己开一个（WAL 下多连接是安全的）
        import_folder_sync(&mut conn, &path, category_id)
    })
    .await
    .map_err(|e| format!("导入任务异常：{e}"))??;

    // 有新增、或发生了「改挂 / 清壳」才值得触发；开关关闭 / 已有任务时是空操作。
    // 只看 added 会漏掉自愈场景：重复导入时 added=0，但单集刚被改挂到手建剧集下，
    // 而那条手建剧集往往正是一条还没刮过、最需要刮削的条目。
    if result.added > 0 || result.repointed > 0 || result.dropped_shells > 0 {
        crate::commands::scrape::maybe_auto_scrape(&app, scrape.inner());
    }
    Ok(result)
}

/// 把一个文件夹的视频**指定**归到某条剧集下（界面上的「添加文件到这部剧」）。
///
/// 与 `import_folder` 的区别：不靠文件名推断归属，而是以用户在详情页上
/// 指定的那条剧集为准。用于 `86版西游记.01.mkv` / `XYJ.01.mkv` 这类
/// 「文件名与目录名都不含剧名」、推断必然失败的情况。
#[tauri::command]
pub async fn import_folder_into(
    app: AppHandle,
    scrape: State<'_, Arc<ScrapeControl>>,
    media_id: i64,
    path: String,
    category_id: Option<i64>,
) -> Result<ImportFolderResult, String> {
    let path = path.trim().to_string();
    if path.is_empty() {
        return Err("请先选择要添加的文件夹".to_string());
    }
    let app_for_import = app.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        let mut conn =
            crate::store::db::open(&app_for_import).map_err(|e| format!("打开数据库失败：{e}"))?;
        import_folder_into_sync(&mut conn, media_id, &path, category_id)
    })
    .await
    .map_err(|e| format!("导入任务异常：{e}"))??;

    if result.added > 0 || result.repointed > 0 || result.dropped_shells > 0 {
        crate::commands::scrape::maybe_auto_scrape(&app, scrape.inner());
    }
    Ok(result)
}

/// 列表筛选条件（全部可选，不传就是「不过滤」）。
///
/// 关键词用 `LIKE ... ESCAPE '\'`：用户输入里的 `%` / `_` / `\` 已经被
/// `escape_like` 按字面转义，否则 `100%`、`A_B` 这类真实存在的词会被
/// 当成通配符——检索结果会多匹配（`100%` 连带命中 `1000部`）、不完整。
/// 关键词子句的占位符：运行时由 `list_where()` 按词数展开。
///
/// 为什么要用占位符而不是写死一条 LIKE：检索升级后要支持「多词 AND」
/// （`卧虎 2000` 命中《卧虎藏龙》(2000)），词数不定，SQL 参数个数也就不定。
const KW_SLOT: &str = "/*__KEYWORD__*/";

const LIST_WHERE: &str = "
         WHERE COALESCE(?1, 'all') IN ('all', m.type)
           AND (?2 IS NULL OR EXISTS (SELECT 1 FROM media_categories mc
                                      WHERE mc.media_id = m.id AND mc.category_id = ?2))
           AND (?3 IS NULL OR EXISTS (SELECT 1 FROM media_tags mt
                                      WHERE mt.media_id = m.id AND mt.tag_id = ?3))
           AND (?4 IS NULL OR ?4 = ''
                OR (/*__KEYWORD__*/))
           AND (?5 IS NULL
                OR (CASE WHEN EXISTS (SELECT 1 FROM media_categories mc2
                                      WHERE mc2.media_id = m.id) THEN 0 ELSE 1 END) = ?5)";

/// 关键词子句：LIKE（正确性保证）∪ FTS 候选（加速）。
///
/// 取**并集**而不是直接换成 FTS：FTS 不可用时 `in_clause` 退化成 `0`，
/// 整个条件与改动前逐字等价——这就是「一键回落」。
pub fn list_where_with_fts(term_count: usize, fts_ids: Option<&[i64]>) -> String {
    let like_part = search::keyword_clause(term_count);
    let clause = match fts_ids {
        Some(ids) if !ids.is_empty() => {
            format!("{like_part} OR ({})", search::in_clause(ids))
        }
        _ => like_part,
    };
    LIST_WHERE.replace(KW_SLOT, &clause)
}

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
    let raw_kw = keyword.unwrap_or_default().trim().to_string();

    // 检索升级：按空白拆词，词与词 AND、字段之间 OR。
    // 每个词项都过 `escape_like`——元字符按字面处理，否则 `100%` 会命中 `1000部`。
    let terms: Vec<String> = search::split_terms(&raw_kw)
        .iter()
        .map(|t| search::escape_like(t))
        .collect();
    // `?4` 仍是「有没有关键词」的门闩（空串 / None 都放行），真正的匹配参数从 ?6 起。
    let kw_gate: Option<String> = if raw_kw.is_empty() { None } else { Some(raw_kw.clone()) };

    // 关键：`None` 与 `Some(false)` 都必须是「不过滤」。
    // 前端默认会传 `unclassified: false`（而不是不传）；若把它当成 0 传进 SQL，
    // 条件 `(CASE WHEN 有分类 THEN 0 ELSE 1 END) = ?5` 就只剩「已归类」的条目——
    // 历史数据没有分类关联时，「全部 / 电影 / 剧集」三个栏目会整体空白。
    let unclassified = normalize_unclassified(unclassified);

    // FTS 加速：取不到候选（不可用 / 词项不适合）就回落纯 LIKE。
    let fts_ids = if terms.is_empty() {
        None
    } else {
        search::matching_ids(&conn, &raw_kw)
    };
    let sql = format!(
        "SELECT {ITEM_COLUMNS} FROM media m {LIST_WHERE} \
         ORDER BY m.title COLLATE NOCASE ASC, m.year ASC"
    )
    .replace(LIST_WHERE, &list_where_with_fts(terms.len(), fts_ids.as_deref()));

    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let mut bound: Vec<Box<dyn rusqlite::ToSql>> = vec![
        Box::new(filter),
        Box::new(category_id),
        Box::new(tag_id),
        Box::new(kw_gate),
        Box::new(unclassified),
    ];
    for t in &terms {
        bound.push(Box::new(t.clone()));
    }
    let rows = stmt
        .query_map(rusqlite::params_from_iter(bound.iter().map(|b| b.as_ref())), map_item)
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| e.to_string())?);
    }
    // 层级必须由后端给（前端只看得见当前筛选到的条目，父链可能不在里面）
    attach_depth(&conn, &mut out)?;
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
/// `parent_id` 用于层级视图：传了就建在指定剧集内部，不传建在顶层。
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
    parent_id: Option<i64>,
) -> Result<MediaListItem, String> {
    let conn = state.lock().map_err(|e| e.to_string())?;
    let ts = crate::store::db::now_epoch();
    if let Some(p) = parent_id {
        crate::commands::tree::check_child_allowed(&conn, p)?;
    }
    let id = insert_manual_media_at(
        &conn, &title, &media_type, year, genre, overview, parent_id, ts,
    )?;
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
/// 流派 / 简介留空按 NULL 存。`source = 'manual'` ——
/// 重扫不会把它并掉，也不会给它凭空造单集。
///
/// **剧集要写 `group_key`**：`group_key` 是扫描器认剧集的主键，同一部剧的所有
/// 目录都靠它归拢。手建行如果留空，就只能靠「标题逐字相等」这条兜底规则被认出来，
/// 而扫描推出来的标题经常与用户手输的名字不完全一致（`西游记.1986.01.mkv` →
/// `西游记 01`）—— 那样内容会另建一条剧集，手建的条目**永远**是空的。
/// 只给测试用：生产路径一律走 `insert_manual_media_at`（要能指定父节点）。
#[cfg(test)]
fn insert_manual_media(
    conn: &Connection,
    title: &str,
    media_type: &str,
    year: Option<i32>,
    genre: Option<String>,
    overview: Option<String>,
    ts: i64,
) -> Result<i64, String> {
    insert_manual_media_at(conn, title, media_type, year, genre, overview, None, ts)
}

/// 同 `insert_manual_media`，但能指定父节点（层级视图用）。
///
/// 排序号取「同级最大 + 1」，新建的节点一律排在末尾——用户的直觉就是「加在最后」，
/// 需要插到中间时用拖拽。
pub(crate) fn insert_manual_media_at(
    conn: &Connection,
    title: &str,
    media_type: &str,
    year: Option<i32>,
    genre: Option<String>,
    overview: Option<String>,
    parent_id: Option<i64>,
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
    // 电影没有分组概念，留空；剧集用**原样的标题**当分组键——
    // 扫描器的 `group_key` 是目录名原文，保持一致才能被「精确命中」那条规则直接找到。
    let group_key: Option<&str> = if media_type == "tv" { Some(title) } else { None };
    let sort_order: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(sort_order), -1) + 1 FROM media \
              WHERE parent_id IS ?1",
            params![parent_id],
            |r| r.get(0),
        )
        .unwrap_or(0);
    conn.execute(
        "INSERT INTO media (type, title, original_title, group_key, year, genre, overview, source, \
                            parent_id, sort_order, added_at, updated_at) \
         VALUES (?1, ?2, ?2, ?3, ?4, ?5, ?6, 'manual', ?7, ?8, ?9, ?9)",
        params![
            media_type, title, group_key, year, genre, overview, parent_id, sort_order, ts
        ],
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
    let ts = crate::store::db::now_epoch();
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
    let mut media = conn
        .query_row(&sql, params![id], map_item)
        .map_err(|e| format!("未找到媒体 id={id}: {e}"))?;
    // 详情单条同样要真实层级：封面墙拖拽命中父卡片时会拿它做放行判定
    attach_depth(&conn, std::slice::from_mut(&mut media))?;
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

/// 按 id 批量删除某一类记录（单事务），返回实际删除的条数。
///
/// `table` 由调用方以字面量传入（不是用户输入），所以这里拼 SQL 是安全的；
/// id 则逐个绑定占位符，绝不拼接。
///
/// 入参先 `normalize_ids`：重复 id 与 `id <= 0` 都被丢掉，
/// 因此返回值就是「真实删掉了几行」，不会比用户勾选的数量虚高。
fn delete_rows(conn: &Connection, table: &str, ids: &[i64]) -> Result<i64, String> {
    let ids = crate::commands::tree::normalize_ids(ids);
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

/// 删除一条媒体记录（剧集单集由外键级联删除）。
///
/// 删不到（id 不存在、或已经被删过）时**明确报错**，而不是静默返回成功——
/// 「点了删除、界面说删好了、东西却还在」比一句报错更让人困惑。
/// 重复删除同一个 id 属于这种情形，所以这里天然是幂等的：第二次必然报错，
/// 不会把别的行当成它再删一次。
///
/// 走 `spawn_blocking` + 自己的连接：删除是写库 IO，同步命令会把主线程（界面）锁住。
#[tauri::command]
pub async fn delete_media(app: AppHandle, id: i64) -> Result<(), String> {
    let result = tauri::async_runtime::spawn_blocking(move || {
        let mut conn =
            crate::store::db::open(&app).map_err(|e| format!("打开数据库失败：{e}"))?;
        let n = crate::commands::tree::delete_nodes_sync(&mut conn, &[id])?;
        if n == 0 {
            return Err(format!("找不到 id 为 {id} 的资源，可能已经被删除了"));
        }
        Ok(())
    })
    .await
    .map_err(|e| format!("删除任务异常：{e}"))??;
    Ok(result)
}

/// 批量删除媒体（界面上的「多选 → 删除所选」）。
///
/// 只删 `ids` 里的条目——不在列表里的一律不动，这是批量操作的底线。
/// 整个批次在**一个事务**里完成：中途出错整体回滚，不会出现「删了一半」。
/// 返回实际删除条数，供界面提示「已删除 N 条」；条数少于请求数说明
/// 有些 id 已经不在库里（重复删除 / 别处刚删过），这不是失败，界面应当说清楚。
#[tauri::command]
pub async fn delete_media_batch(app: AppHandle, ids: Vec<i64>) -> Result<i64, String> {
    if ids.is_empty() {
        return Ok(0);
    }
    let result = tauri::async_runtime::spawn_blocking(move || {
        let mut conn =
            crate::store::db::open(&app).map_err(|e| format!("打开数据库失败：{e}"))?;
        crate::commands::tree::delete_nodes_sync(&mut conn, &ids)
    })
    .await
    .map_err(|e| format!("批量删除任务异常：{e}"))??;
    Ok(result)
}

/// 批量删除单集（聚合剧目详情页的「删减资源」）。
///
/// 只删传入的单集 id；同一剧目剩下的单集、以及**其他剧目的资源都不受影响**。
/// 剧目主记录保留（删光了也还是一条「空剧目」，方便以后重新导入）。
#[tauri::command]
pub async fn delete_episodes(app: AppHandle, ids: Vec<i64>) -> Result<i64, String> {
    if ids.is_empty() {
        return Ok(0);
    }
    let result = tauri::async_runtime::spawn_blocking(move || {
        let conn =
            crate::store::db::open(&app).map_err(|e| format!("打开数据库失败：{e}"))?;
        delete_rows(&conn, "episodes", &ids)
    })
    .await
    .map_err(|e| format!("删除单集任务异常：{e}"))??;
    Ok(result)
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
    tmdb_id: Option<i64>,
    /// TheTVDB 剧集 id。有它就说明这条**被剧集数据源认领过**，是 R1 的产出点之一。
    tvdb_id: Option<i64>,
    group_key: Option<String>,
    locked: bool,
}

/* -------------------- R1：数据源标识（SourceHint）接线 -------------------- */

/// `SourceHint` 能否把这条记录改判 —— 五条 gate 全部满足才动手。
///
/// 为什么 gate 放在这里而不是 `core`：`core` 的规则是「信号 → 结论」的纯函数，
/// 而「**允许**这个信号在什么条件下生效」是策略，属于 `commands`。
/// 分层保持干净：`core` 一行没改，只是不再有 `allow(dead_code)` 挡着。
///
/// 五条 gate：
/// 1. `locked = 0`（用户确认过的条目谁也不许动）
/// 2. 名下没有任何单集行（已经有单集 ⇒ 它已经是剧集，不需要改判）
/// 3. 目标类型 ≠ 当前类型（一样就没必要写库）
/// 4. 方向限制：只允许「待确认 → 明确」与「电影 → 剧集」，
///    **禁止「剧集 → 电影」**——这是本项目历史伤疤的方向
/// 5. 改判写进 `type_audit`，事后可追溯
///
/// 返回 `Ok(true)` 表示确实改了。
/// 前四条 gate 的**判定**（只读，不写库）。
///
/// 拆出来是因为「能不能改」和「怎么改」是两件事：改判成剧集时不能只改 `type`，
/// 还得把文件转成单集挂进剧集容器（见 `reclassify_library` 里的调用点），
/// 否则会留下「剧集行挂着文件却没有单集」的畸形数据。
pub fn source_hint_allowed(
    conn: &Connection,
    media_id: i64,
    hint: SourceHint,
) -> Result<bool, String> {
    // 取当前状态
    let (cur, locked): (String, i64) = conn
        .query_row(
            "SELECT type, COALESCE(locked,0) FROM media WHERE id = ?1",
            params![media_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("条目不存在：{media_id}"))?;

    // gate ①：锁住的条目不动（User is always right）
    if locked != 0 {
        return Ok(false);
    }
    // gate ②：已经有单集的，说明它事实上就是剧集容器
    let eps: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM episodes WHERE media_id = ?1",
            params![media_id],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    if eps > 0 {
        return Ok(false);
    }

    let target = hint.as_str();
    // gate ③：没变化就不写
    if target == cur {
        return Ok(false);
    }
    // gate ④：方向限制（禁止 Tv → Movie）
    let allowed = match hint {
        SourceHint::Tv => cur == "unknown" || cur != "tv", // unknown/movie → tv
        SourceHint::Movie => cur == "unknown",             // 只放行 unknown → movie
    };
    if !allowed {
        return Ok(false);
    }
    Ok(true)
}

/// 直接把某条记录改成数据源给定的类型（**只改 `type`**，不动文件结构）。
///
/// 适用于目标为电影 / 待确认的场景；目标为剧集时应走 `convert_to_episode`
/// 把文件变成单集，而不是把类型一改了事。返回是否真的写了库。
pub fn apply_source_hint(
    conn: &Connection,
    media_id: i64,
    hint: SourceHint,
) -> Result<bool, String> {
    if !source_hint_allowed(conn, media_id, hint)? {
        return Ok(false);
    }
    let cur: String = conn
        .query_row("SELECT type FROM media WHERE id = ?1", params![media_id], |r| {
            r.get(0)
        })
        .map_err(|e| e.to_string())?;
    let target = hint.as_str();
    let ts = crate::store::db::now_epoch();
    let audit = format!("{cur} → {target}（数据源标识）");
    conn.execute(
        "UPDATE media SET type = ?2, type_audit = ?3, updated_at = ?4 WHERE id = ?1",
        params![media_id, target, audit, ts],
    )
    .map_err(|e| e.to_string())?;
    Ok(true)
}

/// 从库里已有的字段推导 `SourceHint`（没有明确来源就返回 None，绝不猜）。
///
/// 产出点之一：TheTVDB 只收剧集，所以**挂上 tvdb_id 就等于是剧集**。
pub fn hint_from_row(tvdb_id: Option<i64>) -> Option<SourceHint> {
    if tvdb_id.is_some() {
        return Some(SourceHint::Tv);
    }
    None
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
    mediatype::fill_episode_numbers(&mut parsed, &name, Some(&crate::util::dir_name(Path::new(&row.file_path)).unwrap_or_default()));
    let parsed_title = if parsed.title.trim().is_empty() {
        row.title.clone()
    } else {
        // 与扫描入库同一个口径：剧集标题不拖着尾部集号（`大明风华 01` → `大明风华`）。
        mediatype::series_title_of(&parsed.title)
    };
    let dir_name = crate::util::dir_name(Path::new(&row.file_path)).unwrap_or_default();
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
    // 这条行即将被删：先把挂在它下面的条目过继出去，别留悬挂的 parent_id
    crate::commands::tree::adopt_children(conn, &[row.id])?;
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
                    duration, year, tmdb_id, tvdb_id, group_key, COALESCE(locked,0) \
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
                    tmdb_id: r.get(8)?,
                    tvdb_id: r.get(9)?,
                    group_key: r.get(10)?,
                    locked: r.get::<_, i64>(11)? != 0,
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
            .entry(crate::util::parent_dir(Path::new(&r.file_path)))
            .or_default()
            .push((name, parsed.clone()));
        parsed_of_media.insert(r.id, parsed);
    }
    let mut parsed_of_ep: BTreeMap<i64, ParsedName> = BTreeMap::new();
    for e in &eps {
        let name = row_name(&e.2, &e.3);
        let parsed = parse_file_name(&name);
        by_dir
            .entry(crate::util::parent_dir(Path::new(&e.3)))
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

    let ts = crate::store::db::now_epoch();
    let tx = conn.transaction().map_err(|e| e.to_string())?;

    // 4) 媒体行：重新判定 → 修正
    for r in &rows {
        if r.locked {
            out.skipped_locked += 1;
            continue;
        }
        let parsed = parsed_of_media.get(&r.id).cloned().unwrap_or_default();
        let name = row_name(&r.file_name, &r.file_path);

        // R1：数据源已经认领过这条（有 tvdb_id ⇒ 剧集）。这比任何文件名猜测都权威，
        // 先过一遍 gate（locked / 已有单集 / 方向限制）再决定要不要改判。
        if let Some(hint) = hint_from_row(r.tvdb_id) {
            if source_hint_allowed(&tx, r.id, hint)? {
                match hint {
                    // 改判成剧集必须走「文件转单集」，不能只改 type
                    SourceHint::Tv => {
                        convert_to_episode(&tx, r, parsed.clone(), ts)?;
                        out.to_tv += 1;
                        continue;
                    }
                    SourceHint::Movie => {
                        let cur = r.kind.as_str();
                        let audit = format!("{cur} → movie（数据源标识）");
                        tx.execute(
                            "UPDATE media SET type = 'movie', type_audit = ?3, updated_at = ?2 \
                             WHERE id = ?1",
                            params![r.id, ts, audit],
                        )
                        .map_err(|e| e.to_string())?;
                        out.to_movie += 1;
                        continue;
                    }
                }
            }
        }

        let dir = crate::util::parent_dir(Path::new(&r.file_path));
        let dname = crate::util::dir_name(Path::new(&r.file_path)).unwrap_or_default();
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
        sig.folder_compilation_count = st.compilation_count;
        sig.folder_consensus_title = st.consensus_title.as_deref();
        sig.folder_number_prefix = st.numbered_prefix.as_deref();
        sig.has_metadata = r.tmdb_id.is_some() || r.year.is_some();
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
        let dir = crate::util::parent_dir(Path::new(&e.3));
        let dname = crate::util::dir_name(Path::new(&e.3)).unwrap_or_default();
        let st = stats.get(&dir).cloned().unwrap_or_default();
        let mut sig = Signals::from_parsed(&parsed, &name);
        sig.folder = Some(dname.as_str());
        sig.parent = Some(dname.as_str());
        sig.folder_total = st.total;
        sig.folder_tv_count = st.tv_count;
        sig.folder_numbered_count = st.numbered_count;
        sig.folder_compilation_count = st.compilation_count;
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
        // 这条 tv 行要被删掉换成 movie 行：先把子级过继出去再删
        crate::commands::tree::adopt_children(&tx, &[e.1])?;
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
        let mut conn = crate::store::db::open(&app).map_err(|e| format!("打开数据库失败：{e}"))?;
        reclassify_library(&mut conn, ids.as_deref())
    })
    .await
    .map_err(|e| format!("重新识别任务异常：{e}"))?
}


/// 清空媒体库（保留设置）。
#[tauri::command]
pub fn clear_library(state: State<'_, Mutex<Connection>>) -> Result<(), String> {
    let conn = state.lock().map_err(|e| e.to_string())?;
    clear_rows_healing(&conn)
}

/// 清空三张业务表；若因检索索引损坏而失败，重建索引后重试一次。
///
/// 抽出来是为了能单测——`clear_library` 要 `State`，测试里造不出来。
fn clear_rows_healing(conn: &Connection) -> Result<(), String> {
    match clear_once(conn) {
        Err(e) if crate::store::schema::heal_fts_if_malformed(conn, &e) => clear_once(conn),
        other => other,
    }
}

fn clear_once(conn: &Connection) -> Result<(), String> {
    conn.execute_batch("DELETE FROM episodes; DELETE FROM media; DELETE FROM scan_history;")
        .map_err(|e| e.to_string())
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
#[path = "tests/media.rs"]
mod tests;
