mod commands;
mod db;

use serde::Serialize;
use std::sync::Mutex;
use tauri::Manager;

#[derive(Serialize)]
struct Ping {
    ok: bool,
}

#[tauri::command]
fn ping() -> Ping {
    Ping { ok: true }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            // 打开（必要时创建）SQLite 数据库并注入全局连接。
            let conn = db::open(app.handle())
                .map_err(|e| std::io::Error::other(format!("数据库初始化失败: {e}")))?;
            app.manage(Mutex::new(conn));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            ping,
            commands::settings::get_settings,
            commands::settings::set_settings,
            commands::settings::db_info,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
