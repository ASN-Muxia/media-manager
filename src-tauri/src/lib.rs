mod commands;
mod db;
mod parser;
mod scanner;
mod tmdb;

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
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            // 打开（必要时创建）SQLite 数据库并注入全局连接。
            let conn = db::open(app.handle())
                .map_err(|e| std::io::Error::other(format!("数据库初始化失败: {e}")))?;
            app.manage(Mutex::new(conn));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            ping,
            // 设置
            commands::settings::get_settings,
            commands::settings::set_settings,
            commands::settings::db_info,
            commands::settings::open_data_dir,
            // 媒体库
            commands::media::scan_library,
            commands::media::get_media_list,
            commands::media::get_media_detail,
            commands::media::get_episodes,
            commands::media::delete_media,
            commands::media::clear_library,
            commands::media::get_scan_history,
            // 刮削
            commands::scrape::search_tmdb,
            commands::scrape::scrape_media,
            commands::scrape::scrape_all,
            commands::scrape::apply_tmdb_match,
            commands::scrape::has_metadata,
            // 播放
            commands::play::detect_player,
            commands::play::play_media,
            commands::play::reveal_in_explorer,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
