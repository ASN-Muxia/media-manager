//! 文件名解析：从视频文件名推断 标题 / 年份 / 季 / 集 / TMDB id。
//!
//! 覆盖常见命名：
//! - `The.Matrix.1999.1080p.BluRay.x264.mkv`
//! - `Inception (2010) [1080p].mkv`
//! - `Show.Name.S01E02.1080p.WEB-DL.mkv`
//! - `Show Name - S02E10 - Episode Title.mkv` / `Show Name - 01.mkv`
//! - `Doctor Who Season 4 Episode 3.mkv`
//! - `剧名.第一季.E01.2021.1080p.mkv`
//! - `[字幕组] 剧名 - 01 [1080P][简体].mp4`
//! - `动画名 第01话.mp4`
//! - `Movie.2021.2160p.WEB-DL.DDP5.1.HDR.x265-GROUP.mkv`

use regex::Regex;
use std::sync::OnceLock;

/// 视频扩展名白名单（小写，含点）。
pub const VIDEO_EXTS: &[&str] = &[
    ".mkv", ".mp4", ".avi", ".rmvb", ".rm", ".mov", ".wmv", ".flv", ".ts", ".m2ts",
    ".mpg", ".mpeg", ".m4v", ".webm", ".vob", ".iso", ".3gp",
];

/// 解析结果。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParsedName {
    pub title: String,
    pub year: Option<i32>,
    pub season: Option<i32>,
    pub episode: Option<i32>,
    pub tmdb_id: Option<i64>,
}

impl ParsedName {
    /// 带季/集信息即视为剧集，否则视为电影。
    pub fn media_type(&self) -> &'static str {
        if self.season.is_some() || self.episode.is_some() {
            "tv"
        } else {
            "movie"
        }
    }
}

/// 判断是否为受支持的视频文件。
pub fn is_video_file(file_name: &str) -> bool {
    let lower = file_name.to_ascii_lowercase();
    VIDEO_EXTS.iter().any(|e| lower.ends_with(e))
}

/* ------------------------- 正则（惰性编译） ------------------------- */

macro_rules! lazy_re {
    ($fn_name:ident, $pat:expr) => {
        fn $fn_name() -> &'static Regex {
            static R: OnceLock<Regex> = OnceLock::new();
            R.get_or_init(|| Regex::new($pat).expect("内置正则应可编译"))
        }
    };
}

lazy_re!(re_sxxexx, r"(?i)s(\d{1,2})[\s\-_]*e(\d{1,3})");
lazy_re!(re_1x02, r"(?i)\b(\d{1,2})x(\d{2,3})\b");
lazy_re!(re_ep_cn, r"第\s*(\d{1,3})\s*[集话話回]");
lazy_re!(re_season_cn, r"第\s*([0-9一二三四五六七八九十]{1,3})\s*季");
lazy_re!(re_season_en, r"(?i)\bseason[\s\-_]*(\d{1,2})\b");
lazy_re!(re_episode_word, r"(?i)\bepisodes?[\s\-_.]*(\d{1,3})\b");
lazy_re!(re_ep_en, r"(?i)\b(?:ep|e)[\s\-_.]*(\d{1,3})\b");
lazy_re!(re_year, r"\b(18\d{2}|19\d{2}|20\d{2})\b");
lazy_re!(re_tmdb, r"(?i)tmdb[\-_=]?\s*(\d{3,})");
lazy_re!(re_dash_ep, r"[\-–—]\s*(\d{2,3})\s*$");

// 注意：这里必须是**单行**字面量。Rust 原始字符串不支持行尾反斜杠续行，
// 换行会被当成模式的一部分，导致被折行的那几个标签永远匹配不上。
lazy_re!(
    re_ascii_tag,
    r"(?i)\b(2160p|1440p|1080p|720p|576p|480p|8k|4k|bluray|blu-ray|bdrip|brrip|bdmv|webrip|web-dl|webdl|web|hdtv|dvdrip|dvd|hdrip|remux|hdr10|hdr|dolbyvision|dv|x264|x265|h264|h265|hevc|avc|10bit|8bit|aac|ac3|eac3|dts|ddp|dd5|truehd|atmos|flac|mp3|opus|repack|proper|extended|unrated|imax|internal|multi|dual|chs|cht|eng|vostfr|subs|sub)\b"
);

/// CJK 标签（`\b` 对中文无效，用子串查找）。
const CJK_TAGS: &[&str] = &[
    "中字", "英字", "双字", "双语", "简体", "繁体", "简中", "繁中", "国语", "粤语", "国粤",
    "中英", "内嵌", "内封", "外挂", "字幕", "压制", "高清", "蓝光", "完整版", "未删减",
    "加长版", "收藏版", "特效", "无水印", "重制版", "合集",
];

