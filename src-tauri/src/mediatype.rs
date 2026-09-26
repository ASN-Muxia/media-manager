//! 媒体类型判定（**集中 + 可配置**）。
//!
//! ## 为什么要有这个模块
//!
//! 之前的判定只有一条规则：「文件名里带季号或集号就是剧集，否则是电影」。
//! 这一条规则把大量剧集误判成了电影——因为很多剧集文件**根本不带可解析的
//! 季集标记**：
//!
//! - `剧名 01.mkv`（空格分隔，正则认不出）
//! - `剧名/01.mkv`（信息只写在目录里）
//! - `[字幕组] 剧名 - 01v2 [1080P].mkv`（编号后面带了版本号）
//! - `剧名.S01.01.mkv`（季集之间用了点）
//!
//! 这些文件全部落进了「电影」。反过来，带 `剧场版` / `电影版` 的动画剧场版
//! 又常常被当成剧集。单一规则两头不讨好，所以这里改成**多信号打分**。
//!
//! ## 判定流程（按顺序执行，规则集中在本文件）
//!
//! | 规则 | 信号 | 归属 |
//! |---|---|---|
//! | R1 | 数据源（TMDB / TVDB）明确给出的类型标识 | 直接覆盖结论 |
//! | R2 | 合集 / 合辑 / 全集（一个文件就是一整部） | → 电影 |
//! | R3 | 特别篇 / OVA / OAD / SP | + 剧集（季号按 0 存） |
//! | R4 | 明确集号（`S01E01` / `1x02` / `第01集` / `第01话` / `EP01` / `- 01`） | + 剧集 |
//! | R5 | 明确季号（`第2季` / `Season 2` / `S01`） | + 剧集 |
//! | R6 | 迷你剧 / 限定剧 / 短剧 | + 剧集 |
//! | R7 | 目录共识：同目录 ≥ 2 个文件带集号，且本文件属于同一部剧 | + 剧集 |
//! | R8 | 编号共识：同目录有一串同前缀编号文件，且本文件没有年份 | + 剧集 |
//! | R9 | 所在目录名带季标记（`Season 2` / `第二季`） | + 剧集 |
//! | R10 | 剧场版 / 电影版 / Theatrical | + 电影 |
//! | R11 | 单集时长（默认 8~70 分钟）/ 电影时长（≥ 100 分钟） | 各自加分 |
//! | R12 | 有上映年份且**完全没有**剧集信号 | + 电影 |
//!
//! 最后比较两边得分：剧集分 ≥ `tv_threshold` 且不低于电影分 → 剧集；
//! 电影分 ≥ `movie_threshold` 且更高 → 电影；
//! **两边都不够 → 走 `unknown_policy`（可枚举的默认处理）**。
//!
//! 默认的 `unknown_policy` 是 `Unknown`：宁可把条目标成「待确认」交给用户或
//! 后续刮削去定，也不默默塞进「电影」——这正是老逻辑最大的问题。
//!
//! ## 边界情况的处理约定（都是有意为之，不是巧合）
//!
//! - **迷你剧**：`迷你剧 / 限定剧 / miniseries` 标记按剧集处理（R6）；没有标记
//!   但带集号的照常走 R4。
//! - **特别篇 / OVA**：属于剧集体系，季号记 0（见 `scanner::classify_files`），
//!   不会变成一部「电影」。
//! - **动画**：`第01话` 有集号标记 → 剧集；`剧场版` 有电影标记 → 电影。
//! - **纪录片剧集**：通常按季分目录（目录名带季标记 → R9）或带集号（R4）。
//! - **合集文件**：`合集 / 合辑 / 全集 / 全12集` 一个文件就是一条资源 → 电影（R2）。
//! - **元数据缺失**：时长 `None`、目录里只有它一个、又没有年份 → 证据不足 →
//!   按 `unknown_policy` 处理，绝不静默归电影。
//!
//! ## 可配置
//!
//! 全部阈值与权重都在 [`TypeConfig`] 里；运行时可用 [`configure`] 整体替换，
//! 判定函数一律接受 `&TypeConfig`，所以单测可以为每种边界情况配一套参数。

use crate::grouping::{normalize_title, similarity};
use crate::parser::ParsedName;
use regex::Regex;
use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

/* ------------------------------ 结论类型 ------------------------------ */

/// 媒体类型。**`Unknown` 是一等公民**，不是「出错了」也不是「默认电影」。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MediaKind {
    Movie,
    Tv,
    /// 证据不足，无法明确判定。界面上显示为「待确认」，可重新识别或手动改。
    #[default]
    Unknown,
}

impl MediaKind {
    /// 写进 `media.type` 的值。
    pub fn as_str(&self) -> &'static str {
        match self {
            MediaKind::Movie => "movie",
            MediaKind::Tv => "tv",
            MediaKind::Unknown => "unknown",
        }
    }

    /// 界面上的中文名。
    // 前端目前自己维护中文名映射；这里保留同一份定义，避免两边说法不一致时无从对照。
    #[allow(dead_code)]
    pub fn label(&self) -> &'static str {
        match self {
            MediaKind::Movie => "电影",
            MediaKind::Tv => "剧集",
            MediaKind::Unknown => "待确认",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s {
            "tv" => MediaKind::Tv,
            "movie" => MediaKind::Movie,
            _ => MediaKind::Unknown,
        }
    }
}

/// 数据源给出的类型标识（TMDB 的 `media_type` / TVDB 的剧集等）。
///
/// 这是**权威信号**：用户已经手动选过一次，或元数据 API 明确告诉过我们，
/// 就没必要再猜。
///
/// 目前由 `scrape` 层在应用 TMDB 匹配结果时传入（R1 优先级最高）。
// 变体还没全部被命令层用到，但这是判定规则的**对外可配置项**，不是死代码。
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceHint {
    Movie,
    Tv,
}

impl SourceHint {
    /// 从数据源返回的 `media_type` 字符串解析（认不出来就返回 `None`，不猜）。
    #[allow(dead_code)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "movie" | "film" => Some(SourceHint::Movie),
            "tv" | "series" | "show" => Some(SourceHint::Tv),
            _ => None,
        }
    }
}

