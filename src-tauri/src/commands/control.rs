//! 长任务的「取消 / 运行中」开关：扫描与批量刮削共用一套实现。
//!
//! 这两份原本各抄一遍（字段与方法逐项相同），改一处就会漏另一处。
//! 这里把实现收成 `TaskControl`，再让 `ScanControl` / `ScrapeControl`
//! 通过 `Deref` 复用它。
//!
//! 为什么不让这两个名字合成一个类型：tauri 的 `app.manage` **按 Rust 类型区分**
//! 托管对象，`app.manage(Arc<TaskControl>)` 调两次，后者会把前者覆盖掉。
//! 所以「同实现」不等于「同类型」——保留两个 newtype，各自还是一个独立类型。

use std::ops::Deref;
use std::sync::atomic::{AtomicBool, Ordering};

/// 一个后台长任务的开关：一句 `request_cancel()`，任务循环用 `cancelled()` 轮询退出。
#[derive(Debug, Default)]
pub struct TaskControl {
    cancel: AtomicBool,
    running: AtomicBool,
}

impl TaskControl {
    /// 开始新一轮：先复位取消标志，再置运行中。
    ///
    /// 会**无条件**抢占。自动化触发请改用 [`try_begin`]，避免把用户正在跑的
    /// 任务顶掉。
    pub fn begin(&self) {
        self.cancel.store(false, Ordering::SeqCst);
        self.running.store(true, Ordering::SeqCst);
    }

    /// 尝试开始：已经在跑就返回 `false` 且**不动**当前任务。
    ///
    /// 用 `compare_exchange` 而不是「先 `running()` 再 `begin()`」：后者两步之间
    /// 有窗口，自动触发可能正好插进用户手动启动的任务里，把它的取消标志清掉。
    pub fn try_begin(&self) -> bool {
        if self
            .running
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            self.cancel.store(false, Ordering::SeqCst);
            true
        } else {
            false
        }
    }

    pub fn finish(&self) {
        self.running.store(false, Ordering::SeqCst);
    }

    pub fn request_cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }

    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }

    pub fn running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }
}

macro_rules! task_control {
    ($name:ident, $doc:expr) => {
        #[doc = $doc]
        #[derive(Debug, Default)]
        pub struct $name(TaskControl);

        impl Deref for $name {
            type Target = TaskControl;
            fn deref(&self) -> &TaskControl {
                &self.0
            }
        }
    };
}

// 两个 newtype：实现共用 `TaskControl`，类型上彼此独立。
task_control!(
    ScanControl,
    "扫描的取消 / 运行中开关。放在 `Arc` 里由 app 托管，后台线程与命令共享同一个实例。"
);
task_control!(
    ScrapeControl,
    "批量刮削的取消 / 运行中开关。放在 `Arc` 里由 app 托管，后台线程与命令共享同一个实例。"
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn begin_resets_cancel_and_marks_running() {
        let c = TaskControl::default();
        assert!(!c.running());
        c.begin();
        assert!(c.running());
        assert!(!c.cancelled());
        c.request_cancel();
        assert!(c.cancelled());
        c.finish();
        assert!(!c.running());
    }

    #[test]
    fn next_round_clears_the_previous_cancel() {
        let c = ScanControl::default();
        c.request_cancel();
        assert!(c.cancelled());
        c.begin();
        assert!(!c.cancelled());
    }

    /// 自动触发不能让用户正在跑的任务被打断。
    #[test]
    fn try_begin_refuses_while_running() {
        let c = ScrapeControl::default();
        assert!(c.try_begin(), "空闲时应该能开始");
        assert!(c.running());

        c.request_cancel();
        assert!(!c.try_begin(), "已在运行时必须拒绝");
        assert!(c.cancelled(), "拒绝时不能把用户任务的取消标志清掉");
        assert!(c.running());

        c.finish();
        assert!(c.try_begin(), "上一轮结束后应能再次开始");
        assert!(!c.cancelled(), "新一轮要清掉上一轮的取消标志");
    }
}
