//! 设置项持久化：基于 `settings` 表（key/value）。
//!
//! 前端契约（`src/stores/settingsStore.ts`）：`{ tmdbKey, mediaDirs, potPlayerPath }`。

use crate::core::matching::MatchOptions;
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
/// 扫描 / 导入完成后自动刮削（豆瓣兜底那条链）。
const KEY_AUTO_SCRAPE: &str = "autoScrapeAfterScan";
/// 被用户关掉的数据源（JSON 数组，存的是注册表里的 id）。
///
/// 与「Key 留空」不同：Key 留空是「没配置」，这里是**配置了但选择不用**。
/// 关掉一个源**不会**让后面的源提前（权威顺序不变，B5）。
const KEY_DISABLED_PROVIDERS: &str = "disabledProviders";
const KEY_LLM_ENABLED: &str = "llmEnabled";
const KEY_LLM_BASE_URL: &str = "llmBaseUrl";
const KEY_LLM_API_KEY: &str = "llmApiKey";
const KEY_LLM_MODEL: &str = "llmModel";
const KEY_LLM_VISION_MODEL: &str = "llmVisionModel";
const KEY_LLM_VISION_BASE_URL: &str = "llmVisionBaseUrl";

/// 手动匹配的筛选偏好。
const KEY_MATCH_FUZZY: &str = "matchFuzzy";
const KEY_MATCH_YEAR_TOLERANCE: &str = "matchYearTolerance";
const KEY_MATCH_BLACKLIST: &str = "matchBlacklist";
const KEY_MATCH_LIMIT: &str = "matchLimit";
/// 常用选择操作的快捷键（留空 = 不绑定）。
const KEY_SC_SELECT_ALL: &str = "shortcutSelectAll";
const KEY_SC_INVERT: &str = "shortcutInvert";
const KEY_SC_CONFIRM: &str = "shortcutConfirm";
const KEY_SC_CLEAR: &str = "shortcutClear";
/// 列表排序方式：`name` 名称（拼音）/ `year` 上映·首播年份 / `added` 添加时间。
///
/// 排序**计算在前端**（`src/utils/sort.ts`），后端只负责把用户的选择存下来。
/// KV 表新增 key 即生效，不需要迁移（约束 B6）。
const KEY_SORT_BY: &str = "sortBy";
/// 列表排序方向：`asc` 正序 / `desc` 倒序。
const KEY_SORT_DIR: &str = "sortDir";
/// 已被用户**手工排过序**的聚合（分组）id 列表（JSON 数组）。
///
/// 这是「这一组的次序是用户亲手定的吗」的**唯一**判据，与 `media.sort_order`
/// 的数值无关（理由见 `Settings::manual_order_groups` 的文档）。
/// KV 表新增 key 即生效，不需要迁移（约束 B6）。
const KEY_MANUAL_ORDER_GROUPS: &str = "manualOrderGroups";
/// 被用户**亲手折叠**过的分组 id 列表（封面墙的展开态记忆）。
///
/// 为什么要落库：封面墙挂在 `v-if/v-else` 链里（`App.vue`），点开详情页会把它
/// **卸载**，`expanded` / `collapsedByUser` 两个 ref 随之销毁；返回主页时组件重建，
/// 「新分组自动展开」那段 watch 又把一切摊开 —— 用户看到的正是「我上次明明收起来了，
/// 回来又全开了」。所以折叠态必须离开组件实例，存进 settings（KV 表，新增 key 免迁移，B6）。
///
/// ⚠️ 只记**折叠**的，不记展开的。展开的分组会随列表刷新不断新增，记「展开」等于
/// 存一份只会变长、且被删条目越长越脏的名单；记折叠则天然是少数派（多数分组默认开）。
/// 语义上这也是「用户明确否决了自动展开」，与组件里 `collapsedByUser` 同一个意思。
const KEY_COLLAPSED_GROUPS: &str = "collapsedGroups";

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
    /// 扫描 / 导入完成后，自动为**还没有海报**的条目跑一遍刮削。
    ///
    /// 默认关闭：自动往第三方站点发请求应该由用户显式同意。
    /// 开启后仍受 `doubanEnabled` 等各 provider 开关约束——没配 TMDb Key
    /// 也能跑，此时豆瓣会承担主要工作。
    #[serde(default)]
    pub auto_scrape_after_scan: bool,
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
    /// 被用户**关掉**的数据源 id 列表（如 `["douban"]`）。
    ///
    /// 空白名单 = 全部启用。名单里只认注册表认识的 id，写进别的值一律忽略。
    #[serde(default)]
    pub disabled_providers: Vec<String>,

    // ── 手动匹配的筛选偏好 ──
    /// 是否启用模糊匹配（关掉就只剩「完全一致」与「包含」）。
    #[serde(default = "default_true")]
    pub match_fuzzy: bool,
    /// 年份容差：`0` = 必须同一年，`1` = 前后一年也算命中。
    #[serde(default = "default_year_tolerance")]
    pub match_year_tolerance: i32,
    /// 排除词（逗号 / 换行分隔）：候选标题里出现任一个就整条剔除。
    #[serde(default)]
    pub match_blacklist: String,
    /// 一次最多拉多少条候选（TMDB 每页 20 条）。
    #[serde(default = "default_match_limit")]
    pub match_limit: i32,

    // ── 快捷键 ──
    /// 组合键写法：`Ctrl+A`、`Ctrl+Shift+A`、`Ctrl+Enter`、`Esc`。留空 = 不绑定。
    #[serde(default = "default_sc_select_all")]
    pub shortcut_select_all: String,
    #[serde(default = "default_sc_invert")]
    pub shortcut_invert: String,
    #[serde(default = "default_sc_confirm")]
    pub shortcut_confirm: String,
    #[serde(default = "default_sc_clear")]
    pub shortcut_clear: String,

    // ── 列表排序（展示层偏好，计算在前端）──
    /// 排序键：`name` / `year` / `added`。脏值一律回落 `name`。
    #[serde(default = "default_sort_by")]
    pub sort_by: String,
    /// 排序方向：`asc` / `desc`。脏值一律回落 `asc`。
    #[serde(default = "default_sort_dir")]
    pub sort_dir: String,

    /// 已被用户手工排过序的聚合（分组）id 列表。
    /// 只回答「这一组的次序是用户亲手定的吗」—— 与 media.sort_order 的数值无关：
    /// sort_order 会被任何一次 move_nodes_sync（含拖动聚合）整组重编号，
    /// 所以"有没有 >0 的值"不能证明用户排过（实测真实库里全是历史重编号的产物）。
    /// 列表里的分组 → 按库内 sort_order 显示；不在列表里 → 按顶栏排序偏好显示。
    #[serde(default)]
    pub manual_order_groups: Vec<i64>,

    /// 被用户亲手折叠过的分组 id 列表（封面墙的展开态记忆）。
    ///
    /// 封面墙被 `v-if/v-else` 卸载后重建时用它还原展开态，否则「新分组自动展开」
    /// 那段 watch 会把用户亲手收起来的分组又摊开。只记折叠、不记展开（理由见
    /// `KEY_COLLAPSED_GROUPS` 的文档）。
    #[serde(default)]
    pub collapsed_groups: Vec<i64>,
}

