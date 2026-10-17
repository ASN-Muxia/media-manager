//! 候选匹配打分：把「本地条目」和「数据源返回的一组候选」对上。
//!
//! ## 为什么单独一层
//!
//! 自动刮削原来只做一件事：年份精确相等就取它，否则取第一条（`pick_tmdb_id`）。
//! 手动匹配更是把 TMDB 的原样结果直接丢给界面。结果是：
//!
//! - 译名不同就对不上（`The Garden of Words` ↔ `言叶之庭`）——**原标题没参与比对**；
//! - 多了个副标题、少了个空格就掉到 0 分——**没有模糊匹配**；
//! - 同名不同年份的片子排在前面——**年份只是个过滤器，不是打分项**；
//! - 想排除「剧场版 / 演唱会 / 幕后花絮」只能靠手挑——**没有黑名单**。
//!
//! 这一层把上面四件事做成**可配置、可单测**的纯函数，`provider` 只负责把各自
//! 的数据结构映射进来。
//!
//! ## 刻意不在这里做的事
//!
//! - **不认识 `TmdbSearchItem` / `SuggestItem`**：本层只吃字符串，保证
//!   `core` 不反向依赖 `provider`（分层红线 B3），也让用例不用造网络结构。
//! - **不参与「要不要刮削」的决策**：打分只回答「哪个候选更像」。
//! - **分辨率不参与打分**：TMDB / 豆瓣的候选里根本没有分辨率这个字段，
//!   硬编进分数只会制造假信号。分辨率只用来**清洗检索词**（见 [`strip_tech_tags`]）。

use crate::core::grouping::similarity;

/// 手动匹配一次最多拉多少条候选（TMDB 每页 20 条）。
///
/// 只拿第一页（20 条）时，用户想要的那个常常就在第 21 条之后，只能反复改
/// 关键词碰运气——这正是「候选太少」的由来。设置项与命令共用这一个默认值，
/// 避免两处各写一个数字。
pub const DEFAULT_LIMIT: usize = 60;
/// 允许的上限：再大就是拿一个泛关键词把整个库翻一遍了。
pub const MAX_LIMIT: usize = 200;

/// 模糊匹配的下限：低于这个相似度的候选一律按「不命中」处理。
///
/// 0.7 是实测出来的：`言叶之庭` 与 `言叶之庭剧场版` 约 0.71（会被 contains 接住），
/// 而两部不相干的片子极少能到 0.7。低于此值给分只会把噪声抬到前面。
pub const FUZZY_ACCEPT: f64 = 0.72;

/// 本地条目这一侧的检索意图。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MatchQuery {
    /// 条目标题（通常已经过文件名解析，不含集号）。
    pub title: String,
    /// 已知的其他叫法：原标题、别名、目录名……顺序无所谓，取最高分那个。
    pub aliases: Vec<String>,
    pub year: Option<i32>,
    /// 季数：只影响剧集的**首播年份推断**（第 N 季通常比第 1 季晚 N-1 年）。
    pub season: Option<i32>,
    /// 分辨率：只用于清洗检索词，不参与打分。
    pub resolution: Option<String>,
}

/// 打分权重（「匹配优先级规则」的可调部分）。
///
/// 优先级从高到低：**展示名精确 > 原标题 / 别名精确 > 包含 > 模糊**，
/// 年份与季数作为修正项叠加在其上。
#[derive(Debug, Clone, PartialEq)]
pub struct MatchWeights {
    /// 展示名归一化后完全一致。
    pub exact: i32,
    /// 原标题 / 别名完全一致。
    pub alias: i32,
    /// 一边包含另一边（副标题、续作标记这类场景）。
    pub contains: i32,
    /// 模糊匹配满分（实际按相似度缩放）。
    pub fuzzy: i32,
    /// 年份完全一致。
    pub year_hit: i32,
    /// 年份超出容差（负值，压低）。
    pub year_miss: i32,
    /// 首播年份与季数推断吻合。
    pub season_hit: i32,
    /// 评分带来的最多加分（0..10 映射到此值）。
    pub popularity: i32,
}

impl Default for MatchWeights {
    fn default() -> Self {
        Self {
            exact: 100,
            alias: 90,
            contains: 55,
            fuzzy: 45,
            year_hit: 20,
            year_miss: -30,
            season_hit: 8,
            popularity: 6,
        }
    }
}

