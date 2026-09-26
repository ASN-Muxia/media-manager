//! TMDB 刮削命令：搜索候选、抓取元数据并回填 media 表。
//!
//! ## 为什么不在命令里直接发请求
//! Tauri 的**同步**命令是在**主线程**上执行的，而主线程同时负责窗口消息循环和
//! WebView。批量刮削要在主线程里串行发几十个 HTTP 请求（每个最长 20 秒超时），
//! 期间界面完全收不到消息 → 「无响应 / 卡死」，连窗口都拖不动。
//!
//! 因此这里的做法是：
//! - 命令一律改成 `async`，Tauri 会把它们放到异步运行时，不再占主线程；
//! - 单条刮削 / 搜索这类"要拿结果"的操作，用 `async_runtime::spawn_blocking`
//!   把阻塞代码挪到后台线程，返回值保持不变；
//! - 批量刮削用 `std::thread::spawn` 起一个后台任务，命令**立即返回**，
//!   期间通过 `scrape-progress` 事件推送进度，前端随时可以 `cancel_scrape` 取消；
//! - 后台任务自己开一个数据库连接，不在网络等待期间占用全局锁。
//!
//! ## 数据源的分工（重要）
//!
//! **元数据 API 是地基，AI 只是可选补漏层**，顺序固定不可颠倒：
//!
//! ```text
//! TMDb（片名/简介/评分/图，权威） ──┐
//! TheTVDB（季 / 集结构，TMDb 缺集时兜底） ──┼─► 回填 media / episodes
//! Fanart.tv（TMDb 缺图时补海报背景） ──┤
//! OMDb（IMDb 评分补充，缺评分时才写） ──┘
//!        ▲
//!        │ 只影响「拿什么去检索」，绝不改写元数据本身
//!   大模型脏文件名解析（可选，失败即回退正则）
//! ```
//!
//! 任何 provider 没配 Key 就整体缺席，链路照常跑完；
//! 关掉大模型，刮削行为与加这个模块之前**完全一致**。

use crate::commands::catalog;
use crate::commands::settings::{
    load_fanart_key, load_hosts, load_llm_config, load_omdb_key, load_tmdb_key,
    load_tmdb_language, load_tvdb_key,
};
use crate::db::now_epoch;
use crate::fanart::{Art, FanartClient};
use crate::llm::LlmConfig;
use crate::omdb::OmdbClient;
use crate::tmdb::{TmdbClient, TmdbSearchItem};
use crate::tvdb::TvdbClient;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, State};

/// 进度事件名（前端 `listen("scrape-progress", ...)`）。
pub const PROGRESS_EVENT: &str = "scrape-progress";

/// 单条刮削结果。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScrapeOutcome {
    pub ok: bool,
    pub media_id: i64,
    pub tmdb_id: Option<i64>,
    pub title: Option<String>,
    /// 本次同步到的单集条数（电影恒为 0）。
    pub episodes: i64,
    pub message: String,
}

/// 批量刮削的进度（事件载荷）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScrapeProgress {
    /// 是否还有任务在跑。
    pub running: bool,
    /// 本次任务是否已结束（无论正常完成还是被取消）。
    pub finished: bool,
    pub cancelled: bool,
    pub total: i64,
    pub done: i64,
    pub ok: i64,
    pub failed: i64,
    /// 当前正在处理的条目标题。
    pub current: String,
    pub message: String,
}

impl ScrapeProgress {
    fn tick(total: i64, done: i64, ok: i64, failed: i64, current: String) -> Self {
        Self {
            running: true,
            finished: false,
            cancelled: false,
            total,
            done,
            ok,
            failed,
            current,
            message: String::new(),
        }
    }

    fn done(total: i64, ok: i64, failed: i64, cancelled: bool) -> Self {
        Self {
            running: false,
            finished: true,
            cancelled,
            total,
            done: ok + failed,
            ok,
            failed,
            current: String::new(),
            message: if cancelled {
                format!("已取消（已完成 {ok} 条，跳过 {} 条）", total - ok - failed)
            } else {
                format!("批量刮削完成：共 {total} 条，成功 {ok}，失败 {failed}")
            },
        }
    }

    fn aborted(message: String) -> Self {
        Self {
            running: false,
            finished: true,
            cancelled: false,
            total: 0,
            done: 0,
            ok: 0,
            failed: 0,
            current: String::new(),
            message,
        }
    }
}

/// 启动批量刮削的返回值：命令不等待结果，只告诉前端"起没起来"。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScrapeStart {
    pub started: bool,
    pub total: i64,
    pub no_key: bool,
    pub message: String,
}

/// 批量刮削的控制开关：取消标志 + 运行中标志。
///
/// 放在 `Arc` 里由 app 托管，后台线程与命令共享同一个实例。
#[derive(Debug, Default)]
pub struct ScrapeControl {
    cancel: AtomicBool,
    running: AtomicBool,
}

