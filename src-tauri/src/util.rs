//! 跨模块共用的两个小工具类。
//!
//! 单独抽一层的原因：它们以前在多个文件里各抄了一遍——改动其中一份，
//! 另一份就会悄悄漂移（比如某处超时变了、某处错误处理少了一个分支）。

use std::path::Path;

/// 惰性编译一次正则，之后每次调用复用同一个实例。
///
/// 为什么要有这个宏：文件扫描要处理上万个文件名，`Regex::new` 每次都重编一遍
/// 是纯粹的浪费；用 `OnceLock` 缓存后每个模式只编译一次。
/// 三份完全相同的 `macro_rules!` 曾散落在 `parser` / `mediatype` / `collection`
/// 里，这里收成唯一一份。
///
/// 用法：`lazy_re!(re_name, r"(?i)abc");` 之后用 `re_name()` 取 `&'static Regex`。
/// 调用侧需要自己引入 `std::sync::OnceLock` 与 `regex::Regex`（宏展开里会用到）。
macro_rules! lazy_re {
    ($fn_name:ident, $pat:expr) => {
        fn $fn_name() -> &'static Regex {
            static R: OnceLock<Regex> = OnceLock::new();
            R.get_or_init(|| Regex::new($pat).expect("内置正则应可编译"))
        }
    };
}
// Rust 2018 起宏也可以按路径导入，这样别的模块 `use crate::util::lazy_re;` 即可。
pub(crate) use lazy_re;

/// 文件所在目录的**完整路径**（同一目录 = 同一批「兄弟姐妹」，用作分组键）。
pub fn parent_dir(path: &Path) -> String {
    path.parent()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default()
}

/// 文件所在目录的**目录名**（不含上层路径）。
///
/// 「这里的目录叫什么」是最强的归属证据——文件名常被发行组前缀弄脏，
/// 目录名通常干净（见 `grouping::belongs_to_parent`）。
pub fn dir_name(path: &Path) -> Option<String> {
    path.parent()
        .and_then(|p| p.file_name())
        .map(|s| s.to_string_lossy().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parent_dir_and_dir_name_split_the_same_path() {
        let p = Path::new(r"D:\电视剧\神秘博士\S07\Doctor.Who.S07E01.mkv");
        assert_eq!(parent_dir(p), r"D:\电视剧\神秘博士\S07");
        assert_eq!(dir_name(p).as_deref(), Some("S07"));
    }

    #[test]
    fn root_relative_path_has_no_parent() {
        let p = Path::new("01.mkv");
        assert_eq!(parent_dir(p), "");
        assert_eq!(dir_name(p), None);
    }
}
