//! 增量扫描指纹（**纯函数**，不碰 IO）。
//!
//! ## 为什么是「指纹」而不是「只遍历变更目录」
//! 最省事的做法是只遍历 mtime 变过的目录，但那会直接破坏类型判定：
//! `mediatype::build_folder_stats` 的**目录共识**（R7/R8）依赖「这个目录里到底
//! 有几个带集号的文件」。只遍历一部分文件，统计基数就变了，
//! 一整季里只改了 1 集，其余 19 集就会被当成「这个目录只有 1 个文件」——
//! 判定结果随之翻案。这正是本项目最怕的「重扫把用户已确认的分类推翻」。
//!
//! 所以增量只发生在**写入阶段**：目录照样全量遍历、共识统计照样基于全目录，
//! 只是「没变过的文件不写库」。省掉的是 SQL 写入与事务体积，判定一行不改。
//!
//! ## 接线状态：**已实现，尚未接入扫描流程**（刻意）
//! 接上它要在 `import_reported` 里加一层过滤。虽然设计上不影响判定，
//! 但它改的是扫描主流程（B1），必须单独验证一轮再启用 —— 尤其是
//! `file_mtime` 在老库里恒为 NULL，「缺 mtime 一律视为变了」这条兜底
//! 是否真的让第一次增量扫描等价于全量，需要实测而不是推演。
//! 在接入之前，本模块的公开项都不会被引用到：下面的 `allow` 是**刻意保留**。
#![allow(dead_code)]

/// 一个文件的变更指纹。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fingerprint {
    /// 字节大小。
    pub size: u64,
    /// 修改时间（Unix 秒）。取不到时为 `None`（此时一律视为「可能变了」）。
    pub mtime: Option<i64>,
}

impl Fingerprint {
    /// 与库里记录的那一份相比，是否发生了变化。
    ///
    /// 任一侧缺 mtime 就返回 `true`——宁可多写一次，也不能漏掉真改动。
    pub fn changed_from(&self, prev: Option<Fingerprint>) -> bool {
        match prev {
            None => true,
            Some(p) => {
                if p.size != self.size {
                    return true;
                }
                match (self.mtime, p.mtime) {
                    (Some(a), Some(b)) => a != b,
                    _ => true,
                }
            }
        }
    }
}

/// 从 `std::fs::Metadata` 取指纹（取不到 mtime 也不失败）。
pub fn of_meta(size: u64, modified: Option<i64>) -> Fingerprint {
    Fingerprint { size, mtime: modified }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_previous_record_means_changed() {
        let fp = Fingerprint { size: 10, mtime: Some(1) };
        assert!(fp.changed_from(None));
    }

    #[test]
    fn identical_fingerprint_is_unchanged() {
        let a = Fingerprint { size: 10, mtime: Some(5) };
        assert!(!a.changed_from(Some(a)));
    }

    #[test]
    fn size_or_mtime_change_is_detected() {
        let old = Fingerprint { size: 10, mtime: Some(5) };
        assert!(Fingerprint { size: 11, mtime: Some(5) }.changed_from(Some(old)));
        assert!(Fingerprint { size: 10, mtime: Some(6) }.changed_from(Some(old)));
    }

    /// 缺 mtime 时保守地当成「变了」——漏写比漏更新更糟。
    #[test]
    fn missing_mtime_is_treated_as_changed() {
        let old = Fingerprint { size: 10, mtime: Some(5) };
        assert!(Fingerprint { size: 10, mtime: None }.changed_from(Some(old)));
        assert!(old.changed_from(Some(Fingerprint { size: 10, mtime: None })));
    }

    /// 关键回归：判定必须建立在**全目录**统计上，而不是「本次变更的文件」上。
    /// 这里用指纹函数表达同一件事——单看一个文件永远得不出目录结论。
    #[test]
    fn one_changed_file_does_not_shrink_the_folder_view() {
        let files: Vec<Fingerprint> = (0..20)
            .map(|i| Fingerprint { size: 100, mtime: Some(i) })
            .collect();
        let changed: Vec<&Fingerprint> = files
            .iter()
            .filter(|f| f.changed_from(Some(Fingerprint { size: 100, mtime: Some(0) })))
            .collect();
        assert_eq!(changed.len(), 19, "19 个文件与基准不同");
        assert_eq!(files.len(), 20, "但目录视图始终是 20 —— 统计基数不能被增量裁剪");
    }
}