impl ScrapeControl {
    /// 开始新一轮：先复位取消标志，再置运行中。
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

/// 一次刮削要用的全部外部数据源。
///
/// 只有 TMDb 是**必需**的（没有它就谈不上刮削）；其余全部可选，
/// 没配 Key 就是 `None`，对应步骤整段跳过，不影响主链路。
pub struct Providers {
    pub tmdb: TmdbClient,
    /// 剧集季 / 集结构（TMDb 缺集信息时用）。
    pub tvdb: Option<TvdbClient>,
    /// 补充图源（TMDb 缺图时用）。
    pub fanart: Option<FanartClient>,
    /// IMDb 评分补充。
    pub omdb: Option<OmdbClient>,
    /// 大模型脏文件名解析（**可选增强层**，默认关闭）。
    pub llm: LlmConfig,
    /// 自定义 hosts 映射（所有客户端共用）。
    pub hosts: Vec<String>,
}

impl Providers {
    /// 从设置里装配。Key 为空的 provider 直接缺席，不报错。
    pub fn load(conn: &Connection, tmdb_key: &str, hosts: Vec<String>, language: &str) -> Self {
        let tvdb_key = load_tvdb_key(conn);
        let fanart_key = load_fanart_key(conn);
        let omdb_key = load_omdb_key(conn);
        let tvdb = TvdbClient::with_hosts(tvdb_key, &hosts);
        let fanart = FanartClient::with_hosts(fanart_key, &hosts);
        let omdb = OmdbClient::with_hosts(omdb_key, &hosts);
        Providers {
            tmdb: TmdbClient::with_hosts_and_language(tmdb_key, &hosts, language),
            tvdb: if tvdb.enabled() { Some(tvdb) } else { None },
            fanart: if fanart.enabled() { Some(fanart) } else { None },
            omdb: if omdb.enabled() { Some(omdb) } else { None },
            llm: load_llm_config(conn),
            hosts,
        }
    }

