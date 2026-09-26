//! Fanart.tv 客户端：海报与背景图的补充来源。
//!
//! 定位：TMDb 自己的图库偶尔缺图（尤其老剧、冷门地区版本），
//! Fanart.tv 作为**补充**图源，只在 TMDb 没给图时才用，不会覆盖已有图。
//!
//! 端点：
//! - `GET /v3/movies/{tmdb_id}`  电影
//! - `GET /v3/tv/{tvdb_id}`      剧集（注意用的是 TheTVDB 的 id）

use crate::provider::http;
use serde_json::Value;

const API_BASE: &str = "https://webservice.fanart.tv/v3";

/// 取到的一对图（海报 / 背景）。
///
/// 只保留地址：Fanart.tv 给的是**完整 URL**（不是 TMDb 那种相对路径），
/// 前端 `imageUrl()` 会识别 `http(s)://` 前缀并原样使用。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Art {
    pub poster: Option<String>,
    pub backdrop: Option<String>,
}

pub struct FanartClient {
    agent: ureq::Agent,
    api_key: String,
}

impl FanartClient {
    pub fn with_hosts(api_key: impl Into<String>, hosts: &[String]) -> Self {
        Self {
            agent: http::build_agent(hosts, 20),
            api_key: api_key.into(),
        }
    }

    pub fn enabled(&self) -> bool {
        !self.api_key.trim().is_empty()
    }

    fn get(&self, path: &str) -> Result<Value, String> {
        http::get_json::<Value>(
            &self.agent,
            "Fanart.tv",
            &format!("{API_BASE}{path}?api_key={}", crate::provider::tmdb::urlencode(&self.api_key)),
            &[],
        )
    }

    /// 电影图源。
    pub fn movie_art(&self, tmdb_id: i64) -> Result<Art, String> {
        let v = self.get(&format!("/movies/{tmdb_id}"))?;
        Ok(Art {
            poster: first_url(&v, &["movieposter", "movieposterhd"]),
            backdrop: first_url(&v, &["moviebackground", "moviebackgroundhd"]),
        })
    }

    /// 剧集图源（用 TheTVDB 的 id）。
    pub fn tv_art(&self, tvdb_id: i64) -> Result<Art, String> {
        let v = self.get(&format!("/tv/{tvdb_id}"))?;
        Ok(Art {
            poster: first_url(&v, &["tvposter", "tvposterhd"]),
            backdrop: first_url(&v, &["showbackground", "showbackgroundhd"]),
        })
    }
}

/// 按候选键顺序找第一张有 url 的图（点赞数高的排前面，取质量最好的那张）。
fn first_url(v: &Value, keys: &[&str]) -> Option<String> {
    for key in keys {
        let mut best: Option<(i64, String)> = None;
        if let Some(arr) = v.get(*key).and_then(|a| a.as_array()) {
            for item in arr {
                let url = item.get("url").and_then(|u| u.as_str());
                let likes = item
                    .get("likes")
                    .and_then(|l| l.as_str())
                    .and_then(|l| l.parse::<i64>().ok())
                    .unwrap_or(0);
                if let Some(u) = url {
                    let candidate = (likes, u.to_string());
                    if best.as_ref().map_or(true, |b| candidate.0 > b.0) {
                        best = Some(candidate);
                    }
                }
            }
        }
        if let Some((_, url)) = best {
            return Some(url);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_without_api_key() {
        assert!(!FanartClient::with_hosts("", &[]).enabled());
        assert!(FanartClient::with_hosts("k", &[]).enabled());
    }

    #[test]
    fn picks_most_liked_image() {
        let v: Value = serde_json::json!({
            "tvposter": [
                {"url": "https://a/1.jpg", "likes": "3"},
                {"url": "https://a/2.jpg", "likes": "9"},
                {"url": "https://a/3.jpg", "likes": "1"}
            ]
        });
        assert_eq!(first_url(&v, &["tvposter"]).as_deref(), Some("https://a/2.jpg"));
    }

    #[test]
    fn falls_back_to_secondary_key() {
        let v: Value = serde_json::json!({"showbackgroundhd": [{"url": "https://b/hd.jpg"}]});
        assert_eq!(
            first_url(&v, &["showbackground", "showbackgroundhd"]).as_deref(),
            Some("https://b/hd.jpg")
        );
    }

    #[test]
    fn missing_everything_is_none() {
        let v: Value = serde_json::json!({"error": "not found"});
        assert!(first_url(&v, &["tvposter", "tvposterhd"]).is_none());
        assert!(first_url(&v, &[]).is_none());
    }
}