/// 无法明确判定时的默认处理（**可枚举**，不是静默归类）。
// 同上：三种策略都要留在代码里，改默认行为就是改 `TypeConfig::default()` 一行。
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnknownPolicy {
    /// 标成待确认，等用户或后续刮削来定（默认）。
    Unknown,
    /// 一律按电影入库（老的兼容行为）。
    Movie,
    /// 一律按剧集入库。
    Tv,
}

/* ------------------------------ 配置 ------------------------------ */

/// 各条信号的权重。
#[derive(Debug, Clone, PartialEq)]
pub struct Weights {
    /// 明确集号（R4）
    pub episode_marker: i32,
    /// 明确季号（R5）
    pub season_marker: i32,
    /// 特别篇 / OVA（R3）
    pub special: i32,
    /// 迷你剧 / 限定剧（R6）
    pub tv_word: i32,
    /// 目录共识（R7）
    pub folder_consensus: i32,
    /// 编号共识（R8）
    pub numbered_consensus: i32,
    /// 目录名带季标记（R9）
    pub season_folder: i32,
    /// 单集时长（R11）
    pub duration_episode: i32,
    /// 剧场版 / 电影版（R10）
    pub movie_word: i32,
    /// 电影时长（R11）
    pub duration_movie: i32,
    /// 有年份且完全没有剧集信号（R12）
    pub year_no_tv: i32,
    /// 数据源标识（R1）
    pub source_hint: i32,
}

impl Default for Weights {
    fn default() -> Self {
        Self {
            episode_marker: 4,
            season_marker: 3,
            special: 3,
            tv_word: 3,
            folder_consensus: 3,
            numbered_consensus: 3,
            season_folder: 3,
            duration_episode: 2,
            movie_word: 3,
            duration_movie: 2,
            year_no_tv: 2,
            source_hint: 4,
        }
    }
}

/// 判定规则的全部可调参数。
#[derive(Debug, Clone, PartialEq)]
pub struct TypeConfig {
    /// 判为剧集所需的最低剧集分。
    pub tv_threshold: i32,
    /// 判为电影所需的最低电影分。
    pub movie_threshold: i32,
    /// 单集时长的典型区间（分钟）。
    pub episode_minutes: (i64, i64),
    /// 达到这个时长（分钟）算电影时长。
    pub movie_minutes: i64,
    /// R7：同目录至少要有这么多个**带集号**的文件，才认定「这是个剧集目录」。
    pub folder_consensus_min: usize,
    /// R8：纯编号文件（`01.mkv`）的编号共识门槛——电影几乎不会叫 `01.mkv`，
    /// 所以这个门槛很低。
    pub pure_number_min: usize,
    /// R8：带前缀的编号文件（`剧名 01.mkv`）的门槛——`Rocky 1/2/3` 这种
    /// 系列电影也是这个形状，所以门槛要比纯编号高一些。
    /// 零填充编号（`剧名 001`）不走这个门槛（见 `matches_numbered_consensus`）。
    pub prefix_number_min: usize,
    /// 标题相似度阈值（目录共识 / 前缀比对都用它）。
    pub consensus_similarity: f64,
    pub weights: Weights,
    /// 证据不足时的默认处理。
    pub unknown_policy: UnknownPolicy,
}

impl Default for TypeConfig {
    fn default() -> Self {
        Self {
            tv_threshold: 3,
            movie_threshold: 2,
            episode_minutes: (8, 70),
            movie_minutes: 100,
            folder_consensus_min: 2,
            pure_number_min: 2,
            prefix_number_min: 3,
            consensus_similarity: 0.9,
            weights: Weights::default(),
            unknown_policy: UnknownPolicy::Unknown,
        }
    }
}

static CONFIG: OnceLock<Mutex<TypeConfig>> = OnceLock::new();

/// 当前生效的配置（默认配置的副本）。
pub fn config() -> TypeConfig {
    CONFIG
        .get_or_init(|| Mutex::new(TypeConfig::default()))
        .lock()
        .map(|g| g.clone())
        .unwrap_or_default()
}

/// 整体替换判定配置（规则集中在这里，改一处即可全库生效）。
///
/// 现在还没有界面开关：想调阈值就改 `TypeConfig::default()`，
/// 或在这里接一个设置项。留着入口是为了不让规则散落回各个调用点。
#[allow(dead_code)]
pub fn configure(cfg: TypeConfig) {
    if let Ok(mut g) = CONFIG.get_or_init(|| Mutex::new(TypeConfig::default())).lock() {
        *g = cfg;
    }
}

/* ------------------------------ 信号 ------------------------------ */

/// 判定所需的全部信号。**每一条都可以缺席**，缺席即视为「没有这条证据」。
#[derive(Debug, Clone, Default)]
pub struct Signals<'a> {
    pub file_name: &'a str,
    pub title: &'a str,
    /// 文件名里解析出的年份。
    pub year: Option<i32>,
    /// 库里**已经存着的**年份（刮削写入的）。
    ///
    /// 只用于「有年份且无剧集特征 → 电影」这条（R12），**不参与**编号共识的
    /// 排除条件（R8）：老数据里那些被误判成电影的单集，刮削很可能已经给它补了
    /// 一个上映年份，若拿它去卡 R8，整季就再也翻不回剧集了。
    pub stored_year: Option<i32>,
    /// 集号来自明确标记（`S01E01` / `第01集` / `EP01` / `1x02` / `- 01`）。
    pub episode_marker: bool,
    /// 季号来自明确标记（`第2季` / `Season 2` / `S01`）。
    pub season_marker: bool,
    /// 单集时长（秒）。没探测过就是 `None`。
    pub duration_secs: Option<i64>,
    /// 一级目录名（媒体根目录下的第一层）。
    pub folder: Option<&'a str>,
    /// 文件所在目录名。
    pub parent: Option<&'a str>,
    pub folder_total: usize,
    /// 同目录中带明确集号标记的文件数。
    pub folder_tv_count: usize,
    /// 同目录中「标题以编号结尾但没有标记」的文件数。
    pub folder_numbered_count: usize,
    /// 同目录带集号文件的众数标题。
    pub folder_consensus_title: Option<&'a str>,
    /// 同目录编号文件的公共前缀（去掉尾部编号后）。
    pub folder_number_prefix: Option<&'a str>,
    pub source_hint: Option<SourceHint>,
}

