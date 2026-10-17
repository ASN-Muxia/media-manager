//! 剧集层级（树）的**纯规则**。
//!
//! 这一层刻意不碰数据库：输入是一份 `(id, parent_id)` 边表，输出是判定结论。
//! 好处是「禁止循环引用」「层级上限」这类最容易出错的规则可以脱离连接单测，
//! 命令层只负责取数、套规则、写库。
//!
//! 层级定义：
//! - 顶层节点的 `depth` 为 0；
//! - `MAX_DEPTH` 是**允许的层数**（4 层 = depth 0..3）；
//! - `height` 是子树高度，叶子为 0。

use std::collections::BTreeSet;

/// 允许的层数（顶层算第 1 层）。
pub const MAX_DEPTH: usize = 4;

/// 树里的一条边。用 `Vec<(i64, Option<i64>)>` 直接表达，避免为纯函数引入结构体。
pub type Edges = [(i64, Option<i64>)];

/// 非法放置的原因。命令层再按它去查标题、拼面向用户的中文提示。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DropError {
    /// 拖到自己身上。
    SelfTarget { id: i64 },
    /// 拖到自己的子孙里（会形成环）。
    Cycle { mover: i64, target: i64 },
    /// 超出层数上限。
    TooDeep { depth: usize, limit: usize },
    /// 边表里找不到这个 id（数据不一致，或条目被删了）。
    Missing { id: i64 },
}

/// 取父节点；找不到返回 `Missing`。
fn parent_of(edges: &Edges, id: i64) -> Result<Option<i64>, DropError> {
    for (n, p) in edges {
        if *n == id {
            return Ok(*p);
        }
    }
    Err(DropError::Missing { id })
}

/// 往上走到根，返回**经过的边数**（顶层 = 0）。
///
/// 数据里万一已经成环（老数据或被外部改过），靠 `seen` 兜住不死循环。
pub fn depth_of(edges: &Edges, id: i64) -> Result<usize, DropError> {
    let mut seen: BTreeSet<i64> = BTreeSet::new();
    let mut cur = id;
    let mut d = 0usize;
    while let Some(p) = parent_of(edges, cur)? {
        if !seen.insert(cur) {
            return Ok(d); // 已成环，就地停下，别把界面拖死
        }
        d += 1;
        cur = p;
    }
    Ok(d)
}

/// `ancestor` 是不是 `node` 的祖先（含成环时的早退）。
pub fn is_descendant(edges: &Edges, ancestor: i64, node: i64) -> bool {
    let mut seen: BTreeSet<i64> = BTreeSet::new();
    let mut cur = node;
    while let Ok(Some(p)) = parent_of(edges, cur) {
        if p == ancestor {
            return true;
        }
        if !seen.insert(cur) {
            return false;
        }
        cur = p;
    }
    false
}

/// 子树里所有节点（含自己）。
pub fn subtree_of(edges: &Edges, id: i64) -> Vec<i64> {
    let mut out = vec![id];
    let mut i = 0usize;
    // 边表通常只有几百行，反复线性扫比建索引更好读；`seen` 防环。
    let mut seen: BTreeSet<i64> = BTreeSet::new();
    seen.insert(id);
    while i < out.len() {
        let cur = out[i];
        for (n, p) in edges {
            if *p == Some(cur) && seen.insert(*n) {
                out.push(*n);
            }
        }
        i += 1;
    }
    out
}

/// 子树高度：叶子为 0，有一个子层为 1。
pub fn height_of(edges: &Edges, id: i64) -> usize {
    let nodes = subtree_of(edges, id);
    if nodes.len() <= 1 {
        return 0;
    }
    let mut best = 0usize;
    for n in nodes {
        if let Ok(d) = depth_of(edges, n) {
            best = best.max(d);
        }
    }
    // 子树内最大深度减去自己的深度 = 高度
    let base = depth_of(edges, id).unwrap_or(0);
    best.saturating_sub(base)
}

/// 一批待移动的节点里，只保留「祖先不在本批内」的那些。
///
/// 同时选中父与子时，子会跟着父一起走；若把子也当成独立搬运单位，
/// 索引计算会错乱，还会把父子拆开。
pub fn roots_of(edges: &Edges, ids: &[i64]) -> Vec<i64> {
    let set: BTreeSet<i64> = ids.iter().copied().collect();
    let mut out: Vec<i64> = Vec::new();
    for id in ids {
        let mut cur = *id;
        let mut has_ancestor_inside = false;
        let mut seen: BTreeSet<i64> = BTreeSet::new();
        while let Ok(Some(p)) = parent_of(edges, cur) {
            if set.contains(&p) {
                has_ancestor_inside = true;
                break;
            }
            if !seen.insert(cur) {
                break;
            }
            cur = p;
        }
        if !has_ancestor_inside {
            out.push(*id);
        }
    }
    out.sort_by_key(|id| depth_of(edges, *id).unwrap_or(0));
    out.dedup();
    out
}