/// 排序键白名单（顺序即 UI 下拉的顺序）。
const SORT_BY_VALUES: [&str; 3] = ["name", "year", "added"];
/// 排序方向白名单。
const SORT_DIR_VALUES: [&str; 2] = ["asc", "desc"];

fn default_true() -> bool {
    true
}
fn default_year_tolerance() -> i32 {
    1
}
fn default_match_limit() -> i32 {
    60
}
fn default_sc_select_all() -> String {
    "Ctrl+A".to_string()
}
fn default_sc_invert() -> String {
    "Ctrl+Shift+A".to_string()
}
fn default_sc_confirm() -> String {
    "Ctrl+Enter".to_string()
}
fn default_sc_clear() -> String {
    "Esc".to_string()
}
fn default_sort_by() -> String {
    "name".to_string()
}
fn default_sort_dir() -> String {
    "asc".to_string()
}

/// 白名单归一化：不在名单内（空串 / 老库缺失 / 拼错）一律回落默认值。
///
/// 读与写**都过这一道**：落库前清洗，脏值就永远进不了 `settings` 表，
/// 不会出现「读出来是个不认识的值，前端排序直接乱掉」的情况。
fn normalize_sort(v: &str, allowed: &[&str], fallback: &str) -> String {
    let t = v.trim();
    if allowed.iter().any(|a| a.eq_ignore_ascii_case(t)) {
        t.to_ascii_lowercase()
    } else {
        fallback.to_string()
    }
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
            auto_scrape_after_scan: false,
            llm_enabled: false,
            llm_base_url: String::new(),
            llm_api_key: String::new(),
            llm_model: String::new(),
            llm_vision_model: String::new(),
            llm_vision_base_url: String::new(),
            disabled_providers: Vec::new(),
            match_fuzzy: true,
            match_year_tolerance: 1,
            match_blacklist: String::new(),
            match_limit: 60,
            shortcut_select_all: default_sc_select_all(),
            shortcut_invert: default_sc_invert(),
            shortcut_confirm: default_sc_confirm(),
            shortcut_clear: default_sc_clear(),
            sort_by: default_sort_by(),
            sort_dir: default_sort_dir(),
            manual_order_groups: Vec::new(),
        collapsed_groups: Vec::new(),
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
        .map(parse_bool)
        .unwrap_or(false);
    (enabled, read_value(conn, KEY_DOUBAN_COOKIE).unwrap_or_default())
}

