//! `grouping.rs` 的单元测试。
//!
//! 用 `#[path]` 挂回所属模块（见 `grouping.rs` 末尾），
//! 这样生产代码文件里只剩业务实现，单文件体积能降一大截，
//! 而 `use super::*` 仍能访问到私有成员，可见性一个字没改。

use super::*;
fn input_ex(
    root: usize,
    folder: &str,
    title: &str,
    y: Option<i32>,
    s: Option<i32>,
    e: Option<i32>,
    explicit: bool,
) -> GroupInput {
    GroupInput {
        root,
        folder: folder.to_string(),
        parent: None,
        title: title.to_string(),
        year: y,
        season: s,
        episode: e,
        explicit,
    }
}

/// 带 parent（文件所在目录名）的输入。
fn input_in(
    root: usize,
    folder: &str,
    parent: &str,
    title: &str,
    y: Option<i32>,
    s: Option<i32>,
    e: Option<i32>,
) -> GroupInput {
    let mut it = input(root, folder, title, y, s, e);
    it.parent = Some(parent.to_string());
    it
}

/// 带季 / 集号 → 视为「真的解析出了编号」。
fn input(root: usize, folder: &str, title: &str, y: Option<i32>, s: Option<i32>, e: Option<i32>) -> GroupInput {
    input_ex(root, folder, title, y, s, e, s.is_some() || e.is_some())
}

fn groups_of(plan: &GroupPlan) -> usize {
    plan.keys.len()
}

fn episodes_of(plan: &GroupPlan) -> Vec<(i32, i32)> {
    plan.episodes.clone()
}

#[test]
fn normalize_strips_season_year_and_noise() {
    assert_eq!(normalize_title("Breaking.Bad.S01.E01.1080p"), "breaking bad");
    assert_eq!(normalize_title("Show Season 2"), "show");
    assert_eq!(normalize_title("剧名 第2季"), "剧名");
    assert_eq!(normalize_title("Movie (2019)"), "movie");
    assert_eq!(normalize_title("The Show!"), "the show");
}

/// 目录名只有季号（`Season 1`）时归一为空 —— 有意的行为：
/// 这类名字没有可比对信息，不该参与跨目录合并。
#[test]
fn pure_season_folder_normalizes_to_empty_on_purpose() {
    assert_eq!(normalize_title("Season 1"), "");
    assert_eq!(normalize_title("S01"), "");
}

#[test]
fn similarity_is_one_for_same_title_different_punctuation() {
    assert_eq!(similarity("Breaking.Bad", "Breaking Bad"), 1.0);
    assert_eq!(similarity("The Show", "show the"), 1.0);
    assert!(similarity("Breaking Bad", "Better Call Saul") < MERGE_THRESHOLD);
}

#[test]
fn same_folder_stays_one_group() {
    let inputs = vec![
        input(0, "Show", "Show", Some(2020), Some(1), Some(1)),
        input(0, "Show", "Show", Some(2020), Some(1), Some(2)),
        input(0, "Show", "Show", Some(2020), Some(2), Some(1)),
    ];
    let plan = plan_groups(&inputs);
    assert_eq!(groups_of(&plan), 1);
    assert_eq!(plan.assign, vec![0, 0, 0]);
    assert_eq!(plan.keys, vec!["Show".to_string()]);
}

/// 跨一级目录：标题一致、编号不重叠 → 合并（避免一部剧被拆成多个实体）
#[test]
fn sibling_folders_with_disjoint_episodes_are_merged() {
    let inputs = vec![
        input(0, "Show Season 1", "Show", Some(2020), Some(1), Some(1)),
        input(0, "Show Season 1", "Show", Some(2020), Some(1), Some(2)),
        input(0, "Show Season 2", "Show", Some(2020), Some(2), Some(1)),
    ];
    let plan = plan_groups(&inputs);
    assert_eq!(groups_of(&plan), 1, "keys={:?}", plan.keys);
    assert_eq!(plan.assign, vec![0, 0, 0]);
}

/// 重名但季集重叠 → 绝不合并（防跨文件夹误合并的核心）
#[test]
fn same_title_with_overlapping_episodes_is_not_merged() {
    let inputs = vec![
        input(0, "Show A", "Show", Some(2020), Some(1), Some(1)),
        input(0, "Show B", "Show", Some(2020), Some(1), Some(1)),
    ];
    let plan = plan_groups(&inputs);
    assert_eq!(groups_of(&plan), 2, "keys={:?}", plan.keys);
}