impl<'a> Signals<'a> {
    /// 从解析结果填最常用的那几个字段（目录类信号另填）。
    pub fn from_parsed(parsed: &'a ParsedName, file_name: &'a str) -> Self {
        Self {
            file_name,
            title: &parsed.title,
            year: parsed.year,
            episode_marker: parsed.episode_marker,
            season_marker: parsed.season_marker,
            ..Default::default()
        }
    }
}

/// 判定结论（带分数与理由，便于排查「为什么判成这样」）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verdict {
    pub kind: MediaKind,
    pub tv_score: i32,
    pub movie_score: i32,
    pub reasons: Vec<&'static str>,
}

impl Verdict {
    fn new(kind: MediaKind, tv: i32, movie: i32, reasons: &[&'static str]) -> Self {
        Self {
            kind,
            tv_score: tv,
            movie_score: movie,
            reasons: reasons.to_vec(),
        }
    }
}

/* ------------------------------ 标记识别 ------------------------------ */

macro_rules! lazy_re {
    ($fn_name:ident, $pat:expr) => {
        fn $fn_name() -> &'static Regex {
            static R: OnceLock<Regex> = OnceLock::new();
            R.get_or_init(|| Regex::new($pat).expect("内置正则应可编译"))
        }
    };
}

// 迷你剧 / 限定剧：这类作品集数很少，常常没有季号，只能靠这个词认出来。
lazy_re!(
    re_tv_word,
    r"(?i)\b(miniseries|mini[- ]series|limited[- ]series|tv[- ]series)\b|迷你剧|限定剧|短剧|连续剧"
);
// 特别篇 / OVA：属于剧集体系，但不占正片的集号（季号记 0）。
// 英文必须认 `special` / `christmas`：Doctor Who 的圣诞特辑一律写成
// `Doctor.Who.2005.Christmas.Special.2015...`，少了这一半，它们跟正剧
// `doctor.who.2005.s08e01...` 的归一化标题永远差着一截，会被拆成独立条目。
lazy_re!(
    re_special,
    r"(?i)\b(ova|oad|sp|specials?|christmas)\b|特别篇|番外篇|番外|特别版|特辑"
);
// 剧场版 / 电影版：动画 IP 里这两个词指的是**一部电影**。
lazy_re!(re_movie_word, r"(?i)\b(theatrical|movie|film)\b|剧场版|电影版");
// 合集：一个文件装了很多集，按一条资源处理。
lazy_re!(
    re_compilation,
    r"(?i)\b(collection)\b|合集|合辑|全集|全套|全\s*\d+\s*[集话話回]|完结合集"
);
// 目录名里的季标记（目录名不走文件名解析，单独认）
lazy_re!(
    re_dir_season,
    r"(?i)\bseason[\s\-_.]*\d{1,2}\b|\bs\d{1,2}\b|第\s*[0-9一二三四五六七八九十]{1,3}\s*季"
);
// 「特辑目录」：`Doctor.Who.2005.S07...\Specials\The.Time.of.the.Doctor.mkv`
// 这种文件自己一个标号都没有，只有目录名在说它是哪一部的特辑。
lazy_re!(
    re_dir_special,
    r"(?i)\b(specials?|extras?|featurettes?|bonus|ova|oad)\b|特辑|特别篇|番外"
);

/// 合集 / 合辑文件（R2）。
pub fn is_compilation(name: &str) -> bool {
    re_compilation().is_match(name)
}

/// 特别篇 / OVA（R3）。
pub fn is_special(name: &str) -> bool {
    re_special().is_match(name)
}

/// 迷你剧 / 限定剧（R6）。
pub fn is_tv_word(name: &str) -> bool {
    re_tv_word().is_match(name)
}

/// 剧场版 / 电影版（R10）。
pub fn is_movie_word(name: &str) -> bool {
    re_movie_word().is_match(name)
}

/// 目录名里是否带季标记（R9）。
pub fn dir_has_season_marker(name: &str) -> bool {
    re_dir_season().is_match(name)
}

/// 目录名是不是「特辑目录」（R9 的另一半：`Specials` / `Extras` / `特辑`）。
pub fn dir_has_special_marker(name: &str) -> bool {
    re_dir_special().is_match(name)
}

// 编号 token：`01` / `102` / `0001`，也认 `0001v2` 这种带版本号的写法。
lazy_re!(re_num_token, r"^(\d{1,4})(?:[vV]\d+)?$");

/// 一眼看上去是「分辨率」的数字。只在**看整个文件名末尾**时才需要排除——
/// 文件名几乎总以 `1080p` / `720` 收尾，而标题里的 `720` 更可能是第 720 集。
const RESOLUTION_NUMBERS: &[i32] = &[240, 360, 480, 576, 720, 1080, 1440, 2160, 4320];

/// 一个 token 是不是「编号」，是的话返回数值。
///
/// - 最多 4 位（`0001`）：上千集的动画（`哆啦A梦 1000`）也得住得下；
/// - 落在 1800..2100 的 4 位数一律当**年份**（`1917` / `2012` 是电影名），不算编号；
/// - 允许 `v2` 版本后缀（`[字幕组] 剧名 - 01v2 [1080P]` 是很常见的写法）。
fn number_token_value(token: &str) -> Option<i32> {
    let c = re_num_token().captures(token)?;
    let n = c.get(1)?.as_str().parse::<i32>().ok()?;
    if n <= 0 || (1800..2100).contains(&n) {
        return None; // 年份不是集号
    }
    Some(n)
}

/// 标题尾部的编号 token（`01` / `102` / `0001` / `0001v2`），没有则 `None`。
pub fn trailing_number_token(title: &str) -> Option<&str> {
    let last = title.split_whitespace().next_back()?;
    if number_token_value(last).is_some() {
        Some(last)
    } else {
        None
    }
}

/// 标题尾部的编号（`01` / `102` / `1000`），没有则 `None`。
pub fn trailing_number(title: &str) -> Option<i32> {
    trailing_number_token(title).and_then(number_token_value)
}