    fn has_tvdb(&self) -> bool {
        self.tvdb.is_some()
    }
}

/// 待刮削条目的最小信息。
struct PendingRow {
    media_type: String,
    title: String,
    year: Option<i32>,
    /// 用于脏文件名兜底解析的**原始文件名**（剧集取第一集的文件名）。
    file_name: Option<String>,
}

fn read_pending(conn: &Connection, id: i64) -> Result<PendingRow, String> {
    conn.query_row(
        "SELECT id, type, title, year, \
                COALESCE(file_name, (SELECT e.file_name FROM episodes e \
                    WHERE e.media_id = media.id ORDER BY e.season, e.episode LIMIT 1)) \
         FROM media WHERE id = ?1",
        params![id],
        |r| {
            Ok(PendingRow {
                media_type: r.get(1)?,
                title: r.get(2)?,
                year: r.get(3)?,
                file_name: r.get(4)?,
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

/// 单集级刮削：按库中已存在的季号逐季拉取单集元数据并回填。
///
/// 某一季请求失败**不中断**其余季（网络抖动时尽量多回填一些）。
fn fill_episodes(
    conn: &Connection,
    client: &TmdbClient,
    media_id: i64,
    tv_id: i64,
) -> Result<i64, String> {
    let seasons: Vec<i64> = {
        let mut stmt = conn
            .prepare("SELECT DISTINCT season FROM episodes WHERE media_id = ?1 ORDER BY season")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![media_id], |r| r.get::<_, i64>(0))
            .map_err(|e| e.to_string())?;
        rows.filter_map(Result::ok).collect()
    };

    let mut updated = 0i64;
    for season in seasons {
        let Ok(list) = client.season_episodes(tv_id, season) else {
            continue;
        };
        for ep in list {
            let n = conn
                .execute(
                    "UPDATE episodes SET \
                        tmdb_id    = COALESCE(?3, tmdb_id), \
                        title      = COALESCE(NULLIF(?4, ''), title), \
                        overview   = COALESCE(?5, overview), \
                        still_path = COALESCE(?6, still_path), \
                        air_date   = COALESCE(?7, air_date) \
                     WHERE media_id = ?1 AND season = ?2 AND episode = ?8",
                    params![
                        media_id,
                        season,
                        if ep.id > 0 { Some(ep.id) } else { None },
                        ep.name.clone().unwrap_or_default(),
                        ep.overview,
                        ep.still_path,
                        ep.air_date,
                        ep.episode_number
                    ],
                )
                .map_err(|e| e.to_string())?;
            updated += n as i64;
        }
    }
    Ok(updated)
}

/// 按标题 + 年份在 TMDB 里挑一个最合适的 id（优先同年份，其次第一条）。
fn pick_tmdb_id(
    client: &TmdbClient,
    media_type: &str,
    title: &str,
    year: Option<i32>,
) -> Option<i64> {
    if title.trim().is_empty() {
        return None;
    }
    client
        .search(media_type, title, year)
        .ok()
        .and_then(|items| {
            items
                .iter()
                .find(|it| {
                    year.is_some()
                        && crate::tmdb::year_from_date(it.date().as_deref()) == year
                })
                .or_else(|| items.first())
                .map(|it| it.id)
        })
}

/* --------------------- 可选 provider（只补漏，不覆盖） --------------------- */

/// 补图规则：原来**没有**（NULL 或空串）才补，已有的一律保留。
///
/// 抽成纯函数是为了能在不联网的情况下验证规则本身。
fn merge_art(
    poster: Option<String>,
    backdrop: Option<String>,
    art: &Art,
) -> (Option<String>, Option<String>) {
    let missing = |s: &Option<String>| s.as_deref().map_or(true, |v| v.trim().is_empty());
    (
        if missing(&poster) {
            art.poster.clone()
        } else {
            None
        },
        if missing(&backdrop) {
            art.backdrop.clone()
        } else {
            None
        },
    )
}

/// 评分规则：已有评分（>0）不动；OMDb 没给或给了无效值就当没查到。
fn merge_rating(current: Option<f64>, fetched: Option<f64>) -> Option<f64> {
    if current.map_or(false, |v| v > 0.0) {
        return None;
    }
    fetched.filter(|v| (0.0..=10.0).contains(v))
}

/// 用 Fanart.tv 补**缺失**的海报 / 背景图。
///
/// 剧集要用 TheTVDB 的 id（Fanart.tv 的 tv 端点就是按 tvdb_id 组织的），
/// 所以顺序上放在 TheTVDB 之后。
fn fill_art_from_fanart(
    p: &Providers,
    conn: &Connection,
    id: i64,
    media_type: &str,
    tmdb_id: i64,
) -> bool {
    let Some(client) = p.fanart.as_ref() else {
        return false;
    };
    let Ok((poster, backdrop)) = conn.query_row(
        "SELECT poster_path, backdrop_path FROM media WHERE id = ?1",
        params![id],
        |r| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, Option<String>>(1)?)),
    ) else {
        return false;
    };

    let art = if media_type == "tv" {
        let tvdb_id: Option<i64> = conn
            .query_row("SELECT tvdb_id FROM media WHERE id = ?1", params![id], |r| r.get(0))
            .unwrap_or(None);
        match tvdb_id {
            Some(v) => client.tv_art(v).ok(),
            None => None,
        }
    } else {
        client.movie_art(tmdb_id).ok()
    };

    let Some(art) = art else {
        return false;
    };
    let (new_poster, new_backdrop) = merge_art(poster, backdrop, &art);
    if new_poster.is_none() && new_backdrop.is_none() {
        return false;
    }
    conn.execute(
        "UPDATE media SET poster_path = COALESCE(?2, poster_path), \
                backdrop_path = COALESCE(?3, backdrop_path), updated_at = ?4 \
         WHERE id = ?1",
        params![id, new_poster, new_backdrop, now_epoch()],
    )
    .map(|n| n > 0)
    .unwrap_or(false)
}

/// 用 OMDb 的 IMDb 评分补**缺失**的评分。
fn fill_rating_from_omdb(
    p: &Providers,
    conn: &Connection,
    id: i64,
    title: &str,
    year: Option<i32>,
) -> bool {
    let Some(client) = p.omdb.as_ref() else {
        return false;
    };
    if title.trim().is_empty() {
        return false;
    }
    let current: Option<f64> = conn
        .query_row("SELECT vote_average FROM media WHERE id = ?1", params![id], |r| r.get(0))
        .unwrap_or(None);
    let Some(rating) = merge_rating(
        current,
        client.by_title(title, year).ok().and_then(|e| e.rating()),
    ) else {
        return false;
    };
    conn.execute(
        "UPDATE media SET vote_average = ?2, updated_at = ?3 WHERE id = ?1",
        params![id, rating, now_epoch()],
    )
    .map(|n| n > 0)
    .unwrap_or(false)
}

/// TMDb 没给集信息时的兜底：拿 TheTVDB 的季 / 集结构补。
///
/// 返回 `(更新的集数, TheTVDB 剧集 id)`；id 会顺手存进 `media.tvdb_id`，
/// 之后 Fanart.tv 补图就不用再查一次。
fn fill_episodes_from_tvdb(
    p: &Providers,
    conn: &Connection,
    id: i64,
    title: &str,
) -> (i64, Option<i64>) {
    let Some(client) = p.tvdb.as_ref() else {
        return (0, None);
    };
    let tvdb_id = match conn
        .query_row("SELECT tvdb_id FROM media WHERE id = ?1", params![id], |r| r.get(0))
        .unwrap_or(None)
    {
        Some(v) => Some(v),
        None => client
            .search(title)
            .ok()
            .and_then(|list| list.into_iter().find(|s| s.id > 0).map(|s| s.id)),
    };
    let Some(tvdb_id) = tvdb_id else {
        return (0, None);
    };
    (apply_tvdb_episodes(conn, client, id, tvdb_id), Some(tvdb_id))
}

/// 把 TheTVDB 的季 / 集结构写进 `episodes`（自动兜底与手动应用共用）。
fn apply_tvdb_episodes(
    conn: &Connection,
    client: &TvdbClient,
    id: i64,
    tvdb_id: i64,
) -> i64 {
    let _ = conn.execute(
        "UPDATE media SET tvdb_id = ?2, updated_at = ?3 WHERE id = ?1",
        params![id, tvdb_id, now_epoch()],
    );

    let Ok(list) = client.episodes(tvdb_id) else {
        return 0;
    };
    let mut updated = 0i64;
    for ep in list {
        let (season, number) = match (ep.season_number, ep.number) {
            (Some(s), Some(n)) => (s, n),
            _ => continue,
        };
        updated += conn
            .execute(
                "UPDATE episodes SET \
                    tvdb_id    = COALESCE(?3, tvdb_id), \
                    title      = COALESCE(NULLIF(?4, ''), title), \
                    overview   = COALESCE(?5, overview), \
                    still_path = COALESCE(?6, still_path), \
                    air_date   = COALESCE(?7, air_date) \
                 WHERE media_id = ?1 AND season = ?2 AND episode = ?8",
                params![
                    id,
                    season,
                    if ep.id > 0 { Some(ep.id) } else { None },
                    ep.name.unwrap_or_default(),
                    ep.overview,
                    ep.image,
                    ep.aired,
                    number
                ],
            )
            .unwrap_or(0) as i64;
    }
    updated
}

/// 识别失败时的**可选**兜底：让大模型解析脏文件名，拿到更靠谱的检索词。
///
/// - 只在「正则没解析出有效标题」时才值得叫模型（宁可保守）；
/// - 模型没配 / 调用失败 / 没给出片名 → 返回 `None`，调用方走原来的失败路径；
/// - **只影响拿什么去检索，不改写元数据**：真正的标题仍以 TMDb 为准。
fn refine_title_with_llm(p: &Providers, conn: &Connection, id: i64, pending: &PendingRow) -> Option<(String, Option<i32>)> {
    let file_name = pending.file_name.as_deref()?.trim();
    if file_name.is_empty() {
        return None;
    }
    // 正则已经认出来了就别打扰模型
    let base = crate::parser::parse_file_name(file_name);
    let stem = file_name
        .rsplit_once('.')
        .map(|(s, _)| s)
        .unwrap_or(file_name);
    let junk = pending.title.trim().is_empty() || pending.title.trim() == stem.trim();
    if !junk && !crate::llm::needs_enhancement(&base, file_name) {
        return None;
    }

    let (improved, fields) = crate::llm::enhance(base, file_name, &p.llm, &p.hosts);
    let Some(f) = fields else {
        return None; // 未启用 / 调用失败 → 静默回退
    };
    let title = improved.title.trim().to_string();
    if title.is_empty() || title == pending.title.trim() {
        return None;
    }

    // 顺手把模型认出来的分辨率 / 来源落到现有字段（不新建表）
    if f.resolution.is_some() || f.release_source.is_some() {
        let _ = conn.execute(
            "UPDATE media SET resolution = COALESCE(resolution, ?2), \
                    release_source = COALESCE(release_source, ?3), updated_at = ?4 \
             WHERE id = ?1",
            params![id, f.resolution, f.release_source, now_epoch()],
        );
    }
    Some((title, improved.year))
}

/// 刮削一条记录并回填。**纯后台逻辑**，调用方必须保证不在主线程上跑。
fn scrape_one(
    conn: &Connection,
    p: &Providers,
    id: i64,
    tmdb_id: Option<i64>,
) -> Result<ScrapeOutcome, String> {
    let pending = read_pending(conn, id)?;

    // 1) 确定 TMDB id（TMDb 永远是第一步，也是权威来源）
    let mut resolved = match tmdb_id {
        Some(t) => Some(t),
        None => pick_tmdb_id(&p.tmdb, &pending.media_type, &pending.title, pending.year),
    };
    // 1b) 识别失败才轮到可选增强：大模型把脏文件名翻成人话，再重试一次检索
    let mut refined: Option<(String, Option<i32>)> = None;
    if resolved.is_none() {
        if let Some(r) = refine_title_with_llm(p, conn, id, &pending) {
            resolved = pick_tmdb_id(&p.tmdb, &pending.media_type, &r.0, r.1);
            refined = Some(r);
        }
    }

    let Some(tmdb_id) = resolved else {
        // 连大模型也救不回来：至少把能认出来的标题写回去，别让列表里一直显示乱码
        if let Some((t, y)) = refined {
            let _ = conn.execute(
                "UPDATE media SET title = ?2, year = COALESCE(year, ?3), updated_at = ?4 \
                 WHERE id = ?1",
                params![id, t, y, now_epoch()],
            );
        }
        return Ok(ScrapeOutcome {
            ok: false,
            media_id: id,
            tmdb_id: None,
            title: None,
            episodes: 0,
            message: "未在 TMDB 找到匹配条目".to_string(),
        });
    };

    // 2) 取详情并回填
    let meta = p.tmdb.detail(&pending.media_type, tmdb_id)?;
    update_meta(conn, id, tmdb_id, &meta)?;
    // 2b) TMDb 认出来的类型 ↔ 左侧栏目保持同步（失败不影响刮削结果）
    if let Some(genre) = meta.genre.as_deref() {
        let _ = catalog::link_genre_categories(conn, id, genre);
    }

    // 3) 剧集则继续做单集级回填；TMDb 没给集信息时用 TheTVDB 兜底
    let mut episodes = 0i64;
    if pending.media_type == "tv" {
        episodes = fill_episodes(conn, &p.tmdb, id, tmdb_id).unwrap_or(0);
        if episodes == 0 && p.has_tvdb() {
            let (n, _) = fill_episodes_from_tvdb(p, conn, id, &meta.title);
            episodes = n;
        }
    }

    // 4) 评分 / 图：只在缺的时候补
    let mut extra: Vec<&str> = Vec::new();
    if fill_rating_from_omdb(p, conn, id, &meta.title, meta.year.or(pending.year)) {
        extra.push("IMDb 评分");
    }
    if fill_art_from_fanart(p, conn, id, &pending.media_type, tmdb_id) {
        extra.push("Fanart 补图");
    }

    Ok(ScrapeOutcome {
        ok: true,
        media_id: id,
        tmdb_id: Some(tmdb_id),
        title: Some(meta.title.clone()),
        episodes,
        message: {
            let mut msg = if episodes > 0 {
                format!("刮削成功，同步了 {episodes} 集")
            } else {
                "刮削成功".to_string()
            };
            if !extra.is_empty() {
                msg.push_str(&format!("（补充：{}）", extra.join("、")));
            }
            msg
        },
    })
}

/// 读一下标题用于进度显示（查不到就退回 id）。
fn read_title(conn: &Connection, id: i64) -> String {
    conn.query_row("SELECT title FROM media WHERE id = ?1", params![id], |r| {
        r.get::<_, String>(0)
    })
    .unwrap_or_else(|_| format!("#{id}"))
}

/* --------------------------- 命令 --------------------------- */

/// 搜索 TMDB 候选（供详情页手动匹配）。
#[tauri::command]
pub async fn search_tmdb(
    state: State<'_, Mutex<Connection>>,
    media_type: String,
    query: String,
    year: Option<i32>,
) -> Result<Vec<TmdbSearchItem>, String> {
    let (key, hosts, language) = {
        let conn = state.lock().map_err(|e| e.to_string())?;
        (
            load_tmdb_key(&conn),
            load_hosts(&conn),
            load_tmdb_language(&conn),
        )
    };
    if key.trim().is_empty() {
        return Err("尚未配置 TMDB API Key，请先到「设置」填写".to_string());
    }
    // 网络请求挪到后台线程，主线程继续响应界面。
    tauri::async_runtime::spawn_blocking(move || {
        TmdbClient::with_hosts_and_language(key, &hosts, &language).search(
            &media_type,
            &query,
            year,
        )
    })
    .await
    .map_err(|e| format!("搜索任务异常退出: {e}"))?
}

/// 刮削单条：给了 `tmdb_id` 就直接取详情，否则按标题+年份自动匹配第一条。
#[tauri::command]
pub async fn scrape_media(
    app: AppHandle,
    state: State<'_, Mutex<Connection>>,
    id: i64,
    tmdb_id: Option<i64>,
) -> Result<ScrapeOutcome, String> {
    let (key, hosts, language) = {
        let conn = state.lock().map_err(|e| e.to_string())?;
        (
            load_tmdb_key(&conn),
            load_hosts(&conn),
            load_tmdb_language(&conn),
        )
    };
    if key.trim().is_empty() {
        return Err("尚未配置 TMDB API Key，请先到「设置」填写".to_string());
    }

    // 后台线程用自己的连接：WAL 模式下多连接并发读写是安全的。
    tauri::async_runtime::spawn_blocking(move || {
        let conn = crate::db::open(&app).map_err(|e| format!("打开数据库失败: {e}"))?;
        // 后台线程自己读设置装配 provider，不占主线程的连接锁
        let providers = Providers::load(&conn, &key, hosts, &language);
        scrape_one(&conn, &providers, id, tmdb_id)
    })
    .await
    .map_err(|e| format!("刮削任务异常退出: {e}"))?
}

/// 批量刮削尚未获取到海报的条目（最多 `limit` 条）。
///
/// 命令**立即返回**，实际工作交给后台线程；进度通过 `scrape-progress` 事件推送。
#[tauri::command]
pub async fn scrape_all(
    app: AppHandle,
    state: State<'_, Mutex<Connection>>,
    control: State<'_, Arc<ScrapeControl>>,
    limit: Option<i64>,
) -> Result<ScrapeStart, String> {
    let (key, hosts, language, targets) = {
        let conn = state.lock().map_err(|e| e.to_string())?;
        let key = load_tmdb_key(&conn);
        let hosts = load_hosts(&conn);
        let language = load_tmdb_language(&conn);
        let mut stmt = conn
            .prepare(
                "SELECT id FROM media WHERE poster_path IS NULL OR poster_path = '' \
                 ORDER BY id ASC LIMIT ?1",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![limit.unwrap_or(50)], |r| r.get::<_, i64>(0))
            .map_err(|e| e.to_string())?;
        (
            key,
            hosts,
            language,
            rows.filter_map(Result::ok).collect::<Vec<i64>>(),
        )
    };

    if key.trim().is_empty() {
        return Ok(ScrapeStart {
            started: false,
            total: targets.len() as i64,
            no_key: true,
            message: "尚未配置 TMDB API Key，请先到「设置」填写".to_string(),
        });
    }
    if control.running() {
        return Ok(ScrapeStart {
            started: false,
            total: 0,
            no_key: false,
            message: "已有刮削任务在进行中".to_string(),
        });
    }
    let total = targets.len() as i64;
    if total == 0 {
        return Ok(ScrapeStart {
            started: false,
            total: 0,
            no_key: false,
            message: "没有需要刮削的条目（都已刮削过）".to_string(),
        });
    }

    let control = control.inner().clone();
    control.begin();
    std::thread::spawn(move || {
        run_batch(app, control, key, hosts, language, targets);
    });

    Ok(ScrapeStart {
        started: true,
        total,
        no_key: false,
        message: format!("已开始刮削 {total} 条"),
    })
}

/// 后台批量刮削：逐条处理，中途可被 `cancel_scrape` 打断。
fn run_batch(
    app: AppHandle,
    control: Arc<ScrapeControl>,
    key: String,
    hosts: Vec<String>,
    language: String,
    ids: Vec<i64>,
) {
    let emit = |p: ScrapeProgress| {
        let _ = app.emit(PROGRESS_EVENT, p);
    };

    let conn = match crate::db::open(&app) {
        Ok(c) => c,
        Err(e) => {
            emit(ScrapeProgress::aborted(format!("打开数据库失败: {e}")));
            control.finish();
            return;
        }
    };
    // 用后台线程自己的连接装配 provider（TMDb 必需，其余没配 Key 就缺席）
    let providers = Providers::load(&conn, &key, hosts, &language);
    let total = ids.len() as i64;

    let mut ok = 0i64;
    let mut failed = 0i64;
    for id in ids {
        if control.cancelled() {
            break;
        }
        let title = read_title(&conn, id);
        emit(ScrapeProgress::tick(total, ok + failed, ok, failed, title));
        match scrape_one(&conn, &providers, id, None) {
            Ok(r) if r.ok => ok += 1,
            _ => failed += 1,
        }
    }

    let cancelled = control.cancelled();
    emit(ScrapeProgress::done(total, ok, failed, cancelled));
    control.finish();
}

/// 取消正在进行的批量刮削（已经发出的单个请求不会被强杀，下一轮循环即停止）。
#[tauri::command]
pub fn cancel_scrape(control: State<'_, Arc<ScrapeControl>>) -> Result<bool, String> {
    control.request_cancel();
    Ok(control.running())
}

/// 手动绑定某个 TMDB 条目（详情页「应用」按钮）。
#[tauri::command]
pub async fn apply_tmdb_match(
    app: AppHandle,
    state: State<'_, Mutex<Connection>>,
    id: i64,
    tmdb_id: i64,
) -> Result<ScrapeOutcome, String> {
    scrape_media(app, state, id, Some(tmdb_id)).await
}

/// 测试 TMDB 连通性（设置页「测试连接」）：真实发一次搜索请求。
///
/// 用来验证 API Key 与 hosts 配置是否生效——比"随便刮一条看看"快得多。
#[tauri::command]
pub async fn tmdb_test(
    state: State<'_, Mutex<Connection>>,
) -> Result<String, String> {
    let (key, hosts, language) = {
        let conn = state.lock().map_err(|e| e.to_string())?;
        (
            load_tmdb_key(&conn),
            load_hosts(&conn),
            load_tmdb_language(&conn),
        )
    };
    if key.trim().is_empty() {
        return Err("尚未配置 TMDB API Key".to_string());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let client = TmdbClient::with_hosts_and_language(key, &hosts, &language);
        match client.search("movie", "test", None) {
            Ok(items) => Ok(format!("连接正常，返回 {} 条结果", items.len())),
            Err(e) => Err(e),
        }
    })
    .await
    .map_err(|e| format!("测试任务异常退出: {e}"))?
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

/* --------------------- 可选 provider 的手动入口 --------------------- */
/* 这几个命令存在的意义：
   1) 设置页要有「配好了没有」的验证入口，否则用户只能靠猜；
   2) 大模型与 TheTVDB 都允许**手动指定**，不能只有自动兜底一条路；
   3) 视觉模型的调用必须能被审计——它只允许在文件名完全不可推断时使用。 */

/// 大模型接入预设（设置页下拉框用，免得用户自己猜模型名）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmPreset {
    pub label: String,
    pub base_url: String,
    pub model: String,
    pub vision_model: String,
    /// 视觉模型的推荐端点（与文本端点不同源时单独给出）。
    pub vision_base_url: String,
}

/// 手动应用 TheTVDB 的结果。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TvdbApplyResult {
    pub tvdb_id: i64,
    pub episodes: i64,
    pub genre: String,
}

/// OMDb 查到的信息（评分补充的可读回执）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OmdbInfo {
    pub title: String,
    pub year: String,
    pub rating: Option<f64>,
    pub genre: String,
    pub plot: String,
}