/// 删除一批节点前的「子级过继」方案：返回 `(子级 id, 新的父 id)`。
///
/// 为什么需要它：`media.parent_id` 刻意**没有外键约束**（层级规则在应用层校验），
/// 所以直接删父节点会留下一串指向已删行的孤儿——它们在封面墙上永远不显示，
/// 详情页却还挂着「脱离聚合」按钮、显示 `#<已删id>`，等于数据丢了。
///
/// 为什么放在纯规则层：挑「谁接手」是**规则**（遇到环怎么办、候选新父自己也在
/// 删除名单里怎么办），不是 SQL。放这里才能单测；也让「批量删除」（`commands`）
/// 与「扫描收编 / 空壳清理」（`core`）两个方向共用同一份判据——之前只有前者做
/// 了过继，后者裸删 `media`，孤儿照样会出现。
///
/// 规则：
/// - 每个待删节点 `d` 的直接子级，挂到 `d` 的**最近的不在删除名单里的祖先**；
/// - 一路祖先都在名单里 → 挂顶层（`None`）；
/// - **父链成环**（老数据被外部改脏）→ 一律挂顶层，绝不挑一个「不在名单里的
///   子孙」当新父——那样会造出「自己挂自己」，比孤儿还难查；
/// - 子级自己也在删除名单里 → 不出现在方案里（它马上也要被删，改它没意义）。
pub fn plan_adoption(edges: &Edges, doomed: &[i64]) -> Vec<(i64, Option<i64>)> {
    let gone: BTreeSet<i64> = doomed.iter().copied().collect();
    let mut out: Vec<(i64, Option<i64>)> = Vec::new();
    for d in doomed {
        let target = adoption_target(edges, *d, &gone);
        for (n, p) in edges {
            if *p != Some(*d) {
                continue;
            }
            if gone.contains(n) {
                continue;
            }
            // 新父就是它现在的父 = 不用动（方案里不该塞无意义的 UPDATE）
            if target == Some(*n) {
                continue;
            }
            out.push((*n, target));
        }
    }
    out
}

/// 沿父链往上找第一个**不在删除名单里**的祖先；到顶或断链返回该祖先，成环返回 `None`。
///
/// ⚠️ 必须把整条链**走到底**再回答案，不能一遇到「不在名单里的」就停：
/// `parent_id` 没有外键，存量库里可能有环（A 挂 B、B 又挂 A）。那种情况下
/// 「第一个不在名单里的祖先」恰好是自己的子孙，把子级挂上去就成了自己挂自己
/// —— 比孤儿还难查。所以撞到环一律按「挂到顶层」处理。
fn adoption_target(edges: &Edges, id: i64, gone: &BTreeSet<i64>) -> Option<i64> {
    let mut seen: BTreeSet<i64> = BTreeSet::new();
    seen.insert(id);
    let mut first_alive: Option<i64> = None;
    let mut cur = parent_of(edges, id).ok()?;
    while let Some(p) = cur {
        if !seen.insert(p) {
            return None; // 父链成环：挂顶层
        }
        if first_alive.is_none() && !gone.contains(&p) {
            first_alive = Some(p);
        }
        cur = parent_of(edges, p).ok()?;
    }
    first_alive
}

/// 校验一次放置是否合法。
///
/// `target_parent` 为 `None` 表示拖到顶层。
pub fn validate_drop(
    edges: &Edges,
    movers: &[i64],
    target_parent: Option<i64>,
) -> Result<(), DropError> {
    let Some(target) = target_parent else {
        // 拖到顶层永远不会成环，只需要查层数
        return check_depth(edges, movers, None);
    };
    parent_of(edges, target)?;

    for m in movers {
        if *m == target {
            return Err(DropError::SelfTarget { id: *m });
        }
        // 目标在待搬节点的子树里 → 搬完就成环了
        if is_descendant(edges, *m, target) {
            return Err(DropError::Cycle { mover: *m, target });
        }
    }
    check_depth(edges, movers, target_parent)
}

fn check_depth(
    edges: &Edges,
    movers: &[i64],
    target_parent: Option<i64>,
) -> Result<(), DropError> {
    let base = match target_parent {
        Some(p) => depth_of(edges, p)? + 1,
        None => 0,
    };
    for m in movers {
        let deepest = base + height_of(edges, *m);
        if deepest > MAX_DEPTH - 1 {
            return Err(DropError::TooDeep {
                depth: deepest + 1,
                limit: MAX_DEPTH,
            });
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "tests/tree.rs"]
mod tests;