/// 「扫描 / 导入后自动刮削」是否开启（默认关闭）。
pub fn load_auto_scrape(conn: &Connection) -> bool {
    read_value(conn, KEY_AUTO_SCRAPE).map(parse_bool).unwrap_or(false)
}

/// 读取「被关掉的数据源」列表。
///
/// 三道清洗：JSON 解析失败回空（不因脏数据崩）、去重、只保留注册表认识的 id
/// （防止设置里塞进 `llm` 这种不在注册表里的源）。
pub fn load_disabled_providers(conn: &Connection) -> Vec<String> {
    let raw = read_value(conn, KEY_DISABLED_PROVIDERS).unwrap_or_default();
    let mut out: Vec<String> = serde_json::from_str::<Vec<String>>(&raw)
        .unwrap_or_default()
        .into_iter()
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| crate::provider::registry::is_known(s))
        .collect();
    out.sort();
    out.dedup();
    out
}

/// 读取「手动匹配」的筛选偏好（供 `search_tmdb` 兜底）。
///
/// 界面上每次都会把当前值传进来；这里的作用是**老调用方 / 未传参时也有合理默认**，
/// 免得设置页改了却不生效。
pub fn load_match_options(conn: &Connection) -> MatchOptions {
    MatchOptions {
        fuzzy: read_value(conn, KEY_MATCH_FUZZY).map(parse_bool).unwrap_or(true),
        year_tolerance: read_value(conn, KEY_MATCH_YEAR_TOLERANCE)
            .and_then(|v| v.trim().parse::<i32>().ok())
            .unwrap_or(1)
            .clamp(0, 10),
        blacklist: crate::core::matching::split_blacklist(
            read_value(conn, KEY_MATCH_BLACKLIST).unwrap_or_default().as_str(),
        ),
        ..Default::default()
    }
}

/// 读取「一次最多拉多少条候选」。
pub fn load_match_limit(conn: &Connection) -> usize {
    read_value(conn, KEY_MATCH_LIMIT)
        .and_then(|v| v.trim().parse::<usize>().ok())
        .unwrap_or(crate::core::matching::DEFAULT_LIMIT)
}

/// 读取「已被用户手工排过序的聚合」id 列表。
///
/// 解析失败 / 脏值（非法 JSON、不是数组、数组里混了非整数）一律**回落空数组**、
/// 绝不报错：这是一份展示层的提示名单，读不出来最多是「按顶栏排序显示」，
/// 让整次 `get_settings` 失败会把用户所有的设置一起挡在界面外，代价完全不对等。
/// 同时顺手去掉非正数的 id —— 它们不可能是任何一条 media 行的 id。
pub fn load_manual_order_groups(conn: &Connection) -> Vec<i64> {
    read_value(conn, KEY_MANUAL_ORDER_GROUPS)
        .and_then(|raw| serde_json::from_str::<Vec<i64>>(&raw).ok())
        .unwrap_or_default()
        .into_iter()
        .filter(|id| *id > 0)
        .collect()
}

