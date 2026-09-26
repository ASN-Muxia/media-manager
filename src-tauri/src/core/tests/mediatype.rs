//! `mediatype.rs` 的单元测试。
//!
//! 用 `#[path]` 挂回所属模块（见 `mediatype.rs` 末尾），
//! 这样生产代码文件里只剩业务实现，单文件体积能降一大截，
//! 而 `use super::*` 仍能访问到私有成员，可见性一个字没改。

use super::*;
use crate::core::parser::parse_file_name;

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

/* ======================================================================
 * 回归：体积大的剧集全集必须判成剧集，而不是电影
 *
 * 背景（真实 bug）：《大明风华》全集 62 集，因为整季体量大、被压成单体大文件，
 * 被识别成电影。判定里**根本没有任何一条规则读文件大小**——真正的原因是
 * 「体积大」在磁盘上的表现形式踩中了两条为单部资源设计的规则：
 *   1. 文件名带「全集」（R2）曾无条件短路成电影，无视同目录几十集的文件；
 *   2. 单体大文件时长必然 ≥ 100 分钟（R11），被当成电影证据。
 * 下面每一组用例都钉住这两条修复后的行为。
 * ====================================================================== */

/// 造一个「同一部剧、同目录」的信号；`comp` 是标题带合集词的文件数。
fn tv_sig(fname: &str, dir: &str, tv_count: usize, total: usize, comp: usize) -> Signals<'static> {
    // 为了返回 'static 借用，这里把字符串泄漏掉——测试进程里无所谓。
    let leak = |s: &str| -> &'static str { Box::leak(s.to_string().into_boxed_str()) };
    let name = leak(fname);
    let dir = leak(dir);
    let parsed = Box::leak(Box::new(parse_file_name(name)));
    let mut sig = Signals::from_parsed(parsed, name);
    sig.folder = Some(dir);
    sig.parent = Some(dir);
    sig.folder_total = total;
    sig.folder_tv_count = tv_count;
    sig.folder_compilation_count = comp;
    if tv_count > 0 || comp > 0 {
        sig.folder_consensus_title = Some(dir);
    }
    sig
}

/// 核心回归：同一个目录里 62 集，其中一个文件叫「全集」，且时长 3 小时。
/// 修复前：R2 短路 → Movie(tv=0, movie=3)。修复后：R0 结构证据 → Tv。
#[test]
fn big_series_marked_as_full_collection_is_tv() {
    let mut sig = tv_sig("大明风华.全集.1080p.mkv", "大明风华", 62, 62, 62);
    sig.duration_secs = Some(3 * 3600); // 体积大 → 单体时长长
    let v = classify(&sig, &TypeConfig::default());
    assert_eq!(v.kind, MediaKind::Tv, "{v:?}");
    assert_eq!(v.tv_score, 3);
    assert_eq!(v.movie_score, 0);
}

/// 「目录里每个文件都写着全集」——这是按集存放的典型形状，必须判剧集。
#[test]
fn folder_where_every_file_says_complete_is_tv() {
    for fname in [
        "大明风华.全集.mkv",
        "大明风华.全62集.mkv",
        "[高清影视]大明风华 全集 1080P.mkv",
        "大明风华.Complete.1080p.mkv",
        "大明风华.2019.全集.1080p.mkv",
    ] {
        let sig = tv_sig(fname, "大明风华", 0, 62, 62);
        let v = classify(&sig, &TypeConfig::default());
        assert_eq!(v.kind, MediaKind::Tv, "{fname} → {v:?}");
    }
}

/// 时长超过 100 分钟不能把已经有剧集证据的文件推成电影。
#[test]
fn long_duration_never_overrides_tv_evidence() {
    // 带集号的长片（两小时特别篇）
    let mut sig = tv_sig("Show.S01E01.1080p.mkv", "Show", 0, 1, 0);
    sig.duration_secs = Some(150 * 60);
    assert_eq!(classify(&sig, &TypeConfig::default()).kind, MediaKind::Tv);

    // 目录里只有 2 个文件也算结构证据，时长再长也不翻
    let mut sig2 = tv_sig("大明风华.全集.mkv", "大明风华", 0, 2, 2);
    sig2.duration_secs = Some(5 * 3600);
    let v = classify(&sig2, &TypeConfig::default());
    assert_eq!(v.kind, MediaKind::Tv, "{v:?}");
    assert_eq!(v.movie_score, 0, "时长不应再给电影加分：{v:?}");
}

