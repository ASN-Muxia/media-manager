//! 统一后台任务模型。
//!
//! 扫描 / 刮削 / 导入 / 导出各有各的事件名与进度结构，新增一个后台能力就要
//! 把「发事件、算百分比、显示什么文字」重新写一遍。这里收拢成一个形状，
//! 前端订阅一次就能显示任何任务。
//!
//! ⚠️ 旧事件 `scan-progress` / `scrape-progress` **保留不动**（B8）：
//! 新事件是**并行**发出的，不是替换。前端可以慢慢迁移，两边都能收到。
//!
//! 百分比**不由后端算**：载荷里给了 `current` / `total`，前端自己除即可——
//! 少一个可能和后端口径打架的字段（`total <= 0` 就是「总数未知」，
//! 前端据此显示不确定态动画）。

use serde::Serialize;

/// 统一任务进度事件名（前端 `listen("job-progress", ...)`）。
pub const JOB_EVENT: &str = "job-progress";

/// 任务种类。
pub const KIND_SCAN: &str = "scan";
pub const KIND_SCRAPE: &str = "scrape";
pub const KIND_IMPORT: &str = "import";
pub const KIND_EXPORT: &str = "export";
/// ffprobe 技术信息探测专用。`provider/probe.rs` 尚未接入扫描流程
/// （刻意的，见该模块头部说明），接入前不会有人发这个种类的事件。
#[allow(dead_code)]
pub const KIND_PROBE: &str = "probe";

/// 一次进度回报。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobProgress {
    /// 任务种类（见 `KIND_*`）。
    pub kind: String,
    /// 阶段名（「遍历目录」「写入数据库」…）。
    pub label: String,
    /// 已完成数量。
    pub current: i64,
    /// 总数；`0` 表示未知（前端显示不确定态动画）。
    pub total: i64,
    /// 当前项的名字（正在处理哪个文件）。
    pub detail: String,
    /// 这一批是否已结束。
    pub settled: bool,
    /// 是否是被取消结束的。
    pub cancelled: bool,
    /// 结束时是否成功（未结束时无意义）。
    pub ok: bool,
    /// 结束时的提示语。
    pub message: String,
}

impl JobProgress {
    /// 进行中。
    pub fn tick(kind: &str, label: &str, current: i64, total: i64, detail: &str) -> Self {
        Self {
            kind: kind.to_string(),
            label: label.to_string(),
            current,
            total,
            detail: detail.to_string(),
            settled: false,
            cancelled: false,
            ok: false,
            message: String::new(),
        }
    }

    /// 已结束（成功）。
    pub fn done(kind: &str, message: &str) -> Self {
        Self {
            kind: kind.to_string(),
            label: "完成".to_string(),
            current: 0,
            total: 0,
            detail: String::new(),
            settled: true,
            cancelled: false,
            ok: true,
            message: message.to_string(),
        }
    }

    /// 已结束（被取消）。
    pub fn cancelled(kind: &str, message: &str) -> Self {
        Self {
            kind: kind.to_string(),
            label: "已取消".to_string(),
            current: 0,
            total: 0,
            detail: String::new(),
            settled: true,
            cancelled: true,
            ok: false,
            message: message.to_string(),
        }
    }

    /// 已结束（出错）。
    pub fn failed(kind: &str, message: &str) -> Self {
        Self {
            kind: kind.to_string(),
            label: "出错".to_string(),
            current: 0,
            total: 0,
            detail: String::new(),
            settled: true,
            cancelled: false,
            ok: false,
            message: message.to_string(),
        }
    }

    /// 百分比（总数未知时返回 None，前端据此显示不确定态）。
    ///
    /// 后端并不需要这个数（它只负责汇报进度），留在这里是为了把
    /// 「`total <= 0` = 总数未知」这条规则定义一次；前端会用同样的口径。
    #[allow(dead_code)]
    pub fn percent(&self) -> Option<f64> {
        crate::commands::job::fraction(self.current, self.total)
    }
}

/// 「已完成 / 总数」→ 百分比；`total <= 0` 表示**总数未知**，返回 `None`。
///
/// 同样只被测试与前端口径引用——后端自己不需要百分比。
#[allow(dead_code)]
pub fn fraction(current: i64, total: i64) -> Option<f64> {
    if total <= 0 || current <= 0 {
        return None;
    }
    let p = current as f64 / total as f64 * 100.0;
    Some(if p > 100.0 { 100.0 } else { p })
}

/// 把进度发出去（沿用现有 `Emitter` 写法，失败就忽略——不能因为发不出事件就中断任务）。
pub fn emit(app: &tauri::AppHandle, p: &JobProgress) {
    use tauri::Emitter;
    let _ = app.emit(JOB_EVENT, p.clone());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_is_none_when_total_unknown() {
        let p = JobProgress::tick(KIND_SCAN, "遍历目录", 10, 0, "x");
        assert_eq!(p.percent(), None, "总数未知时应返回 None（前端显示不确定态）");
    }

    #[test]
    fn percent_is_clamped_at_100() {
        let p = JobProgress::tick(KIND_SCAN, "写入", 150, 100, "x");
        assert_eq!(p.percent(), Some(100.0));
    }

    #[test]
    fn percent_computes_normally() {
        let p = JobProgress::tick(KIND_SCRAPE, "刮削", 25, 100, "x");
        assert_eq!(p.percent(), Some(25.0));
    }

    #[test]
    fn terminal_states_are_distinguishable() {
        let d = JobProgress::done(KIND_SCAN, "好了");
        assert!(d.settled && d.ok && !d.cancelled);
        let c = JobProgress::cancelled(KIND_SCAN, "停了");
        assert!(c.settled && c.cancelled && !c.ok);
        let f = JobProgress::failed(KIND_SCAN, "坏了");
        assert!(f.settled && !f.ok && !f.cancelled);
    }

    /// 旧事件名必须保持存在——前端还在订阅它们（B8）。
    #[test]
    fn legacy_events_still_exist() {
        assert_eq!(crate::commands::media::PROGRESS_EVENT, "scan-progress");
        assert_eq!(crate::commands::scrape::PROGRESS_EVENT, "scrape-progress");
        assert_ne!(JOB_EVENT, crate::commands::media::PROGRESS_EVENT);
    }
}