/// 大模型接入预设：推荐的排在最前，其余是同协议的备选。
#[tauri::command]
pub fn llm_presets() -> Vec<LlmPreset> {
    // 推荐的文本模型与视觉模型不同源（DeepSeek / 百炼），所以两个地址都给全。
    let mut out = vec![LlmPreset {
        label: "推荐 · DeepSeek + 通义千问 VL".to_string(),
        base_url: crate::llm::RECOMMENDED_BASE_URL.to_string(),
        model: crate::llm::RECOMMENDED_MODEL.to_string(),
        vision_model: crate::llm::RECOMMENDED_VISION_MODEL.to_string(),
        vision_base_url: crate::llm::RECOMMENDED_VISION_BASE_URL.to_string(),
    }];
    for (label, base) in crate::llm::ALTERNATIVE_MODELS {
        out.push(LlmPreset {
            label: label.to_string(),
            base_url: base.to_string(),
            // 备选只给地址：模型名各家不同，留空让用户在设置页自己填
            model: String::new(),
            vision_model: String::new(),
            vision_base_url: String::new(),
        });
    }
    out
}

/// 手动让大模型解析一个脏文件名（设置页「试解析」）。
///
/// 这是**增强层**的入口，不是刮削入口：它只产出检索用的结构化字段，
/// 真正的标题 / 简介 / 评分仍然来自 TMDb。
#[tauri::command]
pub async fn llm_parse_name(
    state: State<'_, Mutex<Connection>>,
    file_name: String,
) -> Result<crate::llm::ParsedFields, String> {
    let (cfg, hosts) = {
        let conn = state.lock().map_err(|e| e.to_string())?;
        (load_llm_config(&conn), load_hosts(&conn))
    };
    if !cfg.usable() {
        return Err("大模型解析未启用或未填写接口地址".to_string());
    }
    tauri::async_runtime::spawn_blocking(move || {
        crate::llm::LlmClient::new(cfg, &hosts).parse_name(&file_name)
    })
    .await
    .map_err(|e| format!("解析任务异常退出: {e}"))?
}

