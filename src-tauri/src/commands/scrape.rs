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
//! OMDb（IMDb 评分补充，缺评分时才写） ──┤
//! 豆瓣（中文元数据兜底：TMDb 未命中时整条接管，
//!       命中时只补它没给的中文字段） ──┘
//!        ▲
//!        │ 只影响「拿什么去检索」，绝不改写元数据本身
//!   大模型脏文件名解析（可选，失败即回退正则）
//! ```
//!
//! 任何 provider 没配 Key 就整体缺席，链路照常跑完；
//! 关掉大模型与豆瓣，刮削行为与加这两个模块之前**完全一致**。
//!
//! ## 豆瓣与其他源的关系（容易搞混，单独说明）
//!
//! 豆瓣是**中文兜底**，不是并列的第四个「主源」：
//! - TMDb **没命中**时，豆瓣可以**整条接管**（标题/年份/简介/评分/海报全写），
//!   这样"没有 TMDb Key"或"冷门中文片"也不至于完全刮不出东西；
//! - TMDb **命中**时，豆瓣只补它没给的中文字段（`release_date` / `original_title` /
//!   空掉的 `vote_average`），已有的值一律不动。
//!
//! 两种情况下都遵守同一条铁律：**只补缺失，绝不覆盖**。

use crate::commands::catalog;
use super::control::ScrapeControl;
use crate::commands::settings::{
    load_douban_config, load_fanart_key, load_hosts, load_llm_config, load_omdb_key,
    load_tmdb_key, load_tmdb_language, load_tvdb_key,
};
use crate::store::db::now_epoch;
use crate::provider::douban::{DoubanClient, DoubanMeta};
use crate::provider::fanart::{Art, FanartClient};
use crate::provider::llm::LlmConfig;
use crate::provider::omdb::OmdbClient;
use crate::provider::tmdb::{TmdbClient, TmdbSearchItem};
use crate::provider::tvdb::TvdbClient;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
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
    /// 豆瓣（中文元数据兜底）。**要显式开启**，没开就是 `None`。
    pub douban: Option<DoubanClient>,
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
        let (douban_enabled, douban_cookie) = load_douban_config(conn);
        let tvdb = TvdbClient::with_hosts(tvdb_key, &hosts);
        let fanart = FanartClient::with_hosts(fanart_key, &hosts);
        let omdb = OmdbClient::with_hosts(omdb_key, &hosts);
        // 豆瓣没有 Key，靠开关控制；关了就让步骤整段跳过。
        let douban = DoubanClient::with_hosts(douban_enabled, douban_cookie, &hosts);
        Providers {
            tmdb: TmdbClient::with_hosts_and_language(tmdb_key, &hosts, language),
            tvdb: if tvdb.enabled() { Some(tvdb) } else { None },
            fanart: if fanart.enabled() { Some(fanart) } else { None },
            omdb: if omdb.enabled() { Some(omdb) } else { None },
            douban: if douban.enabled() { Some(douban) } else { None },
            llm: load_llm_config(conn),
            hosts,
        }
    }

    fn has_tvdb(&self) -> bool {
        self.tvdb.is_some()
    }

    /// 是否配了 TMDB Key。没配时豆瓣要承担**整条接管**的角色。
    fn has_tmdb(&self, tmdb_key: &str) -> bool {
        !tmdb_key.trim().is_empty()
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

fn update_meta(conn: &Connection, id: i64, tmdb_id: i64, meta: &crate::provider::tmdb::TmdbMeta) -> Result<(), String> {
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
                        && crate::provider::tmdb::year_from_date(it.date().as_deref()) == year
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

/// 文本补漏规则：当前**为空**（NULL / 空串 / 全空白）才用新值，否则一律保留原值。
///
/// 与 `merge_art` 同一套语义，抽出来给豆瓣的一堆文本字段复用。
fn merge_text(current: Option<&str>, fetched: Option<&str>) -> Option<String> {
    let has_current = current.map_or(false, |v| !v.trim().is_empty());
    if has_current {
        return None;
    }
    fetched
        .map(|v| v.trim())
        .filter(|v| !v.is_empty())
        .map(|v| v.to_string())
}

/// 取第一个非空的字符串；两个都空就返回后者（调用方自己会 trim 判断）。
fn nonempty_or(first: &str, fallback: &str) -> String {
    if first.trim().is_empty() {
        fallback.trim().to_string()
    } else {
        first.trim().to_string()
    }
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

/* --------------------------- 豆瓣兜底 --------------------------- */

/// 豆瓣刮削的结果，供调用方拼提示语。
#[derive(Debug, Default, Clone, PartialEq)]
struct DoubanFill {
    /// 是否整条接管（TMDb 完全没命中，标题/年份都是豆瓣给的）。
    took_over: bool,
    /// 补进去了哪些字段（用于提示「补充：豆瓣简介、豆瓣海报」）。
    fields: Vec<String>,
}

/// 豆瓣**整条接管**：TMDb 没命中时的兜底。
///
/// 这里可以覆盖 `title`/`year`，因为此刻它们是扫描阶段猜出来的、没有权威来源。
/// 但只要豆瓣某项为空，就保留原值不动。
fn apply_douban_takeover(conn: &Connection, id: i64, m: &DoubanMeta) -> Result<DoubanFill, String> {
    let mut fields = Vec::new();
    conn.execute(
        "UPDATE media SET \
             title          = COALESCE(?2, title), \
             original_title = COALESCE(?3, original_title), \
             year           = COALESCE(?4, year), \
             overview       = COALESCE(?5, overview), \
             poster_path    = COALESCE(?6, poster_path), \
             vote_average   = COALESCE(?7, vote_average), \
             genre          = COALESCE(?8, genre), \
             source         = ?9, \
             updated_at     = ?10 \
         WHERE id = ?1",
        params![
            id,
            m.title.as_deref(),
            m.original_title.as_deref(),
            m.year,
            m.overview.as_deref(),
            m.poster.as_deref(),
            m.rating,
            m.genre.as_deref(),
            format!("douban:{}", m.douban_id),
            now_epoch()
        ],
    )
    .map_err(|e| e.to_string())?;

    if m.title.is_some() {
        fields.push("片名".to_string());
    }
    if m.overview.is_some() {
        fields.push("简介".to_string());
    }
    if m.poster.is_some() {
        fields.push("海报".to_string());
    }
    if m.rating.is_some() {
        fields.push("评分".to_string());
    }
    if m.genre.is_some() {
        fields.push("类型".to_string());
    }
    // 类型变了要同步左侧栏目（失败不影响刮削结果）
    if let Some(g) = m.genre.as_deref() {
        let _ = catalog::link_genre_categories(conn, id, g);
    }
    // 工作人员字段没有独立列，挂到 `release_source`（本来就是自由文本）
    if m.release_date.is_some() || m.directors.is_some() || m.actors.is_some() {
        let _ = conn.execute(
            "UPDATE media SET release_source = COALESCE(release_source, ?2) WHERE id = ?1",
            params![id, douban_crew_line(m)],
        );
    }
    Ok(DoubanFill {
        took_over: true,
        fields,
    })
}

/// 豆瓣**只补缺失**：TMDb 命中之后跑。
///
/// 逐个字段判断，已有的（非空）一律不动，返回真正补进去的字段名。
fn apply_douban_fill(conn: &Connection, id: i64, m: &DoubanMeta) -> Result<DoubanFill, String> {
    // 一次读出当前值，逐字段算「谁该补」
    let row: (Option<String>, Option<String>, Option<String>, Option<f64>, Option<String>) = conn
        .query_row(
            "SELECT original_title, overview, poster_path, vote_average, release_source \
             FROM media WHERE id = ?1",
            params![id],
            |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                ))
            },
        )
        .map_err(|e| e.to_string())?;
    let (original_title, overview, poster, vote, release_source) = row;

    let mut fields = Vec::new();

    let new_original = merge_text(original_title.as_deref(), m.original_title.as_deref());
    if new_original.is_some() {
        fields.push("原名".to_string());
    }
    let new_overview = merge_text(overview.as_deref(), m.overview.as_deref());
    if new_overview.is_some() {
        fields.push("简介".to_string());
    }
    let new_poster = merge_text(poster.as_deref(), m.poster.as_deref());
    if new_poster.is_some() {
        fields.push("海报".to_string());
    }
    let new_rating = merge_rating(vote, m.rating);
    if new_rating.is_some() {
        fields.push("评分".to_string());
    }
    // 简介里塞不下的导演/演员/上映日期，拼一行挂到 release_source
    let crew = douban_crew_line(m);
    let new_crew = if crew.is_empty() {
        None
    } else {
        merge_text(release_source.as_deref(), Some(&crew))
    };
    if new_crew.is_some() {
        fields.push("上映/主创".to_string());
    }

    if fields.is_empty() {
        return Ok(DoubanFill::default());
    }
    conn.execute(
        "UPDATE media SET \
             original_title = COALESCE(?2, original_title), \
             overview       = COALESCE(?3, overview), \
             poster_path    = COALESCE(?4, poster_path), \
             vote_average   = COALESCE(?5, vote_average), \
             release_source = COALESCE(?6, release_source), \
             updated_at     = ?7 \
         WHERE id = ?1",
        params![
            id,
            new_original,
            new_overview,
            new_poster,
            new_rating,
            new_crew,
            now_epoch()
        ],
    )
    .map_err(|e| e.to_string())?;

    Ok(DoubanFill {
        took_over: false,
        fields,
    })
}

