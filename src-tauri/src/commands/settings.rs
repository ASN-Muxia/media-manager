//! 设置项持久化：基于 `settings` 表（key/value）。
//!
//! 前端契约（`src/stores/settingsStore.ts`）：`{ tmdbKey, mediaDirs, potPlayerPath }`。

use crate::store::db;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use tauri::State;

const KEY_TMDB_KEY: &str = "tmdbKey";
const KEY_MEDIA_DIRS: &str = "mediaDirs";
const KEY_POTPLAYER: &str = "potPlayerPath";
const KEY_HOSTS: &str = "hosts";
const KEY_LANGUAGE: &str = "tmdbLanguage";
const KEY_TVDB_KEY: &str = "tvdbKey";
const KEY_FANART_KEY: &str = "fanartKey";
const KEY_OMDB_KEY: &str = "omdbKey";
const KEY_DOUBAN_ENABLED: &str = "doubanEnabled";
const KEY_DOUBAN_COOKIE: &str = "doubanCookie";
const KEY_LLM_ENABLED: &str = "llmEnabled";
const KEY_LLM_BASE_URL: &str = "llmBaseUrl";
const KEY_LLM_API_KEY: &str = "llmApiKey";
const KEY_LLM_MODEL: &str = "llmModel";
const KEY_LLM_VISION_MODEL: &str = "llmVisionModel";
const KEY_LLM_VISION_BASE_URL: &str = "llmVisionBaseUrl";

/// 设置结构体，字段与前端 settingsStore 一一对应。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    #[serde(default)]
    pub tmdb_key: String,
    #[serde(default)]
    pub media_dirs: Vec<String>,
    #[serde(default)]
    pub pot_player_path: String,
    /// 自定义域名解析（hosts 文件格式，一行一条：`IP 域名`）。
    /// 只作用于本应用自己发出的 HTTP 请求（TMDB 刮削），不改系统 hosts，
    /// 也不影响 WebView 加载的图片（见设置页说明）。
    #[serde(default)]
    pub hosts: Vec<String>,
    /// 元数据语言（`zh-CN` / `en-US` …）。留空按 `tmdb::DEFAULT_LANGUAGE` 处理。
    #[serde(default)]
    pub tmdb_language: String,
    /// TheTVDB API Key（剧集季 / 集结构）。
    #[serde(default)]
    pub tvdb_key: String,
    /// Fanart.tv API Key（海报 / 背景补充图源）。
    #[serde(default)]
    pub fanart_key: String,
    /// OMDb API Key（IMDb 评分补充）。
    #[serde(default)]
    pub omdb_key: String,
    /// 豆瓣刮削总开关。**默认关闭**——豆瓣没有官方 API，接口随时可能变动，
    /// 也涉及第三方站点的使用条款，交给用户自己决定要不要开。
    #[serde(default)]
    pub douban_enabled: bool,
    /// 豆瓣 Cookie（可选）。带上登录态能显著降低被要求验证码的概率，
    /// 留空也能用（匿名请求受到更严格的限流）。
    #[serde(default)]
    pub douban_cookie: String,
    /// 大模型脏文件名解析总开关（关掉就完全不联网）。
    #[serde(default)]
    pub llm_enabled: bool,
    /// 大模型接口地址（OpenAI 兼容）。
    #[serde(default)]
    pub llm_base_url: String,
    #[serde(default)]
    pub llm_api_key: String,
    #[serde(default)]
    pub llm_model: String,
    /// 视觉模型名（看截图兜底用）；留空表示不启用视觉兜底。
    #[serde(default)]
    pub llm_vision_model: String,
    /// 视觉模型接口地址；留空则与 `llmBaseUrl` 同址。
    #[serde(default)]
    pub llm_vision_base_url: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            tmdb_key: String::new(),
            media_dirs: Vec::new(),
            pot_player_path: String::new(),
            hosts: Vec::new(),
            tmdb_language: crate::provider::tmdb::DEFAULT_LANGUAGE.to_string(),
            tvdb_key: String::new(),
            fanart_key: String::new(),
            omdb_key: String::new(),
            douban_enabled: false,
            douban_cookie: String::new(),
            llm_enabled: false,
            llm_base_url: String::new(),
            llm_api_key: String::new(),
            llm_model: String::new(),
            llm_vision_model: String::new(),
            llm_vision_base_url: String::new(),
        }
    }
}

/// 数据库健康信息。
#[derive(Debug, Serialize)]
pub struct DbInfo {
    pub tables: Vec<String>,
}

