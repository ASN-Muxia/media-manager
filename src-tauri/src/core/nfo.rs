//! NFO 的领域模型（**纯函数**，不碰文件系统）。
//!
//! NFO 是 Kodi / Jellyfin / Emby / Plex / tinyMediaManager 之间事实上的
//! 元数据交换格式。把「NFO 里有什么」和「怎么读写 NFO 文件」分开：
//! 这里只负责前者，读写在 `share/nfo.rs`。
//!
//! ⚠️ NFO 里的**类型**不是结论，只是候选：按 B4，结构证据优先于一切远程/旁证。
//! 所以 NFO 的 `<tvshow>` 只会变成 `SourceHint::Tv` 候选，
//! 由 `commands` 过 gate 后决定是否改判。

use crate::core::mediatype::SourceHint;

/// 从一份 NFO 里读出来的结构化信息（字段缺失即 None，绝不猜）。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NfoInfo {
    pub title: Option<String>,
    pub original_title: Option<String>,
    pub year: Option<i32>,
    pub tmdb_id: Option<i64>,
    pub tvdb_id: Option<i64>,
    pub season: Option<i32>,
    pub episode: Option<i32>,
    /// 根元素暗示的类型（`<movie>` / `<tvshow>` / `<episodedetails>`）。
    pub kind: Option<NfoKind>,
}

/// NFO 的根元素类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NfoKind {
    Movie,
    TvShow,
    Episode,
}

impl NfoKind {
    /// 从根元素名解析。
    pub fn from_root(root: &str) -> Option<Self> {
        match root.trim().to_ascii_lowercase().as_str() {
            "movie" => Some(NfoKind::Movie),
            "tvshow" => Some(NfoKind::TvShow),
            "episodedetails" => Some(NfoKind::Episode),
            _ => None,
        }
    }

    /// 转成判定信号候选。`<episodedetails>` 是单集，等同于剧集。
    pub fn as_source_hint(self) -> SourceHint {
        match self {
            NfoKind::Movie => SourceHint::Movie,
            NfoKind::TvShow | NfoKind::Episode => SourceHint::Tv,
        }
    }

    /// 写 NFO 时用的根元素名。
    pub fn root(self) -> &'static str {
        match self {
            NfoKind::Movie => "movie",
            NfoKind::TvShow => "tvshow",
            NfoKind::Episode => "episodedetails",
        }
    }
}

/// 一个 NFO 里可能给出的类型候选。
pub fn source_hint_of(info: &NfoInfo) -> Option<SourceHint> {
    info.kind.map(|k| k.as_source_hint())
}

/// 解析 `<title>` 之类的简单文本标签（取第一个即可）。
///
/// 只处理没有嵌套的纯文本标签——NFO 的结构非常稳定，
/// 用不上完整的 XML 解析器，正则反而更不容易被畸形文件卡住。
pub fn tag_text(xml: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = xml.find(&open)?;
    let from = start + open.len();
    let end = xml[from..].find(&close)? + from;
    let raw = &xml[from..end];
    let decoded = unescape(raw);
    let trimmed = decoded.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(trimmed.to_string())
}

/// 解析 `<uniqueid type="tmdb">123</uniqueid>` 这类带属性的标签。
pub fn unique_id(xml: &str, kind: &str) -> Option<i64> {
    // 先找所有 uniqueid 标签，再看它的 type 属性
    let mut search_from = 0usize;
    while let Some(rel) = xml[search_from..].find("<uniqueid") {
        let start = search_from + rel;
        let Some(end) = xml[start..].find('>').map(|e| start + e) else {
            break;
        };
        let open_tag = &xml[start..=end];
        let type_ok = open_tag.contains(&format!("type=\"{kind}\""))
            || open_tag.contains(&format!("type='{kind}'"));
        if type_ok {
            if let Some(close_rel) = xml[end..].find("</uniqueid>") {
                let value = xml[end + 1..end + close_rel].trim();
                if let Ok(n) = value.parse::<i64>() {
                    return Some(n);
                }
            }
        }
        search_from = end + 1;
    }
    None
}

/// 极简 XML 转义（写 NFO 用）。
pub fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nfo_kind_maps_roots() {
        assert_eq!(NfoKind::from_root("movie"), Some(NfoKind::Movie));
        assert_eq!(NfoKind::from_root("tvshow"), Some(NfoKind::TvShow));
        assert_eq!(NfoKind::from_root("episodedetails"), Some(NfoKind::Episode));
        assert_eq!(NfoKind::from_root("album"), None, "不认识的根元素不猜");
    }

    /// 单集 NFO 也算剧集证据——这是「目录里只有 1 集」还能认出是剧集的关键。
    #[test]
    fn episode_nfo_is_a_tv_hint() {
        assert_eq!(NfoKind::Episode.as_source_hint(), SourceHint::Tv);
        assert_eq!(NfoKind::Movie.as_source_hint(), SourceHint::Movie);
    }

    #[test]
    fn tag_text_reads_and_unescapes() {
        let xml = "<movie><title>卧虎藏龙 &amp; 续集</title></movie>";
        assert_eq!(tag_text(xml, "title").as_deref(), Some("卧虎藏龙 & 续集"));
        assert_eq!(tag_text(xml, "year"), None);
    }

    #[test]
    fn unique_id_reads_typed_ids() {
        let xml = r#"<movie><uniqueid type="tmdb">12345</uniqueid><uniqueid type="imdb">tt1</uniqueid></movie>"#;
        assert_eq!(unique_id(xml, "tmdb"), Some(12345));
        assert_eq!(unique_id(xml, "tvdb"), None);
    }

    #[test]
    fn escape_is_inverse_of_unescape() {
        let s = r#"a<b>c"d'e&f"#;
        assert_eq!(unescape(&escape(s)), s);
    }
}
