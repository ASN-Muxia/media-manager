//! 跨模块共用的两个小工具类。
//!
//! 单独抽一层的原因：它们以前在多个文件里各抄了一遍——改动其中一份，
//! 另一份就会悄悄漂移（比如某处超时变了、某处错误处理少了一个分支）。

use std::path::Path;
use std::process::Command;

/// `CREATE_NO_WINDOW`（Win32 进程创建标志）。
///
/// 给子进程带上它之后，Windows **不会为该进程分配控制台**。
/// 背景：本应用是 GUI 子系统（`main.rs` 里的 `windows_subsystem = "windows"`），
/// 自身没有控制台；它 `spawn` 一个控制台子系统程序（`cmd.exe`、`ffprobe.exe`、
/// 各种 Python/Node 打包的 CLI）时，系统会临时新建一个控制台窗口来承载子进程
/// 的 stdout——用户看到的就是那一闪而过的黑框。
///
/// 参考：`dwCreationFlags` 位标志表（Windows App SDK / Win32 文档）。
#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// 让子进程**不创建**控制台窗口（Windows）。
///
/// 用法：套在任何一个 `std::process::Command` 的构造链尾部——
/// `util::no_console(Command::new("cmd").args(["/C", ...])).spawn()`。
/// 返回 `&mut Command` 是为了保持链式写法：`.arg()` / `.args()` 返回的也是它。
#[cfg(target_os = "windows")]
pub fn no_console(cmd: &mut Command) -> &mut Command {
    // `creation_flags` 来自这个 trait，只有 Windows 目标才有。
    use std::os::windows::process::CommandExt;
    cmd.creation_flags(CREATE_NO_WINDOW)
}

/// 非 Windows 上的同名空实现：那些平台没有「控制台窗口」的概念。
///
/// 存在的唯一理由：让 `play` / `playback` / `probe` 里的调用点不必再写一遍
/// `#[cfg]` 分支，换平台也能直接编译。
#[cfg(not(target_os = "windows"))]
#[allow(dead_code)] // 跨平台可编译性的占位实现；本项目只构建 Windows 目标，本机永远不会命中。
pub fn no_console(cmd: &mut Command) -> &mut Command {
    cmd
}

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

/// 当前 Unix 时间戳（秒）。
///
/// 放在 `util` 而不是 `store`：时间戳是纯工具，不属于「怎么存」。
/// 以前它住在 `store::db`，结果 `core` 为了拿个时间就得反向依赖数据层
/// （`core/scanner.rs` 曾 `use crate::store::db::now_epoch`），把分层破了个洞。
/// `store::db` 仍 re-export 同名函数，老调用方一行都不用改。
pub fn now_epoch() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn now_epoch_is_a_plausible_unix_seconds_value() {
        let t = now_epoch();
        // 2021-01-01 ~ 2100-01-01 之间都算合理
        assert!(t > 1_609_459_200 && t < 4_102_444_800, "时间戳不合理: {t}");
    }

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

    /// helper 只加窗口标志，**不得**改动程序名或参数——否则播放/探测会静默跑歪。
    ///
    /// 这里刻意不 `spawn`（测试里起真实进程既慢又依赖外部环境），
    /// 只验证构造链前后命令内容一致。
    #[test]
    fn no_console_keeps_program_and_args_intact() {
        let mut cmd = Command::new("ffprobe");
        cmd.arg("-version");
        let out = no_console(&mut cmd);
        assert_eq!(out.get_program(), "ffprobe");
        let args: Vec<_> = out.get_args().collect();
        assert_eq!(args, vec![std::ffi::OsStr::new("-version")]);
    }
}
