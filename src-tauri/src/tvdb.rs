//! TheTVDB v4 客户端：剧集的季 / 集结构。
//!
//! 定位：TMDb 负责「这是什么片子」（标题、简介、评分、图），
//! TheTVDB 负责「季和集到底怎么排」——它的季/集结构对动画、老剧、
//! 以及「绝对集数」命名方式更友好。两者互补，不是替代关系。
//!
//! 用到的端点：
//! - `POST /login`                        换 token（带缓存，不重复登录）
//! - `GET  /search?query=`                按名字找剧集
//! - `GET  /series/{id}/extended`          剧集详情（含类型）
//! - `GET  /series/{id}/episodes/default`  全季集列表

use crate::http;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Mutex;

const API_BASE: &str = "https://api4.thetvdb.com/v4";

/// 剧集条目。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct TvdbSeries {
    #[serde(default)]
    pub id: i64,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub year: Option<String>,
    #[serde(default)]
    pub overview: Option<String>,
    #[serde(default)]
    pub image: Option<String>,
    /// 官方季 / 集顺序（`default` / `absolute` / `dvd`）
    #[serde(default)]
    pub default_season_type: Option<i64>,
}

impl TvdbSeries {
    /// 类型名列表（响应里是对象数组或字符串数组，两种都要吃下）。
    pub fn genres_from(v: &Value) -> Option<String> {
        let arr = v.get("genres").and_then(|g| g.as_array())?;
        let names: Vec<String> = arr
            .iter()
            .filter_map(|g| {
                g.get("name")
                    .and_then(|n| n.as_str())
                    .or_else(|| g.as_str())
                    .map(|s| s.to_string())
            })
            .filter(|s| !s.is_empty())
            .collect();
        if names.is_empty() {
            None
        } else {
            Some(names.join(" / "))
        }
    }

    pub fn year_i32(&self) -> Option<i32> {
        self.year
            .as_deref()
            .and_then(|y| y.get(0..4))
            .and_then(|y| y.parse::<i32>().ok())
    }
}

/// 单集条目。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct TvdbEpisode {
    #[serde(default)]
    pub id: i64,
    #[serde(default)]
    pub season_number: Option<i64>,
    #[serde(default)]
    pub number: Option<i64>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub overview: Option<String>,
    #[serde(default)]
    pub image: Option<String>,
    #[serde(default)]
    pub aired: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
struct LoginResponse {
    #[serde(default)]
    data: LoginData,
}

#[derive(Debug, Deserialize, Default)]
struct LoginData {
    #[serde(default)]
    token: String,
}

#[derive(Debug, Deserialize, Default)]
struct Envelope<T> {
    #[serde(default)]
    data: Option<T>,
}

#[derive(Debug, Deserialize, Default)]
struct SearchData {
    #[serde(default)]
    series: Vec<TvdbSeries>,
}

#[derive(Debug, Deserialize, Default)]
struct EpisodesData {
    #[serde(default)]
    episodes: Vec<TvdbEpisode>,
}

/// TheTVDB 客户端。
pub struct TvdbClient {
    agent: ureq::Agent,
    api_key: String,
    /// 登录换来的 token（懒加载 + 缓存，避免每集都登录一次）。
    token: Mutex<Option<String>>,
}

impl TvdbClient {
    pub fn with_hosts(api_key: impl Into<String>, hosts: &[String]) -> Self {
        Self {
            agent: http::build_agent(hosts, 20),
            api_key: api_key.into(),
            token: Mutex::new(None),
        }
    }

    pub fn enabled(&self) -> bool {
        !self.api_key.trim().is_empty()
    }

    /// 取 token：已有就直接用，没有才登录。
    fn token(&self) -> Result<String, String> {
        if let Some(t) = self.token.lock().ok().and_then(|g| g.clone()) {
            if !t.is_empty() {
                return Ok(t);
            }
        }
        let resp: LoginResponse = http::post_json(
            &self.agent,
            "TheTVDB",
            &format!("{API_BASE}/login"),
            &[("Content-Type", "application/json")],
            &serde_json::json!({ "apikey": self.api_key, "pin": "" }),
        )?;
        if resp.data.token.is_empty() {
            return Err("TheTVDB 登录未返回 token".to_string());
        }
        if let Ok(mut g) = self.token.lock() {
            *g = Some(resp.data.token.clone());
        }
        Ok(resp.data.token)
    }