fn read_value(conn: &Connection, key: &str) -> Option<String> {
    conn.query_row(
        "SELECT value FROM settings WHERE key = ?1",
        params![key],
        |r| r.get::<_, String>(0),
    )
    .ok()
}

/// 读取媒体目录列表（供扫描命令复用）。
pub fn load_media_dirs(conn: &Connection) -> Vec<String> {
    read_value(conn, KEY_MEDIA_DIRS)
        .and_then(|raw| serde_json::from_str::<Vec<String>>(&raw).ok())
        .unwrap_or_default()
}

/// 读取 PotPlayer 路径（供播放命令复用）。
pub fn load_potplayer_path(conn: &Connection) -> String {
    read_value(conn, KEY_POTPLAYER).unwrap_or_default()
}

/// 读取 TMDB API Key（供刮削命令复用）。
pub fn load_tmdb_key(conn: &Connection) -> String {
    read_value(conn, KEY_TMDB_KEY).unwrap_or_default()
}

/// 读取自定义 hosts 映射（供刮削命令复用）。
pub fn load_hosts(conn: &Connection) -> Vec<String> {
    read_value(conn, KEY_HOSTS)
        .and_then(|raw| serde_json::from_str::<Vec<String>>(&raw).ok())
        .unwrap_or_default()
}

/// 读取元数据语言（供刮削命令复用）；未配置时回落默认语言。
pub fn load_tmdb_language(conn: &Connection) -> String {
    let v = read_value(conn, KEY_LANGUAGE).unwrap_or_default();
    if v.trim().is_empty() {
        crate::provider::tmdb::DEFAULT_LANGUAGE.to_string()
    } else {
        v.trim().to_string()
    }
}

/// 读取 TheTVDB Key（剧集季 / 集结构）。
pub fn load_tvdb_key(conn: &Connection) -> String {
    read_value(conn, KEY_TVDB_KEY).unwrap_or_default()
}

/// 读取 Fanart.tv Key（补充图源）。
pub fn load_fanart_key(conn: &Connection) -> String {
    read_value(conn, KEY_FANART_KEY).unwrap_or_default()
}

/// 读取 OMDb Key（评分补充）。
pub fn load_omdb_key(conn: &Connection) -> String {
    read_value(conn, KEY_OMDB_KEY).unwrap_or_default()
}

/// 读取豆瓣刮削配置：`(是否启用, Cookie)`。
///
/// 默认关闭 —— 没写过这个键就是 `false`，不会因为升级而偷偷开始联网抓豆瓣。
pub fn load_douban_config(conn: &Connection) -> (bool, String) {
    let enabled = read_value(conn, KEY_DOUBAN_ENABLED)
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    (enabled, read_value(conn, KEY_DOUBAN_COOKIE).unwrap_or_default())
}

/// 读取大模型配置（脏文件名解析增强层，可选）。
pub fn load_llm_config(conn: &Connection) -> crate::provider::llm::LlmConfig {
    crate::provider::llm::LlmConfig {
        enabled: read_value(conn, KEY_LLM_ENABLED)
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false),
        base_url: read_value(conn, KEY_LLM_BASE_URL).unwrap_or_default(),
        api_key: read_value(conn, KEY_LLM_API_KEY).unwrap_or_default(),
        model: read_value(conn, KEY_LLM_MODEL).unwrap_or_default(),
        vision_model: read_value(conn, KEY_LLM_VISION_MODEL).unwrap_or_default(),
        vision_base_url: read_value(conn, KEY_LLM_VISION_BASE_URL).unwrap_or_default(),
    }
}

fn write_value(conn: &Connection, key: &str, value: &str) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO settings (key, value) VALUES (?1, ?2) \
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

