//! 媒体库扫描：递归遍历媒体目录 → 解析文件名 → 入库（media / episodes / scan_history）。

use crate::store::db::now_epoch;
use crate::core::grouping::{self, plan_groups, GroupInput};
use crate::core::mediatype::{self, FolderEntry, FolderStat, MediaKind, Signals};
use crate::core::parser::{self, is_video_file, parse_file_name, ParsedName};
use crate::util;
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
    /// 证据不足、标成「待确认」的文件数（见 `mediatype` 的 Unknown 默认策略）。
    pub unknown: usize,
    pub added_unknown: usize,
    /// 已入库但**挂错父行**、本次改挂到目标剧集的单集数。
    ///
    /// 存量修复用：手建剧集早期接不进内容，文件都挂到了自动建的同名行上；
    /// 重新导入时光靠 `ON CONFLICT DO NOTHING` 是纠不回来的。
    pub repointed: usize,
    /// 清掉的「空壳」剧集行数（无单集、无元数据、非手建的重复行）。
    pub dropped_shells: usize,
}

/// 一个待入库的视频文件。
#[derive(Debug, Clone)]
pub struct ScanFile {
    pub path: PathBuf,
    pub name: String,
    pub size: u64,
    pub parsed: ParsedName,
    /// 类型判定结果。由 `classify_files` 填入，入库与分组都看它。
    pub kind: MediaKind,
    /// 所属媒体根目录的下标（跨根目录不做剧集合并）。
    pub root_index: usize,
    /// 媒体根目录下的第一层目录名；文件直接躺在根目录时为 None。
    pub folder: Option<String>,
    /// 剧集分组键（电影为 None）。由 `apply_group_plan` 二次校正后写入。
    pub group_key: Option<String>,
    /// 分辨率（`1080p` 等）：正则解析器不产出，只有大模型增强层会填。
    pub resolution: Option<String>,
    /// 发布来源 / 压制组：同上，来自大模型增强层。
    pub release_source: Option<String>,
}

/// 可选的大模型增强层：只对**正则认不出来**的脏文件名调用。
///
/// 调用失败 / 未启用时 `llm::enhance` 原样返回正则结果，所以这里永远安全。
/// 返回值是被真正改进过的文件数（用于扫描报告）。
pub fn apply_llm_enhancement(
    files: &mut [ScanFile],
    cfg: &crate::provider::llm::LlmConfig,
    hosts: &[String],
) -> usize {
    if !cfg.usable() {
        return 0;
    }
    let mut improved = 0usize;
    for f in files.iter_mut() {
        if !crate::provider::llm::needs_enhancement(&f.parsed, &f.name) {
            continue;
        }
        let before = f.parsed.clone();
        let (parsed, fields) =
            crate::provider::llm::enhance(f.parsed.clone(), &f.name, cfg, hosts);
        f.parsed = parsed;
        if let Some(x) = fields {
            f.resolution = x.resolution;
            f.release_source = x.release_source;
        }
        if f.parsed.title != before.title || f.parsed.season != before.season {
            improved += 1;
        }
    }
    improved
}

/// 标题：解析不出就用文件名兜底。
fn title_of(f: &ScanFile) -> String {
    if f.parsed.title.is_empty() {
        f.name.clone()
    } else {
        f.parsed.title.clone()
    }
}

