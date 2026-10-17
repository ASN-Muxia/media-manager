//! `matching.rs` 的单元测试（纯函数，不依赖网络与数据库）。

use super::*;

fn opts() -> MatchOptions {
    MatchOptions::default()
}

fn cand<'a>(primary: &'a str, original: Option<&'a str>, year: Option<i32>) -> Candidate<'a> {
    Candidate {
        primary,
        original,
        year,
        vote: None,
    }
}

/// 造一个带生命周期的候选（`Candidate` 借的是 &str，这里用 'static 字面量即可）。
fn q(title: &str, year: Option<i32>) -> MatchQuery {
    MatchQuery {
        title: title.to_string(),
        aliases: Vec::new(),
        year,
        season: None,
        resolution: None,
    }
}

#[test]
fn fold_drops_punctuation_and_case() {
    assert_eq!(fold("The Matrix"), "thematrix");
    assert_eq!(fold("The.Matrix.1999"), "thematrix1999");
    assert_eq!(fold("言叶之庭 （剧场版）"), "言叶之庭剧场版");
    // 归一化**不能**抹掉年份：1984 版和 2021 版必须还能分开
    assert_ne!(fold("沙丘 1984"), fold("沙丘 2021"));
}

#[test]
fn tech_tags_are_stripped_from_query() {
    assert_eq!(strip_tech_tags("The.Matrix.1999.1080p.BluRay.x264"), "The Matrix 1999");
    assert_eq!(strip_tech_tags("言叶之庭.2013.BD1080p.x265"), "言叶之庭 2013");
    // 片名里恰好带 `4k` 的整词不能被切
    assert_eq!(strip_tech_tags("4k"), "");
    assert_eq!(strip_tech_tags("言叶之庭"), "言叶之庭");
}

#[test]
fn blacklist_string_is_split_on_common_separators() {
    assert_eq!(
        split_blacklist("剧场版, 演唱会\n幕后花絮；OVA"),
        vec!["剧场版", "演唱会", "幕后花絮", "OVA"]
    );
    assert!(split_blacklist("  ").is_empty());
}

#[test]
fn exact_title_match_scores_highest() {
    let o = opts();
    let s = score_candidate(&q("Inception", Some(2010)), &o, &cand("Inception", None, Some(2010)));
    assert_eq!(s.score, o.weights.exact + o.weights.year_hit, "{s:?}");
    assert!(s.reasons.iter().any(|r| r.contains("完全一致")), "{s:?}");
}

/// 多语言：本地叫中文、TMDB 返回英文时，**原标题 / 别名**必须能接住。
#[test]
fn alias_and_original_title_rescue_cross_language_match() {
    let o = opts();
    let mut query = q("言叶之庭", Some(2013));
    query.aliases = vec!["The Garden of Words".to_string()];

    let s = score_candidate(
        &query,
        &o,
        &cand("The Garden of Words", Some("言叶之庭"), Some(2013)),
    );
    assert!(
        s.score >= o.weights.alias,
        "跨语言必须靠别名接住：{s:?}"
    );
    assert!(s.reasons.iter().any(|r| r.contains("别名")), "{s:?}");
}

/// 展示名与原标题**分别对上本地的两种叫法**是最强的证据，要有额外加分。
#[test]
fn both_display_and_original_hit_gets_bonus() {
    let o = opts();
    let mut query = q("言叶之庭", None);
    query.aliases = vec!["The Garden of Words".to_string()];
    let s = score_candidate(
        &query,
        &o,
        &cand("言叶之庭", Some("The Garden of Words"), None),
    );
    let plain = score_candidate(&q("言叶之庭", None), &o, &cand("言叶之庭", None, None));
    assert!(s.score > plain.score, "原标题同时命中应有加分：{s:?} vs {plain:?}");
    assert!(s.reasons.iter().any(|r| r.contains("原标题同时命中")), "{s:?}");
}

#[test]
fn fuzzy_match_is_scaled_by_similarity_and_can_be_switched_off() {
    let mut o = opts();
    let query = q("Inceptoin", None); // 手滑打错
    let c = cand("Inception", None, None);
    let on = score_candidate(&query, &o, &c);
    assert!(on.score > 0, "模糊匹配应接住错字：{on:?}");
    assert!(on.score < o.weights.exact, "模糊分必须低于精确分：{on:?}");

    o.fuzzy = false;
    let off = score_candidate(&query, &o, &c);
    assert_eq!(off.score, 0, "关掉模糊后不应再给分：{off:?}");
}