/// 整个**文件名**末尾的编号。
///
/// 只用于「标题被季标记截断、编号落在标题之外」的写法：
/// `哆啦A梦.S02.0001.mkv` 的标题是 `哆啦A梦`，`0001` 压根没进标题，
/// 只看标题就会把一整季全塞进第 1 集。分辨率数字不算编号。
pub fn trailing_number_of_file(file_name: &str) -> Option<i32> {
    let stem = match file_name.rfind('.') {
        Some(i) if !file_name[i + 1..].contains(' ') => &file_name[..i],
        _ => file_name,
    };
    let flat: String = stem
        .chars()
        .map(|c| if matches!(c, '.' | '_' | '+') { ' ' } else { c })
        .collect();
    let last = flat.split_whitespace().next_back()?;
    let n = number_token_value(last)?;
    if RESOLUTION_NUMBERS.contains(&n) {
        return None;
    }
    Some(n)
}

/// 去掉尾部编号后的前缀；整个标题就是编号时返回空串。
pub fn strip_trailing_number(title: &str) -> &str {
    match title.rsplit_once(' ') {
        Some((head, tail)) if number_token_value(tail).is_some() => head.trim(),
        _ => "",
    }
}

/* ------------------------------ 判定 ------------------------------ */

/// 综合全部信号给出结论。
pub fn classify(sig: &Signals<'_>, cfg: &TypeConfig) -> Verdict {
    // R1：数据源已经明确说过，就不再猜。
    if let Some(hint) = sig.source_hint {
        return match hint {
            SourceHint::Tv => Verdict::new(
                MediaKind::Tv,
                cfg.weights.source_hint,
                0,
                &["数据源标识为剧集"],
            ),
            SourceHint::Movie => Verdict::new(
                MediaKind::Movie,
                0,
                cfg.weights.source_hint,
                &["数据源标识为电影"],
            ),
        };
    }

    // R2：合集 / 合辑——一个文件就是一整部，按单条资源处理。
    if is_compilation(sig.file_name) {
        return Verdict::new(MediaKind::Movie, 0, cfg.weights.movie_word, &["合集/合辑按单条处理"]);
    }

    let w = &cfg.weights;
    let mut tv = 0i32;
    let mut movie = 0i32;
    let mut reasons: Vec<&'static str> = Vec::new();

    // R3 特别篇 / OVA
    if is_special(sig.file_name) {
        tv += w.special;
        reasons.push("特别篇 / OVA 标记");
    }
    // R4 / R5 明确集号 / 季号
    if sig.episode_marker {
        tv += w.episode_marker;
        reasons.push("文件名带集号");
    } else if sig.season_marker {
        tv += w.season_marker;
        reasons.push("文件名带季号");
    }
    // R6 迷你剧 / 限定剧
    if is_tv_word(sig.file_name) {
        tv += w.tv_word;
        reasons.push("迷你剧 / 限定剧标记");
    }
    // R7 目录共识
    if sig.folder_tv_count >= cfg.folder_consensus_min && belongs_to_consensus(sig, cfg) {
        tv += w.folder_consensus;
        reasons.push("同目录多份文件带集号，且本文件属于同一部剧");
    }
    // R8 编号共识
    if sig.year.is_none() && matches_numbered_consensus(sig, cfg) {
        tv += w.numbered_consensus;
        reasons.push("同目录有一串同前缀编号文件，且本文件无年份");
    }
    // R9 目录名的季标记 / 特辑目录
    if sig.folder.map(dir_has_season_marker).unwrap_or(false)
        || sig.parent.map(dir_has_season_marker).unwrap_or(false)
    {
        tv += w.season_folder;
        reasons.push("所在目录名带季标记");
    }
    if sig
        .parent
        .map(dir_has_special_marker)
        .unwrap_or(false)
    {
        tv += w.season_folder;
        reasons.push("位于特辑 / Extras 目录");
    }
    // R10 剧场版 / 电影版
    if is_movie_word(sig.file_name) {
        movie += w.movie_word;
        reasons.push("剧场版 / 电影版标记");
    }
    // R11 时长
    if let Some(sec) = sig.duration_secs {
        let minutes = sec / 60;
        if minutes >= cfg.episode_minutes.0 && minutes <= cfg.episode_minutes.1 {
            tv += w.duration_episode;
            reasons.push("单集时长");
        } else if minutes >= cfg.movie_minutes {
            movie += w.duration_movie;
            reasons.push("电影时长");
        }
    }
    // R12 有年份且没有任何剧集信号（文件名里的年份优先，其次库里已存的）
    if tv == 0 && sig.year.or(sig.stored_year).is_some() {
        movie += w.year_no_tv;
        reasons.push("有上映年份且无剧集特征");
    }

    let kind = if tv >= cfg.tv_threshold && tv >= movie {
        MediaKind::Tv
    } else if movie >= cfg.movie_threshold && movie > tv {
        MediaKind::Movie
    } else {
        reasons.push("证据不足，按默认策略处理");
        match cfg.unknown_policy {
            UnknownPolicy::Unknown => MediaKind::Unknown,
            UnknownPolicy::Movie => MediaKind::Movie,
            UnknownPolicy::Tv => MediaKind::Tv,
        }
    };
    Verdict::new(kind, tv, movie, &reasons)
}

/// R7：本文件是否属于「同目录里那部带集号的剧」。
fn belongs_to_consensus(sig: &Signals<'_>, cfg: &TypeConfig) -> bool {
    let title = sig.title.trim();
    if title.is_empty() {
        return false;
    }
    if let Some(c) = sig.folder_consensus_title {
        if similarity(title, c) >= cfg.consensus_similarity {
            return true;
        }
    }
    // 标题以编号结尾（`剧名 01`）：目录里既然有明确的剧集文件，它多半就是其中一集
    if trailing_number(title).is_some() {
        return true;
    }
    for name in [sig.folder, sig.parent].into_iter().flatten() {
        if similarity(title, name) >= cfg.consensus_similarity {
            return true;
        }
    }
    false
}

