// 始终以 GUI 子系统构建：无论 debug 还是 release，都不为 exe 单独弹出控制台窗口。
// （原来写成 `not(debug_assertions)` 才用 windows 子系统，导致 debug 构建的 exe
//   启动时会在主窗口后方多开一个控制台黑窗。）
#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

fn main() {
    media_manager_lib::run()
}