/// 匹配选项：界面上能调的都在这里。
#[derive(Debug, Clone, PartialEq)]
pub struct MatchOptions {
    /// 是否启用模糊匹配（关掉就只剩精确与包含）。
    pub fuzzy: bool,
    /// 年份容差：`0` = 必须同一年，`1` = 前后一年也算命中。
    pub year_tolerance: i32,
    /// 排除词：候选标题里出现任一个就**整条剔除**（见 [`MatchScore::excluded`]）。
    pub blacklist: Vec<String>,
    /// 低于这个分的候选不进列表。
    pub min_score: i32,
    pub weights: MatchWeights,
}

impl Default for MatchOptions {
    fn default() -> Self {
        Self {
            fuzzy: true,
            year_tolerance: 1,
            blacklist: Vec::new(),
            min_score: 0,
            weights: MatchWeights::default(),
        }
    }
}

/// 一条候选待比对的信息（由 `provider` 从各自的数据结构映射进来）。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Candidate<'a> {
    /// 展示名（电影 `title` / 剧集 `name`）。
    pub primary: &'a str,
    /// 原标题 / 原名：多语言片名主要靠它接住。
    pub original: Option<&'a str>,
    pub year: Option<i32>,
    /// 评分（0..10），只作微弱的加分项。
    pub vote: Option<f64>,
}

/// 打分结果。
#[derive(Debug, Clone, PartialEq)]
pub struct MatchScore {
    pub score: i32,
    /// 给人看的命中原因（界面上直接展示，解释「为什么排前面」）。
    pub reasons: Vec<String>,
    /// 命中排除词：整条剔除，连候选列表都不该进。
    pub excluded: bool,
    pub exclude_reason: Option<String>,
}

impl MatchScore {
    fn empty() -> Self {
        Self {
            score: 0,
            reasons: Vec::new(),
            excluded: false,
            exclude_reason: None,
        }
    }
}

/// 折叠：小写 + 去掉标点与空白，只留有意义字符。
///
/// 刻意**不复用** `grouping::normalize_title`：那个函数会抹掉年份、季集标记和
/// 噪声词，是给「归组」用的；比对片名时把年份抹掉反而会让 1984 版和 2021 版
/// 变成完全一样。
pub fn fold(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c.is_whitespace() {
            continue;
        }
        if matches!(
            c,
            '.' | '-' | '_' | ':' | ',' | '!' | '?' | '\'' | '"' | '(' | ')' | '[' | ']'
            | '、' | '·' | '：' | '，' | '（' | '）' | '【' | '】' | '《' | '》' | '～' | '~'
            | '＋' | '+' | '＆' | '&' | '／' | '/'
        ) {
            continue;
        }
        for l in c.to_lowercase() {
            out.push(l);
        }
    }
    out
}

/// 剥掉技术标签，得到干净的检索词。
///
/// `言叶之庭.2013.BD1080p.x265.mkv` 里的 `1080p` / `x265` 送进 TMDB 只会干扰
/// 检索。文件名解析通常已经剥过了，这里是**兜底**（用户手填的关键词可能带）。
pub fn strip_tech_tags(s: &str) -> String {
    let mut kept: Vec<String> = Vec::new();
    for t in split_tokens(s) {
        let t = strip_resolution_marks(&t);
        if t.is_empty() || is_tech_token(&t) {
            continue;
        }
        kept.push(t);
    }
    // CJK 连写不能插空格（`言叶之庭` 与 `之庭` 是两回事），只在西文之间补空格
    let mut out = String::new();
    for (i, t) in kept.iter().enumerate() {
        if i > 0 && needs_space(kept[i - 1].as_str(), t.as_str()) {
            out.push(' ');
        }
        out.push_str(t);
    }
    out
}

