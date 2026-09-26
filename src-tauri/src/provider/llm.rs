//! 大模型「脏文件名解析」增强层（**可选**）。
//!
//! ## 它在整体里的位置
//!
//! ```text
//! 文件名
//!   │
//!   ├─ ① 正则解析器 `parser::parse_file_name`（默认，永远先跑）
//!   │
//!   ├─ ② 大模型文本解析（可选）：只在 ① 的结果明显不可用时调用
//!   │
//!   └─ ③ 视觉模型看截图（可选）：只在 ① ② 都判不出来、且**提供了画面**时才用
//! ```
//!
//! **它绝不会替代元数据 API**：片名/年份这些结构化字段只是用来
//! 去 TMDb / TheTVDB 检索的**入口**，真正的标题、简介、评分、海报
//! 一律以元数据 API 为准。关掉大模型，整个刮削链路照常工作——
//! 只是脏文件名（一堆乱码 / 无分隔符 / 纯编号）会难认一些。
//!
//! ## 接口开放
//! 走 OpenAI 兼容的 `/chat/completions`，所以 DeepSeek、通义千问、
//! 智谱、 moonshot、本地 Ollama / vLLM 都能直接填进去。

use crate::provider::http;
use crate::core::parser::{is_video_file, ParsedName};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// 推荐模型：DeepSeek `deepseek-chat`（性价比高、中文强、支持 JSON 输出）。
pub const RECOMMENDED_MODEL: &str = "deepseek-chat";
/// 推荐接口地址（OpenAI 兼容）。
pub const RECOMMENDED_BASE_URL: &str = "https://api.deepseek.com/v1";
/// 推荐视觉模型：通义千问 VL（国内可直连、OpenAI 兼容）。
pub const RECOMMENDED_VISION_MODEL: &str = "qwen-vl-max";
/// 视觉模型推荐接口地址（阿里云百炼的 OpenAI 兼容端点）。
pub const RECOMMENDED_VISION_BASE_URL: &str = "https://dashscope.aliyuncs.com/compatible-mode/v1";

/// 备选（都走 OpenAI 兼容协议，改地址和模型名即可切换）。
pub const ALTERNATIVE_MODELS: &[(&str, &str)] = &[
    ("OpenAI · gpt-4o-mini", "https://api.openai.com/v1"),
    ("月之暗面 · moonshot-v1-8k", "https://api.moonshot.cn/v1"),
    ("智谱 · glm-4-flash", "https://open.bigmodel.cn/api/paas/v4"),
    ("本地 Ollama", "http://localhost:11434/v1"),
];

/// 大模型配置（来自设置项）。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct LlmConfig {
    /// 总开关：关掉就完全不联网，行为与加这个模块之前一致。
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default)]
    pub model: String,
    /// 视觉模型（看截图用的）；留空表示不启用视觉兜底。
    #[serde(default)]
    pub vision_model: String,
    /// 视觉模型的接口地址（通常与文本模型同址，可单独覆盖）。
    #[serde(default)]
    pub vision_base_url: String,
}

impl LlmConfig {
    /// 真正可用：开了开关、填了地址与模型。
    ///
    /// 注意 Key 可以为空——本地 Ollama / vLLM 这类不需要鉴权。
    pub fn usable(&self) -> bool {
        self.enabled && !self.base_url.trim().is_empty() && !self.model().is_empty()
    }

    /// 模型名：留空回落到推荐模型，避免用户只填了地址却忘了填模型。
    pub fn model(&self) -> String {
        if self.model.trim().is_empty() {
            RECOMMENDED_MODEL.to_string()
        } else {
            self.model.trim().to_string()
        }
    }

    /// 接口地址：留空回落到推荐地址。
    pub fn base_url(&self) -> String {
        let u = self.base_url.trim().trim_end_matches('/').to_string();
        if u.is_empty() {
            RECOMMENDED_BASE_URL.to_string()
        } else {
            u
        }
    }

    /// 视觉能力可用：开了视觉模型，且地址能确定（单独配了就用单独的，否则跟文本同址）。
    pub fn vision_usable(&self) -> bool {
        self.enabled
            && !self.vision_model.trim().is_empty()
            && !self.vision_base_url().is_empty()
    }

    pub fn vision_model(&self) -> String {
        self.vision_model.trim().to_string()
    }

    pub fn vision_base_url(&self) -> String {
        let u = self.vision_base_url.trim().trim_end_matches('/').to_string();
        if u.is_empty() {
            self.base_url()
        } else {
            u
        }
    }
}

