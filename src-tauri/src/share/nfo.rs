//! NFO 文件的读与写（Kodi / Jellyfin / tinyMediaManager 兼容）。
//!
//! ## 红线：不碰视频文件
//! - 写操作的目标永远是 `*.nfo`；
//! - **默认不覆盖**已存在的文件（`overwrite = false`），只跳过；
//! - 从不以写模式打开任何视频扩展名的文件。
//!
//! 导入时 NFO 给出的类型只是 `SourceHint` 候选，由 `commands` 过 gate，
//! 不会直接推翻 `core` 的结构证据判定（B4）。

use crate::core::nfo::{escape, tag_text, unique_id, NfoInfo, NfoKind};
use std::path::{Path, PathBuf};

/// 视频扩展名（用于「绝不写这些文件」的兜底检查）。
const VIDEO_EXTS: &[&str] = &[
    "mp4", "mkv", "avi", "rmvb", "flv", "mov", "wmv", "ts", "m2ts", "webm", "mpg", "mpeg",
    "m4v", "dat", "vob", "iso", "rm", "asf", "divx", "ogm",
];

/// 这个路径是不是视频文件（写之前用它兜底拦一次）。
pub fn is_video_path(p: &Path) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .map(|e| VIDEO_EXTS.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

/// NFO 文件应有的扩展名。
pub const NFO_EXT: &str = "nfo";

/// 写一份电影 / 剧集的 NFO。
///
/// `dir` 是目标目录；`base` 是不带扩展名的文件名（电影用 `movie`，剧集用 `tvshow`）。
/// 返回实际的文件路径；`overwrite=false` 且文件已存在时返回 `None`。
pub fn write_nfo(
    dir: &Path,
    base: &str,
    kind: NfoKind,
    info: &NfoInfo,
    overwrite: bool,
) -> Result<Option<PathBuf>, String> {
    let path = dir.join(format!("{base}.{NFO_EXT}"));
    // 兜底一：调用方若把「完整文件名」当 base 传进来（带 .mkv 之类），直接拒绝。
    if Path::new(base)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| VIDEO_EXTS.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
    {
        return Err(format!("拒绝以视频文件名作为 NFO 名：{base}"));
    }
    // 兜底二：最终路径也不允许落在视频文件上（双保险）。
    if is_video_path(&path) {
        return Err(format!("拒绝写入视频文件：{}", path.display()));
    }
    if path.exists() && !overwrite {
        return Ok(None);
    }
    let xml = render(kind, info);
    std::fs::write(&path, xml).map_err(|e| format!("写入 NFO 失败（{}）：{e}", path.display()))?;
    Ok(Some(path))
}

/// 渲染 NFO 文本（纯函数，便于单测）。
pub fn render(kind: NfoKind, info: &NfoInfo) -> String {
    let root = kind.root();
    let mut out = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\" ?>\n");
    out.push_str(&format!("<{root}>\n"));
    if let Some(t) = &info.title {
        out.push_str(&format!("  <title>{}</title>\n", escape(t)));
    }
    if let Some(t) = &info.original_title {
        out.push_str(&format!("  <originaltitle>{}</originaltitle>\n", escape(t)));
    }
    if let Some(y) = info.year {
        out.push_str(&format!("  <year>{y}</year>\n"));
    }
    if let Some(s) = info.season {
        out.push_str(&format!("  <season>{s}</season>\n"));
    }
    if let Some(e) = info.episode {
        out.push_str(&format!("  <episode>{e}</episode>\n"));
    }
    if let Some(id) = info.tmdb_id {
        out.push_str(&format!("  <uniqueid type=\"tmdb\">{id}</uniqueid>\n"));
    }
    if let Some(id) = info.tvdb_id {
        out.push_str(&format!("  <uniqueid type=\"tvdb\">{id}</uniqueid>\n"));
    }
    out.push_str(&format!("</{root}>\n"));
    out
}

/// 解析一份 NFO（宽容解析：缺字段就是 None，畸形文本也不 panic）。
pub fn parse(xml: &str) -> NfoInfo {
    let root = root_element(xml);
    NfoInfo {
        title: tag_text(xml, "title"),
        original_title: tag_text(xml, "originaltitle").or_else(|| tag_text(xml, "original_title")),
        year: tag_text(xml, "year")
            .and_then(|y| y.trim().parse::<i32>().ok())
            .or_else(|| {
                // 有些工具把年份写成 2000-2005 或带日期，取前 4 位数字
                tag_text(xml, "premiered")
                    .or_else(|| tag_text(xml, "releasedate"))
                    .and_then(|d| d.get(0..4).and_then(|s| s.parse::<i32>().ok()))
            }),
        tmdb_id: unique_id(xml, "tmdb").or_else(|| tag_text(xml, "tmdbid").and_then(|s| s.parse().ok())),
        tvdb_id: unique_id(xml, "tvdb").or_else(|| tag_text(xml, "tvdbid").and_then(|s| s.parse().ok())),
        season: tag_text(xml, "season").and_then(|s| s.trim().parse::<i32>().ok()),
        episode: tag_text(xml, "episode").and_then(|s| s.trim().parse::<i32>().ok()),
        kind: root.as_deref().and_then(NfoKind::from_root),
    }
}