/// 标题**完全一样**但年份不同：多半是同一部剧的不同年份版本 / 特辑
/// （重拍通常会连目录名一起改），因此**要**合并。
#[test]
fn same_title_different_year_is_merged_when_titles_are_identical() {
    let inputs = vec![
        input(0, "Show (2008)", "Show", Some(2008), Some(1), Some(1)),
        input(0, "Show (2020)", "Show", Some(2020), Some(2), Some(1)),
    ];
    let plan = plan_groups(&inputs);
    assert_eq!(groups_of(&plan), 1, "keys={:?}", plan.keys);
}

/// 标题只是**相似**（不是完全一样）+ 年份冲突 → 不合并
/// （《The Office》美版 / 英版就是这种形状）。
#[test]
fn similar_title_with_conflicting_year_is_not_merged() {
    let inputs = vec![
        input(0, "The Office US", "The Office US", Some(2005), Some(1), Some(1)),
        input(0, "The Office UK", "The Office UK", Some(2001), Some(2), Some(1)),
    ];
    let plan = plan_groups(&inputs);
    assert_eq!(groups_of(&plan), 2, "keys={:?}", plan.keys);
}

/// 跨媒体根目录：即使是同名同季集也不合并
#[test]
fn different_media_roots_are_never_merged() {
    let inputs = vec![
        input(0, "Show", "Show", Some(2020), Some(1), Some(1)),
        input(1, "Show", "Show", Some(2020), Some(2), Some(1)),
    ];
    let plan = plan_groups(&inputs);
    assert_eq!(groups_of(&plan), 2);
}

/// 没有编号 → 只凭标题不敢合并
#[test]
fn folders_without_episode_numbering_are_not_merged() {
    let inputs = vec![
        input(0, "Show", "Show", Some(2020), None, None),
        input(0, "Show Extra", "Show", Some(2020), None, None),
    ];
    let plan = plan_groups(&inputs);
    assert_eq!(groups_of(&plan), 2, "keys={:?}", plan.keys);
}

/// 一个目录里混了两部剧（编号冲突）→ 按标题拆开，且拆出的组键要互不相同
#[test]
fn conflicting_episode_numbers_inside_one_folder_split_the_group() {
    let inputs = vec![
        input(0, "Mix", "Alpha", Some(2020), Some(1), Some(1)),
        input(0, "Mix", "Alpha", Some(2020), Some(1), Some(2)),
        input(0, "Mix", "Beta", Some(2021), Some(1), Some(1)),
        input(0, "Mix", "Beta", Some(2021), Some(1), Some(2)),
    ];
    let plan = plan_groups(&inputs);
    assert_eq!(groups_of(&plan), 2, "keys={:?}", plan.keys);
    assert_ne!(plan.keys[0], plan.keys[1], "拆分后的组键不能撞车");
    // Alpha 两集一组、Beta 两集一组
    let mut per: Vec<usize> = vec![0; 2];
    for a in plan.assign {
        per[a] += 1;
    }
    assert_eq!(per, vec![2, 2]);
}

/// 没有冲突就不拆：「第01话」和「EP01」只是写法不同
#[test]
fn no_conflict_means_no_split() {
    let inputs = vec![
        input(0, "Anime", "Anime", None, Some(1), Some(1)),
        input(0, "Anime", "Anime", None, Some(1), Some(2)),
        input(0, "Anime", "Anime", None, Some(1), Some(3)),
    ];
    let plan = plan_groups(&inputs);
    assert_eq!(groups_of(&plan), 1);
}

/// 文件直接躺在媒体根目录：按解析出的标题归桶
#[test]
fn loose_files_are_grouped_by_parsed_title() {
    let inputs = vec![
        input(0, "", "Breaking Bad", Some(2008), Some(1), Some(1)),
        input(0, "", "Breaking Bad", Some(2008), Some(1), Some(2)),
        input(0, "", "Dune", Some(2021), None, None),
    ];
    let plan = plan_groups(&inputs);
    assert_eq!(groups_of(&plan), 2, "keys={:?}", plan.keys);
    assert_eq!(plan.assign[0], plan.assign[1]);
    assert_ne!(plan.assign[0], plan.assign[2]);
}

/// 结果与输入顺序无关（否则重扫一次分组就变了，会造出重复剧集）
#[test]
fn plan_is_independent_of_input_order() {
    let a = vec![
        input(0, "Show", "Show", Some(2020), Some(1), Some(1)),
        input(0, "Show Extra", "Show", Some(2020), Some(1), Some(2)),
    ];
    let b = vec![a[1].clone(), a[0].clone()];
    let pa = plan_groups(&a);
    let pb = plan_groups(&b);
    assert_eq!(pa.keys, pb.keys);
    assert_eq!(pa.assign[0], pb.assign[1]);
    assert_eq!(pa.assign[1], pb.assign[0]);
}