    fn auth_get<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<T, String> {
        let token = self.token()?;
        http::get_json::<T>(
            &self.agent,
            "TheTVDB",
            &format!("{API_BASE}{path}"),
            &[("Authorization", &format!("Bearer {token}"))],
        )
    }

    /// 按名字找剧集（返回第一页）。
    pub fn search(&self, name: &str) -> Result<Vec<TvdbSeries>, String> {
        let resp: Envelope<SearchData> = self.auth_get(&format!(
            "/search?query={}&type=series",
            crate::tmdb::urlencode(name)
        ))?;
        Ok(resp.data.map(|d| d.series).unwrap_or_default())
    }

    /// 剧集详情（含类型）。
    pub fn series(&self, id: i64) -> Result<(TvdbSeries, Option<String>), String> {
        let raw: Value = self.auth_get(&format!("/series/{id}/extended"))?;
        let series: TvdbSeries = serde_json::from_value(
            raw.get("data").cloned().unwrap_or(Value::Null),
        )
        .unwrap_or_default();
        let genres = TvdbSeries::genres_from(&raw.get("data").cloned().unwrap_or(Value::Null));
        Ok((series, genres))
    }

    /// 全部单集（`default` 季序）。TMDb 没有集信息时用它兜底。
    pub fn episodes(&self, id: i64) -> Result<Vec<TvdbEpisode>, String> {
        let resp: Envelope<EpisodesData> =
            self.auth_get(&format!("/series/{id}/episodes/default"))?;
        Ok(resp.data.map(|d| d.episodes).unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_without_api_key() {
        let c = TvdbClient::with_hosts("", &[]);
        assert!(!c.enabled());
        let c = TvdbClient::with_hosts("abc", &[]);
        assert!(c.enabled());
    }

    #[test]
    fn series_payload_is_tolerated() {
        let v: Value = serde_json::json!({
            "data": {
                "id": 121361, "name": "绝命毒师", "year": "2008",
                "overview": "高中化学老师…", "image": "https://artworks.thetvdb.com/x.jpg",
                "genres": [{"id": 1, "name": "剧情"}, {"id": 2, "name": "惊悚"}]
            }
        });
        let s: TvdbSeries = serde_json::from_value(v["data"].clone()).unwrap();
        assert_eq!(s.id, 121361);
        assert_eq!(s.name.as_deref(), Some("绝命毒师"));
        assert_eq!(s.year_i32(), Some(2008));
        assert_eq!(
            TvdbSeries::genres_from(&v["data"]).as_deref(),
            Some("剧情 / 惊悚")
        );
    }

    #[test]
    fn genres_accept_plain_string_array() {
        let v: Value = serde_json::json!({"genres": ["动画", "奇幻"]});
        assert_eq!(TvdbSeries::genres_from(&v).as_deref(), Some("动画 / 奇幻"));
    }

    #[test]
    fn episodes_payload_is_tolerated() {
        let v: Value = serde_json::json!({
            "data": {"episodes": [
                {"id": 1, "seasonNumber": 1, "number": 1, "name": "试播集",
                 "image": "a.jpg", "aired": "2008-01-20"},
                {"id": 2, "seasonNumber": 1, "number": 2, "name": null}
            ]}
        });
        let d: EpisodesData = serde_json::from_value(v["data"].clone()).unwrap();
        assert_eq!(d.episodes.len(), 2);
        assert_eq!(d.episodes[0].number, Some(1));
        assert!(d.episodes[1].name.is_none());
    }

    /// 缺 data / data 为 null 都不能炸
    #[test]
    fn missing_data_is_tolerated() {
        let v: Value = serde_json::json!({});
        let d: Envelope<EpisodesData> = serde_json::from_value(v).unwrap();
        assert!(d.data.is_none());
    }
}