/// R8：本文件是否属于「同目录里那一串编号文件」。
///
/// 纯编号（`01.mkv`）门槛很低——电影不会叫这个名字；
/// 带前缀（`剧名 01.mkv`）门槛高一些，因为它也可能是 `Rocky 1/2/3` 这种系列电影。
fn matches_numbered_consensus(sig: &Signals<'_>, cfg: &TypeConfig) -> bool {
    let title = sig.title.trim();
    let digits = trailing_number_token(title);
    if title.is_empty() || digits.is_none() {
        return false;
    }
    let base = strip_trailing_number(title);
    if base.is_empty() {
        return sig.folder_numbered_count >= cfg.pure_number_min;
    }
    // 零填充编号（`哆啦A梦 0001` / `剧名 001`）是剧集和动画的命名习惯，
    // 系列电影不会这么写，因此按纯编号的低门槛处理。
    if is_padded_number(digits.unwrap_or("")) {
        return sig.folder_numbered_count >= cfg.pure_number_min;
    }
    if sig.folder_numbered_count < cfg.prefix_number_min {
        return false;
    }
    match sig.folder_number_prefix {
        Some(prefix) => {
            normalize_title(base) == normalize_title(prefix)
                || similarity(base, prefix) >= cfg.consensus_similarity
        }
        None => false,
    }
}

/// 编号是不是「零填充」写法（`001` / `0001`）：长篇动画与剧集的命名习惯。
///
/// 只认**三位以上**的零填充——`01` 这种两位数太常见（系列电影也这么写），
/// 不足以把门槛从「带前缀」降到「纯编号」。
fn is_padded_number(token: &str) -> bool {
    token.len() >= 3 && token.starts_with('0')
}

/// 判成剧集之后补齐季 / 集号。
///
/// - 编号没有明确标记的文件（`01.mkv` / `剧名 01.mkv`）用**尾部编号**当集号，
///   否则一整季会全挤在第 1 集；
/// - 带季标记但编号落在标题之外时（`哆啦A梦.S02.0001.mkv`），回头看整个
///   文件名末尾——标题被 `S02` 截断了，编号根本没进标题；
/// - 特别篇 / OVA 归到**第 0 季**，不占正片集号；文件名里写明的季号必须尊重。
///
/// `parent` 是文件所在目录名：特辑目录（`Specials` / `Extras`）里的文件同样
/// 按特辑处理，否则它们会被塞进第 1 季。
pub fn fill_episode_numbers(parsed: &mut ParsedName, file_name: &str, parent: Option<&str>) {
    if parsed.episode.is_none() {
        parsed.episode = trailing_number(&parsed.title)
            .or_else(|| {
                if parsed.season_marker {
                    trailing_number_of_file(file_name)
                } else {
                    None
                }
            })
            .or(Some(1));
    }
    // 自己写了 `Christmas Special` 算特辑；被收进 `Specials` / `特辑` 目录的
    // 也算——那种文件往往连个「special」都不写（`The.Time.of.the.Doctor`），
    // 但目录已经替它说了。
    let special = is_special(file_name) || parent.map(dir_has_special_marker).unwrap_or(false);
    if parsed.season.is_none() || (special && !parsed.season_marker) {
        parsed.season = Some(if special { 0 } else { parsed.season.unwrap_or(1) });
    }
}

/* ------------------------------ 目录统计 ------------------------------ */

/// 一个目录的统计（供 R7 / R8 使用）。
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct FolderStat {
    /// 目录里的视频文件总数。
    pub total: usize,
    /// 带明确集号 / 季号标记的文件数。
    pub tv_count: usize,
    /// 标题以编号结尾但没有标记的文件数。
    pub numbered_count: usize,
    /// 带标记文件的众数标题。
    pub consensus_title: Option<String>,
    /// 编号文件的公共前缀（众数）。
    pub numbered_prefix: Option<String>,
}

/// 参与目录统计的一项。
pub struct FolderEntry<'a> {
    pub file_name: &'a str,
    pub parsed: &'a ParsedName,
}

