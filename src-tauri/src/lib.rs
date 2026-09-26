mod boot;
mod collection;
mod commands;
mod db;
mod fanart;
mod grouping;
mod mediatype;
mod http;
mod llm;
mod omdb;
mod parser;
mod scanner;
mod tmdb;
mod tvdb;

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
    let context = tauri::generate_context!();
    let identifier = context.config().identifier.clone();

    // WebView2 缓存自愈。必须在创建任何窗口 / WebView 之前做：那时还没有文件
    // 被本进程占用，清理才删得动（详见 boot 模块头注释）。
    boot::prepare(&identifier);

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(move |app| {
            // 打开（必要时创建）SQLite 数据库并注入全局连接。
            let conn = db::open(app.handle())
                .map_err(|e| std::io::Error::other(format!("数据库初始化失败: {e}")))?;
            app.manage(Mutex::new(conn));
            // 批量刮削的取消开关：后台线程读它，前端通过 `cancel_scrape` 置位。
            app.manage(std::sync::Arc::new(commands::scrape::ScrapeControl::default()));
            // 扫描的取消开关（同上，供 `cancel_scan` 置位）。
            app.manage(std::sync::Arc::new(commands::media::ScanControl::default()));
            // 清掉上一次的就绪标记，再看住这一次——前端没起来就等于白屏，
            // 看门狗会兜底重置并重启一次，而不是让用户对着空窗口发呆。
            boot::clear_ready_marker(app.handle());
            boot::arm_watchdog(app.handle().clone(), identifier);
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
            commands::media::cancel_scan,
            commands::media::get_media_list,
            commands::media::get_media_detail,
            commands::media::get_episodes,
            commands::media::delete_media,
            commands::media::clear_library,
            commands::media::get_scan_history,
            commands::media::create_media,
            commands::media::update_media,
            commands::media::delete_media_batch,
            commands::media::delete_episodes,
            commands::media::import_folder,
            commands::media::reclassify_media,
            commands::media::get_movie_collections,
            // 分类与标签
            commands::catalog::list_categories,
            commands::catalog::create_category,
            commands::catalog::rename_category,
            commands::catalog::delete_category,
            commands::catalog::list_tags,
            commands::catalog::create_tag,
            commands::catalog::rename_tag,
            commands::catalog::delete_tag,
            commands::catalog::get_media_categories,
            commands::catalog::set_media_categories,
            commands::catalog::get_media_tags,
            commands::catalog::set_media_tags,
            commands::catalog::sync_categories_from_tmdb,
            // 刮削
            commands::scrape::search_tmdb,
            commands::scrape::scrape_media,
            commands::scrape::scrape_all,
            commands::scrape::cancel_scrape,
            commands::scrape::apply_tmdb_match,
            commands::scrape::tmdb_test,
            commands::scrape::has_metadata,
            commands::scrape::tvdb_apply,
            commands::scrape::omdb_lookup,
            commands::scrape::llm_presets,
            commands::scrape::llm_parse_name,
            commands::scrape::llm_parse_image,
            // 播放
            commands::play::detect_player,
            commands::play::play_media,
            commands::play::reveal_in_explorer,
            // 启动自检与诊断
            commands::diag::frontend_ready,
            commands::diag::frontend_error,
            commands::diag::read_ready_marker,
            commands::diag::read_error_log,
            commands::diag::runtime_info,
        ])
        .run(context)
        .expect("error while running tauri application");
}
