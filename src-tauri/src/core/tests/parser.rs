//! `parser.rs` 的单元测试。
//!
//! 用 `#[path]` 挂回所属模块（见 `parser.rs` 末尾），
//! 这样生产代码文件里只剩业务实现，单文件体积能降一大截，
//! 而 `use super::*` 仍能访问到私有成员，可见性一个字没改。

use super::*;
fn p(name: &str) -> ParsedName {
    parse_file_name(name)
}

/// 扩展名白名单：**忽略大小写**，且不接收非视频格式。
///
/// `D:\某剧\ep01.MP4` 这种大写写法在 D 盘上有 38 个，漏掉就是整季扫不出来。
#[test]
fn video_ext_whitelist() {
    assert!(is_video_file("a.MKV"));
    assert!(is_video_file("b.mp4"));
    assert!(is_video_file("c.MP4"), "大写扩展名也得认");
    assert!(is_video_file("d.RmVB"));
    assert!(is_video_file("e.Mp4"));
    assert!(!is_video_file("cover.jpg"));
    assert!(!is_video_file("subs.srt"));
    assert!(!is_video_file("noext"));
    // D 盘上唯一的 2 个 .dat 是网易云音乐的程序数据，不能被当成视频
    assert!(!is_video_file("icudtl.dat"));
    assert!(!is_video_file("Movie.mkv.part"));
}

/// 新纳入的同族写法（`"扩展名只会越来越多的人"`——跟着实测容器的家族走）
#[test]
fn video_ext_covers_same_container_families() {
    for name in [
        "3D.mkv", "3d.mk3d", "clip.f4v", "old.divx", "cam.mts", "raw.m2v", "net.asf",
    ] {
        assert!(is_video_file(name), "{name} 应在白名单里");
    }
    // 已经支持的格式不能被这次改动挪走
    for name in [
        "a.mkv", "a.mp4", "a.avi", "a.rmvb", "a.rm", "a.mov", "a.wmv", "a.flv", "a.ts",
        "a.m2ts", "a.mpg", "a.mpeg", "a.m4v", "a.webm", "a.vob", "a.iso", "a.3gp",
    ] {
        assert!(is_video_file(name), "{name} 应仍被支持");
    }
}

/// 无扩展名 / 扩展名异常时按**文件头**兜底。
#[test]
fn header_signature_recognizes_video_containers() {
    assert_eq!(
        video_signature(b"\x1a\x45\xdf\xa3\xa3\x42\x86\x81V_MPEG4/ISO/AVC"),
        Some(VideoContainer::Matroska)
    );
    let mut mp4 = vec![0u8; 16];
    mp4[4..8].copy_from_slice(b"ftyp");
    mp4[8..12].copy_from_slice(b"isom");
    assert_eq!(video_signature(&mp4), Some(VideoContainer::IsoBmff));
    assert_eq!(
        video_signature(b"RIFF\x00\x00\x00\x00AVI LIST"),
        Some(VideoContainer::Avi)
    );
    assert_eq!(video_signature(b"FLV\x01\x05\x00\x00"), Some(VideoContainer::FlashVideo));
    assert_eq!(video_signature(b".RMF\x00\x00\x00"), Some(VideoContainer::RealMedia));
    assert_eq!(
        video_signature(&[0x30, 0x26, 0xb2, 0x75, 0x8e, 0x66, 0xcf, 0x11]),
        Some(VideoContainer::Asf)
    );
    let mut ts = vec![0u8; 400];
    ts[0] = 0x47;
    ts[188] = 0x47;
    ts[376] = 0x47;
    assert_eq!(video_signature(&ts), Some(VideoContainer::MpegTs));
}

/// 实测踩到的两个误判源必须被挡掉：
/// - VobSub 字幕（`.sub`）共用 MPEG-PS 的包头；
/// - 纯音频 Matroska（`.mka`）共用 EBML 头，要靠 codec ID 区分。
#[test]
fn header_signature_rejects_sub_and_audio_only() {
    assert_eq!(video_signature(b"\x00\x00\x01\xba\x44\x00"), None, "VobSub 不是视频");
    assert_eq!(
        video_signature(b"\x1a\x45\xdf\xa3\xa3\x42\x86\x81A_AACS_MPEG"),
        None,
        "纯音频 Matroska 不是视频"
    );
    let mut m4a = vec![0u8; 16];
    m4a[4..8].copy_from_slice(b"ftyp");
    m4a[8..12].copy_from_slice(b"M4A ");
    assert_eq!(video_signature(&m4a), None, "m4a 不是视频");
    assert_eq!(video_signature(b"%PDF-1.7\n%\xc7\xec\x8f\xa2"), None);
    assert_eq!(video_signature(b"\x89PNG\r\n\x1a\n"), None);
    assert_eq!(video_signature(b"PK\x03\x04"), None);
}

