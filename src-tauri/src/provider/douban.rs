//! 豆瓣客户端：中文元数据刮削。
//!
//! ## 为什么是这个定位
//!
//! 豆瓣**没有官方公开 API**，所以这里用的是它自己的两个内部接口：
//!
//! 1. **检索**：`https://movie.douban.com/j/subject_suggest?q=` —— 返回一个 JSON 数组，
//!    含 id / title / year / type / episode / img，够挑出候选。
//! 2. **详情**：`https://m.douban.com/rexxar/api/v2/{movie|tv}/{id}` —— 移动端接口，
//!    返回结构化的中文元数据（导演 / 演员 / 类型 / 上映日期 / 评分 / 简介 / 海报）。
//!
//! 之所以**不走 PC 端详情页 HTML**：`movie.douban.com/subject/{id}/` 会 302 到
//! `sec.douban.com`，要求解一道 SHA-512 的工作量证明（浏览器里由 JS 自动提交），
//! 抓取方需要实现 PoW 才能通过。移动端 rexxar 接口不设这道墙，只校验 `Referer`。
//!
//! ## 两个硬约束（改这里之前先读）
//!
//! 1. **`Referer` 是必须的**：不带 `Referer` 请求 rexxar 接口会得到 **HTTP 400**，
//!    不是 200 空数据。所以 `REFERER` 常量不能删。
//! 2. **必须伪装浏览器 UA**：豆瓣按 UA 判定，`media-manager/0.1` 这种自曝身份会被拒。
//!    本模块所有请求都显式覆盖 `User-Agent`，不复用 `http::build_agent` 的默认 UA。
//!
//! ## 数据源分工（不要越界）
//!
//! 豆瓣是**中文本地化补充**，不是 TMDb 的替代品：TMDb 仍然是权威主源，
//! 豆瓣只在 TMDb 未命中、或条目缺中文信息时补位，且**只补缺失、不覆盖已有**。

use crate::provider::http;
use crate::provider::tmdb::urlencode;
use serde::Deserialize;

/// 检索接口（JSON 数组）。
const SUGGEST_BASE: &str = "https://movie.douban.com/j/subject_suggest";
/// 详情接口（移动端 rexxar API）。
const REXXAR_BASE: &str = "https://m.douban.com/rexxar/api/v2";
/// 详情接口**必须**带的 Referer，缺了会 400。
const REFERER: &str = "https://m.douban.com/movie/subject/";
/// 伪装成浏览器；豆瓣会拒绝自曝身份的 UA。
const UA: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";

// ---------------------------------------------------------------- 检索建议

/// `subject_suggest` 数组里的一项。
///
/// 注意 `id` 是**字符串**（豆瓣返回 `"id":"3001114"`），不是数字。
#[derive(Debug, Deserialize, Default, Clone)]
pub struct SuggestItem {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub title: String,
    /// 年份字符串，可能为空。
    #[serde(default)]
    pub year: String,
    /// `movie` / `tv` / 空。
    #[serde(default, rename = "type")]
    pub kind: String,
    /// 英文名/别名，可能为空。
    #[serde(default)]
    pub sub_title: String,
    /// 剧集的集数（`"64"`），电影为空串。
    #[serde(default)]
    pub episode: String,
}

impl SuggestItem {
    /// 豆瓣 id 是否可用（纯数字且非空）。
    pub fn usable_id(&self) -> Option<&str> {
        let id = self.id.trim();
        if !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()) {
            Some(id)
        } else {
            None
        }
    }

    /// 年份 → i32；`""` / 非数字一律当「没有」。
    pub fn year_value(&self) -> Option<i32> {
        parse_year(&self.year)
    }

    /// 豆瓣的类型标 `movie`/`tv` 归一成 `"movie"` / `"tv"`；其它一律 `None`。
    pub fn normalized_kind(&self) -> Option<&'static str> {
        match self.kind.trim() {
            "movie" => Some("movie"),
            "tv" => Some("tv"),
            _ => None,
        }
    }
}

