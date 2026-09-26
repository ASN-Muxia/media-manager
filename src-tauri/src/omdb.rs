//! OMDb 客户端：评分补充。
//!
//! 定位：只做**评分补充**——TMDb 的 `vote_average` 是个 0~10 的内部加权分，
//! 而 OMDb 给的是 IMDb 评分，很多用户更认这个。
//! 仅当条目还没有评分时才写入，不覆盖 TMDb 的结果。

use crate::http;
use serde::Deserialize;

const API_BASE: &str = "https://www.omdbapi.com";

/// OMDb 的字段名是 PascalCase（`Title` / `Year` / `imdbRating`），逐个对齐。
#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct OmdbEntry {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub year: Option<String>,
    /// 可能是 `"8.5"`，也可能是 `"N/A"` —— 所以单独解析。
    #[serde(default)]
    #[serde(rename = "imdbRating")]
    pub imdb_rating: Option<String>,
    #[serde(default)]
    pub genre: Option<String>,
    #[serde(default)]
    pub plot: Option<String>,
}

impl OmdbEntry {
    /// 评分（0~10）；`N/A` 或缺失一律当「没有」。
    pub fn rating(&self) -> Option<f64> {
        let raw = self.imdb_rating.as_deref()?.trim();
        if raw.is_empty() || raw.eq_ignore_ascii_case("n/a") {
            return None;
        }
        let v = raw.parse::<f64>().ok()?;
        if (0.0..=10.0).contains(&v) {
            Some(v)
        } else {
            None
        }
    }
}

pub struct OmdbClient {
    agent: ureq::Agent,
    api_key: String,
}

impl OmdbClient {
    pub fn with_hosts(api_key: impl Into<String>, hosts: &[String]) -> Self {
        Self {
            agent: http::build_agent(hosts, 15),
            api_key: api_key.into(),
        }
    }

    pub fn enabled(&self) -> bool {
        !self.api_key.trim().is_empty()
    }

    /// 按标题（+ 可选年份）查评分。
    pub fn by_title(&self, title: &str, year: Option<i32>) -> Result<OmdbEntry, String> {
        let mut url = format!(
            "{API_BASE}/?apikey={}&t={}",
            crate::tmdb::urlencode(&self.api_key),
            crate::tmdb::urlencode(title)
        );
        if let Some(y) = year {
            url.push_str("&y=");
            url.push_str(&y.to_string());
        }
        http::get_json::<OmdbEntry>(&self.agent, "OMDb", &url, &[])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    #[test]
    fn disabled_without_api_key() {
        assert!(!OmdbClient::with_hosts("", &[]).enabled());
        assert!(OmdbClient::with_hosts("k", &[]).enabled());
    }

    #[test]
    fn rating_parses_numeric_and_rejects_na() {
        let e = OmdbEntry {
            imdb_rating: Some("8.5".to_string()),
            ..Default::default()
        };
        assert_eq!(e.rating(), Some(8.5));

        for bad in ["N/A", "n/a", "", "  ", "abc", "12.5"] {
            let e = OmdbEntry {
                imdb_rating: Some(bad.to_string()),
                ..Default::default()
            };
            assert_eq!(e.rating(), None, "{bad} 应被当成没有评分");
        }
        assert_eq!(OmdbEntry::default().rating(), None);
    }

    #[test]
    fn payload_is_tolerated() {
        let v: Value = serde_json::json!({"Title":"Dune","Year":"2021","imdbRating":"8.0"});
        let e: OmdbEntry = serde_json::from_value(v).unwrap();
        assert_eq!(e.title.as_deref(), Some("Dune"));
        assert_eq!(e.rating(), Some(8.0));
    }
}