/// R2 仍然保留：**孤立**的单体全集文件（目录里没有任何多集证据）照样按电影处理。
#[test]
fn isolated_full_collection_file_is_still_movie() {
    let sig = tv_sig("大明风华.全集.mkv", "下载", 0, 1, 1);
    let v = classify(&sig, &TypeConfig::default());
    assert_eq!(v.kind, MediaKind::Movie, "{v:?}");
    assert_eq!(v.tv_score, 0);
    assert!(v.reasons.contains(&"合集/合辑按单条处理"));
}

/// 真正的电影识别不受影响：单文件、无集号、目录里没有成串的剧集文件。
#[test]
fn genuine_movies_are_unaffected() {
    for (fname, dur, dir) in [
        ("The.Matrix.1999.1080p.mkv", None, Some("Movies")),
        ("流浪地球.2019.1080p.BluRay.mkv", None, None),
        ("Oppenheimer.2023.2160p.WEB-DL.mkv", Some(180 * 60), Some("Movies")),
        ("肖申克的救赎.1994.mkv", None, None),
        ("Interstellar.2014.IMAX.mkv", Some(169 * 60), Some("科幻")),
    ] {
        let name = fname;
        let parsed = parse_file_name(name);
        let mut sig = Signals::from_parsed(&parsed, name);
        // 泄漏无关紧要：借用的是局部变量，这里直接放进块作用域使用
        sig.folder = dir;
        sig.parent = dir;
        sig.folder_total = 1;
        sig.folder_tv_count = 0;
        sig.folder_compilation_count = 0;
        sig.duration_secs = dur;
        let v = classify(&sig, &TypeConfig::default());
        assert_eq!(v.kind, MediaKind::Movie, "{fname} → {v:?}");
        assert!(v.movie_score >= 2, "{fname} 电影分不足：{v:?}");
    }
}

/// 系列电影（`Rocky 1/2/3`）不该被编号共识当成剧集——门槛 `hard_number_min` 生效。
#[test]
fn movie_series_is_not_mistaken_for_tv() {
    // 3 个同前缀编号文件：低于 hard_number_min(4)，且带年份 → 电影
    let mut sig = tv_sig("Rocky 1.mkv", "Rocky", 0, 3, 0);
    sig.folder_numbered_count = 3;
    sig.folder_number_prefix = Some("Rocky");
    sig.year = Some(1976);
    let v = classify(&sig, &TypeConfig::default());
    assert_eq!(v.kind, MediaKind::Movie, "{v:?}");
}

/// 目录里两部作品各有一批「全集」文件时，不能互相借数量作证。
#[test]
fn full_work_consensus_requires_matching_title() {
    // 目录共识是另一部剧 → 本文件不属于它，R0 不触发
    let mut sig = tv_sig("大明风华.全集.mkv", "大明风华", 0, 8, 8);
    sig.folder_consensus_title = Some("庆余年");
    // 目录名也和标题不符 → 不构成同剧证据
    sig.folder = Some("庆余年");
    sig.parent = Some("庆余年");
    let v = classify(&sig, &TypeConfig::default());
    assert_ne!(v.kind, MediaKind::Tv, "不该借用别的作品的数量：{v:?}");
}

/// 已有外部元数据（TMDB / 年份）的条目不被硬覆盖，仍可由电影信号判成电影。
#[test]
fn known_movie_with_metadata_is_not_hard_overridden() {
    let mut sig = tv_sig("The.Matrix.1999.1080p.mkv", "Movies", 2, 3, 0);
    sig.year = Some(1999);
    sig.has_metadata = true;
    sig.duration_secs = Some(136 * 60);
    let v = classify(&sig, &TypeConfig::default());
    assert_eq!(v.kind, MediaKind::Movie, "{v:?}");
}

/* --------------- 目录统计：全集命名的文件也要被数到 --------------- */