/// 取根元素名。
fn root_element(xml: &str) -> Option<String> {
    let start = xml.find('<')?;
    let rest = &xml[start + 1..];
    // 跳过 `<?xml ... ?>` 声明
    let rest = if rest.starts_with("?xml") {
        let end = rest.find("?>")? + 2;
        let s2 = &rest[end..];
        let s = s2.find('<')?;
        &s2[s + 1..]
    } else {
        rest
    };
    let end = rest.find(|c: char| c == '>' || c.is_whitespace())?;
    Some(rest[..end].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let mut d = std::env::temp_dir();
        d.push(format!(
            "mm-nfo-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn renders_movie_nfo_with_escaping() {
        let info = NfoInfo {
            title: Some("卧虎藏龙 & 续集".to_string()),
            year: Some(2000),
            tmdb_id: Some(1390),
            kind: Some(NfoKind::Movie),
            ..Default::default()
        };
        let xml = render(NfoKind::Movie, &info);
        assert!(xml.starts_with("<?xml"));
        assert!(xml.contains("<movie>"));
        assert!(xml.contains("<title>卧虎藏龙 &amp; 续集</title>"));
        assert!(xml.contains("<uniqueid type=\"tmdb\">1390</uniqueid>"));
    }

    #[test]
    fn parses_back_what_it_rendered() {
        let info = NfoInfo {
            title: Some("卧虎藏龙".to_string()),
            original_title: Some("Crouching Tiger".to_string()),
            year: Some(2000),
            tmdb_id: Some(1390),
            kind: Some(NfoKind::Movie),
            ..Default::default()
        };
        let xml = render(NfoKind::Movie, &info);
        let back = parse(&xml);
        assert_eq!(back.title.as_deref(), Some("卧虎藏龙"));
        assert_eq!(back.original_title.as_deref(), Some("Crouching Tiger"));
        assert_eq!(back.year, Some(2000));
        assert_eq!(back.tmdb_id, Some(1390));
        assert_eq!(back.kind, Some(NfoKind::Movie));
    }

    #[test]
    fn parses_tvshow_root_and_season_episode() {
        let xml = "<?xml version=\"1.0\"?>\n<tvshow><title>神秘博士</title>\
                   <year>2005</year><uniqueid type=\"tvdb\">78804</uniqueid></tvshow>";
        let info = parse(xml);
        assert_eq!(info.kind, Some(NfoKind::TvShow));
        assert_eq!(info.tvdb_id, Some(78804));
        assert_eq!(info.year, Some(2005));
    }

    /// 默认不覆盖：这是「不改动已有文件」的硬要求。
    #[test]
    fn write_nfo_skips_existing_file_by_default() {
        let d = temp_dir("skip");
        let info = NfoInfo {
            title: Some("电影".to_string()),
            kind: Some(NfoKind::Movie),
            ..Default::default()
        };
        let first = write_nfo(&d, "movie", NfoKind::Movie, &info, false).unwrap();
        assert!(first.is_some(), "第一次应写入");
        let second = write_nfo(&d, "movie", NfoKind::Movie, &info, false).unwrap();
        assert!(second.is_none(), "默认不应覆盖已存在的文件");

        // 显式允许时才覆盖
        let third = write_nfo(&d, "movie", NfoKind::Movie, &info, true).unwrap();
        assert!(third.is_some(), "显式 overwrite 才允许覆盖");
        std::fs::remove_dir_all(&d).ok();
    }

    /// 兜底：哪怕调用方传了视频文件名，也必须拒绝写入。
    #[test]
    fn refuses_to_write_over_a_video_file() {
        let d = temp_dir("guard");
        let info = NfoInfo {
            title: Some("电影".to_string()),
            kind: Some(NfoKind::Movie),
            ..Default::default()
        };
        // 模拟「调用方手滑把视频文件名当 base 传进来」
        let target = d.join("movie.mkv");
        std::fs::write(&target, b"original").unwrap();
        let err = write_nfo(&d, "movie.mkv", NfoKind::Movie, &info, true);
        assert!(err.is_err(), "带视频扩展名的 base 必须被拒绝：{err:?}");
        assert_eq!(
            std::fs::read(&target).unwrap(),
            b"original".to_vec(),
            "视频文件内容绝不能被改动"
        );
        // 正常调用只会多出一个 .nfo，视频文件依旧原样
        let ok = write_nfo(&d, "movie", NfoKind::Movie, &info, true).unwrap();
        assert!(ok.is_some());
        assert!(d.join("movie.nfo").exists());
        assert_eq!(std::fs::read(&target).unwrap(), b"original".to_vec());
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn is_video_path_recognizes_common_containers() {
        assert!(is_video_path(Path::new("a/b/c.mkv")));
        assert!(is_video_path(Path::new("a/b/c.MP4")));
        assert!(!is_video_path(Path::new("a/b/c.nfo")));
    }

    /// 畸形输入不能 panic（NFO 来自用户磁盘，什么形状都可能有）。
    #[test]
    fn parse_tolerates_garbage() {
        let info = parse("not xml at all");
        assert_eq!(info.kind, None);
        assert_eq!(info.title, None);
        let info2 = parse("<movie><title>无闭合");
        assert_eq!(info2.title, None);
    }
}