/// 大模型输出的结构化字段（对应需求里的片名/年份/季/集/分辨率/来源）。
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ParsedFields {
    pub title: String,
    pub year: Option<i32>,
    pub season: Option<i32>,
    pub episode: Option<i32>,
    pub resolution: Option<String>,
    pub release_source: Option<String>,
}

const SYSTEM_PROMPT: &str = "你是影视文件名解析助手。用户给一个视频文件名，\
请只输出 JSON，不要任何解释文字。字段：title（片名，字符串）、year（年份，整数或 null）、\
season（季号，整数或 null）、episode（集号，整数或 null）、resolution（分辨率，如 1080p，字符串或 null）、\
release_source（发布来源/压制组，字符串或 null）。无法确定的字段给 null。";

/// 从大模型返回的文本里抠出 JSON（容忍 ```json 代码块和前后废话）。
pub fn extract_json(text: &str) -> Option<Value> {
    let t = text.trim();
    let candidates = [
        t.to_string(),
        // 去掉 ```json ... ``` 围栏
        t.trim_start_matches("```json")
            .trim_start_matches("```")
            .trim_end_matches("```")
            .trim()
            .to_string(),
    ];
    for c in candidates {
        if let Ok(v) = serde_json::from_str::<Value>(&c) {
            return Some(v);
        }
        // 截取第一个 `{` 到最后一个 `}`
        if let (Some(a), Some(b)) = (c.find('{'), c.rfind('}')) {
            if let Ok(v) = serde_json::from_str::<Value>(&c[a..=b]) {
                return Some(v);
            }
        }
    }
    None
}

/// 把大模型的 JSON 收成结构化字段；字段类型不规整时逐个容错。
pub fn fields_from_value(v: &Value) -> ParsedFields {
    let s = |k: &str| {
        v.get(k)
            .and_then(|x| x.as_str())
            .map(|x| x.trim().to_string())
            .filter(|x| !x.is_empty() && !x.eq_ignore_ascii_case("null"))
    };
    let i = |k: &str| {
        v.get(k)
            .and_then(|x| x.as_i64())
            .or_else(|| {
                v.get(k)
                    .and_then(|x| x.as_str())
                    .and_then(|x| x.trim().parse::<i64>().ok())
            })
            .and_then(|n| i32::try_from(n).ok())
            .filter(|n| *n > 0)
    };
    ParsedFields {
        title: s("title").unwrap_or_default(),
        year: i("year").filter(|y| (1800..2100).contains(y)),
        season: i("season"),
        episode: i("episode"),
        resolution: s("resolution"),
        release_source: s("release_source")
            .or_else(|| s("source"))
            .or_else(|| s("releaseSource")),
    }
}

/// ① 的结果是否「明显不可用」——只有这时才值得花钱/花时间叫大模型。
///
/// 判据（宁可保守，正则能认出来就别打扰模型）：
/// - 标题为空（整串都是标签 / 编号）；
/// - 标题短到只有 1 个字符（多半是切错了）；
/// - 标题就是把整个文件名原样搬过来（等于没解析）。
pub fn needs_enhancement(parsed: &ParsedName, file_name: &str) -> bool {
    let title = parsed.title.trim();
    if title.is_empty() || title.chars().count() < 2 {
        return true;
    }
    let stem = file_name
        .rsplit_once('.')
        .map(|(s, _)| s)
        .unwrap_or(file_name)
        .trim();
    title == stem
}

/// 文件名是否「完全不可推断」——连大模型文本解析都救不回来，
/// 才轮到视觉模型看截图。
pub fn is_undecidable(file_name: &str) -> bool {
    let stem = file_name
        .rsplit_once('.')
        .map(|(s, _)| s)
        .unwrap_or(file_name);
    // 既没有字母、也没有 CJK，只剩数字和符号（`00123.mkv`、`[0724].mp4`）
    let meaningful = stem
        .chars()
        .any(|c| c.is_ascii_alphabetic() || ('\u{4e00}'..='\u{9fff}').contains(&c));
    !meaningful
}

pub struct LlmClient {
    agent: ureq::Agent,
    cfg: LlmConfig,
}

impl LlmClient {
    pub fn new(cfg: LlmConfig, hosts: &[String]) -> Self {
        Self {
            agent: http::build_agent(hosts, 60),
            cfg,
        }
    }

