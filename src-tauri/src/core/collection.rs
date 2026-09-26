//! 系列电影聚合（同一系列 / 同一 IP 的电影归到一个合集）。
//!
//! ## 聚合依据（一级：标题里的续作标识）
//!
//! 没有引入新的表：合集是**从标题算出来的视图**，因此天然向后兼容——
//! 老数据不用迁移，改了片名重新算一次即可。
//!
//! [`franchise_key`] 把一个片名归成「系列基名」：
//!
//! 1. 砍掉冒号副标题：`Mission: Impossible - Fallout` → `mission`；
//! 2. 砍掉年份：`沙丘 (2021)` → `沙丘`；
//! 3. 循环砍掉**尾部**续作标记：
//!    - 数字：`沙丘 2` / `Rocky 2`；
//!    - 罗马数字：`Rocky II` / `Rocky III`；
//!    - `Part 2` / `Part Two` / `Chapter 2`；
//!    - 中文：`第二部` / `第2部` / `续集` / `前传` / `外传` / `后传` /
//!      `完结篇` / `最终章` / `序章` / `前篇` / `后篇` / `上` / `下`；
//! 4. 走一遍 `grouping::normalize_title`（去噪声词、只留有意义的字符）。
//!
//! 基名相同的电影归到一个合集；**成员少于 `min_members`（默认 2）的组直接丢掉**
//! ——否则每部电影都会变成「自己的合集」，没有任何意义。
//!
//! ## 去重规则
//!
//! 同一部电影常常有好几个版本（1080p / 4K / 不同压制组）。去重键是
//! `(normalize_title(标题), 年份)`：相同的只占**一个位次**，保留信息最完整的
//! 那条（有 TMDB id → 有海报 → 文件更大 → id 更小），其余记进 `duplicates`。
//!
//! 去重**不删数据**：底下的 `media` 行一条不动，只是合集视图里不重复占位。
//!
//! ## 集内排序
//!
//! 默认按**上映时间**（`year` 升序）；同年按片名、再按 id，保证结果稳定。
//! 没有年份的排在最后。

use crate::core::grouping::normalize_title;
use crate::util::lazy_re;
use regex::Regex;
use std::collections::BTreeMap;
use std::sync::OnceLock;

/// 聚合参数。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollectionConfig {
    /// 一个合集至少要有多少部电影（默认 2：1 部的「合集」没意义）。
    pub min_members: usize,
    /// 系列基名的最短字符数（太短的基名容易把不相干的片子并到一起）。
    pub min_base_len: usize,
}

impl Default for CollectionConfig {
    fn default() -> Self {
        Self {
            min_members: 2,
            min_base_len: 2,
        }
    }
}

/// 参与聚合的一条电影。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MovieEntry {
    pub id: i64,
    pub title: String,
    pub year: Option<i32>,
    pub tmdb_id: Option<i64>,
    pub poster_path: Option<String>,
    pub file_size: Option<i64>,
}

/// 一个系列合集。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Collection {
    /// 聚合键（系列基名的归一化结果）。
    pub key: String,
    /// 展示名：成员里最短的那个片名（并列取字典序最小）。
    pub name: String,
    /// 按上映时间排序后的成员 id。
    pub members: Vec<i64>,
    /// 被去重折叠掉的同片多版本 id。
    pub duplicates: Vec<i64>,
    pub start_year: Option<i32>,
    pub end_year: Option<i32>,
}

/// 聚合结果。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CollectionPlan {
    pub collections: Vec<Collection>,
    /// 被折叠掉的同片多版本总数。
    pub duplicates: usize,
}

// `lazy_re!` 在 `crate::util` 里只有一份定义，这里 import 即可。

// 尾部的续作标记：`2` / `II` / `Part 2` / `Part Two` / `第二部` …
lazy_re!(
    re_sequel_number,
    r"(?i)\s*(?:part|chapter|episode)?\s*(?:\d{1,2}|[ivx]+|one|two|three|four|five|six|seven|eight|nine|ten)$"
);
lazy_re!(
    re_sequel_cn,
    r"(?:第\s*[0-9一二三四五六七八九十]{1,3}\s*[部篇章]|续集|前传|外传|后传|番外|完结篇|最终章|序章|前篇|后篇|上部|下部|[上下])$"
);

/// 砍掉尾部的续作标记；砍完为空就**不砍**（`1917` 这种片名不能被削成 `19`）。
fn strip_sequel(s: &str) -> String {
    let t = s.trim_end();
    if t.chars().all(|c| c.is_ascii_digit()) {
        return s.to_string();
    }
    for re in [re_sequel_cn(), re_sequel_number()] {
        if let Some(m) = re.find(t) {
            let head = t[..m.start()].trim_end();
            if !head.is_empty() {
                return head.to_string();
            }
        }
    }
    s.to_string()
}