#[test]
fn folder_stats_counts_complete_named_files() {
    let names: Vec<String> = (1..=10)
        .map(|i| match i % 2 {
            0 => "大明风华.全集.mkv".to_string(),
            _ => "大明风华.Complete.1080p.mkv".to_string(),
        })
        .collect();
    let parsed: Vec<ParsedName> = names.iter().map(|n| parse_file_name(n)).collect();
    let entries: Vec<FolderEntry> = names
        .iter()
        .zip(parsed.iter())
        .map(|(n, p)| FolderEntry {
            file_name: n,
            parsed: p,
        })
        .collect();
    let st = build_folder_stats(&entries);
    assert_eq!(st.total, 10);
    assert_eq!(st.compilation_count, 10, "全集命名的文件必须被数到");
    // 众数标题应退回「完整作品」命名的剧名，否则 R7 的「同剧」判断永远不成立
    assert_eq!(st.consensus_title.as_deref(), Some("大明风华"));
}

#[test]
fn folder_stats_prefers_marker_titles_over_complete_titles() {
    let names = [
        "大明风华.S01E01.mkv",
        "大明风华.S01E02.mkv",
        "大明风华.全集.mkv",
    ];
    let parsed: Vec<ParsedName> = names.iter().map(|n| parse_file_name(n)).collect();
    let entries: Vec<FolderEntry> = names
        .iter()
        .zip(parsed.iter())
        .map(|(n, p)| FolderEntry {
            file_name: n,
            parsed: p,
        })
        .collect();
    let st = build_folder_stats(&entries);
    assert_eq!(st.tv_count, 2);
    assert_eq!(st.compilation_count, 1);
    assert_eq!(st.consensus_title.as_deref(), Some("大明风华"));
}

#[test]
fn full_work_title_extracts_series_name() {
    for (name, want) in [
        ("大明风华.全集.mkv", Some("大明风华")),
        ("大明风华.Complete.1080p.mkv", Some("大明风华")),
        ("大明风华 全62集 1080p.mkv", Some("大明风华")),
        ("[高清影视]大明风华 全集 1080P.mkv", Some("大明风华")),
        ("全集.mkv", None),
        ("The.Matrix.1999.mkv", None),
    ] {
        let p = parse_file_name(name);
        assert_eq!(
            full_work_title(&p.title).as_deref(),
            want,
            "{name} (title={:?})",
            p.title
        );
    }
}

/// 端到端：真实目录 + 真实 scanner 收集 + 真实判定。
/// 覆盖「一个目录里全是『全集』命名的大文件」这一真实形状。
#[test]
fn end_to_end_full_collection_folder_is_tv() {
    for (label, names) in [
        (
            "每个文件都叫「全集」",
            vec![
                "大明风华.全集.1.mkv",
                "大明风华.全集.2.mkv",
                "大明风华.全集.3.mkv",
                "大明风华.全集.4.mkv",
            ],
        ),
        (
            "标准季集命名",
            vec![
                "大明风华.S01E01.1080p.mkv",
                "大明风华.S01E02.1080p.mkv",
                "大明风华.S01E03.1080p.mkv",
            ],
        ),
        (
            "中文集号",
            vec![
                "大明风华 第01集.mkv",
                "大明风华 第02集.mkv",
                "大明风华 第03集.mkv",
            ],
        ),
        (
            "纯编号",
            vec!["01.mkv", "02.mkv", "03.mkv", "04.mkv"],
        ),
    ] {
        let dir = std::env::temp_dir().join(format!(
            "mm-e2e-{}-{}",
            std::process::id(),
            label.len()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for n in &names {
            std::fs::write(dir.join(n), b"x").unwrap();
        }
        let files = crate::core::scanner::collect_files(&[dir.to_string_lossy().to_string()]);
        assert_eq!(files.len(), names.len(), "{label}");
        for f in &files {
            assert_eq!(f.kind, MediaKind::Tv, "{label}: {} → {:?}", f.name, f.kind);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// 端到端：真实电影目录不该被误判。
#[test]
fn end_to_end_movie_folder_stays_movie() {
    let dir = std::env::temp_dir().join(format!("mm-e2e-movies-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let names = [
        "The.Matrix.1999.1080p.mkv",
        "流浪地球.2019.1080p.mkv",
        "Oppenheimer.2023.2160p.mkv",
    ];
    for n in &names {
        std::fs::write(dir.join(n), b"x").unwrap();
    }
    let files = crate::core::scanner::collect_files(&[dir.to_string_lossy().to_string()]);
    assert_eq!(files.len(), names.len());
    for f in &files {
        assert_eq!(f.kind, MediaKind::Movie, "{} → {:?}", f.name, f.kind);
    }
    let _ = std::fs::remove_dir_all(&dir);
}