    fn chat(&self, base: &str, model: &str, content: Value) -> Result<String, String> {
        let url = format!("{base}/chat/completions");
        let auth = format!("Bearer {}", self.cfg.api_key.trim());
        let mut headers: Vec<(&str, &str)> = vec![("Content-Type", "application/json")];
        if !self.cfg.api_key.trim().is_empty() {
            headers.push(("Authorization", auth.as_str()));
        }
        let body = json!({
            "model": model,
            "temperature": 0,
            "messages": [
                {"role": "system", "content": SYSTEM_PROMPT},
                {"role": "user", "content": content}
            ],
        });
        let resp: Value = http::post_json(&self.agent, "大模型", &url, &headers, &body)?;
        resp.get("choices")
            .and_then(|c| c.get(0))
            .and_then(|c| c.get("message"))
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| "大模型响应里没有 content".to_string())
    }

    /// 文本解析：把文件名丢给模型，拿回结构化字段。
    pub fn parse_name(&self, file_name: &str) -> Result<ParsedFields, String> {
        if !self.cfg.usable() {
            return Err("未启用大模型解析".to_string());
        }
        let text = self.chat(
            &self.cfg.base_url(),
            &self.cfg.model(),
            json!(format!("文件名：{file_name}")),
        )?;
        let v = extract_json(&text).ok_or_else(|| "大模型没有返回可解析的 JSON".to_string())?;
        let f = fields_from_value(&v);
        if f.title.is_empty() {
            return Err("大模型没给出片名".to_string());
        }
        Ok(f)
    }

    /// 视觉兜底：只在文件名完全不可推断时才调用，`image_data_url` 是一帧画面
    /// （data URL 或可直接访问的图片地址）。
    pub fn parse_image(&self, file_name: &str, image_data_url: &str) -> Result<ParsedFields, String> {
        if !self.cfg.vision_usable() {
            return Err("未启用视觉模型".to_string());
        }
        let content = json!([
            {"type": "text", "text": format!("这个视频的画面。文件名是：{file_name}。\
             请结合画面与文件名判断它属于哪部影视作品，并给出集数信息。只输出 JSON。")},
            {"type": "image_url", "image_url": {"url": image_data_url}}
        ]);
        let text = self.chat(
            &self.cfg.vision_base_url(),
            &self.cfg.vision_model(),
            content,
        )?;
        let v = extract_json(&text).ok_or_else(|| "视觉模型没有返回可解析的 JSON".to_string())?;
        let f = fields_from_value(&v);
        if f.title.is_empty() {
            return Err("视觉模型没给出片名".to_string());
        }
        Ok(f)
    }
}

/// 把大模型的结果并回正则结果：只在原有字段缺失时补，绝不改已有的。
fn merge(base: &mut ParsedName, f: ParsedFields) {
    if base.title.trim().is_empty() && !f.title.is_empty() {
        base.title = f.title;
    }
    if base.year.is_none() {
        base.year = f.year;
    }
    if base.season.is_none() {
        base.season = f.season;
    }
    if base.episode.is_none() {
        base.episode = f.episode;
    }
}