/// 读取全部设置（缺失项回落默认值）。
#[tauri::command]
pub fn get_settings(state: State<'_, Mutex<Connection>>) -> Result<Settings, String> {
    let conn = state.lock().map_err(|e| e.to_string())?;
    let media_dirs = read_value(&conn, KEY_MEDIA_DIRS)
        .and_then(|raw| serde_json::from_str::<Vec<String>>(&raw).ok())
        .unwrap_or_default();
    Ok(Settings {
        tmdb_key: read_value(&conn, KEY_TMDB_KEY).unwrap_or_default(),
        media_dirs,
        pot_player_path: read_value(&conn, KEY_POTPLAYER).unwrap_or_default(),
        hosts: load_hosts(&conn),
        tmdb_language: load_tmdb_language(&conn),
        tvdb_key: load_tvdb_key(&conn),
        fanart_key: load_fanart_key(&conn),
        omdb_key: load_omdb_key(&conn),
        douban_enabled: load_douban_config(&conn).0,
        douban_cookie: load_douban_config(&conn).1,
        llm_enabled: load_llm_config(&conn).enabled,
        llm_base_url: read_value(&conn, KEY_LLM_BASE_URL).unwrap_or_default(),
        llm_api_key: read_value(&conn, KEY_LLM_API_KEY).unwrap_or_default(),
        llm_model: read_value(&conn, KEY_LLM_MODEL).unwrap_or_default(),
        llm_vision_model: read_value(&conn, KEY_LLM_VISION_MODEL).unwrap_or_default(),
        llm_vision_base_url: read_value(&conn, KEY_LLM_VISION_BASE_URL).unwrap_or_default(),
    })
}