/// 视觉模型兜底：**只允许**文件名完全不可推断（纯数字 / 纯符号）时调用。
///
/// 挡一道 `is_undecidable` 是为了避免视觉模型被当成常规刮削手段滥用——
/// 它又慢又贵，而且准确率不如元数据 API。
#[tauri::command]
pub async fn llm_parse_image(
    state: State<'_, Mutex<Connection>>,
    file_name: String,
    image_data_url: String,
) -> Result<crate::llm::ParsedFields, String> {
    if !crate::llm::is_undecidable(&file_name) {
        return Err("该文件名可以推断出片名，不需要视觉识别".to_string());
    }
    let (cfg, hosts) = {
        let conn = state.lock().map_err(|e| e.to_string())?;
        (load_llm_config(&conn), load_hosts(&conn))
    };
    if !cfg.vision_usable() {
        return Err("未启用视觉模型".to_string());
    }
    tauri::async_runtime::spawn_blocking(move || {
        crate::llm::LlmClient::new(cfg, &hosts).parse_image(&file_name, &image_data_url)
    })
    .await
    .map_err(|e| format!("识别任务异常退出: {e}"))?
}

/// 手动把一条影视绑定到 TheTVDB 剧集，并按它的季 / 集结构回填单集。
///
/// 同时也是「TMDb 缺集信息」时的补救入口（详情页可用）。
#[tauri::command]
pub async fn tvdb_apply(
    app: AppHandle,
    state: State<'_, Mutex<Connection>>,
    media_id: i64,
    tvdb_id: Option<i64>,
) -> Result<TvdbApplyResult, String> {
    let (key, hosts) = {
        let conn = state.lock().map_err(|e| e.to_string())?;
        (load_tvdb_key(&conn), load_hosts(&conn))
    };
    if key.trim().is_empty() {
        return Err("尚未配置 TheTVDB API Key，请先到「设置」填写".to_string());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let conn = crate::db::open(&app).map_err(|e| format!("打开数据库失败: {e}"))?;
        let client = TvdbClient::with_hosts(key, &hosts);
        let id = match tvdb_id {
            Some(v) => v,
            None => {
                let title: String = conn
                    .query_row("SELECT title FROM media WHERE id = ?1", params![media_id], |r| {
                        r.get(0)
                    })
                    .map_err(|e| format!("未找到媒体: {e}"))?;
                client
                    .search(&title)?
                    .into_iter()
                    .find(|s| s.id > 0)
                    .map(|s| s.id)
                    .ok_or_else(|| "未在 TheTVDB 找到匹配剧集".to_string())?
            }
        };
        let (series, genres) = client.series(id)?;
        conn.execute(
            "UPDATE media SET tvdb_id = ?2, year = COALESCE(year, ?3), updated_at = ?4 \
             WHERE id = ?1",
            params![media_id, id, series.year_i32(), now_epoch()],
        )
        .map_err(|e| e.to_string())?;
        let genre = genres.unwrap_or_default();
        if !genre.is_empty() {
            let _ = catalog::link_genre_categories(&conn, media_id, &genre);
        }
        let episodes = apply_tvdb_episodes(&conn, &client, media_id, id);
        Ok(TvdbApplyResult {
            tvdb_id: id,
            episodes,
            genre,
        })
    })
    .await
    .map_err(|e| format!("TheTVDB 应用任务异常退出: {e}"))?
}