/// 增强解析的对外入口：**任何情况下都不会让情况变糟**。
///
/// - 没启用 / 不需要增强 / 调用失败 / 返回不合理 → 原样返回正则结果；
/// - `resolution` 与 `release_source` 正则解析不产出，单独带出（见返回值第二个元素）。
pub fn enhance(
    base: ParsedName,
    file_name: &str,
    cfg: &LlmConfig,
    hosts: &[String],
) -> (ParsedName, Option<ParsedFields>) {
    if !is_video_file(file_name) {
        return (base, None);
    }
    if !cfg.usable() || !needs_enhancement(&base, file_name) {
        return (base, None);
    }
    let client = LlmClient::new(cfg.clone(), hosts);
    match client.parse_name(file_name) {
        Ok(f) => {
            let mut out = base;
            merge(&mut out, f.clone());
            (out, Some(f))
        }
        Err(_) => (base, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(enabled: bool) -> LlmConfig {
        LlmConfig {
            enabled,
            base_url: RECOMMENDED_BASE_URL.to_string(),
            api_key: "k".to_string(),
            model: RECOMMENDED_MODEL.to_string(),
            vision_model: String::new(),
            vision_base_url: String::new(),
        }
    }

    #[test]
    fn unusable_without_switch_or_model() {
        assert!(!LlmConfig::default().usable());
        assert!(!cfg(false).usable());
        assert!(cfg(true).usable());
        let no_model = LlmConfig {
            model: "  ".to_string(),
            ..cfg(true)
        };
        // 模型留空会回落到推荐模型，因此仍然可用（避免用户只填地址却忘了填模型）
        assert!(no_model.usable());
        assert_eq!(no_model.model(), RECOMMENDED_MODEL);
    }

    #[test]
    fn vision_requires_a_model_name() {
        let mut c = cfg(true);
        assert!(!c.vision_usable());
        c.vision_model = RECOMMENDED_VISION_MODEL.to_string();
        assert!(c.vision_usable());
        // 视觉地址没单独配 → 跟文本同址
        assert_eq!(c.vision_base_url(), RECOMMENDED_BASE_URL);
    }

    #[test]
    fn extract_json_handles_fences_and_prose() {
        let raw = "好的，结果如下：\n```json\n{\"title\":\"Dune\",\"year\":2021}\n```";
        let v = extract_json(raw).expect("应能抠出 JSON");
        assert_eq!(v["title"], "Dune");
        assert_eq!(extract_json("{\"title\":\"A\"}").unwrap()["title"], "A");
        assert!(extract_json("完全不是 JSON").is_none());
    }

    #[test]
    fn fields_are_coerced_from_loose_json() {
        let v = serde_json::json!({
            "title": "绝命毒师", "year": "2008", "season": 1, "episode": "2",
            "resolution": "1080p", "source": "WEB-DL"
        });
        let f = fields_from_value(&v);
        assert_eq!(f.title, "绝命毒师");
        assert_eq!(f.year, Some(2008));
        assert_eq!(f.season, Some(1));
        assert_eq!(f.episode, Some(2));
        assert_eq!(f.resolution.as_deref(), Some("1080p"));
        assert_eq!(f.release_source.as_deref(), Some("WEB-DL"));
    }

    #[test]
    fn junk_fields_are_dropped() {
        let v = serde_json::json!({"title": "null", "year": 0, "season": -1, "episode": 99999});
        let f = fields_from_value(&v);
        assert!(f.title.is_empty());
        assert_eq!(f.year, None);
        assert_eq!(f.season, None);
    }

    /// 正则已经解析得很好时，不该去打扰大模型
    #[test]
    fn good_regex_result_needs_no_enhancement() {
        let p = crate::core::parser::parse_file_name("Breaking.Bad.S01E01.1080p.mkv");
        assert!(!needs_enhancement(&p, "Breaking.Bad.S01E01.1080p.mkv"));
    }

    #[test]
    fn empty_or_whole_name_title_needs_enhancement() {
        let p = ParsedName::default();
        assert!(needs_enhancement(&p, "whatever.mkv"));
        let p2 = ParsedName {
            title: "1234567".to_string(),
            ..Default::default()
        };
        assert!(needs_enhancement(&p2, "1234567.mkv"), "标题等于整个文件名 = 没解析");
    }

    /// 只有数字和符号的文件名才算「完全不可推断」
    #[test]
    fn undecidable_only_when_no_letters_at_all() {
        assert!(is_undecidable("00123.mkv"));
        assert!(is_undecidable("[0724].mp4"));
        assert!(!is_undecidable("Breaking.Bad.S01E01.mkv"));
        assert!(!is_undecidable("剧名 第01话.mp4"));
    }

    /// 最关键的一条：调用失败时**必须**原样退回正则结果
    #[test]
    fn enhance_falls_back_to_regex_on_any_failure() {
        let p = ParsedName {
            title: "Fallback".to_string(),
            year: Some(2020),
            ..Default::default()
        };
        // 无法联网（地址指向不可解析的域名），但结果仍应是原样
        let cfg = LlmConfig {
            enabled: true,
            base_url: "https://127.0.0.1:1/v1".to_string(),
            api_key: "k".to_string(),
            model: "m".to_string(),
            vision_model: String::new(),
            vision_base_url: String::new(),
        };
        let (out, fields) = enhance(p.clone(), "whatever.mkv", &cfg, &[]);
        assert_eq!(out, p, "失败时必须原样退回");
        assert!(fields.is_none());
    }

    #[test]
    fn enhance_is_noop_when_disabled() {
        let p = ParsedName::default();
        let (out, fields) = enhance(p.clone(), "00123.mkv", &LlmConfig::default(), &[]);
        assert_eq!(out, p);
        assert!(fields.is_none());
    }

    #[test]
    fn merge_only_fills_gaps() {
        let mut base = ParsedName {
            title: "原名".to_string(),
            year: Some(2019),
            ..Default::default()
        };
        merge(
            &mut base,
            ParsedFields {
                title: "模型给的".to_string(),
                year: Some(2024),
                season: Some(2),
                episode: Some(3),
                ..Default::default()
            },
        );
        assert_eq!(base.title, "原名", "已有标题不应被覆盖");
        assert_eq!(base.year, Some(2019));
        assert_eq!(base.season, Some(2));
        assert_eq!(base.episode, Some(3));
    }
}