/// 把可能带噪声的年份字符串解析成 i32。
///
/// 容忍 `"2021"`、`"2021 "`、`"2021-10-22"`；拒绝 `""`、`"N/A"`、越界值。
pub fn parse_year(raw: &str) -> Option<i32> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    // 取开头连续的 4 位数字即可（`2021-10-22(美国)` 也要能出 2021）。
    let digits: String = raw.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.len() < 4 {
        return None;
    }
    let v = digits[..4].parse::<i32>().ok()?;
    if (1800..=2100).contains(&v) {
        Some(v)
    } else {
        None
    }
}

// ---------------------------------------------------------------- 详情响应

/// 带 `name` 的对象（导演、演员、作者…）。
#[derive(Debug, Deserialize, Default, Clone)]
pub struct NamedEntry {
    #[serde(default)]
    pub name: String,
}

/// 海报（`pic` 字段）。
#[derive(Debug, Deserialize, Default, Clone)]
pub struct Pic {
    #[serde(default)]
    pub large: String,
    #[serde(default)]
    pub normal: String,
}

/// 评分块：`{"count":786384,"max":10,"star_count":4.0,"value":7.7}`。
#[derive(Debug, Deserialize, Default, Clone)]
pub struct Rating {
    /// 评分值；未上映时豆瓣会给 `0` 或 null。
    #[serde(default)]
    pub value: Option<f64>,
    /// 评分人数；0 表示无人评分。
    #[serde(default)]
    pub count: Option<i64>,
}

/// rexxar 详情响应。字段名与豆瓣返回逐字对齐，缺的字段靠 `#[serde(default)]` 兜住。
#[derive(Debug, Deserialize, Default, Clone)]
pub struct SubjectDetail {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub original_title: String,
    /// 年份是**字符串**。
    #[serde(default)]
    pub year: String,
    /// `movie` / `tv`。保留字段是为了和豆瓣响应逐字对齐，读不读由调用方决定。
    #[serde(default, rename = "type")]
    #[allow(dead_code)]
    pub kind: String,
    #[serde(default)]
    pub genres: Vec<String>,
    /// 上映日期数组，形如 `["2021-10-22(美国/中国大陆)"]`。
    #[serde(default)]
    pub pubdate: Vec<String>,
    /// 片长数组，形如 `["156分钟"]`。目前没有写入目标，留作后续使用。
    #[serde(default)]
    #[allow(dead_code)]
    pub durations: Vec<String>,
    /// 简介。
    #[serde(default)]
    pub intro: String,
    #[serde(default)]
    pub pic: Pic,
    #[serde(default)]
    pub cover_url: String,
    #[serde(default)]
    pub rating: Rating,
    #[serde(default)]
    pub directors: Vec<NamedEntry>,
    #[serde(default)]
    pub actors: Vec<NamedEntry>,
    /// 未上映时豆瓣会给出原因，非空即代表「没有评分」。
    #[serde(default)]
    pub null_rating_reason: String,
    /// 剧集集数；电影为 0 或缺失。
    #[serde(default)]
    pub episodes_count: Option<i64>,
}

impl SubjectDetail {
    /// 评分（0~10）。`null_rating_reason` 非空、无人评分、或值为 0 一律当「没有」。
    pub fn rating_value(&self) -> Option<f64> {
        if !self.null_rating_reason.trim().is_empty() {
            return None;
        }
        // count == 0 说明没人打分，豆瓣此时 value 也常是 0。
        if matches!(self.rating.count, Some(0)) {
            return None;
        }
        let v = self.rating.value?;
        if v > 0.0 && (0.0..=10.0).contains(&v) {
            Some(v)
        } else {
            None
        }
    }

    pub fn year_value(&self) -> Option<i32> {
        parse_year(&self.year)
    }

    /// 类型：`["剧情","科幻"]` → `"剧情 / 科幻"`（与 TMDb 的 genre 拼接口径一致）。
    pub fn genre_text(&self) -> Option<String> {
        join_names(&self.genres)
    }

    pub fn director_text(&self) -> Option<String> {
        join_named(&self.directors)
    }

