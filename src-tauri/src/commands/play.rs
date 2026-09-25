//! 外部播放器桥接：优先 PotPlayer，支持自动探测常见安装路径。

use crate::commands::settings::load_potplayer_path;
use rusqlite::Connection;
use std::path::Path;
use std::process::Command;
use std::sync::Mutex;
use tauri::State;

/// 常见的 PotPlayer 安装路径（含 64/32 位与常见自定义目录）。
const POTPLAYER_CANDIDATES: &[&str] = &[
    r"C:\Program Files\DAUM\PotPlayer\PotPlayerMini64.exe",
    r"C:\Program Files\PotPlayer\PotPlayerMini64.exe",
    r"C:\Program Files (x86)\DAUM\PotPlayer\PotPlayerMini.exe",
    r"C:\Program Files (x86)\PotPlayer\PotPlayerMini.exe",
    r"D:\Program Files\DAUM\PotPlayer\PotPlayerMini64.exe",
    r"D:\PotPlayer\PotPlayerMini64.exe",
    r"E:\PotPlayer\PotPlayerMini64.exe",
];

fn file_exists(p: &str) -> bool {
    Path::new(p).is_file()
}

/// 决定使用哪个播放器：优先用户配置，其次自动探测。抽成纯函数便于测试。
pub fn resolve_player(configured: &str, exists: &dyn Fn(&str) -> bool) -> Option<String> {
    let configured = configured.trim();
    if !configured.is_empty() && exists(configured) {
        return Some(configured.to_string());
    }
    POTPLAYER_CANDIDATES
        .iter()
        .find(|c| exists(c))
        .map(|c| (*c).to_string())
}

/// 自动探测到的 PotPlayer 路径（供设置页展示）。
#[tauri::command]
pub fn detect_player(state: State<'_, Mutex<Connection>>) -> Result<Option<String>, String> {
    let configured = {
        let conn = state.lock().map_err(|e| e.to_string())?;
        load_potplayer_path(&conn)
    };
    Ok(resolve_player(&configured, &file_exists))
}

/// 用 PotPlayer 播放媒体文件；找不到播放器时回退到系统默认关联程序。
/// 返回实际使用的播放器路径（或 `"system"` 表示系统默认）。
#[tauri::command]
pub fn play_media(state: State<'_, Mutex<Connection>>, path: String) -> Result<String, String> {
    if !Path::new(&path).is_file() {
        return Err(format!("文件不存在：{path}"));
    }
    let configured = {
        let conn = state.lock().map_err(|e| e.to_string())?;
        load_potplayer_path(&conn)
    };

    match resolve_player(&configured, &file_exists) {
        Some(player) => {
            Command::new(&player)
                .arg(&path)
                .spawn()
                .map_err(|e| format!("启动 PotPlayer 失败：{e}"))?;
            Ok(player)
        }
        None => {
            // 交给系统默认播放器（ShellExecute 语义）
            Command::new("cmd")
                .args(["/C", "start", "", path.as_str()])
                .spawn()
                .map_err(|e| format!("未找到 PotPlayer，且调用系统默认播放器失败：{e}"))?;
            Ok("system".to_string())
        }
    }
}

/// 在资源管理器中定位文件。
#[tauri::command]
pub fn reveal_in_explorer(path: String) -> Result<(), String> {
    if !Path::new(&path).exists() {
        return Err(format!("路径不存在：{path}"));
    }
    Command::new("explorer")
        .arg(format!("/select,{path}"))
        .spawn()
        .map_err(|e| format!("打开资源管理器失败：{e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configured_path_wins_when_it_exists() {
        let exists = |p: &str| p == r"D:\My\PotPlayer.exe";
        assert_eq!(
            resolve_player(r"D:\My\PotPlayer.exe", &exists).as_deref(),
            Some(r"D:\My\PotPlayer.exe")
        );
    }

    #[test]
    fn falls_back_to_candidate_when_configured_missing() {
        let exists = |p: &str| p == POTPLAYER_CANDIDATES[1];
        assert_eq!(
            resolve_player(r"D:\Nope\PotPlayer.exe", &exists).as_deref(),
            Some(POTPLAYER_CANDIDATES[1])
        );
    }

    #[test]
    fn empty_config_falls_back_to_candidate() {
        let exists = |p: &str| p == POTPLAYER_CANDIDATES[0];
        assert_eq!(
            resolve_player("   ", &exists).as_deref(),
            Some(POTPLAYER_CANDIDATES[0])
        );
    }

    #[test]
    fn returns_none_when_nothing_found() {
        let exists = |_p: &str| false;
        assert!(resolve_player("", &exists).is_none());
    }
}
