//! 启动自检：让前端在挂载成功后回写一个标记文件。
//!
//! 存在的理由：WebView2 的页面内容无法用 PrintWindow / BitBlt 可靠抓取
//! （DirectComposition 合成，截出来常是纯白），所以"UI 到底加载没加载"
//! 光看截图会误判。这个标记文件是确定性的证据——文件在、内容对，就说明
//! 前端 JS 跑起来了、命令桥也通了。

use crate::boot::READY_MARKER;
use crate::db;
use std::sync::Mutex;
use tauri::{AppHandle, State};

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadyInfo {
    pub path: String,
    pub url: String,
}

/// 前端挂载完成后调用；写入 `frontend_ready.txt`。
#[tauri::command]
pub fn frontend_ready(
    app: AppHandle,
    url: String,
    user_agent: String,
) -> Result<ReadyInfo, String> {
    let dir = db::data_dir(&app)?;
    let path = dir.join(READY_MARKER);
    let host = std::env::consts::OS;
    let content = format!(
        "url={url}\nuser_agent={user_agent}\nos={host}\nts={}\n",
        db::now_epoch()
    );
    std::fs::write(&path, content).map_err(|e| format!("写自检标记失败: {e}"))?;
    Ok(ReadyInfo {
        path: path.to_string_lossy().to_string(),
        url,
    })
}

/// 读取自检标记（供排查：数据库连不上时也能看到上次成功加载的时间）。
#[tauri::command]
pub fn read_ready_marker(app: AppHandle) -> Result<Option<String>, String> {
    let dir = db::data_dir(&app)?;
    let path = dir.join(READY_MARKER);
    match std::fs::read_to_string(&path) {
        Ok(s) => Ok(Some(s)),
        Err(_) => Ok(None),
    }
}

/// 前端错误日志的体积上限（超过则只保留后半段，避免无上限增长）。
const ERROR_LOG_LIMIT: usize = 262_144;

/// 往 `frontend_error.log` 追加一行。命令入口与内部调用（看门狗）共用。
pub fn append_log(app: &AppHandle, message: &str) -> Result<(), String> {
    let dir = db::data_dir(app)?;
    let path = dir.join("frontend_error.log");
    let mut buf = std::fs::read_to_string(&path).unwrap_or_default();
    buf.push_str(&format!("[{}] {}\n", db::now_epoch(), message));
    if buf.len() > ERROR_LOG_LIMIT {
        let keep = buf.len() - ERROR_LOG_LIMIT / 2;
        buf = buf.split_off(keep);
    }
    std::fs::write(&path, buf).map_err(|e| format!("写前端错误日志失败: {e}"))?;
    Ok(())
}

/// 前端异常上报（诊断用）：追加写入 `frontend_error.log`。
///
/// 存在的理由与 `frontend_ready` 相同——WebView2 的画面抓不到，白屏时
/// 只能靠日志确定「脚本到底跑没跑、在哪一步炸的」。前端在挂载前后都会打点。
#[tauri::command]
pub fn frontend_error(app: AppHandle, message: String) -> Result<(), String> {
    append_log(&app, &message)
}

/// 读取前端错误日志尾部（设置页「诊断」里可看）。
#[tauri::command]
pub fn read_error_log(app: AppHandle, limit: Option<usize>) -> Result<String, String> {
    let dir = db::data_dir(&app)?;
    let path = dir.join("frontend_error.log");
    let mut buf = std::fs::read_to_string(&path).unwrap_or_default();
    let max = limit.unwrap_or(4000);
    if buf.len() <= max {
        return Ok(buf);
    }
    let keep = buf.len() - max;
    Ok(buf.split_off(keep))
}

/// 当前窗口实际使用的 URL（诊断用：可区分 dev 模式与内嵌资源模式）。
#[tauri::command]
pub fn runtime_info(
    app: AppHandle,
    state: State<'_, Mutex<rusqlite::Connection>>,
) -> Result<serde_json::Value, String> {
    let conn = state.lock().map_err(|e| e.to_string())?;
    let tables = db::tables(&conn)?;
    let schema: i32 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    let db_file = db::db_path(&app)?.to_string_lossy().to_string();
    Ok(serde_json::json!({
        "dbPath": db_file,
        "schemaVersion": schema,
        "tables": tables,
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
    }))
}
