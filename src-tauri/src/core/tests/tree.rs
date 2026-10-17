//! `tree.rs` 的单元测试。
//!
//! 用 `#[path]` 挂回所属模块（见 `tree.rs` 末尾），`use super::*` 照样访问私有成员。

use super::*;

/// 一颗固定形状的树：
/// ```text
/// 1 剧
/// ├─ 2 第一季
/// │  └─ 4 第一季-特辑
/// └─ 3 第二季
/// 5 另一部
/// ```
fn edges() -> Vec<(i64, Option<i64>)> {
    vec![
        (1, None),
        (2, Some(1)),
        (3, Some(1)),
        (4, Some(2)),
        (5, None),
    ]
}

#[test]
fn depth_counts_edges_to_root() {
    let e = edges();
    assert_eq!(depth_of(&e, 1).unwrap(), 0);
    assert_eq!(depth_of(&e, 2).unwrap(), 1);
    assert_eq!(depth_of(&e, 4).unwrap(), 2);
    assert!(depth_of(&e, 99).is_err(), "不存在的 id 要报错而不是当顶层");
}

#[test]
fn descendant_detection_walks_upwards() {
    let e = edges();
    assert!(is_descendant(&e, 1, 4), "4 在 1 的子树里");
    assert!(is_descendant(&e, 2, 4));
    assert!(!is_descendant(&e, 4, 2), "反方向不成立");
    assert!(!is_descendant(&e, 5, 4));
}

#[test]
fn subtree_includes_self_and_all_levels() {
    let e = edges();
    let s = subtree_of(&e, 1);
    assert_eq!(s.len(), 4, "1 的子树含 1/2/3/4");
    assert_eq!(subtree_of(&e, 4), vec![4]);
}

#[test]
fn height_is_the_tallest_branch() {
    let e = edges();
    assert_eq!(height_of(&e, 1), 2, "1 → 2 → 4");
    assert_eq!(height_of(&e, 2), 1);
    assert_eq!(height_of(&e, 5), 0, "叶子高度为 0");
}

#[test]
fn moving_into_own_descendant_is_a_cycle() {
    let e = edges();
    assert_eq!(
        validate_drop(&e, &[1], Some(4)),
        Err(DropError::Cycle { mover: 1, target: 4 }),
        "把祖先拖进子孙里必须被挡住"
    );
    assert_eq!(
        validate_drop(&e, &[1], Some(1)),
        Err(DropError::SelfTarget { id: 1 })
    );
    // 顶层永远安全
    assert!(validate_drop(&e, &[4], None).is_ok());
}

#[test]
fn depth_limit_is_enforced() {
    // 造一条刚好 4 层的链：1 → 2 → 3 → 4（depth 0..3）
    let e: Vec<(i64, Option<i64>)> =
        vec![(1, None), (2, Some(1)), (3, Some(2)), (4, Some(3))];
    assert!(validate_drop(&e, &[5], Some(1)).is_ok(), "挂到第 2 层没问题");
    // 再挂一层就到第 5 层了：把高度为 1 的子树（6→7）挂到 depth 3 的节点下
    let e2: Vec<(i64, Option<i64>)> = vec![
        (1, None),
        (2, Some(1)),
        (3, Some(2)),
        (4, Some(3)),
        (6, None),
        (7, Some(6)),
    ];
    // 4 在第 4 层（depth 3），6 进去后自己第 5 层、它的子 7 到第 6 层
    assert_eq!(
        validate_drop(&e2, &[6], Some(4)),
        Err(DropError::TooDeep { depth: 6, limit: MAX_DEPTH }),
        "4 层上限，超出必须给出具体层数"
    );
    // 高度 0 的叶子挂到第 3 层的节点下，正好占满第 4 层，必须放行
    assert!(validate_drop(&e2, &[7], Some(3)).is_ok());
}

#[test]
fn selecting_parent_and_child_moves_only_the_parent() {
    let e = edges();
    // 同时选中 2 和它的子 4：4 会跟着 2 走，不能当独立搬运单位
    assert_eq!(roots_of(&e, &[2, 4]), vec![2]);
    // 选中两个互不隶属的节点，两个都保留，浅的在前
    assert_eq!(roots_of(&e, &[4, 3]), vec![3, 4]);
    assert_eq!(roots_of(&e, &[2, 3]), vec![2, 3]);
}