/// 系列基名：同一个系列 / IP 的电影算出来应该一样。
///
/// 年份由 `normalize_title` 抹掉；但若抹完之后什么都不剩（片名本身就是年份，
/// 比如 `1917`），就退回去用原样，别把片名清空。
pub fn franchise_key(title: &str) -> String {
    let mut s = title.trim().to_ascii_lowercase();

    // 1) 冒号之后是副标题（`Mission: Impossible - Fallout` → `mission`）
    if let Some(i) = s.find([':', '：']) {
        s = s[..i].to_string();
    }
    // 2) 循环砍尾部续作标记（`沙丘 2 第三部` 这种叠加写法也能砍干净）
    for _ in 0..4 {
        let before = s.clone();
        s = strip_sequel(&s).trim().to_string();
        if s == before {
            break;
        }
    }
    let key = normalize_title(&s);
    if key.is_empty() {
        s.split_whitespace().collect::<Vec<_>>().join(" ")
    } else {
        key
    }
}

/// 聚合：分组 → 去重 → 排序。
pub fn plan_collections(movies: &[MovieEntry], cfg: &CollectionConfig) -> CollectionPlan {
    // 1) 按系列基名分组
    let mut groups: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (i, m) in movies.iter().enumerate() {
        let key = franchise_key(&m.title);
        if key.chars().count() < cfg.min_base_len {
            continue;
        }
        groups.entry(key).or_default().push(i);
    }

    let mut plan = CollectionPlan::default();
    for (key, idxs) in groups {
        if idxs.len() < cfg.min_members {
            continue;
        }
        // 2) 去重：同片多版本只留一个代表
        let mut by_dedup: BTreeMap<(String, Option<i32>), Vec<usize>> = BTreeMap::new();
        for &i in &idxs {
            let dedup_key = (normalize_title(&movies[i].title), movies[i].year);
            by_dedup.entry(dedup_key).or_default().push(i);
        }
        let mut reps: Vec<usize> = Vec::new();
        let mut dups: Vec<i64> = Vec::new();
        for (_k, group) in by_dedup {
            let mut best = group[0];
            for &i in &group[1..] {
                if better(movies, i, best) {
                    best = i;
                }
            }
            reps.push(best);
            for &i in &group {
                if i != best {
                    dups.push(movies[i].id);
                }
            }
        }

        // 3) 集内排序：默认按上映时间（无年份的排最后），再按片名 / id
        reps.sort_by(|&a, &b| {
            movies[a]
                .year
                .unwrap_or(i32::MAX)
                .cmp(&movies[b].year.unwrap_or(i32::MAX))
                .then(movies[a].title.cmp(&movies[b].title))
                .then(movies[a].id.cmp(&movies[b].id))
        });

        let years: Vec<i32> = reps.iter().filter_map(|&i| movies[i].year).collect();
        let name = reps
            .iter()
            .map(|&i| movies[i].title.as_str())
            .min_by(|a, b| a.chars().count().cmp(&b.chars().count()).then(a.cmp(b)))
            .unwrap_or_default()
            .to_string();

        plan.duplicates += dups.len();
        plan.collections.push(Collection {
            key,
            name,
            members: reps.iter().map(|&i| movies[i].id).collect(),
            duplicates: dups,
            start_year: years.iter().min().copied(),
            end_year: years.iter().max().copied(),
        });
    }

    // 合集之间的顺序：先按首部上映年份，再按名字（稳定，重算不会跳来跳去）
    plan.collections.sort_by(|a, b| {
        a.start_year
            .unwrap_or(i32::MAX)
            .cmp(&b.start_year.unwrap_or(i32::MAX))
            .then(a.name.cmp(&b.name))
    });
    plan
}

/// `a` 是否比 `b` 更适合当这个版本的代表：
/// 有 TMDB id → 有海报 → 文件更大 → id 更小。
fn better(movies: &[MovieEntry], a: usize, b: usize) -> bool {
    let ka = (
        movies[a].tmdb_id.is_some(),
        movies[a].poster_path.is_some(),
        movies[a].file_size.unwrap_or(0),
    );
    let kb = (
        movies[b].tmdb_id.is_some(),
        movies[b].poster_path.is_some(),
        movies[b].file_size.unwrap_or(0),
    );
    ka.cmp(&kb).then(movies[b].id.cmp(&movies[a].id)) == std::cmp::Ordering::Greater
}

#[cfg(test)]
mod tests {
    use super::*;

    fn movie(id: i64, title: &str, year: Option<i32>) -> MovieEntry {
        MovieEntry {
            id,
            title: title.to_string(),
            year,
            tmdb_id: None,
            poster_path: None,
            file_size: None,
        }
    }

    #[test]
    fn franchise_key_strips_sequel_markers() {
        assert_eq!(franchise_key("沙丘"), "沙丘");
        assert_eq!(franchise_key("沙丘 2"), "沙丘");
        assert_eq!(franchise_key("沙丘2"), "沙丘");
        assert_eq!(franchise_key("沙丘 第二部"), "沙丘");
        assert_eq!(franchise_key("Dune: Part Two"), "dune");
        assert_eq!(franchise_key("Rocky II"), "rocky");
        assert_eq!(franchise_key("Rocky IV"), "rocky");
        assert_eq!(franchise_key("Rocky 2"), "rocky");
        assert_eq!(franchise_key("Mission: Impossible - Fallout"), "mission");
        assert_eq!(franchise_key("Mission: Impossible - Rogue Nation"), "mission");
        assert_eq!(franchise_key("叶问 前传"), "叶问");
        assert_eq!(franchise_key("某片 完结篇"), "某片");
    }