/// 查询 OMDb（评分补充的验证入口）。
#[tauri::command]
pub async fn omdb_lookup(
    state: State<'_, Mutex<Connection>>,
    title: String,
    year: Option<i32>,
) -> Result<OmdbInfo, String> {
    let (key, hosts) = {
        let conn = state.lock().map_err(|e| e.to_string())?;
        (load_omdb_key(&conn), load_hosts(&conn))
    };
    if key.trim().is_empty() {
        return Err("尚未配置 OMDb API Key，请先到「设置」填写".to_string());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let client = OmdbClient::with_hosts(key, &hosts);
        let entry = client.by_title(&title, year)?;
        Ok(OmdbInfo {
            rating: entry.rating(),
            title: entry.title.unwrap_or_default(),
            year: entry.year.unwrap_or_default(),
            genre: entry.genre.unwrap_or_default(),
            plot: entry.plot.unwrap_or_default(),
        })
    })
    .await
    .map_err(|e| format!("OMDb 查询任务异常退出: {e}"))?
}

#[cfg(test)]
mod tests {
    use super::*;

    fn new_control() -> Arc<ScrapeControl> {
        Arc::new(ScrapeControl::default())
    }

    #[test]
    fn control_begin_resets_cancel_flag() {
        let c = new_control();
        c.request_cancel();
        assert!(c.cancelled());
        c.begin();
        assert!(!c.cancelled(), "新一轮开始时应复位取消标志");
        assert!(c.running());
        c.finish();
        assert!(!c.running());
    }