/// 切开 token：CJK 连成一片，西文数字字母连成一片，其余（`. - _ []` 等）是分隔符。
///
/// 必须区分 CJK 与西文——否则 `言叶之庭` 会被切成四个单字，拼回去就成了
/// `言 叶 之 庭`，拿去检索基本搜不到东西。
fn split_tokens(s: &str) -> Vec<String> {
    #[derive(PartialEq, Eq, Clone, Copy)]
    enum Kind {
        Cjk,
        Word,
        Sep,
    }
    fn kind_of(c: char) -> Kind {
        if c.is_alphanumeric() {
            if c.is_ascii() {
                Kind::Word
            } else {
                Kind::Cjk
            }
        } else {
            Kind::Sep
        }
    }

    let mut out: Vec<String> = Vec::new();
    let mut buf = String::new();
    let mut cur = Kind::Sep;
    for c in s.chars() {
        let k = kind_of(c);
        if k != cur && !buf.is_empty() {
            out.push(std::mem::take(&mut buf));
        }
        cur = k;
        if k != Kind::Sep {
            buf.push(c);
        }
    }
    if !buf.is_empty() {
        out.push(buf);
    }
    out
}

/// 西文与 CJK 相邻、或两段西文之间补空格；两段 CJK 之间不补。
fn needs_space(prev: &str, next: &str) -> bool {
    let a = prev.chars().last().map(is_ascii_word).unwrap_or(false);
    let b = next.chars().next().map(is_ascii_word).unwrap_or(false);
    a || b
}

fn is_ascii_word(c: char) -> bool {
    c.is_ascii_alphanumeric()
}

/// 技术标签：整枚 token 命中就丢掉。
fn is_tech_token(t: &str) -> bool {
    const TAGS: [&str; 34] = [
        "2160p", "1440p", "1080p", "720p", "480p", "2160i", "1080i", "4k", "8k", "2k",
        "x264", "x265", "h264", "h265", "hevc", "avc", "10bit", "8bit", "hdr", "hdr10",
        "sdr", "dv", "bluray", "blueray", "brrip", "bdrip", "webrip", "webdl", "hdtv",
        "dvdrip", "remux", "mp4", "mkv", "iso",
    ];
    let low = t.to_lowercase();
    TAGS.iter().any(|x| *x == low)
}

/// 复合 token 里的分辨率标记（`BD1080p` → `BD`）。
fn strip_resolution_marks(t: &str) -> String {
    let low = t.to_lowercase();
    let mut out = low.clone();
    for tag in ["2160p", "1440p", "1080p", "720p", "480p", "2160i", "1080i"] {
        out = out.replace(tag, "");
    }
    // 剥完只剩压制组缩写（`bd` / `web` …）就整枚丢掉，由调用方判空
    if matches!(out.as_str(), "bd" | "br" | "web" | "hd" | "uhd") {
        return String::new();
    }
    if out == low {
        t.to_string()
    } else {
        out
    }
}

/// 把设置里的一整串排除词切成列表（逗号 / 换行 / 分号 / 空格都认）。
pub fn split_blacklist(s: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for part in s
        .replace('\n', ",")
        .replace('；', ",")
        .replace(';', ",")
        .replace('，', ",")
        .split(',')
        .flat_map(|p| p.split_whitespace())
    {
        let t = part.trim();
        if !t.is_empty() {
            out.push(t.to_string());
        }
    }
    out.dedup();
    out
}

/// 一对「想找的」与「候选的」标题之间的相似度分。
///
/// `exact_w` 是「完全一致」时用的权重：展示名对展示名走 [`MatchWeights::exact`]，
/// 其余（别名 / 原标题）一律走 [`MatchWeights::alias`]——这样「匹配优先级」才真的
/// 可调，否则 `alias` 这个权重永远用不上。
fn pair_score(want: &str, have: &str, o: &MatchOptions, exact_w: i32) -> i32 {
    let w = fold(want);
    let h = fold(have);
    if w.is_empty() || h.is_empty() {
        return 0;
    }
    if w == h {
        return exact_w;
    }
    if w.contains(&h) || h.contains(&w) {
        return o.weights.contains;
    }
    if o.fuzzy {
        let sim = similarity(want, have);
        if sim >= FUZZY_ACCEPT {
            return (sim * o.weights.fuzzy as f64).round() as i32;
        }
    }
    0
}