#[test]
fn movie_dotted_with_quality_tags() {
    let r = p("The.Matrix.1999.1080p.BluRay.x264-GROUP.mkv");
    assert_eq!(r.title, "The Matrix");
    assert_eq!(r.year, Some(1999));
    assert_eq!(r.episode, None);
    // 判定不在 parser 里做（见 mediatype），这里只校验「没有解析出季集标记」
    assert!(!r.episode_marker && !r.season_marker);
    assert_eq!(crate::core::mediatype::classify(
        &crate::core::mediatype::Signals::from_parsed(&r, "The.Matrix.1999.1080p.BluRay.x264-GROUP.mkv"),
        &crate::core::mediatype::TypeConfig::default(),
    ).kind, crate::core::mediatype::MediaKind::Movie);
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
    assert!(r.episode_marker && r.season_marker, "S03E07 是明确标记");
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
}

/// 只有季号、没有集号：`re_sxxexx` 认不出来，靠 `re_season_short` 兜住。
#[test]
fn season_only_short_form_is_detected() {
    let r = p("Show.S02.1080p.WEB-DL.mkv");
    assert_eq!(r.season, Some(2));
    assert_eq!(r.episode, None);
    assert!(r.season_marker);
    assert_eq!(r.title, "Show");
}

/// 季号是「补出来的默认值」时不能算明确标记（判定层要区分二者）。
#[test]
fn defaulted_season_is_not_a_marker() {
    let r = p("动画名 第01话.mp4");
    assert_eq!(r.season, Some(1));
    assert!(r.episode_marker);
    assert!(!r.season_marker, "季号 1 是补出来的，不是看到的");
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

/// 裸集号 + 集名：`剧名.01.集名` 是中文库最常见的写法，之前完全认不出来。
///
/// 之前 `西游记1986-2000版.01.猴王初问世.mp4` 会得到 `episode=None`，
/// 于是整条被判成「待确认」的独立条目，永远进不了剧集。
#[test]
fn bare_number_with_episode_title_is_a_marker() {
    for name in [
        "西游记.01.猴王初问世.mp4",
        "西游记 01.猴王初问世.mp4",
        "西游记01.猴王初问世.mkv",
        "西游记.02.官封弼马温.mkv",
    ] {
        let r = p(name);
        assert_eq!(r.episode, Some(if name.contains(".02.") { 2 } else { 1 }));
        assert!(r.episode_marker, "{name} 的集号应算明确标记（{r:?}）");
        assert_eq!(r.season, Some(1), "{name} 无季号时按第 1 季（{r:?}）");
        assert_eq!(r.title, "西游记", "{name} 标题要截断在集号前（{r:?}）");
    }
    // 明确标记优先，结论不变
    let r = p("西游记.E01.猴王初问世.mp4");
    assert_eq!(r.episode, Some(1));
    assert_eq!(r.title, "西游记");
}

/// 数字**结尾**的裸编号（`剧名 01.mkv`）刻意不在这里认——
/// 那是系列电影 / 拆部电影的形状，判定层要靠「目录编号共识」兜（R8）。
#[test]
fn trailing_bare_number_stays_unmarked_for_folder_consensus() {
    for name in ["剧名 01.mkv", "剧名.01.mkv", "01.mkv"] {
        let r = p(name);
        assert_eq!(r.episode, None, "{name} 不该被当成明确集号（{r:?}）");
        assert!(!r.episode_marker, "{name} 不该有集号标记（{r:?}）");
    }
}

/// 版本区间 `1986-2000版` 取起始年，且整段不算标题。
#[test]
fn year_range_is_a_version_marker_not_a_year_tail() {
    let r = p("西游记1986-2000版.01.猴王初问世.mp4");
    assert_eq!(r.year, Some(1986), "应取区间起始年，实际：{r:?}");
    assert_eq!(r.episode, Some(1));
    assert_eq!(r.title, "西游记", "版本区间不能留在标题里，实际：{r:?}");
}

/// 安全阀门：片名里的数字后紧跟汉字时**不能**被当成集号。
#[test]
fn numbers_inside_a_movie_title_are_not_episodes() {
    for (name, title) in [
        ("50度灰.2015.mp4", "50度灰"),
        ("300勇士.2006.mp4", "300勇士"),
        ("12怒汉.1957.mp4", "12怒汉"),
        ("007：大破天幕杀机.2012.mp4", "007：大破天幕杀机"),
    ] {
        let r = p(name);
        assert_eq!(r.episode, None, "{name} 不能被当成单集（{r:?}）");
        assert!(!r.episode_marker, "{name} 不该有集号标记");
        assert_eq!(r.title, title, "{name} 标题应完整保留（{r:?}）");
    }
}

/// 清晰度 / 编码里的数字同样不是集号（`1080p` `x264` `720p`）。
#[test]
fn quality_and_codec_numbers_are_not_episodes() {
    for name in [
        "Some.Movie.1080p.2018.mkv",
        "Some.Movie.720p.BluRay.x264.mkv",
        "Show.S02.1080p.WEB-DL.mkv",
        "Interstellar.2014.1080p.BluRay.x264.DTS-FGT.mkv",
        "Title.2020.10bit.mkv",
    ] {
        let r = p(name);
        assert_eq!(r.episode, None, "{name} 不该被认成单集（{r:?}）");
    }
}

/// 纯 4 位编号（`哆啦A梦 1000`）依旧不算集号——那是集数上千的老番编号。
#[test]
fn four_digit_number_is_still_not_an_episode() {
    let r = p("哆啦A梦 1000.mp4");
    assert_eq!(r.episode, None, "4 位编号不进裸集号规则（{r:?}）");
}

/* --------------------- TMDB 识别码（优先于标题/年份/分辨率） --------------------- */

/// 常见写法都要认：忽略大小写、忽略分隔符差异（`tmdbid` 也算一种关键字）。
#[test]
fn tmdb_id_markers_are_recognized_across_writings() {
    for (name, want) in [
        ("Movie.Name.2013.tmdb-123456.mkv", 123456i64),
        ("Movie.Name.tmdbid:123456.mkv", 123456),
        ("Movie.Name.[tmdb=123456].mkv", 123456),
        ("Movie.Name.(tmdb 123456).mkv", 123456),
        ("Movie.Name.{tmdb_123456}.mkv", 123456),
        ("Movie.Name.TMDB.123456.mkv", 123456),
        ("Movie.Name.tmdb123456.mkv", 123456),
        ("Movie.Name.TmdbId = 123456.mkv", 123456),
        ("Movie.Name.tmdbid-7.mkv", 7),
    ] {
        assert_eq!(
            extract_tmdb_id(name),
            Some(want),
            "{name} 应解析出 TMDB id {want}"
        );
        assert_eq!(p(name).tmdb_id, Some(want), "{name} 走 parse_file_name 也要一致");
    }
}

/// 没有 `tmdb` 关键字的裸数字**一律不算** id——这是防误伤的核心。
#[test]
fn plain_numbers_are_never_tmdb_ids() {
    for name in [
        "2012.2009.1080p.mkv",
        "Se7en.1995.1080p.BluRay.x264.mkv",
        "Show.S01E01.1080p.mkv",
        "1917.2019.mkv",
        "第01集.mkv",
        "300勇士.2006.mkv",
        "50度灰.2015.mkv",
        "12怒汉.1957.mkv",
    ] {
        assert_eq!(extract_tmdb_id(name), None, "{name} 不该凭数字认出 TMDB id");
    }
}

/// 分辨率伪装：`tmdb 1080p` 里的 `1080` 后面紧跟 `p`，不算 id。
#[test]
fn resolution_masquerading_as_tmdb_id_is_rejected() {
    assert_eq!(extract_tmdb_id("Movie.tmdb.1080p.mkv"), None);
    assert_eq!(extract_tmdb_id("Movie.tmdb 720p.mkv"), None);
    assert_eq!(extract_tmdb_id("Movie.tmdb-2160p.mkv"), None);
    // 真正的 id 不能被这条规则误杀
    assert_eq!(extract_tmdb_id("Movie.tmdb-1080.mkv"), Some(1080));
}

/// 多文本按传入顺序取第一个命中的——顺序即优先级。
#[test]
fn tmdb_id_from_texts_follows_priority_order() {
    let file_name = "言叶之庭.tmdbid:42.1080p.mkv";
    let title = "别的东西 tmdb-999";
    assert_eq!(tmdb_id_from_texts(&[file_name, title]), Some(42));
    assert_eq!(tmdb_id_from_texts(&[title, file_name]), Some(999));
    assert_eq!(tmdb_id_from_texts(&["没线索的标题", "2013"]), None);
    assert_eq!(tmdb_id_from_texts(&[]), None);
}

/// 命中 id 的同时，标题 / 年份 / 集号这些**照常解析**——只是匹配依据换成 id。
#[test]
fn tmdb_id_does_not_disturb_other_parsing() {
    let r = p("Show.Name.S01E02.tmdb-555.1080p.mkv");
    assert_eq!(r.tmdb_id, Some(555));
    assert_eq!(r.season, Some(1));
    assert_eq!(r.episode, Some(2));
    assert_eq!(r.title, "Show Name");
}