/// 把豆瓣的导演 / 演员 / 上映日期拼成一行（库里没有各自的列）。
///
/// `"上映：2021-10-22　导演：A　主演：B / C"`；三项都空时返回空串。
fn douban_crew_line(m: &DoubanMeta) -> String {
    let mut parts = Vec::new();
    if let Some(d) = m.release_date.as_deref().filter(|s| !s.trim().is_empty()) {
        parts.push(format!("上映：{d}"));
    }
    if let Some(d) = m.directors.as_deref().filter(|s| !s.trim().is_empty()) {
        parts.push(format!("导演：{d}"));
    }
    if let Some(a) = m.actors.as_deref().filter(|s| !s.trim().is_empty()) {
        parts.push(format!("主演：{a}"));
    }
    parts.join("　")
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
    let base = crate::core::parser::parse_file_name(file_name);
    let stem = file_name
        .rsplit_once('.')
        .map(|(s, _)| s)
        .unwrap_or(file_name);
    let junk = pending.title.trim().is_empty() || pending.title.trim() == stem.trim();
    if !junk && !crate::provider::llm::needs_enhancement(&base, file_name) {
        return None;
    }

    let (improved, fields) = crate::provider::llm::enhance(base, file_name, &p.llm, &p.hosts);
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

/// TMDb 完全没命中时的**豆瓣整条接管**。
///
/// 三种结果分别落在 `ScrapeOutcome` 上：
/// - **刮削成功**：`ok: true`，`message` 里注明来自豆瓣；
/// - **匹配失败**：`ok: false`，`message: "未在 TMDB / 豆瓣找到匹配条目"`；
/// - **网络异常**：`ok: false`，`message` 带出具体错误（不把 `Err` 抛给上层，
///   否则批量刮削会直接中断；单条刮削里用户也看不到是哪一步出了问题）。
fn douban_takeover(
    conn: &Connection,
    p: &Providers,
    tmdb_key: &str,
    id: i64,
    pending: PendingRow,
    refined: Option<(String, Option<i32>)>,
) -> Result<ScrapeOutcome, String> {
    // 大模型认出来的标题如果没有权威来源，也要留着，别让列表里一直显示乱码
    let llm_title = refined.as_ref().map(|(t, _)| t.clone());
    let llm_year = refined.as_ref().and_then(|(_, y)| *y);

    // 用「大模型修正后的标题」优先，否则用扫描时的标题
    let query = llm_title.clone().unwrap_or_else(|| pending.title.clone());
    let year = llm_year.or(pending.year);
    let media_type = pending.media_type.clone();

    // 没配 TMDb Key 时，豆瓣是**主力**而不是兜底，失败提示要换个说法
    let miss_message = if p.has_tmdb(tmdb_key) {
        "未在 TMDB 找到匹配条目".to_string()
    } else {
        "尚未配置 TMDB API Key，且未在豆瓣找到匹配条目".to_string()
    };

    let Some(client) = p.douban.as_ref() else {
        // 没启用豆瓣：保持原有行为（把大模型认出的标题写回去即可）
        persist_llm_hint(conn, id, &llm_title, llm_year);
        return Ok(ScrapeOutcome {
            ok: false,
            media_id: id,
            tmdb_id: None,
            title: None,
            episodes: 0,
            message: miss_message,
        });
    };

    match client.lookup(&query, year) {
        // ① 刮削成功
        Ok(Some(meta)) => {
            let fill = apply_douban_takeover(conn, id, &meta)?;
            // 剧集：豆瓣给了集数就当作提示（单集行的创建仍依赖 TMDb 的季集结构）
            let episodes = 0i64;
            let mut msg = "豆瓣刮削成功".to_string();
            if !fill.fields.is_empty() {
                msg.push_str(&format!("（写入：{}）", fill.fields.join("、")));
            }
            if media_type == "tv" {
                if let Some(n) = meta.episodes {
                    msg.push_str(&format!("，豆瓣标注共 {n} 集"));
                }
            }
            Ok(ScrapeOutcome {
                ok: true,
                media_id: id,
                tmdb_id: None,
                title: meta.title.clone(),
                episodes,
                message: msg,
            })
        }
        // ② 匹配失败：接口通了，但没有可用的候选
        Ok(None) => {
            persist_llm_hint(conn, id, &llm_title, llm_year);
            Ok(ScrapeOutcome {
                ok: false,
                media_id: id,
                tmdb_id: None,
                title: None,
                episodes: 0,
                message: format!("{miss_message}（检索词：{query}）"),
            })
        }
        // ③ 网络异常：超时 / DNS / 非 200 / 被限流
        Err(e) => {
            persist_llm_hint(conn, id, &llm_title, llm_year);
            Ok(ScrapeOutcome {
                ok: false,
                media_id: id,
                tmdb_id: None,
                title: None,
                episodes: 0,
                message: format!("豆瓣访问异常：{e}"),
            })
        }
    }
}

/// 把大模型认出来的标题/年份落库（TMDb 与豆瓣都没命中时的最后一点价值）。
fn persist_llm_hint(conn: &Connection, id: i64, title: &Option<String>, year: Option<i32>) {
    if let Some(t) = title.as_deref().filter(|t| !t.trim().is_empty()) {
        let _ = conn.execute(
            "UPDATE media SET title = ?2, year = COALESCE(year, ?3), updated_at = ?4 WHERE id = ?1",
            params![id, t, year, now_epoch()],
        );
    }
}

/// 刮削一条记录并回填。**纯后台逻辑**，调用方必须保证不在主线程上跑。
///
/// `tmdb_key` 只用于判断"有没有 TMDb 主源"（决定豆瓣是主力还是兜底、以及失败提示语），
/// 真正的 TMDb 客户端在 `p.tmdb` 里。
fn scrape_one(
    conn: &Connection,
    p: &Providers,
    tmdb_key: &str,
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
        // 连大模型也救不回来 → 交给豆瓣做**整条接管**兜底（若已启用）
        // 注意这里仍要带上 tmdb_key，用于判断"有没有 TMDb 主源"以决定提示语
        return douban_takeover(conn, p, tmdb_key, id, pending, refined);
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

    // 5) 豆瓣收尾：TMDb 已经给过的字段一律不动，只补它漏掉的中文信息
    let mut douban_extra: Vec<String> = Vec::new();
    if let Some(client) = p.douban.as_ref() {
        // 优先用 TMDb 的标题检索（比扫描标题干净），拿不到就退回扫描标题
        let query = nonempty_or(&meta.title, &pending.title);
        let year = meta.year.or(pending.year);
        match client.lookup(&query, year) {
            Ok(Some(dm)) => {
                let fill = apply_douban_fill(conn, id, &dm)?;
                douban_extra = fill.fields;
            }
            // 匹配失败 / 网络异常都不该影响已经成功的 TMDb 刮削，静默跳过
            Ok(None) | Err(_) => {}
        }
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
            if !douban_extra.is_empty() {
                msg.push_str(&format!("（豆瓣补全：{}）", douban_extra.join("、")));
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
    let (key, hosts, language, douban_on) = {
        let conn = state.lock().map_err(|e| e.to_string())?;
        (
            load_tmdb_key(&conn),
            load_hosts(&conn),
            load_tmdb_language(&conn),
            load_douban_config(&conn).0,
        )
    };
    // 只开豆瓣也能刮：此时豆瓣会走「整条接管」那条路。
    if key.trim().is_empty() && !douban_on {
        return Err(
            "尚未配置 TMDB API Key；如果只想用豆瓣刮削，请到「设置」打开豆瓣开关".to_string(),
        );
    }

    // 后台线程用自己的连接：WAL 模式下多连接并发读写是安全的。
    tauri::async_runtime::spawn_blocking(move || {
        let conn = crate::store::db::open(&app).map_err(|e| format!("打开数据库失败: {e}"))?;
        // 后台线程自己读设置装配 provider，不占主线程的连接锁
        let providers = Providers::load(&conn, &key, hosts, &language);
        scrape_one(&conn, &providers, &key, id, tmdb_id)
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

    // 单靠豆瓣也能批量跑（每条走「整条接管」）
    let douban_on = {
        let conn = state.lock().map_err(|e| e.to_string())?;
        load_douban_config(&conn).0
    };
    if key.trim().is_empty() && !douban_on {
        return Ok(ScrapeStart {
            started: false,
            total: targets.len() as i64,
            no_key: true,
            message: "尚未配置 TMDB API Key；如果只想用豆瓣刮削，请到「设置」打开豆瓣开关"
                .to_string(),
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

    let conn = match crate::store::db::open(&app) {
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
        match scrape_one(&conn, &providers, &key, id, None) {
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

/// 豆瓣查到的信息（设置页「测试」与详情页手动刮削的回执）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DoubanInfo {
    pub douban_id: String,
    pub title: String,
    pub original_title: String,
    pub year: Option<i32>,
    pub directors: String,
    pub actors: String,
    pub genre: String,
    pub release_date: String,
    pub rating: Option<f64>,
    pub overview: String,
    pub poster: String,
    /// 剧集集数（电影为 0）。
    pub episodes: i64,
}

impl DoubanInfo {
    fn from_meta(m: &DoubanMeta) -> Self {
        Self {
            douban_id: m.douban_id.clone(),
            title: m.title.clone().unwrap_or_default(),
            original_title: m.original_title.clone().unwrap_or_default(),
            year: m.year,
            directors: m.directors.clone().unwrap_or_default(),
            actors: m.actors.clone().unwrap_or_default(),
            genre: m.genre.clone().unwrap_or_default(),
            release_date: m.release_date.clone().unwrap_or_default(),
            rating: m.rating,
            overview: m.overview.clone().unwrap_or_default(),
            poster: m.poster.clone().unwrap_or_default(),
            episodes: m.episodes.unwrap_or(0),
        }
    }
}

/// 大模型接入预设：推荐的排在最前，其余是同协议的备选。
#[tauri::command]
pub fn llm_presets() -> Vec<LlmPreset> {
    // 推荐的文本模型与视觉模型不同源（DeepSeek / 百炼），所以两个地址都给全。
    let mut out = vec![LlmPreset {
        label: "推荐 · DeepSeek + 通义千问 VL".to_string(),
        base_url: crate::provider::llm::RECOMMENDED_BASE_URL.to_string(),
        model: crate::provider::llm::RECOMMENDED_MODEL.to_string(),
        vision_model: crate::provider::llm::RECOMMENDED_VISION_MODEL.to_string(),
        vision_base_url: crate::provider::llm::RECOMMENDED_VISION_BASE_URL.to_string(),
    }];
    for (label, base) in crate::provider::llm::ALTERNATIVE_MODELS {
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
) -> Result<crate::provider::llm::ParsedFields, String> {
    let (cfg, hosts) = {
        let conn = state.lock().map_err(|e| e.to_string())?;
        (load_llm_config(&conn), load_hosts(&conn))
    };
    if !cfg.usable() {
        return Err("大模型解析未启用或未填写接口地址".to_string());
    }
    tauri::async_runtime::spawn_blocking(move || {
        crate::provider::llm::LlmClient::new(cfg, &hosts).parse_name(&file_name)
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
) -> Result<crate::provider::llm::ParsedFields, String> {
    if !crate::provider::llm::is_undecidable(&file_name) {
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
        crate::provider::llm::LlmClient::new(cfg, &hosts).parse_image(&file_name, &image_data_url)
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
        let conn = crate::store::db::open(&app).map_err(|e| format!("打开数据库失败: {e}"))?;
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

/* --------------------------- 豆瓣入口 --------------------------- */

/// 搜索豆瓣候选（供详情页手动匹配 / 设置页试检索）。
///
/// 不返回一堆字段，只回 `DoubanInfo` 列表——用户要的是「挑一条」，
/// 完整元数据等确认后再拉。
#[tauri::command]
pub async fn search_douban(
    state: State<'_, Mutex<Connection>>,
    query: String,
) -> Result<Vec<DoubanInfo>, String> {
    let (enabled, cookie, hosts) = {
        let conn = state.lock().map_err(|e| e.to_string())?;
        let (enabled, cookie) = load_douban_config(&conn);
        (enabled, cookie, load_hosts(&conn))
    };
    if !enabled {
        return Err("豆瓣刮削未启用，请先到「设置」打开".to_string());
    }
    if query.trim().is_empty() {
        return Ok(Vec::new());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let client = DoubanClient::with_hosts(true, cookie, &hosts);
        let items = client.search(&query)?;
        // 只取前若干条候选，逐条拉详情（豆瓣限流不严但没必要时也不刷）
        let mut out = Vec::new();
        for it in items.iter().take(DOUBAN_SEARCH_LIMIT) {
            let Some(id) = it.usable_id() else { continue };
            let kind = it.normalized_kind().unwrap_or("movie");
            match client.detail(kind, id) {
                Ok(d) => out.push(DoubanInfo::from_meta(&DoubanMeta::from_detail(&d))),
                // 某一条拉不到详情就跳过，不要把整次搜索判成失败
                Err(_) => continue,
            }
        }
        Ok(out)
    })
    .await
    .map_err(|e| format!("豆瓣搜索任务异常退出: {e}"))?
}

/// 搜索时最多拉几条详情。
const DOUBAN_SEARCH_LIMIT: usize = 8;

/// 测试豆瓣连通性（设置页「测试连接」）：真实发一次检索 + 详情请求。
///
/// 三种结果明确区分：
/// - 成功：`Ok("连接正常，命中《沙丘》(2021) 评分 7.7")`
/// - 匹配失败：`Err("连接正常，但豆瓣没有「xxx」的匹配条目")`
/// - 网络异常：`Err("豆瓣访问异常：...")`
#[tauri::command]
pub async fn douban_test(
    state: State<'_, Mutex<Connection>>,
) -> Result<String, String> {
    let (enabled, cookie, hosts) = {
        let conn = state.lock().map_err(|e| e.to_string())?;
        let (enabled, cookie) = load_douban_config(&conn);
        (enabled, cookie, load_hosts(&conn))
    };
    if !enabled {
        return Err("豆瓣刮削未启用，请先打开开关".to_string());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let client = DoubanClient::with_hosts(true, cookie, &hosts);
        match client.lookup(DOUBAN_TEST_QUERY, None) {
            Ok(Some(m)) => {
                let mut s = format!(
                    "连接正常，命中《{}》",
                    m.title.as_deref().unwrap_or("(无标题)")
                );
                if let Some(y) = m.year {
                    s.push_str(&format!("({y})"));
                }
                match m.rating {
                    Some(r) => s.push_str(&format!(" 评分 {r}")),
                    None => s.push_str(" 暂无评分"),
                }
                Ok(s)
            }
            Ok(None) => Err(format!(
                "连接正常，但豆瓣没有「{DOUBAN_TEST_QUERY}」的匹配条目"
            )),
            Err(e) => Err(format!("豆瓣访问异常：{e}")),
        }
    })
    .await
    .map_err(|e| format!("豆瓣测试任务异常退出: {e}"))?
}

/// 连通性测试用的固定检索词（挑一部稳定存在的经典片）。
const DOUBAN_TEST_QUERY: &str = "沙丘";

/// 手动用豆瓣刮削指定条目（详情页入口）。
///
/// 语义与自动流程一致：TMDb 有结果时只补缺失，没有时整条接管。
#[tauri::command]
pub async fn douban_apply(
    app: AppHandle,
    state: State<'_, Mutex<Connection>>,
    media_id: i64,
    query: Option<String>,
    year: Option<i32>,
) -> Result<ScrapeOutcome, String> {
    let (enabled, cookie, hosts) = {
        let conn = state.lock().map_err(|e| e.to_string())?;
        let (enabled, cookie) = load_douban_config(&conn);
        (enabled, cookie, load_hosts(&conn))
    };
    if !enabled {
        return Err("豆瓣刮削未启用，请先到「设置」打开".to_string());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let conn = crate::store::db::open(&app).map_err(|e| format!("打开数据库失败: {e}"))?;
        let pending = read_pending(&conn, media_id)?;
        let q = query
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| pending.title.clone());
        let y = year.or(pending.year);

        let client = DoubanClient::with_hosts(true, cookie, &hosts);
        match client.lookup(&q, y) {
            // ① 刮削成功
            Ok(Some(m)) => {
                // 当前是否已有 TMDb 元数据：有 → 只补缺失；没有 → 整条接管
                let has_tmdb: Option<i64> = conn
                    .query_row(
                        "SELECT tmdb_id FROM media WHERE id = ?1",
                        params![media_id],
                        |r| r.get(0),
                    )
                    .unwrap_or(None);
                let fill = if has_tmdb.is_some() {
                    apply_douban_fill(&conn, media_id, &m)?
                } else {
                    apply_douban_takeover(&conn, media_id, &m)?
                };
                let msg = if fill.took_over {
                    format!("豆瓣刮削成功（写入：{}）", fill.fields.join("、"))
                } else if fill.fields.is_empty() {
                    "豆瓣刮削成功，但没有需要补充的字段".to_string()
                } else {
                    format!("豆瓣补全成功（补充：{}）", fill.fields.join("、"))
                };
                Ok(ScrapeOutcome {
                    ok: true,
                    media_id,
                    tmdb_id: has_tmdb,
                    title: m.title.clone(),
                    episodes: 0,
                    message: msg,
                })
            }
            // ② 匹配失败
            Ok(None) => Ok(ScrapeOutcome {
                ok: false,
                media_id,
                tmdb_id: None,
                title: None,
                episodes: 0,
                message: format!("未在豆瓣找到匹配条目（检索词：{q}）"),
            }),
            // ③ 网络异常
            Err(e) => Err(format!("豆瓣访问异常：{e}")),
        }
    })
    .await
    .map_err(|e| format!("豆瓣刮削任务异常退出: {e}"))?
}

#[cfg(test)]
#[path = "tests/scrape.rs"]
mod tests;