/// 读取「被用户亲手折叠过的分组」id 列表。
///
/// 与 `load_manual_order_groups` 同一套口径：解析失败 / 脏值一律**回落空数组**、
/// 绝不报错。读不出来最坏的结果是「全部分组按默认展开」，而让整次 `get_settings`
/// 失败会把用户所有设置一起挡在界面外 —— 代价完全不对等。顺手去掉非正数 id。
pub fn load_collapsed_groups(conn: &Connection) -> Vec<i64> {
    read_value(conn, KEY_COLLAPSED_GROUPS)
        .and_then(|raw| serde_json::from_str::<Vec<i64>>(&raw).ok())
        .unwrap_or_default()
        .into_iter()
        .filter(|id| *id > 0)
        .collect()
}

/// 布尔设置统一口径：`1` / `true`（不分大小写）为真，其余为假。
fn parse_bool(v: String) -> bool {
    v == "1" || v.eq_ignore_ascii_case("true")
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
        auto_scrape_after_scan: load_auto_scrape(&conn),
        llm_enabled: load_llm_config(&conn).enabled,
        llm_base_url: read_value(&conn, KEY_LLM_BASE_URL).unwrap_or_default(),
        llm_api_key: read_value(&conn, KEY_LLM_API_KEY).unwrap_or_default(),
        llm_model: read_value(&conn, KEY_LLM_MODEL).unwrap_or_default(),
        llm_vision_model: read_value(&conn, KEY_LLM_VISION_MODEL).unwrap_or_default(),
        llm_vision_base_url: read_value(&conn, KEY_LLM_VISION_BASE_URL).unwrap_or_default(),
        disabled_providers: load_disabled_providers(&conn),
        match_fuzzy: load_match_options(&conn).fuzzy,
        match_year_tolerance: load_match_options(&conn).year_tolerance,
        match_blacklist: read_value(&conn, KEY_MATCH_BLACKLIST).unwrap_or_default(),
        match_limit: load_match_limit(&conn) as i32,
        shortcut_select_all: read_value(&conn, KEY_SC_SELECT_ALL)
            .unwrap_or_else(default_sc_select_all),
        shortcut_invert: read_value(&conn, KEY_SC_INVERT).unwrap_or_else(default_sc_invert),
        shortcut_confirm: read_value(&conn, KEY_SC_CONFIRM).unwrap_or_else(default_sc_confirm),
        shortcut_clear: read_value(&conn, KEY_SC_CLEAR).unwrap_or_else(default_sc_clear),
        // 老库没有这两个键 → 空串 → 归一化回落 name / asc，行为与升级前一致
        sort_by: normalize_sort(
            &read_value(&conn, KEY_SORT_BY).unwrap_or_default(),
            &SORT_BY_VALUES,
            &default_sort_by(),
        ),
        sort_dir: normalize_sort(
            &read_value(&conn, KEY_SORT_DIR).unwrap_or_default(),
            &SORT_DIR_VALUES,
            &default_sort_dir(),
        ),
        manual_order_groups: load_manual_order_groups(&conn),
        collapsed_groups: load_collapsed_groups(&conn),
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
        KEY_AUTO_SCRAPE,
        if settings.auto_scrape_after_scan { "1" } else { "0" },
    )
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
    // 关掉的数据源：先过一遍注册表白名单，再落库（脏值不写进去）
    let disabled: Vec<String> = settings
        .disabled_providers
        .iter()
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| crate::provider::registry::is_known(s))
        .collect();
    let disabled_json = serde_json::to_string(&disabled).map_err(|e| e.to_string())?;
    write_value(&conn, KEY_DISABLED_PROVIDERS, &disabled_json).map_err(|e| e.to_string())?;

    // ── 手动匹配的筛选偏好 ──
    write_value(
        &conn,
        KEY_MATCH_FUZZY,
        if settings.match_fuzzy { "1" } else { "0" },
    )
    .map_err(|e| e.to_string())?;
    write_value(
        &conn,
        KEY_MATCH_YEAR_TOLERANCE,
        &settings.match_year_tolerance.clamp(0, 10).to_string(),
    )
    .map_err(|e| e.to_string())?;
    write_value(&conn, KEY_MATCH_BLACKLIST, settings.match_blacklist.trim())
        .map_err(|e| e.to_string())?;
    write_value(
        &conn,
        KEY_MATCH_LIMIT,
        &settings.match_limit.clamp(1, 200).to_string(),
    )
    .map_err(|e| e.to_string())?;

    // ── 快捷键（存的就是展示用的组合键字符串，留空 = 不绑定）──
    write_value(&conn, KEY_SC_SELECT_ALL, settings.shortcut_select_all.trim())
        .map_err(|e| e.to_string())?;
    write_value(&conn, KEY_SC_INVERT, settings.shortcut_invert.trim())
        .map_err(|e| e.to_string())?;
    write_value(&conn, KEY_SC_CONFIRM, settings.shortcut_confirm.trim())
        .map_err(|e| e.to_string())?;
    write_value(&conn, KEY_SC_CLEAR, settings.shortcut_clear.trim())
        .map_err(|e| e.to_string())?;

    // ── 列表排序（同样先归一化再落库，脏值不进表）──
    write_value(
        &conn,
        KEY_SORT_BY,
        &normalize_sort(&settings.sort_by, &SORT_BY_VALUES, &default_sort_by()),
    )
    .map_err(|e| e.to_string())?;
    write_value(
        &conn,
        KEY_SORT_DIR,
        &normalize_sort(&settings.sort_dir, &SORT_DIR_VALUES, &default_sort_dir()),
    )
    .map_err(|e| e.to_string())?;

    // ── 手工排序过的聚合 id（与 media_dirs 同一手法：JSON 字符串存进 KV）──
    // 写之前洗一遍：只留正数 id，脏值不进库（与 `load_manual_order_groups` 对称）
    let manual: Vec<i64> = settings
        .manual_order_groups
        .iter()
        .copied()
        .filter(|id| *id > 0)
        .collect();
    let manual_json = serde_json::to_string(&manual).map_err(|e| e.to_string())?;
    write_value(&conn, KEY_MANUAL_ORDER_GROUPS, &manual_json)
        .map_err(|e| e.to_string())?;

    // ── 被折叠的分组 id（同一手法；写前同样只留正数，与 load 侧对称）──
    let collapsed: Vec<i64> = settings
        .collapsed_groups
        .iter()
        .copied()
        .filter(|id| *id > 0)
        .collect();
    let collapsed_json = serde_json::to_string(&collapsed).map_err(|e| e.to_string())?;
    write_value(&conn, KEY_COLLAPSED_GROUPS, &collapsed_json)
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

    /* ---------------- 数据源开关（v6） ---------------- */

    /// 老库没有这个键 → 空名单 = 全部启用，行为与升级前完全一致。
    #[test]
    fn disabled_providers_defaults_to_empty() {
        let conn = memory_db();
        assert!(load_disabled_providers(&conn).is_empty());
    }

    /// 只认注册表认识的 id —— 塞进 `llm` 或瞎写的值必须被过滤掉，
    /// 否则前端一个错字就能让某个源永久消失。
    #[test]
    fn disabled_providers_drops_unknown_ids() {
        let conn = memory_db();
        write_value(&conn, KEY_DISABLED_PROVIDERS, r#"["douban","llm","whatever"]"#).unwrap();
        assert_eq!(load_disabled_providers(&conn), vec!["douban".to_string()]);
    }

    /// 脏 JSON 不能让读设置这件事崩掉（回空而不是报错）。
    #[test]
    fn disabled_providers_survives_broken_json() {
        let conn = memory_db();
        write_value(&conn, KEY_DISABLED_PROVIDERS, "{not json").unwrap();
        assert!(load_disabled_providers(&conn).is_empty());
    }

    /// 大小写与重复都要归一，否则同一份名单会因为写法不同而行为不一致。
    #[test]
    fn disabled_providers_is_case_insensitive_and_deduped() {
        let conn = memory_db();
        write_value(&conn, KEY_DISABLED_PROVIDERS, r#"["TMDB","tmdb","  omdb  "]"#).unwrap();
        assert_eq!(
            load_disabled_providers(&conn),
            vec!["omdb".to_string(), "tmdb".to_string()]
        );
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

    /* ---------------- 列表排序（展示层偏好） ---------------- */

    /// 老库没有这两个键 → 必须回落 `name` / `asc`，与升级前的行为一致。
    #[test]
    fn sort_settings_default_to_name_asc() {
        let conn = memory_db();
        let by = normalize_sort(
            &read_value(&conn, KEY_SORT_BY).unwrap_or_default(),
            &SORT_BY_VALUES,
            &default_sort_by(),
        );
        let dir = normalize_sort(
            &read_value(&conn, KEY_SORT_DIR).unwrap_or_default(),
            &SORT_DIR_VALUES,
            &default_sort_dir(),
        );
        assert_eq!(by, "name");
        assert_eq!(dir, "asc");
    }

    /// 脏值（拼错 / 空串 / 越权）一律回落默认，大小写不敏感地认合法值。
    #[test]
    fn sort_settings_normalizes_dirty_values() {
        assert_eq!(normalize_sort("foo", &SORT_BY_VALUES, &default_sort_by()), "name");
        assert_eq!(normalize_sort("", &SORT_BY_VALUES, &default_sort_by()), "name");
        assert_eq!(normalize_sort("   ", &SORT_BY_VALUES, &default_sort_by()), "name");
        assert_eq!(normalize_sort("YEAR", &SORT_BY_VALUES, &default_sort_by()), "year");
        assert_eq!(normalize_sort("added", &SORT_BY_VALUES, &default_sort_by()), "added");

        assert_eq!(normalize_sort("nope", &SORT_DIR_VALUES, &default_sort_dir()), "asc");
        assert_eq!(normalize_sort("DESC", &SORT_DIR_VALUES, &default_sort_dir()), "desc");
        assert_eq!(normalize_sort("", &SORT_DIR_VALUES, &default_sort_dir()), "asc");
    }

    /// 存得进也读得出；**脏值不会被写进库**（写之前先归一化）。
    #[test]
    fn sort_settings_roundtrip_and_dirty_never_persisted() {
        let conn = memory_db();
        write_value(
            &conn,
            KEY_SORT_BY,
            &normalize_sort("year", &SORT_BY_VALUES, &default_sort_by()),
        )
        .unwrap();
        write_value(
            &conn,
            KEY_SORT_DIR,
            &normalize_sort("desc", &SORT_DIR_VALUES, &default_sort_dir()),
        )
        .unwrap();
        assert_eq!(read_value(&conn, KEY_SORT_BY).as_deref(), Some("year"));
        assert_eq!(read_value(&conn, KEY_SORT_DIR).as_deref(), Some("desc"));

        write_value(
            &conn,
            KEY_SORT_BY,
            &normalize_sort("whatever", &SORT_BY_VALUES, &default_sort_by()),
        )
        .unwrap();
        assert_eq!(
            read_value(&conn, KEY_SORT_BY).as_deref(),
            Some("name"),
            "脏值绝不能落库"
        );
    }

    /// 结构体默认值与「老库 JSON 里没有这两个字段」都要是 name / asc ——
    /// `#[serde(default)]` 的意义就在这里：新增字段不需要迁移。
    #[test]
    fn settings_default_and_partial_json_use_name_asc() {
        let d = Settings::default();
        assert_eq!(d.sort_by, "name");
        assert_eq!(d.sort_dir, "asc");

        let parsed: Settings = serde_json::from_str(r#"{"tmdbKey":"k"}"#).unwrap();
        assert_eq!(parsed.sort_by, "name");
        assert_eq!(parsed.sort_dir, "asc");
    }

    /* ---------------- 手工排序过的聚合（与 sort_order 数值无关） ---------------- */

    /// 老库没有这个键 → 空名单 = 「没有任何一组被手工排过」→ 全部按顶栏排序显示。
    #[test]
    fn manual_order_groups_defaults_to_empty() {
        let conn = memory_db();
        assert!(load_manual_order_groups(&conn).is_empty());
    }

    /// 存得进也读得出：写进 KV 的 JSON 字符串必须原样变回 id 列表。
    #[test]
    fn manual_order_groups_roundtrip() {
        let conn = memory_db();
        let ids = vec![14135_i64, 14150, 7];
        let raw = serde_json::to_string(&ids).unwrap();
        write_value(&conn, KEY_MANUAL_ORDER_GROUPS, &raw).unwrap();
        assert_eq!(load_manual_order_groups(&conn), ids);
    }

    /// 脏值（非法 JSON / 不是数组 / 元素不是整数）一律回落空数组，绝不 panic、
    /// 也绝不把整次 get_settings 拖失败 —— 它只是展示层的提示名单。
    #[test]
    fn manual_order_groups_survives_dirty_values() {
        let conn = memory_db();
        for dirty in [
            "{not json",
            "null",
            "\"14135\"",
            "{\"a\":1}",
            r#"[14135,"x"]"#,
            "",
        ] {
            write_value(&conn, KEY_MANUAL_ORDER_GROUPS, dirty).unwrap();
            assert!(
                load_manual_order_groups(&conn).is_empty(),
                "脏值 {dirty:?} 必须回落空数组"
            );
        }
    }

    /// 非正数的 id 不可能是任何一条 media 行，读的时候顺手滤掉；
    /// 写的时候也一样（`set_settings` 里那道清洗与这里对称）。
    #[test]
    fn manual_order_groups_drops_non_positive_ids() {
        let conn = memory_db();
        write_value(&conn, KEY_MANUAL_ORDER_GROUPS, "[0,-3,14135]").unwrap();
        assert_eq!(load_manual_order_groups(&conn), vec![14135]);
    }

    /// `#[serde(default)]` 的意义：老库 JSON / 老前端 payload 里没有这个字段时，
    /// 反序列化出来的必须是空数组而不是报错。
    #[test]
    fn settings_default_and_partial_json_have_empty_manual_order() {
        assert!(Settings::default().manual_order_groups.is_empty());

        let parsed: Settings = serde_json::from_str(r#"{"tmdbKey":"k"}"#).unwrap();
        assert!(parsed.manual_order_groups.is_empty());

        let parsed: Settings =
            serde_json::from_str(r#"{"manualOrderGroups":[14135,14150]}"#).unwrap();
        assert_eq!(parsed.manual_order_groups, vec![14135, 14150]);
    }

    /* ---------------- 被折叠的分组（封面墙展开态记忆） ---------------- */

    /// 老库没有这个键 → 空名单 = 「没有任何一组被用户折叠过」→ 全部按默认展开。
    #[test]
    fn collapsed_groups_defaults_to_empty() {
        let conn = memory_db();
        assert!(load_collapsed_groups(&conn).is_empty());
    }

    /// 存得进也读得出：这是「回到主页还是我上次那个样子」的**唯一**依据，
    /// roundtrip 失败就会退化成「每次都全展开」，正是本次要修的缺陷。
    #[test]
    fn collapsed_groups_roundtrip() {
        let conn = memory_db();
        let ids = vec![14135_i64, 14150];
        let raw = serde_json::to_string(&ids).unwrap();
        write_value(&conn, KEY_COLLAPSED_GROUPS, &raw).unwrap();
        assert_eq!(load_collapsed_groups(&conn), ids);
    }

    /// 脏值一律回落空数组（与 manual_order_groups 同口径）：读不出来最坏是
    /// 「展开态没记住」，绝不能让整次 get_settings 失败、把用户设置全挡在界面外。
    #[test]
    fn collapsed_groups_survives_dirty_values() {
        let conn = memory_db();
        for dirty in ["{not json", "null", "\"14135\"", "{\"a\":1}", r#"[14135,"x"]"#, ""] {
            write_value(&conn, KEY_COLLAPSED_GROUPS, dirty).unwrap();
            assert!(
                load_collapsed_groups(&conn).is_empty(),
                "脏值 {dirty:?} 必须回落空数组"
            );
        }
    }

    /// 非正数 id 不可能是任何一条 media 行，读时滤掉（写侧在 set_settings 里对称清洗）。
    #[test]
    fn collapsed_groups_drops_non_positive_ids() {
        let conn = memory_db();
        write_value(&conn, KEY_COLLAPSED_GROUPS, "[0,-3,14135]").unwrap();
        assert_eq!(load_collapsed_groups(&conn), vec![14135]);
    }

    /// `#[serde(default)]`：老库 JSON / 老前端 payload 里没有这个字段时必须反序列化成
    /// 空数组而不是报错（前端老版本 payload 少这个键也不能把保存整条打断）。
    #[test]
    fn settings_default_and_partial_json_have_empty_collapsed() {
        assert!(Settings::default().collapsed_groups.is_empty());

        let parsed: Settings = serde_json::from_str(r#"{"tmdbKey":"k"}"#).unwrap();
        assert!(parsed.collapsed_groups.is_empty());

        let parsed: Settings =
            serde_json::from_str(r#"{"collapsedGroups":[14135,14150]}"#).unwrap();
        assert_eq!(parsed.collapsed_groups, vec![14135, 14150]);
    }

    #[test]
    fn media_dirs_json_roundtrip() {
        let dirs = vec!["D:\\Movies".to_string(), "E:\\TV".to_string()];
        let raw = serde_json::to_string(&dirs).unwrap();
        let back: Vec<String> = serde_json::from_str(&raw).unwrap();
        assert_eq!(dirs, back);
    }
}