/// 组键要保留原始写法（用户看到的目录名），不能是归一化后的小写串。
#[test]
fn keys_keep_original_casing() {
    let inputs = vec![
        input(0, "Breaking Bad", "Breaking Bad", Some(2008), Some(1), Some(1)),
        input(0, "Breaking Bad", "Breaking Bad", Some(2008), Some(1), Some(2)),
    ];
    let plan = plan_groups(&inputs);
    assert_eq!(plan.keys, vec!["Breaking Bad".to_string()]);
}

/* ---------------- 聚合回归（Phase F） ---------------- */

/// 归一化必须抹掉**集号**：留着它，一部上千集的动画会被切成上千条。
#[test]
fn normalize_strips_trailing_episode_numbers() {
    assert_eq!(normalize_title("哆啦A梦 1000"), "哆啦a梦");
    assert_eq!(normalize_title("哆啦A梦 0001"), "哆啦a梦");
    assert_eq!(normalize_title("0001"), "", "纯编号文件名里没有标题信息");
    assert_eq!(normalize_title("1917"), "", "年份不是集号，但也不是标题");
    assert_eq!(normalize_title("Blade Runner 2049"), "blade runner");
}

/// 单集类型词（`特辑` / `特别篇` / `OVA`）不是剧名的一部分。
#[test]
fn normalize_strips_episode_type_words() {
    assert_eq!(normalize_title("神秘博士 特辑"), "神秘博士");
    assert_eq!(normalize_title("神秘博士 特辑2"), "神秘博士");
    assert_eq!(normalize_title("某剧 特别篇"), "某剧");
    assert_eq!(normalize_title("某动画 OVA"), "某动画");
}

/// 英文的单集类型词同样不是剧名的一部分（`Christmas Special` / `Extras`）。
#[test]
fn normalize_strips_english_episode_type_words() {
    assert_eq!(normalize_title("神秘博士 Christmas Special"), "神秘博士");
    assert_eq!(normalize_title("Doctor Who 2005 Christmas Special"), "doctor who");
    assert_eq!(normalize_title("doctor.who.2005.christmas.special.2005.1080p"), "doctor who");
    assert_eq!(normalize_title("Show - Special Edition"), "show edition");
    assert_eq!(normalize_title("Show 2015 Extras"), "show");
    assert_eq!(normalize_title("Show-Sample"), "show", "随片样片不是另一部剧");
}

/// 文件名被发行组前缀污染时，救回来的是**目录名**：
/// `...X264-TRiPS[rarbg]\trips-doctor.who.s07.chrimbo.mkv` 虽然解析成
/// `trips doctor who`，但它住的目录明明白白写着这是 Doctor Who 的发行包。
#[test]
fn polluted_filenames_are_pulled_back_by_their_directory() {
    let pkg = "Doctor.Who.2005.S07.1080p.BluRay.x264-MIXED[rartv]";
    let specials_pkg = "Doctor.Who.2005.2012.Christmas.Special.720p.BluRay.X264-TRiPS[rarbg]";
    let inputs = vec![
        input_in(0, "神秘博士", pkg, "doctor who", Some(2005), Some(7), Some(1)),
        input_in(0, "神秘博士", pkg, "doctor who", Some(2005), Some(7), Some(2)),
        input_in(0, "神秘博士", specials_pkg, "trips doctor who", Some(2012), Some(7), Some(1)),
        input_in(0, "神秘博士", specials_pkg, "trips doctor who", Some(2012), Some(7), Some(2)),
    ];
    let plan = plan_groups(&inputs);
    assert_eq!(groups_of(&plan), 1, "keys={:?}", plan.keys);
    let mut eps = episodes_of(&plan);
    eps.sort_unstable();
    assert_eq!(eps, vec![(7, 1), (7, 2), (7, 3), (7, 4)]);
}

/// 反过来：目录名没有指向主组时，混在一处的两部剧**仍然要**拆开。
#[test]
fn mixed_folder_still_splits_when_directory_says_nothing() {
    let inputs = vec![
        input_in(0, "Mix", "Mix", "Alpha", Some(2020), Some(1), Some(1)),
        input_in(0, "Mix", "Mix", "Alpha", Some(2020), Some(1), Some(2)),
        input_in(0, "Mix", "Mix", "Beta", Some(2021), Some(1), Some(1)),
        input_in(0, "Mix", "Mix", "Beta", Some(2021), Some(1), Some(2)),
    ];
    let plan = plan_groups(&inputs);
    assert_eq!(groups_of(&plan), 2, "keys={:?}", plan.keys);
}