    /// 演员：豆瓣可能返回几十个，只取前 [`MAX_ACTORS`] 个，避免把 `overview` 之外的
    /// 字段撑爆（这个值最终会写进 `media.genre` 旁边的展示位）。
    pub fn actor_text(&self) -> Option<String> {
        let names: Vec<&str> = self
            .actors
            .iter()
            .map(|a| a.name.trim())
            .filter(|s| !s.is_empty())
            .take(MAX_ACTORS)
            .collect();
        if names.is_empty() {
            None
        } else {
            Some(names.join(" / "))
        }
    }

    /// 上映日期：优先 `pubdate[0]`，剥掉括号里的地区后缀。
    /// 形如 `"2021-10-22(美国/中国大陆)"` → `"2021-10-22"`。
    pub fn release_date(&self) -> Option<String> {
        let raw = self.pubdate.iter().find(|s| !s.trim().is_empty())?;
        let cut = raw.split(['(', '（']).next().unwrap_or(raw).trim();
        if cut.is_empty() {
            None
        } else {
            Some(cut.to_string())
        }
    }

    /// 简介：剥掉豆瓣附加的 `©豆瓣` 之类尾巴，并压掉换行。
    pub fn overview(&self) -> Option<String> {
        let text = self.intro.trim();
        if text.is_empty() {
            return None;
        }
        let cleaned = text
            .lines()
            .map(|l| l.trim())
            .filter(|l| !l.is_empty() && !l.starts_with('©'))
            .collect::<Vec<_>>()
            .join(" ");
        if cleaned.is_empty() {
            None
        } else {
            Some(cleaned)
        }
    }

    /// 海报：优先大图，退回 `cover_url`、`pic.normal`。
    ///
    /// 豆瓣返回的是**绝对 URL**，与前端 `imageUrl()` 对 `http(s)://` 原样透传的约定一致。
    pub fn poster(&self) -> Option<String> {
        first_non_empty([
            self.pic.large.as_str(),
            self.cover_url.as_str(),
            self.pic.normal.as_str(),
        ])
    }

    /// 片长（分钟）：`["156分钟"]` → `156`。
    ///
    /// 目前 `media.duration` 走的是播放器/探测那条路，豆瓣片长暂时没有写入目标，
    /// 但解析逻辑是完整的、有测试覆盖，接上写入时直接用。
    #[allow(dead_code)]
    pub fn duration_minutes(&self) -> Option<i64> {
        let raw = self.durations.iter().find(|s| !s.trim().is_empty())?;
        // 取开头连续数字；`"2小时36分钟"` 这种也能出个 2，聊胜于无。
        let digits: String = raw
            .trim()
            .chars()
            .skip_while(|c| !c.is_ascii_digit())
            .take_while(|c| c.is_ascii_digit())
            .collect();
        if digits.is_empty() {
            return None;
        }
        let v = digits.parse::<i64>().ok()?;
        if (1..=1000).contains(&v) {
            Some(v)
        } else {
            None
        }
    }
}

/// 演员最多保留的名字数。
pub const MAX_ACTORS: usize = 12;

/// 取第一个非空串。
fn first_non_empty<const N: usize>(items: [&str; N]) -> Option<String> {
    items
        .into_iter()
        .map(|s| s.trim())
        .find(|s| !s.is_empty())
        .map(|s| s.to_string())
}

/// `["剧情","科幻"]` → `"剧情 / 科幻"`；全空返回 `None`。
fn join_names(items: &[String]) -> Option<String> {
    let names: Vec<&str> = items
        .iter()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();
    if names.is_empty() {
        None
    } else {
        Some(names.join(" / "))
    }
}

/// 把带 `name` 的对象列表拼成 `"A / B"`。
fn join_named(items: &[NamedEntry]) -> Option<String> {
    let names: Vec<&str> = items
        .iter()
        .map(|e| e.name.trim())
        .filter(|s| !s.is_empty())
        .collect();
    if names.is_empty() {
        None
    } else {
        Some(names.join(" / "))
    }
}

