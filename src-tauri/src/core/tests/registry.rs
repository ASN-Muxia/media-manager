//! `registry.rs` 的单元测试。
//!
//! 用 `#[path]` 挂回所属模块（见 `registry.rs` 末尾），照 `core/tree.rs` 的写法。

use super::*;
use std::collections::BTreeSet;

/// 造一份磁盘指纹。
fn fp(size: u64, mtime: Option<i64>) -> AssetFingerprint {
    AssetFingerprint { size, mtime }
}

/// 造一行库里已有记录（尺寸 / mtime / 状态）。
fn prev<'a>(size: u64, mtime: Option<i64>, status: &'a str) -> (u64, Option<i64>, &'a str) {
    (size, mtime, status)
}

/* ------------------------------ classify ------------------------------ */

/// 库里没有这个路径 → 新增。
#[test]
fn no_previous_row_is_new() {
    assert_eq!(classify(None, &fp(10, Some(1))), AssetChange::New);
    // mtime 取不到也还是「新增」：它根本没进过库
    assert_eq!(classify(None, &fp(10, None)), AssetChange::New);
}

/// 大小与 mtime 都一样 → 未变（这是增量扫描唯一敢跳过的分支）。
#[test]
fn identical_fingerprint_is_unchanged() {
    assert_eq!(
        classify(Some(prev(10, Some(5), STATUS_PRESENT)), &fp(10, Some(5))),
        AssetChange::Unchanged
    );
}

/// 大小变了 → 变动。
#[test]
fn size_change_is_modified() {
    assert_eq!(
        classify(Some(prev(10, Some(5), STATUS_PRESENT)), &fp(11, Some(5))),
        AssetChange::Modified
    );
}

/// mtime 变了 → 变动。
#[test]
fn mtime_change_is_modified() {
    assert_eq!(
        classify(Some(prev(10, Some(5), STATUS_PRESENT)), &fp(10, Some(6))),
        AssetChange::Modified
    );
}

/// 库里标着 missing，这次又看到了 → 恢复（而不是「未变」或「变动」）。
///
/// 它排在指纹比较之前：文件回来了这件事本身要被单独计数，
/// 哪怕内容一字节没变（从别的盘挪回来就是这种情况）。
#[test]
fn missing_row_that_reappears_is_restored() {
    assert_eq!(
        classify(Some(prev(10, Some(5), STATUS_MISSING)), &fp(10, Some(5))),
        AssetChange::Restored
    );
    // 内容同时变过也仍算「恢复」——状态翻转是更重要的结论
    assert_eq!(
        classify(Some(prev(10, Some(5), STATUS_MISSING)), &fp(99, Some(7))),
        AssetChange::Restored
    );
}

/// 缺 mtime 时保守判「变动」：宁可多写一次，不能漏掉真改动。
#[test]
fn missing_mtime_is_conservatively_modified() {
    // 现在是 None
    assert_eq!(
        classify(Some(prev(10, Some(5), STATUS_PRESENT)), &fp(10, None)),
        AssetChange::Modified
    );
    // 库里的旧记录是 None（老库升级上来的行常见这种）
    assert_eq!(
        classify(Some(prev(10, None, STATUS_PRESENT)), &fp(10, Some(5))),
        AssetChange::Modified
    );
    // 两边都取不到
    assert_eq!(
        classify(Some(prev(10, None, STATUS_PRESENT)), &fp(10, None)),
        AssetChange::Modified
    );
}

/* ------------------------------ reconcile ------------------------------ */

fn seen_of(paths: &[&str]) -> BTreeSet<String> {
    paths.iter().map(|p| p.to_string()).collect()
}

/// 库里还在、这次没遍历到 → 标 missing。
#[test]
fn reconcile_flags_rows_that_were_not_seen() {
    let existing = vec![
        (1i64, r"D:\m\a.mkv".to_string()),
        (2i64, r"D:\m\b.mkv".to_string()),
    ];
    let stale = reconcile(&existing, &seen_of(&[r"D:\m\a.mkv"]));
    assert_eq!(stale, vec![2], "只有没见到的 b 该被标消失");
}

/// 全都见着了 → 一个都不标。
#[test]
fn reconcile_keeps_rows_that_were_seen() {
    let existing = vec![(1i64, r"D:\m\a.mkv".to_string())];
    assert!(reconcile(&existing, &seen_of(&[r"D:\m\a.mkv"])).is_empty());
    // 空名单 + 空所见 = 没有可标的（不该凭空造出 missing）
    assert!(reconcile(&[], &seen_of(&[])).is_empty());
}

/// 已 missing 的行不会被重复标记（`missing_at` 只记「第一次消失」）。
///
/// 状态过滤是**调用方**的责任：它只把 `status='present'` 的行交进来，
/// 所以第 3 行（已 missing）压根不在 `existing` 里。这里把三种行摆在一起验证
/// 结论的分界正好落在「present 且没见到」这一个条件上。
#[test]
fn reconcile_never_touches_rows_already_missing() {
    // 1 = present 且这次没见到 → 该标；2 = present 且见到了 → 不该标；
    // 3 = 已 missing（被调用方的状态过滤挡在 existing 之外，故不出现）
    let existing = vec![
        (1i64, r"D:\m\a.mkv".to_string()),
        (2i64, r"D:\m\b.mkv".to_string()),
    ];
    let stale = reconcile(&existing, &seen_of(&[r"D:\m\b.mkv"]));
    assert_eq!(stale, vec![1], "只有「present 且没见到」的 1 该被标消失");
    assert!(!stale.contains(&2), "见到了就不能标");
    assert!(!stale.contains(&3), "已经 missing 的行不能被重复标记");
}

/// 边界：增量扫描只覆盖了一部分根目录时，没被遍历到的那些行**不能**被误标。
///
/// 这是 `reconcile` 存在的前提：调用方只把「本次真正走通的根目录下」的行
/// 交进来，另一个盘上的老资源就不会被整片判成消失。
#[test]
fn reconcile_does_not_touch_rows_outside_the_scanned_roots() {
    // 只看 D 盘（本次走通的根）：E 盘的行压根没进 existing
    let existing = vec![(1i64, r"D:\m\a.mkv".to_string())];
    let stale = reconcile(&existing, &seen_of(&[]));
    assert_eq!(stale, vec![1], "D 盘这一行确实该标消失");
    // E 盘那一行不在这里，也就永远不会被这次扫描标掉
    assert!(!stale.contains(&2));
}

/// 路径比较是**逐字符**的，不做大小写折叠 —— 与唯一索引 / 入库口径保持一致。
#[test]
fn reconcile_compares_paths_verbatim() {
    let existing = vec![(1i64, r"D:\m\A.MKV".to_string())];
    assert_eq!(
        reconcile(&existing, &seen_of(&[r"D:\m\a.mkv"])),
        vec![1],
        "大小写不同就是不同的字符串（Windows 上罕见，但不能悄悄改口径）"
    );
    assert!(reconcile(&existing, &seen_of(&[r"D:\m\A.MKV"])).is_empty());
}