    #[test]
    fn cancel_is_visible_to_worker_thread() {
        let c = new_control();
        c.begin();
        let worker = c.clone();
        assert!(!worker.cancelled());
        c.request_cancel();
        assert!(worker.cancelled(), "后台线程应能看到取消标志");
    }

    #[test]
    fn progress_tick_reports_counts() {
        let p = ScrapeProgress::tick(10, 3, 2, 1, "某部电影".to_string());
        assert!(p.running);
        assert!(!p.finished);
        assert_eq!((p.total, p.done, p.ok, p.failed), (10, 3, 2, 1));
        assert_eq!(p.current, "某部电影");
    }

    #[test]
    fn progress_done_message_differs_for_cancel() {
        let normal = ScrapeProgress::done(10, 8, 2, false);
        assert!(normal.finished && !normal.running && !normal.cancelled);
        assert!(normal.message.contains("成功 8"));

        let cancelled = ScrapeProgress::done(10, 3, 1, true);
        assert!(cancelled.cancelled);
        assert!(cancelled.message.contains("已取消"));
    }

    #[test]
    fn progress_aborted_carries_reason() {
        let p = ScrapeProgress::aborted("打开数据库失败".to_string());
        assert!(p.finished && !p.running);
        assert_eq!(p.message, "打开数据库失败");
    }

    /// 被取消后应立刻收敛：不再处理后续条目。
    #[test]
    fn cancelled_batch_stops_before_new_items() {
        let c = new_control();
        c.begin();
        let ids = vec![1, 2, 3, 4];
        let mut processed = 0;
        for _ in &ids {
            if c.cancelled() {
                break;
            }
            processed += 1;
            c.request_cancel(); // 模拟处理完第一条后用户点了取消
        }
        assert_eq!(processed, 1, "取消后不应继续处理后续条目");
        assert!(ScrapeProgress::done(ids.len() as i64, 1, 0, true).cancelled);
    }

    /// 取消时 `done` 只统计真正处理过的条数，未处理的不算失败。
    #[test]
    fn cancelled_progress_does_not_count_skipped_as_failed() {
        let p = ScrapeProgress::done(10, 2, 0, true);
        assert_eq!(p.done, 2);
        assert_eq!(p.failed, 0);
        assert!(p.message.contains("跳过 8"));
    }

    /* --------------------- 可选 provider 的规则 --------------------- */

