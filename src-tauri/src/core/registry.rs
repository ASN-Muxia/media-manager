//! 资源目录库的**纯规则**（不碰 IO、不碰数据库连接）。
//!
//! ## 为什么要有这一层
//! `core/diff.rs` 的 `Fingerprint` 只回答「同一个文件变了没有」——它没有路径、
//! 没有库表，所以「上次检测时这个文件在哪、多大、什么时候不见的」全都留不下来。
//! 资源目录库（`resource_assets` 表）把**磁盘上的文件**与**库里的条目**用绝对路径
//! 一一对上，本模块负责其中最容易写错、也最值得单测的两条判定：
//!
//! 1. `classify`：某个路径这一轮该算 新增 / 未变 / 变动 / 回来了；
//! 2. `reconcile`：库里有、这次没遍历到的行，哪些该被标成 `missing`。
//!
//! 放在 `core` 而不是命令层：这两条规则决定了「旧资源还能不能找回来」，
//! 一旦写错就是静默丢数据（比如把整盘没走到的目录全标成消失），
//! 必须能脱离数据库连接单独验证。
//!
//! ## 与 `core::diff` 同一条原则
//! 指纹缺一侧时**宁可保守**：多写一次库不要紧，漏掉一次真实变动会让
//! 「上次检测」的对比结论变成假话。
//!
//! ## status=missing 的行永不删除
//! 本模块只产出「谁该被标成 missing」的 id 列表，**从不产出删除**——
//! 行留下来才有「它上次在哪、多大、什么时候消失」可查，这正是「定位旧资源」的落点。
//! 文件重新出现时 `classify` 给出 `Restored`，命令层把它置回 `present`。

use std::collections::BTreeSet;

/// `status` 取值：最近一次检测时文件还在。
pub const STATUS_PRESENT: &str = "present";
/// `status` 取值：最近一次检测时文件已经不在了（**行保留**，见模块说明）。
pub const STATUS_MISSING: &str = "missing";

/// 一个文件当前的磁盘指纹。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AssetFingerprint {
    /// 字节大小。
    pub size: u64,
    /// 修改时间（Unix 秒）。取不到时为 `None`（此时一律保守当成「变了」）。
    pub mtime: Option<i64>,
}

/// 一个路径相对库里那一行的变化结论。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssetChange {
    /// 库里没有这个路径：本次新收录。
    New,
    /// 与库里记录完全一致：不写库（增量扫描省下的就是这部分写入）。
    Unchanged,
    /// 大小或修改时间变了。**缺 mtime 时也走这里**（保守优先）。
    Modified,
    /// 库里那一行是 `missing`，这次又在磁盘上看到了：置回 `present`。
    Restored,
}

/// 判定一个路径相对库里那一行的变化。
///
/// `prev` 是库里已有的 `(size, mtime, status)`；`None` 表示这个路径还没入库。
///
/// 判据顺序有意如此：
/// - 先看「库里有没有」——没有就是 `New`；
/// - 再看「上次是不是被判过消失」——是则 `Restored`。它排在指纹比较之前，
///   因为「文件回来了」这件事比「大小变没变」更重要，也更该被单独计数；
/// - 最后比指纹：大小不同 → `Modified`；mtime 两侧都取到且相同才敢说
///   `Unchanged`，**任一侧取不到 mtime 就保守当成 `Modified`**。
pub fn classify(prev: Option<(u64, Option<i64>, &str)>, now: &AssetFingerprint) -> AssetChange {
    let Some((size, mtime, status)) = prev else {
        return AssetChange::New;
    };
    if status == STATUS_MISSING {
        return AssetChange::Restored;
    }
    if size != now.size {
        return AssetChange::Modified;
    }
    match (now.mtime, mtime) {
        (Some(a), Some(b)) if a == b => AssetChange::Unchanged,
        // 缺一侧 mtime（或两边都缺）：无法证明没变，按「变了」处理。
        // 与 `core::diff::Fingerprint::changed_from` 同一条兜底逻辑。
        _ => AssetChange::Modified,
    }
}

/// 从「库里 status=present 的行」与「本次磁盘所见路径」算出**应当置为 missing** 的 id。
///
/// `existing` 是 `(id, path)`，调用方只传**本次真正遍历到的那几个根目录下**的行
/// ——盘没挂上 / 目录被删时整盘都不该被判成消失，那是灾难性的误报。
/// 已 `missing` 的行不该出现在 `existing` 里；即便传进来也会被状态判断挡掉，
/// 因为它们本来就已经是 missing（重复标记只会反复刷新 `missing_at`，
/// 把「什么时候消失的」这个最关键的线索冲掉）。
///
/// 纯函数、不排序、不查库：调用方拿到的 id 顺序稳定（按传入顺序），便于断言。
pub fn reconcile(existing: &[(i64, String)], seen: &BTreeSet<String>) -> Vec<i64> {
    let mut out: Vec<i64> = Vec::new();
    for (id, path) in existing {
        if !seen.contains(path.as_str()) {
            out.push(*id);
        }
    }
    out
}

#[cfg(test)]
#[path = "tests/registry.rs"]
mod tests;