/// 写入全部设置（upsert）。
#[tauri::command]
pub fn set_settings(
    state: State<'_, Mutex<Connection>>,
    settings: Settings,
) -> Result<(), String> {
    let conn = state.lock().map_err(|e| e.to_string())?;
    let dirs = serde_json::to_string(&settings.media_dirs).map_err(|e| e.to_string())?;
    let hosts = serde_json::to_string(&settings.hosts).map_err(|e| e.to_string())?;
    write_value(&conn, KEY_TMDB_KEY, &settings.tmdb_key).map_err(|e| e.to_string())?;
    write_value(&conn, KEY_MEDIA_DIRS, &dirs).map_err(|e| e.to_string())?;
    write_value(&conn, KEY_POTPLAYER, &settings.pot_player_path).map_err(|e| e.to_string())?;
    write_value(&conn, KEY_HOSTS, &hosts).map_err(|e| e.to_string())?;
    write_value(&conn, KEY_LANGUAGE, settings.tmdb_language.trim()).map_err(|e| e.to_string())?;
    write_value(&conn, KEY_TVDB_KEY, settings.tvdb_key.trim()).map_err(|e| e.to_string())?;
    write_value(&conn, KEY_FANART_KEY, settings.fanart_key.trim()).map_err(|e| e.to_string())?;
    write_value(&conn, KEY_OMDB_KEY, settings.omdb_key.trim()).map_err(|e| e.to_string())?;
    write_value(
        &conn,
        KEY_DOUBAN_ENABLED,
        if settings.douban_enabled { "1" } else { "0" },
    )
    .map_err(|e| e.to_string())?;
    write_value(&conn, KEY_DOUBAN_COOKIE, settings.douban_cookie.trim())
        .map_err(|e| e.to_string())?;
    write_value(
        &conn,
        KEY_LLM_ENABLED,
        if settings.llm_enabled { "1" } else { "0" },
    )
    .map_err(|e| e.to_string())?;
    write_value(&conn, KEY_LLM_BASE_URL, settings.llm_base_url.trim()).map_err(|e| e.to_string())?;
    write_value(&conn, KEY_LLM_API_KEY, settings.llm_api_key.trim()).map_err(|e| e.to_string())?;
    write_value(&conn, KEY_LLM_MODEL, settings.llm_model.trim()).map_err(|e| e.to_string())?;
    write_value(&conn, KEY_LLM_VISION_MODEL, settings.llm_vision_model.trim())
        .map_err(|e| e.to_string())?;
    write_value(&conn, KEY_LLM_VISION_BASE_URL, settings.llm_vision_base_url.trim())
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// 数据层自检：返回已建的表名。
#[tauri::command]
pub fn db_info(state: State<'_, Mutex<Connection>>) -> Result<DbInfo, String> {
    let conn = state.lock().map_err(|e| e.to_string())?;
    Ok(DbInfo {
        tables: db::tables(&conn)?,
    })
}

/// 在资源管理器中打开应用数据目录（数据库所在位置，便于排查问题）。
#[tauri::command]
pub fn open_data_dir(app: tauri::AppHandle) -> Result<String, String> {
    let dir = db::data_dir(&app)?;
    let path = dir.to_string_lossy().to_string();
    std::process::Command::new("explorer")
        .arg(&path)
        .spawn()
        .map_err(|e| format!("打开数据目录失败: {e}"))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn memory_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);",
        )
        .unwrap();
        conn
    }

    #[test]
    fn write_then_read_roundtrip() {
        let conn = memory_db();
        write_value(&conn, KEY_TMDB_KEY, "abc123").unwrap();
        assert_eq!(read_value(&conn, KEY_TMDB_KEY).as_deref(), Some("abc123"));
    }

    #[test]
    fn upsert_overwrites_same_key() {
        let conn = memory_db();
        write_value(&conn, KEY_POTPLAYER, "old").unwrap();
        write_value(&conn, KEY_POTPLAYER, "new").unwrap();
        assert_eq!(read_value(&conn, KEY_POTPLAYER).as_deref(), Some("new"));
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM settings", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1, "upsert 不应产生重复行");
    }

    #[test]
    fn missing_key_returns_none() {
        let conn = memory_db();
        assert!(read_value(&conn, KEY_MEDIA_DIRS).is_none());
    }

    /// 语言没配过（老库升级上来就是这种情况）必须回落默认，不能把空串发给 TMDB。
    #[test]
    fn language_falls_back_to_default_when_unset() {
        let conn = memory_db();
        assert_eq!(load_tmdb_language(&conn), crate::provider::tmdb::DEFAULT_LANGUAGE);
    }

    #[test]
    fn language_roundtrip_and_blank_is_default() {
        let conn = memory_db();
        write_value(&conn, KEY_LANGUAGE, "en-US").unwrap();
        assert_eq!(load_tmdb_language(&conn), "en-US");

        write_value(&conn, KEY_LANGUAGE, "  ").unwrap();
        assert_eq!(load_tmdb_language(&conn), crate::provider::tmdb::DEFAULT_LANGUAGE);
    }

    /// 新增供应方的 Key 也要能存能读（重启后不丢）
    #[test]
    fn provider_keys_roundtrip() {
        let conn = memory_db();
        for (k, v) in [
            (KEY_TVDB_KEY, "tvdb-1"),
            (KEY_FANART_KEY, "fanart-1"),
            (KEY_OMDB_KEY, "omdb-1"),
        ] {
            write_value(&conn, k, v).unwrap();
            assert_eq!(read_value(&conn, k).as_deref(), Some(v));
        }
        assert_eq!(load_tvdb_key(&conn), "tvdb-1");
        assert_eq!(load_fanart_key(&conn), "fanart-1");
        assert_eq!(load_omdb_key(&conn), "omdb-1");
    }

    /// 豆瓣默认必须是关闭的：老库升级上来不能因为多了个 provider 就悄悄开始联网。
    #[test]
    fn douban_defaults_off_and_roundtrips() {
        let conn = memory_db();
        let (enabled, cookie) = load_douban_config(&conn);
        assert!(!enabled, "豆瓣刮削默认必须关闭");
        assert!(cookie.is_empty());

        write_value(&conn, KEY_DOUBAN_ENABLED, "1").unwrap();
        write_value(&conn, KEY_DOUBAN_COOKIE, "bid=abc; dbcl2=xyz").unwrap();
        let (enabled, cookie) = load_douban_config(&conn);
        assert!(enabled);
        assert_eq!(cookie, "bid=abc; dbcl2=xyz");

        // `true` 这种写法也要认（前端可能按布尔序列化）
        write_value(&conn, KEY_DOUBAN_ENABLED, "true").unwrap();
        assert!(load_douban_config(&conn).0);

        write_value(&conn, KEY_DOUBAN_ENABLED, "0").unwrap();
        assert!(!load_douban_config(&conn).0);
    }

    #[test]
    fn llm_config_roundtrip_and_defaults_off() {        let conn = memory_db();
        assert!(!load_llm_config(&conn).enabled, "默认必须关闭");

        write_value(&conn, KEY_LLM_ENABLED, "1").unwrap();
        write_value(&conn, KEY_LLM_BASE_URL, "https://api.deepseek.com/v1").unwrap();
        write_value(&conn, KEY_LLM_MODEL, "deepseek-chat").unwrap();
        let c = load_llm_config(&conn);
        assert!(c.enabled);
        assert!(c.usable());
        assert_eq!(c.base_url(), "https://api.deepseek.com/v1");
        assert!(!c.vision_usable(), "没配视觉模型就不启用视觉兜底");
    }

    #[test]
    fn media_dirs_json_roundtrip() {
        let dirs = vec!["D:\\Movies".to_string(), "E:\\TV".to_string()];
        let raw = serde_json::to_string(&dirs).unwrap();
        let back: Vec<String> = serde_json::from_str(&raw).unwrap();
        assert_eq!(dirs, back);
    }
}