/// 作品目录识别：分类层 / 结构目录不是作品，`Disc 2` 这类载体编号也不是。
#[test]
fn work_name_detection_skips_containers_and_categories() {
    for dir in ["神秘博士", "Breaking Bad", "哆啦A梦", "Legal High", "Doctor Who (2005)"] {
        assert!(looks_like_work_name(dir), "{dir} 应认作作品目录");
    }
    // 分类层目录。注意这里传的是**单层目录名**，不带路径——`work_dir_of`
    // 传进来的永远是路径的一段。
    for dir in ["Season 1", "S01", "电视剧", "电影", "动漫", "新建文件夹", "Movies", "TV"] {
        assert!(!looks_like_work_name(dir), "{dir} 不是作品目录");
    }
    // 载体 / 分片编号：去掉末尾数字后剩下的词在黑名单里
    for dir in ["Disc 1", "CD2", "DVD part 2", "Vol 3", "Vol.2"] {
        assert!(!looks_like_work_name(dir), "{dir} 不是作品目录");
    }
}

/// 没有编号的特辑也要并入同一部剧（标题完全一致是硬条件）。
#[test]
fn unnumbered_specials_join_the_numbered_series() {
    let inputs = vec![
        input(0, "神秘博士 (2005)", "神秘博士", Some(2005), Some(1), Some(1)),
        input(0, "神秘博士 (2005)", "神秘博士", Some(2005), Some(1), Some(2)),
        input_ex(0, "神秘博士 (2023)", "神秘博士 特辑", Some(2023), Some(0), Some(1), false),
    ];
    let plan = plan_groups(&inputs);
    assert_eq!(groups_of(&plan), 1, "keys={:?}", plan.keys);
}

/// 两边都没编号 → 不合并（保持「不猜」的底线）。
#[test]
fn two_unnumbered_groups_are_still_not_merged() {
    let inputs = vec![
        input_ex(0, "神秘博士 (2023)", "神秘博士 特辑", Some(2023), Some(0), Some(1), false),
        input_ex(0, "神秘博士 (2024)", "神秘博士 特辑", Some(2024), Some(0), Some(1), false),
    ];
    let plan = plan_groups(&inputs);
    assert_eq!(groups_of(&plan), 2, "keys={:?}", plan.keys);
}

/// 纯编号文件名（`0001.mp4`）：靠目录名归一，不能被切成一堆碎条目。
#[test]
fn pure_number_files_in_one_folder_stay_one_group() {
    let inputs = vec![
        input(0, "哆啦A梦", "0001", None, Some(1), Some(1)),
        input(0, "哆啦A梦", "0002", None, Some(1), Some(2)),
        input(0, "哆啦A梦", "0003", None, Some(1), Some(3)),
    ];
    let plan = plan_groups(&inputs);
    assert_eq!(groups_of(&plan), 1, "keys={:?}", plan.keys);
    assert_eq!(plan.keys, vec!["哆啦A梦".to_string()]);
    assert_eq!(episodes_of(&plan), vec![(1, 1), (1, 2), (1, 3)]);
}

/// 集号连续化：撞号的补空位，而不是互相覆盖成一集。
#[test]
fn colliding_episode_numbers_are_renumbered_within_a_group() {
    let inputs = vec![
        input(0, "Show", "Show", None, Some(1), Some(1)),
        input(0, "Show", "Show", None, Some(1), Some(2)),
        // 两份都没有明确编号的特辑，占位值都是第 1 集
        input_ex(0, "Show", "Show 特辑", None, Some(0), Some(1), false),
        input_ex(0, "Show", "Show 特辑2", None, Some(0), Some(1), false),
    ];
    let plan = plan_groups(&inputs);
    assert_eq!(groups_of(&plan), 1, "keys={:?}", plan.keys);
    let mut eps = episodes_of(&plan);
    eps.sort_unstable();
    assert_eq!(
        eps,
        vec![(0, 1), (0, 2), (1, 1), (1, 2)],
        "特辑应补成第 1、2 集，而不是两集都叫第 1 集"
    );
}

/// 真实的稀疏集号要保留（`S01E05` 不能被硬掰成第 3 集）。
#[test]
fn unique_episode_numbers_are_kept_as_is() {
    let inputs = vec![
        input(0, "Show", "Show", None, Some(1), Some(1)),
        input(0, "Show", "Show", None, Some(1), Some(2)),
        input(0, "Show", "Show", None, Some(1), Some(5)),
    ];
    let plan = plan_groups(&inputs);
    assert_eq!(episodes_of(&plan), vec![(1, 1), (1, 2), (1, 5)]);
}

#[test]
fn empty_input_gives_empty_plan() {
    let plan = plan_groups(&[]);
    assert!(plan.assign.is_empty() && plan.keys.is_empty());
}
