// 分层要点：`core`（领域规则）/ `provider`（外部数据源）/ `store`（持久化）
// 三者互不反向依赖，只有最上面这层的 `commands` 会同时用到它们。
// 好处是改 UI 或换数据库时不用碰判定逻辑，反之亦然。
mod assets;
mod boot;
mod commands;
mod core;
mod provider;
mod share;
mod store;
mod util;

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
    // 前端资源改为**运行时优先**：磁盘上的 dist 认得就用，认不到再回退到
    // `generate_context!` 编译期嵌入的那份（分发包没有 dist，走的正是回退）。
    // 详见 src/assets.rs 的模块头注释。
    let context = crate::assets::RuntimeAssets::install(context);
    let identifier = context.config().identifier.clone();

    // WebView2 缓存自愈。必须在创建任何窗口 / WebView 之前做：那时还没有文件
    // 被本进程占用，清理才删得动（详见 boot 模块头注释）。
    boot::prepare(&identifier);

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(move |app| {
            // 打开（必要时创建）SQLite 数据库并注入全局连接。
            let conn = store::db::open(app.handle())
                .map_err(|e| std::io::Error::other(format!("数据库初始化失败: {e}")))?;
            app.manage(Mutex::new(conn));
            // 批量刮削的取消开关：后台线程读它，前端通过 `cancel_scrape` 置位。
            app.manage(std::sync::Arc::new(commands::control::ScrapeControl::default()));
            // 扫描的取消开关（同上，供 `cancel_scan` 置位）。
            app.manage(std::sync::Arc::new(commands::control::ScanControl::default()));
            // 层级操作的撤销栈（进程内，只记「改之前的样子」）。
            app.manage(commands::tree::TreeUndo::default());
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
            commands::media::import_folder_into,
            commands::media::reclassify_media,
            // 资源目录库（每次检测 / 增量添加时对比并定位旧资源）
            commands::registry::sync_resource_catalog,
            commands::registry::get_resource_catalog,
            commands::registry::diff_resource_catalog,
            // 剧集层级（树形拖拽 / 嵌套 / 另存为 / 撤销）
            commands::tree::get_media_tree,
            commands::tree::move_media_nodes,
            commands::tree::copy_media_node,
            commands::tree::undo_tree_op,
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
            // 豆瓣（中文元数据兜底）
            commands::scrape::search_douban,
            commands::scrape::douban_apply,
            commands::scrape::douban_test,
            commands::scrape::llm_presets,
            commands::scrape::llm_parse_name,
            commands::scrape::llm_parse_image,
            // 播放
            commands::play::detect_player,
            commands::play::play_media,
            commands::play::reveal_in_explorer,
            // 观看状态 / 续播 / 连播（v6 新增）
            commands::playback::get_playback,
            commands::playback::get_playback_many,
            commands::playback::recent_playback,
            commands::playback::mark_watched,
            commands::playback::mark_watched_batch,
            commands::playback::set_playback_position,
            commands::playback::clear_playback,
            commands::playback::play_episode,
            // NFO 互通 / 备份（v6 新增）
            commands::share::export_nfo,
            commands::share::import_nfo,
            commands::share::backup_database,
            // 启动自检与诊断
            commands::diag::frontend_ready,
            commands::diag::frontend_error,
            commands::diag::read_ready_marker,
            commands::diag::read_error_log,
            commands::diag::runtime_info,
            commands::diag::library_health,
            commands::diag::clear_metadata_cache,
            commands::diag::purge_expired_cache,
            commands::diag::list_providers,
        ])
        .run(context)
        .expect("error while running tauri application");
}
