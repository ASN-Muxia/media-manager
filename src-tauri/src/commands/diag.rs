//! 启动自检：让前端在挂载成功后回写一个标记文件。
//!
//! 存在的理由：WebView2 的页面内容无法用 PrintWindow / BitBlt 可靠抓取
//! （DirectComposition 合成，截出来常是纯白），所以"UI 到底加载没加载"
//! 光看截图会误判。这个标记文件是确定性的证据——文件在、内容对，就说明
//! 前端 JS 跑起来了、命令桥也通了。

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
    let path = dir.join("frontend_ready.txt");
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
    let path = dir.join("frontend_ready.txt");
    match std::fs::read_to_string(&path) {
        Ok(s) => Ok(Some(s)),
        Err(_) => Ok(None),
    }
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