/// 统计一个目录里的「剧集证据」。
///
/// 结果与输入顺序无关（众数并列时取字典序最小），否则重扫一次结论就变了。
pub fn build_folder_stats(entries: &[FolderEntry<'_>]) -> FolderStat {
    let mut stat = FolderStat::default();
    let mut titles: BTreeMap<String, usize> = BTreeMap::new();
    let mut prefixes: BTreeMap<String, usize> = BTreeMap::new();

    for e in entries {
        stat.total += 1;
        let title = e.parsed.title.trim();
        if e.parsed.episode_marker || e.parsed.season_marker {
            stat.tv_count += 1;
            if !title.is_empty() {
                *titles.entry(title.to_string()).or_insert(0) += 1;
            }
            continue;
        }
        // 合集文件（`全集.mkv`）本身就是一个编号串的「邻居」，
        // 但不能拿它去给别的编号文件作证。
        if !is_compilation(e.file_name) && trailing_number(title).is_some() {
            stat.numbered_count += 1;
            let base = strip_trailing_number(title).trim().to_string();
            // 纯编号没有前缀可比，不参与众数
            if !base.is_empty() {
                *prefixes.entry(base).or_insert(0) += 1;
            }
        }
    }

    stat.consensus_title = majority(&titles);
    stat.numbered_prefix = majority(&prefixes);
    stat
}

/// 取出现次数最多的键；并列时取字典序最小（保证确定性）。
fn majority(counts: &BTreeMap<String, usize>) -> Option<String> {
    counts
        .iter()
        .max_by(|(ka, va), (kb, vb)| va.cmp(vb).then(kb.cmp(ka).reverse()))
        .map(|(k, _)| k.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse_file_name;

    fn kind_of(name: &str) -> MediaKind {
        let p = parse_file_name(name);
        let sig = Signals::from_parsed(&p, name);
        classify(&sig, &TypeConfig::default()).kind
    }

    fn sig_of(name: &str) -> (ParsedName, String) {
        let p = parse_file_name(name);
        (p, name.to_string())
    }

    /* ---------------- 基本判定 ---------------- */

    #[test]
    fn movie_with_year_is_movie() {
        assert_eq!(kind_of("The.Matrix.1999.1080p.BluRay.x264.mkv"), MediaKind::Movie);
        assert_eq!(kind_of("Inception (2010) [1080p].mkv"), MediaKind::Movie);
        assert_eq!(kind_of("Dune.2021.2160p.WEB-DL.DDP5.1.HDR.x265.mkv"), MediaKind::Movie);
    }

    #[test]
    fn explicit_episode_markers_are_tv() {
        for name in [
            "Breaking.Bad.S03E07.720p.HDTV.mkv",
            "Some.Show.1x02.720p.mkv",
            "动画名 第01话.mp4",
            "剧名.第一季.E01.2021.1080p.mkv",
            "Doctor Who Season 4 Episode 3.mkv",
            "Show Name - 01.mkv",
            "[字幕组] 剧名 - 01 [1080P][简体].mp4",
        ] {
            assert_eq!(kind_of(name), MediaKind::Tv, "{name} 应判为剧集");
        }
    }

    #[test]
    fn explicit_season_marker_is_tv() {
        assert_eq!(kind_of("剧名 第二季 1080p.mkv"), MediaKind::Tv);
        assert_eq!(kind_of("Show.S02.1080p.WEB-DL.mkv"), MediaKind::Tv);
    }

    /// 边界：迷你剧。没有季集号，只有「迷你剧」这个词。
    #[test]
    fn miniseries_word_is_tv() {
        assert_eq!(kind_of("Chernobyl.迷你剧.1080p.mkv"), MediaKind::Tv);
        assert_eq!(kind_of("Some.Show.Miniseries.1080p.mkv"), MediaKind::Tv);
        assert_eq!(kind_of("限定剧.某剧.2160p.mkv"), MediaKind::Tv);
    }

    /// 边界：特别篇 / OVA 属于剧集体系（季号在入库时记 0，见 scanner）。
    #[test]
    fn specials_and_ova_are_tv_not_movie() {
        assert_eq!(kind_of("某动画 OVA 第01话.mkv"), MediaKind::Tv);
        assert_eq!(kind_of("某剧 特别篇 1080p.mkv"), MediaKind::Tv);
        assert_eq!(kind_of("Show.SP.01.mkv"), MediaKind::Tv);
    }

    /// 边界：动画剧场版是**电影**，不能被当成剧集。
    #[test]
    fn theatrical_anime_is_movie() {
        assert_eq!(kind_of("名侦探柯南 剧场版 2023.mkv"), MediaKind::Movie);
        assert_eq!(kind_of("Some.Anime.Movie.2020.1080p.mkv"), MediaKind::Movie);
    }

    /// 边界：合集文件按单条资源处理。
    #[test]
    fn compilation_files_are_single_movie_entries() {
        assert_eq!(kind_of("某剧 合集 1080p.mkv"), MediaKind::Movie);
        assert_eq!(kind_of("某剧 全12集 1080p.mkv"), MediaKind::Movie);
        assert_eq!(kind_of("Some.Show.Collection.1080p.mkv"), MediaKind::Movie);
    }

    /// 边界：元数据缺失（无年份、无标记、目录里只有它一个）→ 待确认，
    /// **不能**静默归成电影。
    #[test]
    fn metadata_missing_falls_back_to_unknown() {
        let v = {
            let p = parse_file_name("Dune.mkv");
            classify(&Signals::from_parsed(&p, "Dune.mkv"), &TypeConfig::default())
        };
        assert_eq!(v.kind, MediaKind::Unknown, "{v:?}");
        assert_eq!(v.tv_score, 0);
        assert_eq!(v.movie_score, 0);
    }

    /// 默认策略可切换：改成 Movie 后，同样的输入就归电影（老行为兼容）。
    #[test]
    fn unknown_policy_is_configurable() {
        let (p, name) = sig_of("Dune.mkv");
        let mut cfg = TypeConfig::default();
        cfg.unknown_policy = UnknownPolicy::Movie;
        assert_eq!(classify(&Signals::from_parsed(&p, &name), &cfg).kind, MediaKind::Movie);
        cfg.unknown_policy = UnknownPolicy::Tv;
        assert_eq!(classify(&Signals::from_parsed(&p, &name), &cfg).kind, MediaKind::Tv);
    }

    /// 有剧集迹象但不够（只有 2 分）时，同样走默认策略，而不是归电影。
    #[test]
    fn weak_tv_evidence_does_not_silently_become_movie() {
        let (p, name) = sig_of("Show.Name.mkv");
        let mut sig = Signals::from_parsed(&p, &name);
        sig.duration_secs = Some(45 * 60); // 单集时长 +2
        let v = classify(&sig, &TypeConfig::default());
        assert_eq!(v.kind, MediaKind::Unknown, "{v:?}");
        assert_eq!(v.tv_score, 2);
    }

    /* ---------------- 时长信号 ---------------- */

    #[test]
    fn duration_pushes_towards_episode_or_movie() {
        let (p, name) = sig_of("Plain.Title.mkv");
        let mut sig = Signals::from_parsed(&p, &name);
        sig.duration_secs = Some(24 * 60);
        assert_eq!(classify(&sig, &TypeConfig::default()).kind, MediaKind::Unknown);
        assert_eq!(classify(&sig, &TypeConfig::default()).tv_score, 2);

        sig.duration_secs = Some(148 * 60);
        let v = classify(&sig, &TypeConfig::default());
        assert_eq!(v.kind, MediaKind::Movie, "{v:?}");
        assert_eq!(v.movie_score, 2);
    }

    /// 时长只是**一个**信号：带集号的长片（两小时的特别篇）仍然是剧集。
    #[test]
    fn episode_marker_beats_movie_length_duration() {
        let (p, name) = sig_of("Show.S01E01.1080p.mkv");
        let mut sig = Signals::from_parsed(&p, &name);
        sig.duration_secs = Some(150 * 60);
        assert_eq!(classify(&sig, &TypeConfig::default()).kind, MediaKind::Tv);
    }

    /* ---------------- 目录共识 ---------------- */

    /// 边界：文件名完全没有集号，但同目录里有多份带集号的文件 → 剧集。
    #[test]
    fn folder_consensus_recovers_unmarked_episodes() {
        let (p, name) = sig_of("剧名 01.mkv");
        let mut sig = Signals::from_parsed(&p, &name);
        sig.folder = Some("剧名");
        sig.folder_tv_count = 3;
        sig.folder_consensus_title = Some("剧名");
        sig.folder_total = 12;
        let v = classify(&sig, &TypeConfig::default());
        assert_eq!(v.kind, MediaKind::Tv, "{v:?}");
    }

    /// 电影跟目录里的剧**不同名**时，不能被目录共识带跑。
    #[test]
    fn unrelated_movie_is_not_pulled_into_folder_consensus() {
        let (p, name) = sig_of("The.Matrix.1999.1080p.mkv");
        let mut sig = Signals::from_parsed(&p, &name);
        sig.folder = Some("Breaking Bad");
        sig.folder_tv_count = 3;
        sig.folder_consensus_title = Some("Breaking Bad");
        sig.folder_total = 6;
        assert_eq!(classify(&sig, &TypeConfig::default()).kind, MediaKind::Movie);
    }

    #[test]
    fn season_named_folder_is_tv() {
        let (p, name) = sig_of("01.mkv");
        let mut sig = Signals::from_parsed(&p, &name);
        sig.parent = Some("Season 2");
        assert_eq!(classify(&sig, &TypeConfig::default()).kind, MediaKind::Tv);

        let mut sig2 = sig.clone();
        sig2.parent = Some("第二季");
        assert_eq!(classify(&sig2, &TypeConfig::default()).kind, MediaKind::Tv);
    }

    /// 边界：纯编号文件（`01.mkv`）门槛低；带前缀的编号门槛高（系列电影也是这个形状）。
    #[test]
    fn numbered_consensus_has_two_thresholds() {
        // 纯编号：两个就够
        let (p, name) = sig_of("01.mkv");
        let mut sig = Signals::from_parsed(&p, &name);
        sig.folder_numbered_count = 2;
        assert_eq!(classify(&sig, &TypeConfig::default()).kind, MediaKind::Tv);

        // 带前缀：两个不够（避免 Rocky 1/2 这类系列电影被判成剧集）
        let (p2, name2) = sig_of("剧名 01.mkv");
        let mut sig2 = Signals::from_parsed(&p2, &name2);
        sig2.folder_numbered_count = 2;
        sig2.folder_number_prefix = Some("剧名");
        assert_ne!(classify(&sig2, &TypeConfig::default()).kind, MediaKind::Tv);

        // 攒够门槛（默认 3 个）且前缀一致才是剧集
        sig2.folder_numbered_count = 3;
        assert_eq!(classify(&sig2, &TypeConfig::default()).kind, MediaKind::Tv);

        // 前缀不一致（不是同一部）→ 不算
        sig2.folder_number_prefix = Some("别的剧");
        assert_ne!(classify(&sig2, &TypeConfig::default()).kind, MediaKind::Tv);
    }

    /// 系列电影最常见的防线：带年份的编号文件不算剧集（`Rocky.1976`）。
    #[test]
    fn numbered_file_with_year_stays_movie() {
        let (p, name) = sig_of("Rocky.1976.1080p.mkv");
        let mut sig = Signals::from_parsed(&p, &name);
        sig.folder_numbered_count = 9;
        sig.folder_number_prefix = Some("Rocky");
        assert_eq!(classify(&sig, &TypeConfig::default()).kind, MediaKind::Movie);
    }

    /* ---------------- 数据源标识 ---------------- */

    #[test]
    fn source_hint_overrides_everything() {
        let (p, name) = sig_of("The.Matrix.1999.1080p.mkv");
        let mut sig = Signals::from_parsed(&p, &name);
        sig.source_hint = Some(SourceHint::Tv);
        assert_eq!(classify(&sig, &TypeConfig::default()).kind, MediaKind::Tv);

        let (p2, name2) = sig_of("Show.S01E01.mkv");
        let mut sig2 = Signals::from_parsed(&p2, &name2);
        sig2.source_hint = Some(SourceHint::Movie);
        assert_eq!(classify(&sig2, &TypeConfig::default()).kind, MediaKind::Movie);
    }

    #[test]
    fn source_hint_parses_common_spellings() {
        assert_eq!(SourceHint::from_str("tv"), Some(SourceHint::Tv));
        assert_eq!(SourceHint::from_str("Movie"), Some(SourceHint::Movie));
        assert_eq!(SourceHint::from_str("series"), Some(SourceHint::Tv));
        assert_eq!(SourceHint::from_str(""), None);
        assert_eq!(SourceHint::from_str("???") , None);
    }

    /* ---------------- 编号工具 ---------------- */

    #[test]
    fn trailing_number_ignores_years() {
        assert_eq!(trailing_number("剧名 01"), Some(1));
        assert_eq!(trailing_number("剧名 102"), Some(102));
        assert_eq!(trailing_number("01"), Some(1));
        assert_eq!(trailing_number("1917"), None, "年份不能当集号");
        assert_eq!(trailing_number("2012"), None);
        assert_eq!(trailing_number("The Matrix"), None);
        assert_eq!(trailing_number(""), None);
        assert_eq!(strip_trailing_number("剧名 01"), "剧名");
        assert_eq!(strip_trailing_number("01"), "");
    }

    /// 集数上千的动画：`哆啦A梦 1000` 的第 1000 集必须认得出来。
    #[test]
    fn trailing_number_covers_four_digit_episode_numbers() {
        assert_eq!(trailing_number("哆啦A梦 1000"), Some(1000));
        assert_eq!(trailing_number("0001"), Some(1), "纯编号文件名");
        assert_eq!(trailing_number("剧名 0001"), Some(1));
        assert_eq!(trailing_number("剧名 01v2"), Some(1), "带版本号的编号");
        assert_eq!(strip_trailing_number("剧名 0001"), "剧名");
        assert_eq!(strip_trailing_number("哆啦A梦 1000"), "哆啦A梦");
        assert_eq!(trailing_number("剧名 12345"), None, "五位数不是集号");
    }

    /// 标题被季标记截断时，编号要从**整个文件名**末尾找回来。
    #[test]
    fn episode_number_after_season_marker_is_recovered() {
        assert_eq!(trailing_number_of_file("哆啦A梦.S02.0001.mkv"), Some(1));
        assert_eq!(trailing_number_of_file("哆啦A梦.S02.1000.mkv"), Some(1000));
        assert_eq!(trailing_number_of_file("Show.S02.1080p.mkv"), None, "分辨率不是集号");
        assert_eq!(trailing_number_of_file("Inception (2010) [1080p].mkv"), None);
    }

    /// 特辑是剧集体系的一员（归第 0 季），不能被判成电影。
    #[test]
    fn chinese_special_marker_is_tv() {
        assert_eq!(kind_of("神秘博士.2023.特辑.1080p.mkv"), MediaKind::Tv);
        assert_eq!(kind_of("某剧 圣诞特辑 1080p.mkv"), MediaKind::Tv);
    }

    /// 英文的圣诞特辑（`Christmas Special`）同样是剧集——
    /// 少了这一条，Doctor Who 的每一部圣诞特辑都会各自成一条独立资源。
    #[test]
    fn english_christmas_special_is_tv() {
        for name in [
            "doctor.who.2005.christmas.special.2005.1080p.bluray.x264.mkv",
            "Doctor.Who.2005.Christmas.Special.2015.1080p.BluRay.x264.mkv",
            "doctor.who.2005.a.christmas.carol.2010.special.1080p.mkv",
            "Doctor.Who.2005.S08.Christmas.Special.1080p.BluRay.x264-RRH.mkv",
        ] {
            let p = parse_file_name(name);
            let v = classify(&Signals::from_parsed(&p, name), &TypeConfig::default());
            assert_eq!(v.kind, MediaKind::Tv, "{name}：{v:?}");
        }
    }

    /// 特辑目录里的文件自己一个标记都没有，只能靠目录名认母带。
    #[test]
    fn specials_folder_makes_unmarked_files_tv() {
        for dir in ["Specials", "Extras", "特辑"] {
            let (p, name) = sig_of("Doctor.Who.The.Time.of.the.Doctor.2013.1080p.BluRay.x264.mkv");
            let mut sig = Signals::from_parsed(&p, &name);
            sig.parent = Some(dir);
            let v = classify(&sig, &TypeConfig::default());
            assert_eq!(v.kind, MediaKind::Tv, "{dir} 里的特辑：{v:?}");
        }
        // 没有这条证据时，它还是会被当成电影（有年份、无剧集特征）
        let (p, name) = sig_of("Doctor.Who.The.Time.of.the.Doctor.2013.1080p.BluRay.x264.mkv");
        assert_eq!(
            classify(&Signals::from_parsed(&p, &name), &TypeConfig::default()).kind,
            MediaKind::Movie
        );
    }

    /// 特辑归第 0 季：文件名里写明季号的除外（`S08.Christmas.Special` 认 S08）。
    #[test]
    fn specials_are_numbered_into_season_zero() {
        let name = "doctor.who.2005.christmas.special.2005.1080p.bluray.x264.mkv";
        let mut p = parse_file_name(name);
        fill_episode_numbers(&mut p, name, None);
        assert_eq!(p.season, Some(0));

        let name2 = "Doctor.Who.2005.S08.Christmas.Special.1080p.BluRay.x264-RRH.mkv";
        let mut p2 = parse_file_name(name2);
        fill_episode_numbers(&mut p2, name2, None);
        assert_eq!(p2.season, Some(8), "文件名自己写了季号就照它的来");

        // 连 `Christmas` 都没写、但住在 `Specials` 目录里 → 也是第 0 季
        let name3 = "Doctor.Who.The.Time.of.the.Doctor.2013.1080p.BluRay.x264.mkv";
        let mut p3 = parse_file_name(name3);
        fill_episode_numbers(&mut p3, name3, Some("Specials"));
        assert_eq!(p3.season, Some(0));
    }

    /* ---------------- 目录统计 ---------------- */

    #[test]
    fn folder_stats_count_evidence() {
        let names = [
            "Show.S01E01.mkv",
            "Show.S01E02.mkv",
            "01.mkv",
            "剧名 01.mkv",
            "剧名 02.mkv",
        ];
        let parsed: Vec<ParsedName> = names.iter().map(|n| parse_file_name(n)).collect();
        let entries: Vec<FolderEntry> = names
            .iter()
            .zip(parsed.iter())
            .map(|(n, p)| FolderEntry { file_name: n, parsed: p })
            .collect();
        let st = build_folder_stats(&entries);
        assert_eq!(st.total, 5);
        assert_eq!(st.tv_count, 2);
        assert_eq!(st.numbered_count, 3);
        assert_eq!(st.consensus_title.as_deref(), Some("Show"));
        assert_eq!(st.numbered_prefix.as_deref(), Some("剧名"));
    }

    /// 统计结果必须与输入顺序无关，否则重扫一次结论就变了。
    #[test]
    fn folder_stats_are_order_independent() {
        let a = ["Show.S01E01.mkv", "Show.S01E02.mkv", "Other.S01E01.mkv"];
        let b = ["Other.S01E01.mkv", "Show.S01E02.mkv", "Show.S01E01.mkv"];
        let build = |names: [&str; 3]| {
            let parsed: Vec<ParsedName> = names.iter().map(|n| parse_file_name(n)).collect();
            let entries: Vec<FolderEntry> = names
                .iter()
                .zip(parsed.iter())
                .map(|(n, p)| FolderEntry { file_name: n, parsed: p })
                .collect();
            build_folder_stats(&entries)
        };
        assert_eq!(build(a), build(b));
    }

    #[test]
    fn kind_roundtrips_through_str() {
        assert_eq!(MediaKind::from_str(MediaKind::Tv.as_str()), MediaKind::Tv);
        assert_eq!(MediaKind::from_str(MediaKind::Movie.as_str()), MediaKind::Movie);
        assert_eq!(MediaKind::from_str("weird"), MediaKind::Unknown);
        assert_eq!(MediaKind::Unknown.label(), "待确认");
    }

    /// 全局配置可替换（`configure` → `config`）。
    #[test]
    fn global_config_can_be_replaced() {
        let mut cfg = config();
        cfg.unknown_policy = UnknownPolicy::Movie;
        configure(cfg);
        assert_eq!(config().unknown_policy, UnknownPolicy::Movie);
        configure(TypeConfig::default());
        assert_eq!(config().unknown_policy, UnknownPolicy::Unknown);
    }
}