/// 类型判定：先统计每个目录的剧集证据，再逐个文件打分。
///
/// 必须先做这一步再入库——剧集/电影的分流、以及剧集分组都依赖判定结果。
/// 判定规则集中在 `mediatype`，这里只负责**喂信号**。
pub fn classify_files(files: &mut [ScanFile]) {
    // 1) 按目录汇总「剧集证据」（目录共识是找回无标记剧集文件的关键信号）
    let mut by_dir: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (i, f) in files.iter().enumerate() {
        by_dir.entry(util::parent_dir(&f.path)).or_default().push(i);
    }
    let mut stats: BTreeMap<String, FolderStat> = BTreeMap::new();
    for (dir, idxs) in by_dir.iter() {
        let entries: Vec<FolderEntry> = idxs
            .iter()
            .map(|&i| FolderEntry {
                file_name: &files[i].name,
                parsed: &files[i].parsed,
            })
            .collect();
        stats.insert(dir.clone(), mediatype::build_folder_stats(&entries));
    }

    // 2) 逐个判定
    let cfg = mediatype::config();
    for f in files.iter_mut() {
        let parent_name = util::dir_name(&f.path);
        let st = stats.get(&util::parent_dir(&f.path));
        let empty = FolderStat::default();
        let st = st.unwrap_or(&empty);
        let mut sig = Signals::from_parsed(&f.parsed, &f.name);
        sig.folder = f.folder.as_deref();
        sig.parent = parent_name.as_deref();
        sig.folder_total = st.total;
        sig.folder_tv_count = st.tv_count;
        sig.folder_numbered_count = st.numbered_count;
        sig.folder_compilation_count = st.compilation_count;
        sig.folder_consensus_title = st.consensus_title.as_deref();
        sig.folder_number_prefix = st.numbered_prefix.as_deref();
        f.kind = mediatype::classify(&sig, &cfg).kind;

        // 判成剧集后再补齐编号（见 `mediatype::fill_episode_numbers`）。
        if f.kind == MediaKind::Tv {
            mediatype::fill_episode_numbers(&mut f.parsed, &f.name, parent_name.as_deref());
        }
    }
}

/// 二次校正：把「按目录归集」的结果再过一遍 `grouping::plan_groups`。
///
/// 只处理剧集（电影一条一个实体，不需要归组）。组键由校正结果给出，
/// 因此跨目录的同一部剧会被并到一起，而重名但季集冲突的不会被误并。
pub fn apply_group_plan(files: &mut [ScanFile]) {
    let tv: Vec<usize> = (0..files.len())
        .filter(|&i| files[i].kind == MediaKind::Tv)
        .collect();
    if tv.is_empty() {
        return;
    }
    let inputs: Vec<GroupInput> = tv
        .iter()
        .map(|&i| {
            let f = &files[i];
            let title = title_of(f);
            // 「真的解析出了编号」：有明确标记，或标题尾部就带着编号。
            // 只有 `fill_episode_numbers` 补出来的占位值（第 1 集）不算证据。
            let explicit = f.parsed.episode_marker
                || f.parsed.season_marker
                || mediatype::trailing_number(&title).is_some();
            GroupInput {
                root: f.root_index,
                folder: f.folder.clone().unwrap_or_default(),
                parent: util::dir_name(&f.path),
                title,
                year: f.parsed.year,
                season: f.parsed.season,
                episode: f.parsed.episode,
                explicit,
            }
        })
        .collect();

    let plan = plan_groups(&inputs);
    for (k, &i) in tv.iter().enumerate() {
        files[i].group_key = Some(plan.keys[plan.assign[k]].clone());
        // 组内连续化之后的季 / 集号写回解析结果，入库时用这一份
        let (season, episode) = plan.episodes[k];
        files[i].parsed.season = Some(season);
        files[i].parsed.episode = Some(episode);
    }
}

/// 需要跳过的系统/隐藏目录。
fn is_skippable_dir(name: &str) -> bool {
    matches!(
        name,
        "$RECYCLE.BIN" | "System Volume Information" | "RECYCLER" | "lost+found"
    ) || name.starts_with('.')
}

/// 找出代表**一部作品**的目录（作品目录 / 剧集根目录），用作剧集分组单位。
///
/// 这是「文件夹优先」的关键：先认出作品，再把目录下的季、集、特辑都归到它
/// 名下，而不是逐个文件各自归类。挑选顺序：
///
/// 1. 相对根目录的**第一层目录**，只要它像个作品名就用它
///    （`D:\电视剧\神秘博士\Doctor.Who.2005.S09...\x.mkv` → `神秘博士`）；
/// 2. 第一层不像（`Season 1` / `S01` 这种纯结构名）→ 说明用户是把**作品目录
///    本身**当扫描根的（`D:\电视剧\Legal High\S01\x.mkv`），这时用扫描根的
///    目录名（`Legal High`）；
/// 3. 两者都不像 → 退回第一层，至少保持「同一目录同一批」的原有行为。
pub fn work_dir_of(root: &Path, file: &Path) -> Option<String> {
    let rel = file.strip_prefix(root).ok()?;
    let comps: Vec<&std::ffi::OsStr> = rel
        .components()
        .filter_map(|c| match c {
            std::path::Component::Normal(s) => Some(s),
            _ => None,
        })
        .collect();
    if comps.len() < 2 {
        return None; // 文件直接躺在根目录下，目录树里没有作品信息
    }
    let first = comps[0].to_string_lossy().to_string();
    if grouping::looks_like_work_name(&first) {
        return Some(first);
    }
    let own = root
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    if grouping::looks_like_work_name(&own) {
        Some(own)
    } else {
        Some(first)
    }
}