/// 把 `.` `_` 等分隔符统一成空格，并压缩空白。
fn normalize_separators(stem: &str) -> String {
    let mut out = String::with_capacity(stem.len());
    for ch in stem.chars() {
        match ch {
            '.' | '_' | '＋' => out.push(' '),
            _ => out.push(ch),
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// 去掉**所有**括号组（`[] () {} 【】 〔〕`）并压缩空白。
///
/// 关键点：`[字幕组]` 这类发行组前缀必须在标签识别**之前**去掉，否则其中的
/// “字幕”二字会被当成字幕语言标签，把标题整段截掉（曾经的线上 bug）。
/// 年份等数字信息另有来源（见 `parse_stem`），因此这里可以放心全部去除。
fn strip_all_groups(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut depth = 0i32;
    for ch in s.chars() {
        match ch {
            '[' | '(' | '{' | '【' | '〔' => depth += 1,
            ']' | ')' | '}' | '】' | '〕' => {
                if depth > 0 {
                    depth -= 1;
                }
            }
            _ => {
                if depth == 0 {
                    out.push(ch);
                }
            }
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// 找出版本标签在字符串中最早出现的字节下标。
fn first_tag_index(s: &str) -> Option<usize> {
    let mut best = re_ascii_tag().find(s).map(|m| m.start());
    for tag in CJK_TAGS {
        if let Some(i) = s.find(tag) {
            best = Some(best.map_or(i, |b| b.min(i)));
        }
    }
    best
}

/// 找出季/集/年份标记片段的起始下标（用于截断标题）。
fn first_se_index(s: &str) -> Option<usize> {
    let mut best: Option<usize> = None;
    let mut consider = |i: usize| {
        best = Some(best.map_or(i, |b| b.min(i)));
    };
    for re in [
        re_sxxexx(),
        re_1x02(),
        re_ep_cn(),
        re_season_cn(),
        re_season_en(),
        re_episode_word(),
        re_ep_en(),
        re_dash_ep(),
        re_tmdb(),
    ] {
        if let Some(m) = re.find(s) {
            consider(m.start());
        }
    }
    best
}

fn parse_cn_number(s: &str) -> Option<i32> {
    if let Ok(n) = s.parse::<i32>() {
        return Some(n);
    }
    let mut total = 0i32;
    let mut cur = 0i32;
    for ch in s.chars() {
        let v = match ch {
            '一' => 1,
            '二' | '两' => 2,
            '三' => 3,
            '四' => 4,
            '五' => 5,
            '六' => 6,
            '七' => 7,
            '八' => 8,
            '九' => 9,
            '十' => {
                cur = if cur == 0 { 1 } else { cur };
                total += cur * 10;
                cur = 0;
                continue;
            }
            _ => return None,
        };
        cur = v;
    }
    Some(total + cur)
}

/// 清理标题：去括号残骸、压缩空白、修剪两端分隔符。
fn clean_title(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '[' | ']' | '{' | '}' | '【' | '】' | '〔' | '〕' => out.push(' '),
            _ => out.push(ch),
        }
    }
    let cleaned = out.split_whitespace().collect::<Vec<_>>().join(" ");
    cleaned
        .trim_matches(|c: char| c == '-' || c == '–' || c == '—' || c.is_whitespace())
        .trim()
        .to_string()
}

/// 解析文件名（可含扩展名），返回结构化结果。
pub fn parse_file_name(file_name: &str) -> ParsedName {
    let stem = match file_name.rfind('.') {
        Some(i) if !file_name[i + 1..].contains(' ') => &file_name[..i],
        _ => file_name,
    };
    parse_stem(stem)
}

/// 解析不含扩展名的文件名主干。
pub fn parse_stem(stem: &str) -> ParsedName {
    let norm = normalize_separators(stem);
    let mut result = ParsedName::default();

    // 1) TMDB id：在**未去括号**的串上找，因为标记本身写在 `{tmdb-123}` 里
    if let Some(c) = re_tmdb().captures(&norm) {
        result.tmdb_id = c.get(1).and_then(|m| m.as_str().parse::<i64>().ok());
    }

    // 2) 去掉所有括号组，得到用于识别季/集与标题的干净串
    let det = strip_all_groups(&norm);

    // 3) 季 / 集
    if let Some(c) = re_sxxexx().captures(&det) {
        result.season = c.get(1).and_then(|m| m.as_str().parse().ok());
        result.episode = c.get(2).and_then(|m| m.as_str().parse().ok());
    } else if let Some(c) = re_1x02().captures(&det) {
        result.season = c.get(1).and_then(|m| m.as_str().parse().ok());
        result.episode = c.get(2).and_then(|m| m.as_str().parse().ok());
    } else if let Some(c) = re_ep_cn().captures(&det) {
        result.episode = c.get(1).and_then(|m| m.as_str().parse().ok());
    } else if let Some(c) = re_episode_word().captures(&det) {
        result.episode = c.get(1).and_then(|m| m.as_str().parse().ok());
    } else if let Some(c) = re_ep_en().captures(&det) {
        result.episode = c.get(1).and_then(|m| m.as_str().parse().ok());
    } else if let Some(c) = re_dash_ep().captures(&det) {
        // `Show Name - 01`：无季信息，按第 1 季处理
        result.episode = c.get(1).and_then(|m| m.as_str().parse().ok());
    }
    if let Some(c) = re_season_cn().captures(&det) {
        result.season = c.get(1).and_then(|m| parse_cn_number(m.as_str()));
    } else if let Some(c) = re_season_en().captures(&det) {
        result.season = c.get(1).and_then(|m| m.as_str().parse().ok());
    }
    if result.episode.is_some() && result.season.is_none() {
        result.season = Some(1);
    }

    // 4) 标题区域 = 第一个标签 / 第一个季集标记之前的部分
    let end = {
        let tag = first_tag_index(&det);
        let se = first_se_index(&det);
        match (tag, se) {
            (Some(a), Some(b)) => a.min(b),
            (Some(a), None) => a,
            (None, Some(b)) => b,
            (None, None) => det.len(),
        }
    };
    let mut region = det[..end].to_string();

    // 5) 年份：优先取标题区域内最后一个年份（并把它从标题里摘掉）；
    //    区域里没有才回退到全串查找——回退时**不改动** region，
    //    避免用全串偏移去切割 region 造成越界/错切。
    let year_in = |s: &str| -> Option<(i32, std::ops::Range<usize>)> {
        let mut last: Option<(i32, std::ops::Range<usize>)> = None;
        for m in re_year().find_iter(s) {
            if let Ok(y) = m.as_str().parse::<i32>() {
                last = Some((y, m.range()));
            }
        }
        last
    };
    match year_in(&region) {
        Some((y, range)) => {
            result.year = Some(y);
            region.replace_range(range, " ");
        }
        None => {
            result.year = year_in(&norm).map(|(y, _)| y);
        }
    }

    // 6) 兜底清理残留标记
    let mut region = re_sxxexx().replace_all(&region, " ").to_string();
    region = re_1x02().replace_all(&region, " ").to_string();
    region = re_ep_cn().replace_all(&region, " ").to_string();
    region = re_episode_word().replace_all(&region, " ").to_string();
    region = re_ep_en().replace_all(&region, " ").to_string();
    region = re_dash_ep().replace_all(&region, " ").to_string();
    region = re_season_cn().replace_all(&region, " ").to_string();
    region = re_season_en().replace_all(&region, " ").to_string();

    let title = clean_title(&region);
    result.title = if title.is_empty() {
        clean_title(&det[..end.min(det.len())])
    } else {
        title
    };

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(name: &str) -> ParsedName {
        parse_file_name(name)
    }

    #[test]
    fn video_ext_whitelist() {
        assert!(is_video_file("a.MKV"));
        assert!(is_video_file("b.mp4"));
        assert!(!is_video_file("cover.jpg"));
        assert!(!is_video_file("subs.srt"));
        assert!(!is_video_file("noext"));
    }

    #[test]
    fn movie_dotted_with_quality_tags() {
        let r = p("The.Matrix.1999.1080p.BluRay.x264-GROUP.mkv");
        assert_eq!(r.title, "The Matrix");
        assert_eq!(r.year, Some(1999));
        assert_eq!(r.episode, None);
        assert_eq!(r.media_type(), "movie");
    }

    #[test]
    fn movie_with_parenthesised_year() {
        let r = p("Inception (2010) [1080p].mkv");
        assert_eq!(r.title, "Inception");
        assert_eq!(r.year, Some(2010));
    }

    #[test]
    fn movie_title_that_is_a_year() {
        let r = p("1917.2019.1080p.BluRay.x264.mkv");
        assert_eq!(r.title, "1917");
        assert_eq!(r.year, Some(2019));
    }

    #[test]
    fn movie_title_ends_with_year_like_number() {
        let r = p("Blade.Runner.2049.2017.2160p.WEB-DL.mkv");
        assert_eq!(r.title, "Blade Runner 2049");
        assert_eq!(r.year, Some(2017));
    }

    #[test]
    fn tv_sxxexx() {
        let r = p("Breaking.Bad.S03E07.720p.HDTV.mkv");
        assert_eq!(r.title, "Breaking Bad");
        assert_eq!(r.season, Some(3));
        assert_eq!(r.episode, Some(7));
        assert_eq!(r.media_type(), "tv");
    }

    #[test]
    fn tv_season_episode_with_spaces() {
        let r = p("Show Name - S02E10 - Episode Title.mkv");
        assert_eq!(r.season, Some(2));
        assert_eq!(r.episode, Some(10));
        assert_eq!(r.title, "Show Name");
    }

    #[test]
    fn tv_1x02_form() {
        let r = p("Some.Show.1x02.720p.mkv");
        assert_eq!(r.season, Some(1));
        assert_eq!(r.episode, Some(2));
        assert_eq!(r.title, "Some Show");
    }

    #[test]
    fn tv_dash_absolute_episode() {
        let r = p("Show Name - 01.mkv");
        assert_eq!(r.title, "Show Name");
        assert_eq!(r.episode, Some(1));
        assert_eq!(r.season, Some(1));
    }

    #[test]
    fn tv_chinese_episode_marker() {
        let r = p("动画名 第01话.mp4");
        assert_eq!(r.title, "动画名");
        assert_eq!(r.episode, Some(1));
        assert_eq!(r.season, Some(1));
    }

    #[test]
    fn tv_chinese_season_and_ep() {
        let r = p("剧名.第一季.E01.2021.1080p.mkv");
        assert_eq!(r.season, Some(1));
        assert_eq!(r.episode, Some(1));
        assert_eq!(r.year, Some(2021));
        assert_eq!(r.title, "剧名");
    }

    #[test]
    fn tv_season_word_form() {
        let r = p("Doctor Who Season 4 Episode 3.mkv");
        assert_eq!(r.season, Some(4));
        assert_eq!(r.episode, Some(3));
        assert_eq!(r.title, "Doctor Who");
    }

    #[test]
    fn fansub_leading_group_is_stripped() {
        let r = p("[字幕组] 剧名 - 01 [1080P][简体].mp4");
        assert_eq!(r.title, "剧名");
        assert_eq!(r.episode, Some(1));
    }

    /// 回归用例：`[字幕组]` 里的“字幕”二字历史上会被误判成字幕语言标签，
    /// 导致标题被整段截掉为空串。
    #[test]
    fn fansub_group_is_not_mistaken_for_subtitle_tag() {
        for name in [
            "[字幕组] Movie.2020.1080p.mkv",
            "[某某字幕组] Some Show S01E01.mkv",
            "[字幕组]剧名.第01话.mkv",
        ] {
            let r = p(name);
            assert!(
                !r.title.is_empty(),
                "{name} 的标题不应为空串（实际 {r:?}）"
            );
        }
    }

    #[test]
    fn tmdb_id_tag_is_extracted() {
        let r = p("Some Movie (2020) {tmdb-12345} 1080p.mkv");
        assert_eq!(r.tmdb_id, Some(12345));
        assert_eq!(r.year, Some(2020));
        assert_eq!(r.title, "Some Movie");
    }

    #[test]
    fn hevc_multi_tag_movie() {
        let r = p("Dune.2021.2160p.WEB-DL.DDP5.1.HDR.x265-CMCT.mkv");
        assert_eq!(r.title, "Dune");
        assert_eq!(r.year, Some(2021));
        assert_eq!(r.media_type(), "movie");
    }

    #[test]
    fn year_after_quality_tag_still_detected() {
        let r = p("Some.Movie.1080p.2018.mkv");
        assert_eq!(r.year, Some(2018));
        assert_eq!(r.title, "Some Movie");
    }

    #[test]
    fn empty_and_extension_only_input_is_safe() {
        assert_eq!(p("").title, "");
        assert_eq!(p(".mkv").title, "");
        let r = p("PlainName.mkv");
        assert_eq!(r.title, "PlainName");
        assert_eq!(r.year, None);
    }

    #[test]
    fn trailing_release_group_after_tags_is_ignored() {
        let r = p("Interstellar.2014.1080p.BluRay.x264.DTS-FGT.mkv");
        assert_eq!(r.title, "Interstellar");
        assert_eq!(r.year, Some(2014));
    }

    #[test]
    fn tag_regex_matches_every_listed_token() {
        // 防止原始字符串被折行导致部分标签失效
        for token in ["bdmv", "dv", "opus", "1080p", "x265", "web-dl", "hdr10"] {
            let s = format!("Title.2020.{token}.mkv");
            let r = p(&s);
            assert!(r.year == Some(2020), "{token} 应能被识别为标签（{r:?}）");
        }
    }
}