// ---------------------------------------------------------------- 归一化结果

/// 刮削用的**归一化**结果：只保留要写进 `media` 字段的部分。
#[derive(Debug, Default, Clone, PartialEq)]
pub struct DoubanMeta {
    /// 豆瓣条目 id（字符串形式，写入 `media.source` 关联用）。
    pub douban_id: String,
    pub title: Option<String>,
    /// 原名/英文名。
    pub original_title: Option<String>,
    pub year: Option<i32>,
    pub directors: Option<String>,
    pub actors: Option<String>,
    pub genre: Option<String>,
    pub release_date: Option<String>,
    pub rating: Option<f64>,
    pub overview: Option<String>,
    pub poster: Option<String>,
    /// 剧集集数（电影为 `None`）。
    pub episodes: Option<i64>,
}

impl DoubanMeta {
    /// 从详情响应归一化。
    pub fn from_detail(d: &SubjectDetail) -> Self {
        let id = d.id.trim();
        Self {
            douban_id: id.to_string(),
            title: non_empty(&d.title),
            original_title: non_empty(&d.original_title),
            year: d.year_value(),
            directors: d.director_text(),
            actors: d.actor_text(),
            genre: d.genre_text(),
            release_date: d.release_date(),
            rating: d.rating_value(),
            overview: d.overview(),
            poster: d.poster(),
            episodes: d.episodes_count.filter(|n| *n > 0),
        }
    }

    /// 这条元数据是否有任何**值得写库**的内容。
    ///
    /// 用来区分「刮削成功」与「接口通了但什么都没拿到」——
    /// 后者应该报匹配失败，而不是写一堆空值。
    pub fn is_meaningful(&self) -> bool {
        self.title.is_some()
            || self.rating.is_some()
            || self.overview.is_some()
            || self.poster.is_some()
    }
}

fn non_empty(s: &str) -> Option<String> {
    let t = s.trim();
    if t.is_empty() {
        None
    } else {
        Some(t.to_string())
    }
}

// ---------------------------------------------------------------- 客户端

pub struct DoubanClient {
    agent: ureq::Agent,
    /// 可选 Cookie；带登录态能提高配额、减少验证码概率，留空也能用。
    cookie: String,
    /// 会话开关：调用方拿到 `false` 时不应调用本客户端。
    enabled: bool,
}

impl DoubanClient {
    pub fn with_hosts(enabled: bool, cookie: impl Into<String>, hosts: &[String]) -> Self {
        Self {
            agent: http::build_agent(hosts, 15),
            cookie: cookie.into(),
            enabled,
        }
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// 请求头：豆瓣认 UA + Referer，Cookie 可选。
    fn headers<'a>(&'a self, referer: &'a str) -> Vec<(&'a str, &'a str)> {
        let mut h = vec![("User-Agent", UA), ("Referer", referer)];
        let c = self.cookie.trim();
        if !c.is_empty() {
            h.push(("Cookie", c));
        }
        h
    }

    /// 检索候选。`query` 为空时直接返回空列表（不发请求）。
    pub fn search(&self, query: &str) -> Result<Vec<SuggestItem>, String> {
        let q = query.trim();
        if q.is_empty() {
            return Ok(Vec::new());
        }
        let url = format!("{SUGGEST_BASE}?q={}", urlencode(q));
        // 检索接口在 movie.douban.com 下，Referer 给站点根即可。
        let headers = self.headers("https://movie.douban.com/");
        let items: Vec<SuggestItem> = http::get_json(&self.agent, "豆瓣", &url, &headers)?;
        Ok(items.into_iter().filter(|i| i.usable_id().is_some()).collect())
    }

    /// 拉取条目详情。`kind` 只认 `"movie"` / `"tv"`。
    pub fn detail(&self, kind: &str, id: &str) -> Result<SubjectDetail, String> {
        let kind = match kind {
            "tv" => "tv",
            _ => "movie",
        };
        let id = id.trim();
        if id.is_empty() || !id.bytes().all(|b| b.is_ascii_digit()) {
            return Err(format!("豆瓣 id 不合法：{id}"));
        }
        let url = format!("{REXXAR_BASE}/{kind}/{id}?ck=&for_mobile=1");
        let referer = format!("{REFERER}{id}/");
        let headers = self.headers(&referer);
        http::get_json::<SubjectDetail>(&self.agent, "豆瓣", &url, &headers)
    }