/// 给一条候选打分。
///
/// 打分顺序（每一步都会往 `reasons` 里写一句人话，界面直接展示）：
/// 1. 排除词命中 → 整条出局，不再往下算；
/// 2. 片名：展示名精确 > 原标题 / 别名精确 > 包含 > 模糊，取最高的一项；
/// 3. 年份：一致 / 容差内 / 超差（扣分）；
/// 4. 季数：剧集且季数 > 1 时，首播年份应≈条目年份 -（季数-1）；
/// 5. 评分：微弱的加分，只用来给同分项定序。
pub fn score_candidate(q: &MatchQuery, o: &MatchOptions, c: &Candidate<'_>) -> MatchScore {
    let mut out = MatchScore::empty();

    // 1) 排除词：命中就整条剔除，不做任何加分
    let haystacks = [Some(c.primary), c.original];
    for term in &o.blacklist {
        let t = fold(term);
        if t.is_empty() {
            continue;
        }
        for h in haystacks.iter().flatten() {
            if fold(h).contains(&t) {
                out.excluded = true;
                out.exclude_reason = Some(format!("排除词：{term}"));
                return out;
            }
        }
    }

    // 2) 片名：展示名优先，原标题 / 别名其次
    let mut title_score = 0i32;
    let mut title_reason = String::new();
    let primary = pair_score(&q.title, c.primary, o, o.weights.exact);
    if primary > 0 {
        title_score = primary;
        title_reason = if primary >= o.weights.exact {
            "片名完全一致".to_string()
        } else if primary >= o.weights.contains {
            "片名包含匹配".to_string()
        } else {
            format!("片名模糊匹配 {:.2}", similarity(&q.title, c.primary))
        };
    }
    for alias in &q.aliases {
        for have in [Some(c.primary), c.original].iter().flatten() {
            let s = pair_score(alias, have, o, o.weights.alias);
            if s > title_score {
                title_score = s;
                title_reason = if s >= o.weights.alias {
                    format!("别名「{alias}」完全一致")
                } else if s >= o.weights.contains {
                    format!("别名「{alias}」包含匹配")
                } else {
                    format!("别名「{alias}」模糊匹配 {:.2}", similarity(alias, have))
                };
            }
        }
    }
    // 展示名与原标题**分别对上本地的两种叫法**是最强的证据，额外加一点
    let wanted: Vec<&str> = std::iter::once(q.title.as_str())
        .chain(q.aliases.iter().map(|s| s.as_str()))
        .collect();
    let hits = |have: &str| wanted.iter().any(|w| pair_score(w, have, o, 1) > 0);
    let both = c.original.is_some() && hits(c.primary) && hits(c.original.unwrap_or_default());
    if both {
        title_score += o.weights.season_hit;
        title_reason.push_str("（原标题同时命中）");
    }
    if title_score > 0 && !title_reason.is_empty() {
        out.reasons.push(title_reason);
    }
    out.score += title_score;

    // 3) 年份
    if let (Some(want), Some(have)) = (q.year, c.year) {
        let diff = (want - have).abs();
        if diff == 0 {
            out.score += o.weights.year_hit;
            out.reasons.push("年份一致".to_string());
        } else if diff <= o.year_tolerance.max(0) {
            out.score += o.weights.year_hit / 2;
            out.reasons.push(format!("年份相差 {diff} 年"));
        } else {
            out.score += o.weights.year_miss;
            out.reasons.push(format!("年份相差 {diff} 年（超出容差）"));
        }
    }

    // 4) 季数：第 N 季的首播年份 ≈ 条目年份 - (N-1)
    if let (Some(want), Some(have), Some(season)) = (q.year, c.year, q.season) {
        if season > 1 {
            let expected = want - (season - 1);
            if (expected - have).abs() <= o.year_tolerance.max(0) {
                out.score += o.weights.season_hit;
                out.reasons.push(format!("首播年份与第 {season} 季吻合"));
            }
        }
    }

    // 5) 评分：只用来给同分项定序
    if let Some(v) = c.vote {
        if v > 0.0 {
            let add = (v / 10.0 * o.weights.popularity as f64).round() as i32;
            if add > 0 {
                out.score += add;
            }
        }
    }
    out
}

/// 这组选项下，这条候选要不要留在列表里。
pub fn keeps(s: &MatchScore, o: &MatchOptions) -> bool {
    !s.excluded && s.score >= o.min_score
}

#[cfg(test)]
#[path = "tests/matching.rs"]
mod tests;