/// 模糊匹配不能变成「什么都像」：不相干的两部片子必须是 0 分。
#[test]
fn unrelated_titles_score_zero() {
    let o = opts();
    let s = score_candidate(&q("星际穿越", None), &o, &cand("教父", None, None));
    assert_eq!(s.score, 0, "{s:?}");
}

#[test]
fn year_tolerance_and_miss() {
    let o = opts();
    let exact = score_candidate(&q("沙丘", Some(2021)), &o, &cand("沙丘", None, Some(2021)));
    let near = score_candidate(&q("沙丘", Some(2021)), &o, &cand("沙丘", None, Some(2020)));
    let far = score_candidate(&q("沙丘", Some(2021)), &o, &cand("沙丘", None, Some(1984)));
    assert!(exact.score > near.score, "{exact:?} vs {near:?}");
    assert!(near.score > far.score, "{near:?} vs {far:?}");
    assert!(far.reasons.iter().any(|r| r.contains("超出容差")), "{far:?}");

    // 容差调到 0：差一年也要扣分
    let mut strict = opts();
    strict.year_tolerance = 0;
    let strict_near = score_candidate(&q("沙丘", Some(2021)), &strict, &cand("沙丘", None, Some(2020)));
    assert!(strict_near.score < near.score, "容差 0 时差一年应扣分：{strict_near:?}");
}

/// 季数：第 2 季的首播年份应比条目年份早约 1 年。
#[test]
fn season_inference_rewards_matching_first_air_year() {
    let o = opts();
    let mut query = q("某剧", Some(2015));
    query.season = Some(2);
    let hit = score_candidate(&query, &o, &cand("某剧", None, Some(2014)));
    let miss = score_candidate(&query, &o, &cand("某剧", None, Some(2004)));
    assert!(hit.score > miss.score, "首播年份吻合应加分：{hit:?} vs {miss:?}");
    assert!(hit.reasons.iter().any(|r| r.contains("第 2 季")), "{hit:?}");
}

#[test]
fn blacklisted_candidates_are_excluded_outright() {
    let mut o = opts();
    o.blacklist = split_blacklist("剧场版, OVA");
    let s = score_candidate(&q("哆啦A梦", None), &o, &cand("哆啦A梦 剧场版", None, None));
    assert!(s.excluded, "命中排除词应整条出局：{s:?}");
    assert_eq!(s.exclude_reason.as_deref(), Some("排除词：剧场版"));
    assert_eq!(s.score, 0, "出局后不该再算分：{s:?}");
    assert!(!keeps(&s, &o));
}

#[test]
fn min_score_filters_weak_candidates() {
    let mut o = opts();
    o.min_score = 50;
    let strong = score_candidate(&q("Inception", None), &o, &cand("Inception", None, None));
    let weak = score_candidate(&q("Inception", None), &o, &cand("别的片", None, None));
    assert!(keeps(&strong, &o));
    assert!(!keeps(&weak, &o));
}

/// 权重可调 = 匹配优先级可配置：把「别名」调到最高，别名命中就该压过包含匹配。
#[test]
fn weights_are_configurable_priority_rules() {
    let mut o = opts();
    o.weights.alias = 500;
    let mut query = q("某片", None);
    query.aliases = vec!["某片 第二部".to_string()];
    let s = score_candidate(&query, &o, &cand("某片 第二部", None, None));
    assert!(s.score >= 500, "权重提高后分数应随之抬高：{s:?}");
}

/// 分辨率**不参与**打分：TMDb 候选里根本没有这个字段，硬编进分数是假信号。
#[test]
fn resolution_does_not_change_score() {
    let o = opts();
    let mut query = q("Inception", Some(2010));
    query.resolution = Some("2160p".to_string());
    let with = score_candidate(&query, &o, &cand("Inception", None, Some(2010)));
    query.resolution = None;
    let without = score_candidate(&query, &o, &cand("Inception", None, Some(2010)));
    assert_eq!(with.score, without.score);
}

#[test]
fn popularity_only_breaks_ties() {
    let o = opts();
    let plain = score_candidate(&q("Inception", None), &o, &cand("Inception", None, None));
    let mut c = cand("Inception", None, None);
    c.vote = Some(9.0);
    let voted = score_candidate(&q("Inception", None), &o, &c);
    assert!(
        voted.score - plain.score <= o.weights.popularity,
        "评分只能带来很小的加分：{voted:?} vs {plain:?}"
    );
}