    fn memory_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        crate::db::migrate(&conn).unwrap();
        conn
    }

    /// 设置项键名与 `commands/settings.rs` 保持一致。
    fn put_setting(conn: &Connection, key: &str, value: &str) {
        conn.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2) \
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )
        .unwrap();
    }

    #[test]
    fn optional_providers_are_absent_without_keys() {
        let conn = memory_db();
        let p = Providers::load(&conn, "tmdb-key", vec![], "zh-CN");
        assert!(p.tvdb.is_none(), "没配 Key 时 TheTVDB 应整段缺席");
        assert!(p.fanart.is_none());
        assert!(p.omdb.is_none());
        assert!(!p.llm.usable(), "大模型默认必须是关闭的");
        assert!(!p.has_tvdb());
    }

    #[test]
    fn providers_appear_once_keys_are_configured() {
        let conn = memory_db();
        put_setting(&conn, "tvdbKey", "tv-1");
        put_setting(&conn, "fanartKey", "fa-1");
        put_setting(&conn, "omdbKey", "om-1");
        put_setting(&conn, "llmEnabled", "1");
        put_setting(&conn, "llmBaseUrl", crate::llm::RECOMMENDED_BASE_URL);
        let p = Providers::load(&conn, "tmdb-key", vec![], "zh-CN");
        assert!(p.tvdb.is_some() && p.fanart.is_some() && p.omdb.is_some());
        assert!(p.has_tvdb());
        assert!(p.llm.usable());
        assert_eq!(p.llm.model(), crate::llm::RECOMMENDED_MODEL, "没填模型时应回落到推荐模型");
    }

    #[test]
    fn art_is_only_filled_when_missing() {
        let art = Art {
            poster: Some("https://f/p.jpg".to_string()),
            backdrop: Some("https://f/b.jpg".to_string()),
        };
        // 全空 → 两张都补
        let (p, b) = merge_art(None, None, &art);
        assert_eq!(p.as_deref(), Some("https://f/p.jpg"));
        assert_eq!(b.as_deref(), Some("https://f/b.jpg"));

        // 空串也算"没有"
        let (p, _) = merge_art(Some("".to_string()), None, &art);
        assert!(p.is_some());

        // 已有 TMDb 的图 → 一律不动
        let (p, b) = merge_art(Some("/tmdb.jpg".to_string()), Some("/bg.jpg".to_string()), &art);
        assert!(p.is_none() && b.is_none(), "已有图不应被 Fanart 覆盖");

        // 只缺背景时只补背景
        let (p, b) = merge_art(Some("/tmdb.jpg".to_string()), None, &art);
        assert!(p.is_none());
        assert!(b.is_some());
    }

    #[test]
    fn rating_is_only_filled_when_missing() {
        assert_eq!(merge_rating(None, Some(8.0)), Some(8.0));
        assert_eq!(merge_rating(Some(0.0), Some(8.0)), Some(8.0), "0 分视为没有评分");
        assert_eq!(merge_rating(Some(7.3), Some(8.0)), None, "已有评分不应被覆盖");
        assert_eq!(merge_rating(None, None), None);
        assert_eq!(merge_rating(None, Some(12.0)), None, "越界值当没查到");
    }

    /// 没配 OMDb / Fanart 时，补漏步骤应该是零成本的空转，而不是报错
    #[test]
    fn missing_providers_make_supplements_noop() {
        let conn = memory_db();
        conn.execute(
            "INSERT INTO media (type, title, added_at, updated_at) VALUES ('movie', 'X', 0, 0)",
            [],
        )
        .unwrap();
        let id = conn.last_insert_rowid();
        let p = Providers::load(&conn, "tmdb-key", vec![], "zh-CN");
        assert!(!fill_rating_from_omdb(&p, &conn, id, "X", None));
        assert!(!fill_art_from_fanart(&p, &conn, id, "movie", 1));
        assert_eq!(fill_episodes_from_tvdb(&p, &conn, id, "X"), (0, None));
    }

    /// 大模型回流：脏文件名被认出来后，分辨率 / 来源要落到现有字段
    #[test]
    fn llm_refine_is_silent_when_disabled() {
        let conn = memory_db();
        conn.execute(
            "INSERT INTO media (type, title, file_name, added_at, updated_at) \
             VALUES ('movie', '00123', '00123.mkv', 0, 0)",
            [],
        )
        .unwrap();
        let id = conn.last_insert_rowid();
        let p = Providers::load(&conn, "tmdb-key", vec![], "zh-CN");
        let pending = read_pending(&conn, id).unwrap();
        assert_eq!(pending.title, "00123");
        assert_eq!(pending.file_name.as_deref(), Some("00123.mkv"));
        // 未启用 → 静默返回 None，绝不抛错
        assert!(refine_title_with_llm(&p, &conn, id, &pending).is_none());
    }

    /// 预设要让设置页能「一键填入」，推荐项必须四个字段都齐
    #[test]
    fn llm_presets_put_recommendation_first() {
        let list = llm_presets();
        assert_eq!(list[0].label, "推荐 · DeepSeek + 通义千问 VL");
        assert_eq!(list[0].model, crate::llm::RECOMMENDED_MODEL);
        assert_eq!(list[0].base_url, crate::llm::RECOMMENDED_BASE_URL);
        assert_eq!(list[0].vision_model, crate::llm::RECOMMENDED_VISION_MODEL);
        assert!(!list[0].vision_base_url.is_empty());
        // 备选至少给出地址，方便用户照着填
        assert!(list.len() > 1);
        assert!(list[1].base_url.starts_with("http"));
    }

    /// 刮削回填里，剧集的 file_name 要能回退到第一集（脏文件名兜底要用）
    #[test]
    fn pending_file_name_falls_back_to_first_episode() {
        let conn = memory_db();
        conn.execute(
            "INSERT INTO media (type, title, added_at, updated_at) VALUES ('tv', '剧', 0, 0)",
            [],
        )
        .unwrap();
        let id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO episodes (media_id, season, episode, file_name, added_at) \
             VALUES (?1, 2, 1, 'S02E01.mkv', 0)",
            params![id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO episodes (media_id, season, episode, file_name, added_at) \
             VALUES (?1, 1, 1, 'S01E01.mkv', 0)",
            params![id],
        )
        .unwrap();
        let pending = read_pending(&conn, id).unwrap();
        assert_eq!(pending.file_name.as_deref(), Some("S01E01.mkv"));
    }
}