/// 扫描的两个阶段（进度回报用）。
pub const PHASE_COLLECT: &str = "collect";
pub const PHASE_IMPORT: &str = "import";

/// 阶段进度的回报间隔：每积累这么多条回调一次。
///
/// 遍历一个大型媒体库会碰到几万个文件，逐个回调既拖慢扫描（回调里要发事件）
/// 又把界面刷爆，所以按条数节流。
const REPORT_EVERY: usize = 25;

/// 进度回报回调：`(阶段, 已处理条数, 当前路径/文件名)`。
///
/// 返回 `false` 表示「中止扫描」（用户在界面上点了取消）；中止时已收集到的
/// 部分仍会返回，由调用方决定要不要入库。
pub type ScanTick<'a> = &'a mut dyn FnMut(&'static str, usize, &str) -> bool;

/// 递归收集并解析视频文件（不做任何数据库操作）。
///
/// 命令侧走带进度回报的 `collect_files_reported`，这个无回报的入口留给单测
/// 和将来可能的脚本化调用。
#[allow(dead_code)]
pub fn collect_files(roots: &[String]) -> Vec<ScanFile> {
    collect_files_reported(roots, &mut |_, _, _| true)
}

/// 同 `collect_files`，但沿途回报进度；回调返回 `false` 时提前收工。
pub fn collect_files_reported(roots: &[String], tick: ScanTick) -> Vec<ScanFile> {
    let mut files = Vec::new();
    let mut reported_at = 0usize;

    for (root_index, root) in roots.iter().enumerate() {
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
            let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
            // 扩展名认得出来就直接过；认不出来（没有扩展名、或被改名过的文件）
            // 才读文件头兜底。多数 junk 文件体积不到 512 KB，会被第一步挡掉，
            // 所以真正读到盘的只有极少数异常大文件。
            if !is_video_file(&name) && !parser::looks_like_video_file(entry.path(), size) {
                continue;
            }
            let parsed = parse_file_name(&name);
            // 先认「作品目录」，再以它为聚合单位（见 `work_dir_of`）
            let folder = work_dir_of(root_path, entry.path());
            files.push(ScanFile {
                path: entry.path().to_path_buf(),
                name,
                size,
                parsed,
                root_index,
                folder,
                group_key: None,
                resolution: None,
                release_source: None,
                // 先标成「待确认」，`classify_files` 之后才有真正的类型。
                kind: MediaKind::Unknown,
            });

            // 节流后回报：回调里发事件有开销，不能每个文件都发。
            if files.len() - reported_at >= REPORT_EVERY {
                reported_at = files.len();
                let dir = util::parent_dir(entry.path());
                if !tick(PHASE_COLLECT, files.len(), &dir) {
                    return files;
                }
            }
        }
    }

    // 收集完之后统一做类型判定 + 二次校正：目录归集是 L0，这里补上
    // 「同目录拆分 / 兄弟目录合并」。判定必须在校正**之前**——校正只处理剧集。
    // 取消时提前返回的部分不校正——它们不入库，组键没有意义。
    classify_files(&mut files);
    apply_group_plan(&mut files);
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

/// 手建剧集的查找（**归一化标题**匹配）。
///
/// 为什么手建行不能靠 `group_key` 找：`group_key` 是**目录名派生**的，而手建剧集
/// 没有目录，两者天然对不上。所以手建行只能靠标题匹配。
///
/// 为什么不能逐字比较标题：扫描推出来的标题经常多带一段没被识别成集号的数字
/// （`西游记.1986.01.mkv` → `西游记 01`，因为 `01` 这种短编号被有意排除在
/// 「集号」之外，以免把 `Rocky 2` 当成剧集）。逐字比较会让手建条目**永远**
/// 接不进内容 —— 这正是「新建了《西游记》却始终导入不进去」的根因。
/// 用 `normalize_title` 抹掉尾部编号后比较，两种写法就能对上。
///
/// 年份守卫：两边都有年份且不一致时**不匹配**，避免把不同年份的两版并到一起
/// （用户没填年份是常态，所以只有双方都有值时才校验）。
///
/// 命中范围是「用户拥有」的行：`source = 'manual'`（手建）**或** `locked = 1`
/// （用户编辑过）。只看 `source` 不够——刮削可能改写 `source` 的取值，
/// 用户手动改过名的自动行也应该继续接住后续导入。
fn find_manual_series(
    conn: &Connection,
    title: &str,
    year: Option<i32>,
) -> Result<Option<(i64, bool)>, String> {
    let want = grouping::normalize_title(title);
    if want.is_empty() {
        return Ok(None);
    }
    let mut stmt = conn
        .prepare(
            "SELECT id, title, original_title, group_key IS NULL, year FROM media \
             WHERE type = 'tv' \
               AND (source = 'manual' OR COALESCE(locked, 0) = 1) ORDER BY id",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, bool>(3)?,
                r.get::<_, Option<i32>>(4)?,
            ))
        })
        .map_err(|e| e.to_string())?;

    for row in rows {
        let (id, name, orig, no_key, my_year) = row.map_err(|e| e.to_string())?;
        let hit = grouping::normalize_title(&name) == want
            || orig
                .as_deref()
                .map(|o| grouping::normalize_title(o) == want)
                .unwrap_or(false);
        if !hit {
            continue;
        }
        if let (Some(a), Some(b)) = (my_year, year) {
            if a != b {
                continue;
            }
        }
        return Ok(Some((id, no_key)));
    }
    Ok(None)
}