#[test]
fn broken_data_with_a_cycle_does_not_hang() {
    let e: Vec<(i64, Option<i64>)> = vec![(1, Some(2)), (2, Some(1))];
    // 成环时各函数都必须能返回，不能把界面拖死
    let _ = depth_of(&e, 1);
    let _ = is_descendant(&e, 1, 2);
    let s = subtree_of(&e, 1);
    assert_eq!(s.len(), 2);
}

/* ---------------- 删除前的子级过继 ---------------- */

#[test]
fn adoption_goes_to_the_nearest_surviving_ancestor() {
    let e = edges();
    // 删 2（其父 1 还在）：子级 4 应该过继给 1
    let plan = plan_adoption(&e, &[2]);
    assert_eq!(plan, vec![(4, Some(1))], "中间层被删，子级上提一层");

    // 删 1（顶层）：它的两个直接子级 2 / 3 都挂到顶层，4 跟着 2 走不用动
    let plan = plan_adoption(&e, &[1]);
    assert_eq!(plan, vec![(2, None), (3, None)], "删顶层 → 直接子级挂顶层");
}

#[test]
fn adoption_skips_ancestors_that_are_also_being_deleted() {
    let e = edges();
    // 同时删 1 和 2：4 的候选新父 1 也在名单里，只能挂顶层
    let plan = plan_adoption(&e, &[1, 2]);
    assert!(
        plan.iter().all(|(child, target)| {
            if *child == 4 {
                *target == None
            } else {
                *target == None
            }
        }),
        "祖先全在删除名单里 → 一律挂顶层: {plan:?}"
    );
    // 3 是 1 的直接子级，也挂顶层
    assert!(plan.contains(&(3, None)));
}

#[test]
fn adoption_never_targets_a_node_that_is_being_deleted() {
    let e = edges();
    for doomed in [vec![1], vec![2], vec![1, 2], vec![1, 2, 4], vec![4]] {
        let plan = plan_adoption(&e, &doomed);
        for (child, target) in &plan {
            assert!(
                !doomed.contains(child),
                "待删的子级自己不该出现在方案里: {plan:?}"
            );
            assert!(
                target.map_or(true, |t| !doomed.contains(&t)),
                "新父不能是自己也要被删的节点: {plan:?}"
            );
        }
    }
}

#[test]
fn adoption_on_a_cycle_falls_back_to_top_level() {
    let e: Vec<(i64, Option<i64>)> = vec![(1, Some(2)), (2, Some(1)), (3, Some(1))];
    // 成环时绝不能挑一个「不在名单里的子孙」当新父——那会造出自己挂自己。
    // 1 的子级是 2 和 3（2 同时又是 1 的父），两个都只能挂顶层。
    let plan = plan_adoption(&e, &[1]);
    assert_eq!(plan, vec![(2, None), (3, None)], "父链成环 → 挂顶层: {plan:?}");
}

#[test]
fn adoption_of_a_leaf_or_unknown_id_is_empty() {
    let e = edges();
    assert!(plan_adoption(&e, &[4]).is_empty(), "叶子没有子级，方案为空");
    assert!(plan_adoption(&e, &[]).is_empty(), "没有待删节点，方案为空");
    assert!(
        plan_adoption(&e, &[999]).is_empty(),
        "边表里没有的 id 不该凭空造出子级"
    );
}

#[test]
fn adoption_does_not_emit_a_no_op_move() {
    let e = edges();
    // 2 现在的父就是 1；删 1 之后 2 挂顶层是有变化的，但删 4 时它没子级 → 空。
    // 关键用例：父链上最近的存活祖先恰好等于「子级现在的父」是不可能的
    // （那个父正在被删），所以方案里不该出现 (child, 它现在的父)。
    let plan = plan_adoption(&e, &[1, 2]);
    for (child, target) in &plan {
        let now = e.iter().find(|(n, _)| n == child).map(|(_, p)| *p).flatten();
        assert_ne!(*target, now, "不该产生没有变化的移动: {plan:?}");
    }
}