    #[test]
    fn franchise_key_keeps_distinct_works_apart() {
        assert_ne!(franchise_key("沙丘"), franchise_key("星际穿越"));
        // 没有续作标记的电影，基名就是它自己（不会被误拆）
        assert_eq!(franchise_key("Inception"), "inception");
        assert_eq!(franchise_key("1917"), "1917");
    }

    #[test]
    fn franchisees_are_grouped_and_ordered_by_release() {
        let movies = vec![
            movie(3, "沙丘 2", Some(2024)),
            movie(1, "沙丘", Some(2021)),
            movie(2, "沙丘 3", Some(2026)),
            movie(9, "星际穿越", Some(2014)),
        ];
        let plan = plan_collections(&movies, &CollectionConfig::default());
        assert_eq!(plan.collections.len(), 1, "{plan:?}");
        let c = &plan.collections[0];
        assert_eq!(c.name, "沙丘");
        assert_eq!(c.members, vec![1, 3, 2], "默认按上映时间排序");
        assert_eq!(c.start_year, Some(2021));
        assert_eq!(c.end_year, Some(2026));
    }

    /// 单部电影不成合集（否则每部片子都会变成「自己的合集」）。
    #[test]
    fn single_movie_is_not_a_collection() {
        let movies = vec![movie(1, "沙丘", Some(2021)), movie(2, "星际穿越", Some(2014))];
        let plan = plan_collections(&movies, &CollectionConfig::default());
        assert!(plan.collections.is_empty(), "{plan:?}");
    }

    /// 去重：同一部电影的多个版本只占一个位次，保留信息最完整的那条。
    #[test]
    fn duplicate_versions_collapse_into_one_slot() {
        let mut movies = vec![movie(1, "沙丘", Some(2021)), movie(2, "沙丘", Some(2021))];
        movies[1].tmdb_id = Some(438631);
        movies[1].poster_path = Some("/dune.jpg".to_string());
        movies.push(movie(3, "沙丘 2", Some(2024)));

        let plan = plan_collections(&movies, &CollectionConfig::default());
        assert_eq!(plan.collections.len(), 1);
        let c = &plan.collections[0];
        assert_eq!(c.members, vec![2, 3], "有 TMDB 元数据的那条当代表");
        assert_eq!(c.duplicates, vec![1]);
        assert_eq!(plan.duplicates, 1);
    }

    /// 去重不改数据：底下的行一条都没动，只是合集视图里不重复占位。
    #[test]
    fn dedup_prefers_bigger_file_when_no_metadata() {
        let mut movies = vec![movie(1, "沙丘", Some(2021)), movie(2, "沙丘", Some(2021))];
        movies[0].file_size = Some(4_000_000_000);
        movies[1].file_size = Some(20_000_000_000);
        movies.push(movie(3, "沙丘 2", Some(2024)));
        let plan = plan_collections(&movies, &CollectionConfig::default());
        assert_eq!(plan.collections[0].members, vec![2, 3], "文件更大的那条当代表");
    }

    #[test]
    fn movies_without_year_sort_last() {
        let movies = vec![
            movie(1, "沙丘 2", None),
            movie(2, "沙丘", Some(2021)),
        ];
        let plan = plan_collections(&movies, &CollectionConfig::default());
        assert_eq!(plan.collections[0].members, vec![2, 1]);
    }

    /// 太短的基名不该并到一起（`min_base_len` 可配）。
    #[test]
    fn too_short_base_is_ignored() {
        let movies = vec![movie(1, "X", Some(2000)), movie(2, "X 2", Some(2002))];
        let plan = plan_collections(&movies, &CollectionConfig::default());
        assert!(plan.collections.is_empty(), "{plan:?}");

        let loose = CollectionConfig { min_base_len: 1, ..Default::default() };
        assert_eq!(plan_collections(&movies, &loose).collections.len(), 1);
    }

    #[test]
    fn collections_are_sorted_by_first_release_year() {
        let movies = vec![
            movie(1, "教父", Some(1972)),
            movie(2, "教父 2", Some(1974)),
            movie(3, "沙丘", Some(2021)),
            movie(4, "沙丘 2", Some(2024)),
        ];
        let plan = plan_collections(&movies, &CollectionConfig::default());
        let names: Vec<&str> = plan.collections.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["教父", "沙丘"], "{names:?}");
    }

    #[test]
    fn empty_library_gives_empty_plan() {
        let plan = plan_collections(&[], &CollectionConfig::default());
        assert!(plan.collections.is_empty());
        assert_eq!(plan.duplicates, 0);
    }
}