    /// 一步到位：检索 + 取最佳候选详情。
    ///
    /// 这是给刮削链路用的主入口。返回 `Ok(None)` 表示**匹配失败**（检索无结果或
    /// 候选都不可用），`Err(_)` 表示**网络/接口异常**。
    pub fn lookup(&self, title: &str, year: Option<i32>) -> Result<Option<DoubanMeta>, String> {
        let items = self.search(title)?;
        let Some(pick) = pick_best(&items, title, year) else {
            return Ok(None);
        };
        let Some(id) = pick.usable_id() else {
            return Ok(None);
        };
        let kind = pick.normalized_kind().unwrap_or("movie");
        let detail = self.detail(kind, id)?;
        let meta = DoubanMeta::from_detail(&detail);
        if meta.is_meaningful() {
            Ok(Some(meta))
        } else {
            Ok(None)
        }
    }
}

/// 从候选里挑最优的一条。
///
/// 打分规则（越高越好），全部离线可测：
/// - 标题**完全相同** +100；去空格去符号后相同 +80；包含关系 +40；否则 0
/// - 年份完全一致 +20；候选无年份 +0；年份冲突 **-30**（否决项）
/// - 有副标题/英文名 +2（说明条目信息更全）
/// - 剧集标 `tv` 且带集数 +2
///
/// 分数相同时保持豆瓣返回的原始顺序（豆瓣本身按相关性排过）。
pub fn pick_best<'a>(items: &'a [SuggestItem], title: &str, year: Option<i32>) -> Option<&'a SuggestItem> {
    let want = fold_title(title);
    let mut best: Option<(i32, &SuggestItem)> = None;
    for it in items {
        if it.usable_id().is_none() {
            continue;
        }
        let score = score_item(it, &want, year);
        // 严格大于才替换 → 同分保留先到的。
        if best.map_or(true, |(s, _)| score > s) {
            best = Some((score, it));
        }
    }
    best.map(|(_, it)| it)
}

/// 候选得分。分离成独立函数便于单测逐条断言。
pub fn score_item(it: &SuggestItem, folded_want: &str, year: Option<i32>) -> i32 {
    let folded = fold_title(&it.title);
    let mut score = if folded.is_empty() || folded_want.is_empty() {
        0
    } else if folded == folded_want {
        100
    } else if folded.contains(folded_want) || folded_want.contains(&folded) {
        40
    } else {
        0
    };

    match (year, it.year_value()) {
        (Some(a), Some(b)) if a == b => score += 20,
        (Some(_), Some(_)) => score -= 30,
        _ => {}
    }

    if !it.sub_title.trim().is_empty() {
        score += 2;
    }
    if it.normalized_kind() == Some("tv") && !it.episode.trim().is_empty() {
        score += 2;
    }
    score
}

