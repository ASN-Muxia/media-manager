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