/// 剧集分组查找。
///
/// 匹配优先级（从高到低）：
/// 1. **手建剧集**（`source = 'manual'`）按归一化标题命中 —— 用户亲手建的条目
///    就是他想要的目标，优先级最高；
/// 2. `group_key` 精确命中（同一目录下的同一部剧）；
/// 3. 退回标题逐字相等，**仅匹配 `group_key` 仍为空的老数据**——
///    这样既能兼容升级前建的行，又不会把两部同名但不同目录的剧集并成一条。
///
/// 返回值第二项表示是否需要把 `group_key` 补上（自愈）。
fn find_series_id(
    conn: &Connection,
    group_key: &str,
    title: &str,
    year: Option<i32>,
) -> Result<Option<(i64, bool)>, String> {
    // 1) 手建剧集优先
    if let Some(hit) = find_manual_series(conn, title, year)? {
        return Ok(Some(hit));
    }
    // 2) group_key 精确命中 / 3) 老数据的标题兜底
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

/// 入库一条「单文件」资源（电影，或判定不出来的「待确认」条目）。
///
/// 两者共用同一套字段：`unknown` 只是 `type` 不同，界面上单独列出来供用户
/// 重新识别或手动改类型，不会因为判不出来就被塞进「电影」。
fn insert_single(conn: &Connection, kind: &str, f: &ScanFile, ts: i64) -> Result<bool, String> {
    let path = f.path.to_string_lossy().to_string();
    let title = if f.parsed.title.is_empty() {
        f.name.clone()
    } else {
        f.parsed.title.clone()
    };
    let n = conn
        .execute(
            "INSERT INTO media (type, title, original_title, year, tmdb_id, file_path, file_name, \
                    file_size, added_at, updated_at, resolution, release_source) \
             VALUES (?1, ?2, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8, ?9, ?10) \
             ON CONFLICT(file_path) DO NOTHING",
            params![
                kind, title, f.parsed.year, f.parsed.tmdb_id, path, f.name, f.size as i64, ts,
                f.resolution, f.release_source,
            ],
        )
        .map_err(|e| e.to_string())?;
    Ok(n > 0)
}

pub(crate) fn upsert_series(
    conn: &Connection,
    group_key: &str,
    title: &str,
    year: Option<i32>,
    ts: i64,
) -> Result<(i64, bool), String> {
    if let Some((id, needs_heal)) = find_series_id(conn, group_key, title, year)? {
        if needs_heal {
            // 没有分组键的行（手建的新条目 / 老库升级上来的行）顺手补上（自愈）
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

pub(crate) fn insert_episode(
    conn: &Connection,
    media_id: i64,
    f: &ScanFile,
    ts: i64,
) -> Result<bool, String> {
    let path = f.path.to_string_lossy().to_string();
    let title = if f.parsed.title.is_empty() {
        f.name.clone()
    } else {
        f.parsed.title.clone()
    };
    let n = conn
        .execute(
            "INSERT INTO episodes (media_id, season, episode, title, file_path, file_name, \
                    file_size, added_at, resolution, release_source) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10) \
             ON CONFLICT(file_path) DO NOTHING",
            params![
                media_id,
                f.parsed.season.unwrap_or(1),
                f.parsed.episode.unwrap_or(1),
                title,
                path,
                f.name,
                f.size as i64,
                ts,
                f.resolution,
                f.release_source,
            ],
        )
        .map_err(|e| e.to_string())?;
    Ok(n > 0)
}

/// 这一行是不是用户手建的剧集（`source = 'manual'`）。
fn is_manual_series(conn: &Connection, media_id: i64) -> Result<bool, String> {
    let src: Option<String> = conn
        .query_row(
            "SELECT source FROM media WHERE id = ?1",
            params![media_id],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?
        .flatten();
    Ok(src.as_deref() == Some("manual"))
}

/// 把已入库、但挂在**别的**剧集下的单集改挂到目标剧集。
///
/// 这是「手建剧集接不进内容」那个历史 bug 的**存量修复**：文件早就以错误的父行
/// 入库了，重新导入时 `ON CONFLICT(file_path) DO NOTHING` 只会跳过，
/// 父行永远纠正不过来。补上这一句，用户不必删库重扫。
///
/// 只在目标行是**手建剧集**时才调用——那是用户明确指定的归属，改挂不会有歧义；
/// 自动分组的结果一律不动，避免把正常条目搬来搬去。
fn repoint_episode(conn: &Connection, media_id: i64, path: &str) -> Result<bool, String> {
    let n = conn
        .execute(
            "UPDATE episodes SET media_id = ?2 WHERE file_path = ?1 AND media_id <> ?2",
            params![path, media_id],
        )
        .map_err(|e| e.to_string())?;
    Ok(n > 0)
}

/// 清掉「空壳」重复剧集行。
///
/// 历史 bug 会在手建剧集之外多造一条同名行；单集改挂回去之后，那条行只剩空壳。
/// 只在**能证明它是空壳**时才删：非手建、未锁定、没有任何元数据、名下一条单集
/// 都没有。任何一样不满足就原样保留（宁可留个多余条目，也不能误删用户的数据）。
pub(crate) fn drop_empty_series_shells(conn: &Connection, group_key: &str) -> Result<usize, String> {
    let ids: Vec<i64> = {
        let mut stmt = conn
            .prepare(
                "SELECT id FROM media WHERE type = 'tv' AND group_key = ?1 \
                   AND COALESCE(source, '') <> 'manual' AND COALESCE(locked, 0) = 0 \
                   AND tmdb_id IS NULL AND tvdb_id IS NULL \
                   AND COALESCE(poster_path, '') = '' AND COALESCE(backdrop_path, '') = '' \
                   AND COALESCE(overview, '') = '' \
                   AND NOT EXISTS (SELECT 1 FROM episodes e WHERE e.media_id = media.id)",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![group_key], |r| r.get::<_, i64>(0))
            .map_err(|e| e.to_string())?;
        rows.filter_map(Result::ok).collect()
    };
    for id in &ids {
        conn.execute("DELETE FROM media WHERE id = ?1", params![id])
            .map_err(|e| e.to_string())?;
    }
    Ok(ids.len())
}
///
/// 命令侧走带进度回报的 `import_reported`，这个无回报的入口留给单测。
#[allow(dead_code)]
pub fn import(conn: &mut Connection, files: &[ScanFile], roots: &[String]) -> Result<ScanStats, String> {
    import_reported(conn, files, roots, &mut |_, _, _| true)
}

/// 同 `import`，但沿途回报进度（入库阶段不可取消：整批在一个事务里，
/// 中途停下会留下半套数据；这一阶段通常只有几秒）。
pub fn import_reported(
    conn: &mut Connection,
    files: &[ScanFile],
    roots: &[String],
    tick: ScanTick,
) -> Result<ScanStats, String> {
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

    // 可选增强层：把脏文件名交给大模型翻一遍（未启用 / 调用失败都是空操作）。
    // 放在开事务**之前**：网络请求不能拿着写事务等，否则整库被锁住几十秒。
    let llm_cfg = crate::commands::settings::load_llm_config(conn);
    let hosts = crate::commands::settings::load_hosts(conn);
    let mut prepared: Vec<ScanFile> = files.to_vec();
    if llm_cfg.usable() {
        apply_llm_enhancement(&mut prepared, &llm_cfg, &hosts);
    }
    let files: &[ScanFile] = &prepared;

    let tx = conn.transaction().map_err(|e| e.to_string())?;

    // 剧集按（标题）聚合：BTreeMap 保证处理顺序稳定
    let mut series_cache: BTreeMap<String, i64> = BTreeMap::new();
    // 发生过「单集改挂」的分组键：只有这些分组才需要清理空壳行。
    let mut healed_groups: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();

    let mut processed = 0usize;
    let mut reported_at = 0usize;

    for f in files {
        processed += 1;
        if processed - reported_at >= REPORT_EVERY {
            reported_at = processed;
            let _ = tick(PHASE_IMPORT, processed, &f.name);
        }

        match f.kind {
            MediaKind::Tv => {
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
                    // 文件已入库但**挂在别的剧集下**：如果目标行是用户手建的条目，
                    // 说明这是「手建剧集接不进内容」的存量数据，改挂过来。
                    // （自动分组的结果一律不动，避免把正常条目搬来搬去。）
                    if is_manual_series(&tx, media_id).unwrap_or(false) {
                        let path = f.path.to_string_lossy().to_string();
                        if repoint_episode(&tx, media_id, &path).unwrap_or(false) {
                            stats.repointed += 1;
                            healed_groups.insert(group_key.clone());
                        }
                    }
                }
                Err(_) => stats.failed += 1,
            }
            }
            MediaKind::Movie | MediaKind::Unknown => {
                let is_movie = f.kind == MediaKind::Movie;
                match insert_single(&tx, f.kind.as_str(), f, ts) {
                    Ok(true) => {
                        if is_movie {
                            stats.movies += 1;
                            stats.added_movies += 1;
                        } else {
                            stats.unknown += 1;
                            stats.added_unknown += 1;
                        }
                    }
                    Ok(false) => {
                        if is_movie {
                            stats.movies += 1;
                        } else {
                            stats.unknown += 1;
                        }
                        stats.skipped += 1;
                    }
                    Err(_) => stats.failed += 1,
                }
            }
        }
    }

    // 单集改挂完成后，那些被搬空的重复剧集行只剩空壳，清掉（严格证明才删）
    for gk in &healed_groups {
        stats.dropped_shells += drop_empty_series_shells(&tx, gk)?;
    }
    stats.series = stats.series.saturating_sub(stats.dropped_shells);

    tx.commit().map_err(|e| e.to_string())?;

    conn.execute(
        "UPDATE scan_history SET finished_at = ?1, status = 'done', found = ?2, added = ?3 WHERE id = ?4",
        params![
            now_epoch(),
            stats.scanned as i64,
            (stats.added_movies + stats.added_episodes + stats.added_unknown) as i64,
            history_id
        ],
    )
    .map_err(|e| e.to_string())?;

    Ok(stats)
}

/// 一步到位：扫描 + 入库。
///
/// 命令侧已改成后台任务（`commands::media::run_scan`），这里保留一步到位的
/// 同步形式供单测使用。
#[allow(dead_code)]
pub fn scan_and_import(conn: &mut Connection, roots: &[String]) -> Result<ScanStats, String> {
    if roots.is_empty() {
        return Err("尚未配置媒体目录，请先在设置中添加".to_string());
    }
    let files = collect_files(roots);
    import(conn, &files, roots)
}

#[cfg(test)]
#[path = "tests/scanner.rs"]
mod tests;