/// 标题归一化：小写、去空格、去常见标点，用于宽松比对。
pub fn fold_title(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_whitespace())
        .filter(|c| !matches!(c, ':' | '：' | '·' | '.' | '-' | '_' | '—' | '～' | '~' | '!' | '！' | '?' | '？' | ',' | '，' | '、'))
        .flat_map(|c| c.to_lowercase())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: &str, title: &str, year: &str, kind: &str, episode: &str) -> SuggestItem {
        SuggestItem {
            id: id.to_string(),
            title: title.to_string(),
            year: year.to_string(),
            kind: kind.to_string(),
            sub_title: String::new(),
            episode: episode.to_string(),
        }
    }

    #[test]
    fn suggested_id_must_be_numeric() {
        assert_eq!(item("3001114", "沙丘", "2021", "movie", "").usable_id(), Some("3001114"));
        for bad in ["", " ", "abc", "12a", "3001114 "] {
            // 尾随空格 trim 后还是纯数字，这里要能过
            if bad.trim().bytes().all(|b| b.is_ascii_digit()) && !bad.trim().is_empty() {
                continue;
            }
            assert_eq!(item(bad, "x", "", "movie", "").usable_id(), None, "{bad:?} 应不可用");
        }
    }

    #[test]
    fn year_parsing_tolerates_noise() {
        assert_eq!(parse_year("2021"), Some(2021));
        assert_eq!(parse_year(" 2019 "), Some(2019));
        assert_eq!(parse_year("2021-10-22(美国/中国大陆)"), Some(2021));
        for bad in ["", "  ", "N/A", "abc", "20", "1799", "2200"] {
            assert_eq!(parse_year(bad), None, "{bad:?} 应解析不出年份");
        }
    }

    #[test]
    fn kind_is_normalized() {
        assert_eq!(item("1", "x", "", "movie", "").normalized_kind(), Some("movie"));
        assert_eq!(item("1", "x", "", "tv", "").normalized_kind(), Some("tv"));
        assert_eq!(item("1", "x", "", "", "").normalized_kind(), None);
        assert_eq!(item("1", "x", "", "documentary", "").normalized_kind(), None);
    }

    #[test]
    fn title_folding_strips_punctuation_and_case() {
        assert_eq!(fold_title("Dune: Part Two"), "duneparttwo");
        assert_eq!(fold_title("沙丘 2"), "沙丘2");
        assert_eq!(fold_title("Dune：Part·One"), "dunepartone");
        assert_eq!(fold_title("卧虎藏龙"), "卧虎藏龙");
    }

    #[test]
    fn best_candidate_prefers_exact_title_and_matching_year() {
        let items = vec![
            item("1293092", "沙丘", "1984", "movie", ""),
            item("3001114", "沙丘", "2021", "movie", ""),
            item("35575567", "沙丘2", "2024", "movie", ""),
        ];
        let picked = pick_best(&items, "沙丘", Some(2021)).unwrap();
        assert_eq!(picked.id, "3001114", "应挑中年份一致的那条");
    }

    #[test]
    fn a_sequel_cannot_steal_the_match() {
        // 「沙丘2」包含「沙丘」，只拿 40 分；「沙丘」满分 100。
        let items = vec![
            item("35575567", "沙丘2", "2024", "movie", ""),
            item("3001114", "沙丘", "2021", "movie", ""),
        ];
        let picked = pick_best(&items, "沙丘", None).unwrap();
        assert_eq!(picked.id, "3001114");
    }

    #[test]
    fn year_conflict_is_penalized() {
        // 同名但年份错的那个应该输给年份对的那个。
        let want = fold_title("沙丘");
        let right = score_item(&item("a", "沙丘", "2021", "movie", ""), &want, Some(2021));
        let wrong = score_item(&item("b", "沙丘", "1984", "movie", ""), &want, Some(2021));
        assert!(right > wrong, "年份一致({right}) 应高于年份冲突({wrong})");
        assert_eq!(right - wrong, 50, "差别应是 +20 与 -30 之和");
    }

    #[test]
    fn no_year_hint_still_picks_something() {
        let items = vec![item("3001114", "沙丘", "2021", "movie", "")];
        assert_eq!(pick_best(&items, "沙丘", None).unwrap().id, "3001114");
    }

    #[test]
    fn empty_candidates_yield_none() {
        assert!(pick_best(&[], "沙丘", Some(2021)).is_none());
        // 候选 id 非法时也要返回 None，而不是 panic。
        assert!(pick_best(&[item("", "沙丘", "2021", "movie", "")], "沙丘", None).is_none());
    }

    #[test]
    fn ties_keep_douban_ordering() {
        let items = vec![
            item("111", "同名剧", "2020", "movie", ""),
            item("222", "同名剧", "2020", "movie", ""),
        ];
        // 完全同分 → 保留先到的（豆瓣自己的相关性排序）。
        assert_eq!(pick_best(&items, "同名剧", None).unwrap().id, "111");
    }

    #[test]
    fn detail_fields_are_normalized() {
        let raw = serde_json::json!({
            "id": "3001114",
            "title": "沙丘",
            "original_title": "Dune",
            "year": "2021",
            "type": "movie",
            "genres": ["剧情", "科幻", "冒险"],
            "pubdate": ["2021-10-22(美国/中国大陆)"],
            "durations": ["156分钟"],
            "intro": "电影《沙丘》为观众呈现了一段神秘而感人至深的英雄之旅。\n©豆瓣",
            "pic": {
                "large": "https://img9.doubanio.com/view/photo/m_ratio_poster/public/p2687443734.jpg",
                "normal": "https://img9.doubanio.com/view/photo/s_ratio_poster/public/p2687443734.jpg"
            },
            "cover_url": "https://img9.doubanio.com/view/photo/m_ratio_poster/public/p2687443734.jpg",
            "rating": {"count": 786384, "max": 10, "star_count": 4.0, "value": 7.7},
            "directors": [{"name": "丹尼斯·维伦纽瓦"}],
            "actors": [{"name": "提莫西·查拉梅"}, {"name": "丽贝卡·弗格森"}],
            "null_rating_reason": ""
        });
        let d: SubjectDetail = serde_json::from_value(raw).unwrap();
        let m = DoubanMeta::from_detail(&d);

        assert_eq!(m.douban_id, "3001114");
        assert_eq!(m.title.as_deref(), Some("沙丘"));
        assert_eq!(m.original_title.as_deref(), Some("Dune"));
        assert_eq!(m.year, Some(2021));
        assert_eq!(m.genre.as_deref(), Some("剧情 / 科幻 / 冒险"));
        assert_eq!(m.directors.as_deref(), Some("丹尼斯·维伦纽瓦"));
        assert_eq!(m.actors.as_deref(), Some("提莫西·查拉梅 / 丽贝卡·弗格森"));
        assert_eq!(m.release_date.as_deref(), Some("2021-10-22"));
        assert_eq!(m.rating, Some(7.7));
        assert_eq!(m.poster.as_deref(), Some("https://img9.doubanio.com/view/photo/m_ratio_poster/public/p2687443734.jpg"));
        assert!(!m.overview.as_deref().unwrap().contains("©豆瓣"), "版权尾巴应被剥掉");
        assert!(m.is_meaningful());
    }

    #[test]
    fn tv_detail_carries_episode_count() {
        let raw = serde_json::json!({
            "id": "27094895",
            "title": "大明风华",
            "original_title": "",
            "year": "2019",
            "type": "tv",
            "genres": ["剧情", "古装"],
            "pubdate": ["2019-12-17(中国大陆)"],
            "intro": "故事背景设定在明朝建文四年。",
            "rating": {"count": 86809, "value": 6.0},
            "directors": [{"name": "张挺"}],
            "actors": [{"name": "汤唯"}, {"name": "朱亚文"}],
            "episodes_count": 64
        });
        let d: SubjectDetail = serde_json::from_value(raw).unwrap();
        let m = DoubanMeta::from_detail(&d);
        assert_eq!(m.title.as_deref(), Some("大明风华"));
        assert_eq!(m.year, Some(2019));
        assert_eq!(m.rating, Some(6.0));
        assert_eq!(m.episodes, Some(64));
        assert_eq!(m.release_date.as_deref(), Some("2019-12-17"));
        assert!(m.original_title.is_none(), "豆瓣返回空串应归一成 None");
    }

    #[test]
    fn rating_rejects_unreleased_and_unrated() {
        // 未上映：豆瓣给出处说明，没有评分。
        let mut d = SubjectDetail {
            rating: Rating { value: Some(0.0), count: Some(0) },
            null_rating_reason: "尚未上映".to_string(),
            ..Default::default()
        };
        assert_eq!(d.rating_value(), None);

        d.null_rating_reason.clear();
        assert_eq!(d.rating_value(), None, "count=0 视为无人评分");

        d.rating = Rating { value: Some(7.7), count: Some(100) };
        assert_eq!(d.rating_value(), Some(7.7));

        // 越界值要被挡住。
        d.rating = Rating { value: Some(99.0), count: Some(10) };
        assert_eq!(d.rating_value(), None);
        d.rating = Rating { value: None, count: Some(10) };
        assert_eq!(d.rating_value(), None);
    }

    #[test]
    fn actors_are_capped() {
        let many: Vec<NamedEntry> = (0..40)
            .map(|i| NamedEntry { name: format!("演员{i}") })
            .collect();
        let d = SubjectDetail { actors: many, ..Default::default() };
        let text = d.actor_text().unwrap();
        assert_eq!(text.split(" / ").count(), MAX_ACTORS, "演员数应被截断到 {MAX_ACTORS}");
        assert_eq!(SubjectDetail::default().actor_text(), None);
    }

    #[test]
    fn poster_falls_back_through_candidates() {
        let mut d = SubjectDetail::default();
        assert_eq!(d.poster(), None);

        d.pic.normal = "n.jpg".to_string();
        assert_eq!(d.poster().as_deref(), Some("n.jpg"));

        d.cover_url = "c.jpg".to_string();
        assert_eq!(d.poster().as_deref(), Some("c.jpg"), "cover_url 优先于 normal");

        d.pic.large = "l.jpg".to_string();
        assert_eq!(d.poster().as_deref(), Some("l.jpg"), "large 优先级最高");
    }

    #[test]
    fn duration_parsing() {
        let d = SubjectDetail { durations: vec!["156分钟".into()], ..Default::default() };
        assert_eq!(d.duration_minutes(), Some(156));
        let d = SubjectDetail { durations: vec![], ..Default::default() };
        assert_eq!(d.duration_minutes(), None);
        let d = SubjectDetail { durations: vec!["片长未知".into()], ..Default::default() };
        assert_eq!(d.duration_minutes(), None);
        // 多段片长只取第一段；越界值要挡住
        let d = SubjectDetail {
            durations: vec!["50分钟".into(), "60分钟".into()],
            ..Default::default()
        };
        assert_eq!(d.duration_minutes(), Some(50));
        let d = SubjectDetail { durations: vec!["99999分钟".into()], ..Default::default() };
        assert_eq!(d.duration_minutes(), None);
    }

    #[test]
    fn meaningless_detail_is_not_a_success() {
        // 接口通了但一片空白 → 不能算"刮削成功"。
        let d = SubjectDetail::default();
        assert!(!DoubanMeta::from_detail(&d).is_meaningful());
    }

    #[test]
    fn client_respects_enabled_flag_and_blank_query() {
        assert!(!DoubanClient::with_hosts(false, "", &[]).enabled());
        assert!(DoubanClient::with_hosts(true, "", &[]).enabled());
        let c = DoubanClient::with_hosts(true, "", &[]);
        // 空查询不该发网络请求，直接返回空列表。
        assert!(c.search("   ").unwrap().is_empty());
    }

    #[test]
    fn detail_rejects_bad_id_without_request() {
        let c = DoubanClient::with_hosts(true, "", &[]);
        let err = c.detail("movie", "abc").unwrap_err();
        assert!(err.contains("id 不合法"), "实际错误：{err}");
        assert!(c.detail("movie", "").is_err());
    }

    #[test]
    fn headers_carry_ua_and_optional_cookie() {
        let c = DoubanClient::with_hosts(true, "", &[]);
        let h = c.headers("https://example.com/");
        assert!(h.iter().any(|(k, v)| *k == "User-Agent" && v.contains("Mozilla")));
        assert!(h.iter().any(|(k, _)| *k == "Referer"));
        assert!(!h.iter().any(|(k, _)| *k == "Cookie"), "没配 Cookie 就不该带");

        let c = DoubanClient::with_hosts(true, "bid=abc", &[]);
        let h = c.headers("https://example.com/");
        assert!(h.iter().any(|(k, v)| *k == "Cookie" && *v == "bid=abc"));
    }
}
