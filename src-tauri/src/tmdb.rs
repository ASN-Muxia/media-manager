//! TMDB v3 客户端（基于 ureq，同步阻塞，在命令线程中调用）。
//!
//! 只依赖公开的三个端点：
//! - `GET /search/movie`、`GET /search/tv`
//! - `GET /movie/{id}`、`GET /tv/{id}`

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::Duration;

const API_BASE: &str = "https://api.themoviedb.org/3";

/// 把字符串做百分号编码（避免引入额外依赖）。
pub fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// 从 `YYYY-MM-DD` 里取年份。
pub fn year_from_date(date: Option<&str>) -> Option<i32> {
    let d = date?.trim();
    if d.len() < 4 {
        return None;
    }
    d.get(0..4)?.parse::<i32>().ok()
}

/// 搜索结果项。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TmdbSearchItem {
    pub id: i64,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub overview: Option<String>,
    #[serde(default)]
    pub poster_path: Option<String>,
    #[serde(default)]
    pub backdrop_path: Option<String>,
    #[serde(default)]
    pub vote_average: Option<f64>,
    #[serde(default)]
    pub release_date: Option<String>,
    #[serde(default)]
    pub first_air_date: Option<String>,
}

impl TmdbSearchItem {
    /// 电影用 `title`、剧集用 `name`，取非空的那个。
    /// 前端也有同名逻辑（`tmdbTitle`），此处保留作为 Rust 侧 API 的一部分。
    #[allow(dead_code)]
    pub fn display_title(&self) -> String {
        self.title
            .clone()
            .or_else(|| self.name.clone())
            .unwrap_or_default()
    }

    pub fn date(&self) -> Option<String> {
        self.release_date
            .clone()
            .or_else(|| self.first_air_date.clone())
    }
}

/// 归一化后的元数据。
#[derive(Debug, Clone, Serialize, Default, PartialEq)]
pub struct TmdbMeta {
    pub tmdb_id: i64,
    pub title: String,
    pub year: Option<i32>,
    pub overview: Option<String>,
    pub poster_path: Option<String>,
    pub backdrop_path: Option<String>,
    pub vote_average: Option<f64>,
    pub genre: Option<String>,
}

impl TmdbMeta {
    /// 从 TMDB 详情响应构造（纯函数，可离线测试）。
    pub fn from_value(id: i64, kind: &str, v: &Value) -> TmdbMeta {
        let s = |k: &str| {
            v.get(k)
                .and_then(|x| x.as_str())
                .map(|x| x.to_string())
                .filter(|x| !x.is_empty())
        };
        let is_tv = kind == "tv";
        let title = if is_tv {
            s("name").or_else(|| s("original_name"))
        } else {
            s("title").or_else(|| s("original_title"))
        }
        .unwrap_or_default();
        let date = if is_tv {
            s("first_air_date")
        } else {
            s("release_date")
        };
        let genre = v
            .get("genres")
            .and_then(|g| g.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|g| g.get("name").and_then(|n| n.as_str()))
                    .collect::<Vec<_>>()
                    .join(" / ")
            })
            .filter(|g| !g.is_empty());

        TmdbMeta {
            tmdb_id: id,
            title,
            year: year_from_date(date.as_deref()),
            overview: s("overview"),
            poster_path: s("poster_path"),
            backdrop_path: s("backdrop_path"),
            vote_average: v.get("vote_average").and_then(|x| x.as_f64()),
            genre,
        }
    }
}

#[derive(Debug, Deserialize)]
struct SearchResponse {
    #[serde(default)]
    results: Vec<TmdbSearchItem>,
}

/// 某一季里的单集条目。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TmdbEpisodeItem {
    #[serde(default)]
    pub id: i64,
    #[serde(default)]
    pub episode_number: i64,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub overview: Option<String>,
    #[serde(default)]
    pub still_path: Option<String>,
    #[serde(default)]
    pub air_date: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SeasonResponse {
    #[serde(default)]
    episodes: Vec<TmdbEpisodeItem>,
}

/// TMDB 客户端。
pub struct TmdbClient {
    agent: ureq::Agent,
    api_key: String,
}

impl TmdbClient {
    pub fn new(api_key: impl Into<String>) -> Self {
        let agent = ureq::AgentBuilder::new()
            .timeout(Duration::from_secs(20))
            .user_agent("media-manager/0.1 (+tauri)")
            .build();
        Self {
            agent,
            api_key: api_key.into(),
        }
    }

    fn get_json<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, String)],
    ) -> Result<T, String> {
        let mut url = format!("{API_BASE}{path}?api_key={}", urlencode(&self.api_key));
        for (k, v) in query {
            url.push('&');
            url.push_str(k);
            url.push('=');
            url.push_str(&urlencode(v));
        }
        let resp = self.agent.get(&url).call().map_err(|e| match e {
            ureq::Error::Status(401, _) => {
                "TMDB 鉴权失败（401）：请检查设置里的 API Key".to_string()
            }
            ureq::Error::Status(404, _) => "TMDB 未找到对应资源（404）".to_string(),
            ureq::Error::Status(code, _) => format!("TMDB 请求失败（HTTP {code}）"),
            other => format!("TMDB 网络请求失败：{other}"),
        })?;
        resp.into_json::<T>()
            .map_err(|e| format!("TMDB 响应解析失败：{e}"))
    }

    /// 搜索电影 / 剧集。`kind` 为 `movie` 或 `tv`。
    pub fn search(
        &self,
        kind: &str,
        query: &str,
        year: Option<i32>,
    ) -> Result<Vec<TmdbSearchItem>, String> {
        let (path, year_key) = if kind == "tv" {
            ("/search/tv", "first_air_date_year")
        } else {
            ("/search/movie", "year")
        };
        let mut q: Vec<(&str, String)> = vec![
            ("query", query.to_string()),
            ("language", "zh-CN".to_string()),
            ("include_adult", "false".to_string()),
        ];
        if let Some(y) = year {
            q.push((year_key, y.to_string()));
        }
        let resp: SearchResponse = self.get_json(path, &q)?;
        Ok(resp.results)
    }

    /// 取详情并归一化。
    pub fn detail(&self, kind: &str, id: i64) -> Result<TmdbMeta, String> {
        let path = if kind == "tv" {
            format!("/tv/{id}")
        } else {
            format!("/movie/{id}")
        };
        let raw: Value = self.get_json(&path, &[("language", "zh-CN".to_string())])?;
        Ok(TmdbMeta::from_value(id, kind, &raw))
    }

    /// 取某一季的单集列表（单集级刮削用）。
    pub fn season_episodes(&self, tv_id: i64, season: i64) -> Result<Vec<TmdbEpisodeItem>, String> {
        let path = format!("/tv/{tv_id}/season/{season}");
        let resp: SeasonResponse = self.get_json(&path, &[("language", "zh-CN".to_string())])?;
        Ok(resp.episodes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urlencode_escapes_and_keeps_unreserved() {
        assert_eq!(urlencode("abcXYZ019-_.~"), "abcXYZ019-_.~");
        assert_eq!(urlencode("a b"), "a%20b");
        assert_eq!(urlencode("卧虎藏龙"), "%E5%8D%A7%E8%99%8E%E8%97%8F%E9%BE%99");
        assert_eq!(urlencode("a&b=c"), "a%26b%3Dc");
    }

    #[test]
    fn year_from_date_handles_shapes() {
        assert_eq!(year_from_date(Some("2010-07-16")), Some(2010));
        assert_eq!(year_from_date(Some("1999")), Some(1999));
        assert_eq!(year_from_date(Some("")), None);
        assert_eq!(year_from_date(Some("abc")), None);
        assert_eq!(year_from_date(None), None);
    }

    #[test]
    fn meta_from_movie_payload() {
        let raw: Value = serde_json::from_str(
            r#"{
              "id": 27205,
              "title": "盗梦空间",
              "original_title": "Inception",
              "release_date": "2010-07-16",
              "overview": "简介",
              "poster_path": "/poster.jpg",
              "backdrop_path": "/back.jpg",
              "vote_average": 8.369,
              "genres": [{"id": 28, "name": "动作"}, {"id": 878, "name": "科幻"}]
            }"#,
        )
        .unwrap();
        let meta = TmdbMeta::from_value(27205, "movie", &raw);
        assert_eq!(meta.tmdb_id, 27205);
        assert_eq!(meta.title, "盗梦空间");
        assert_eq!(meta.year, Some(2010));
        assert_eq!(meta.poster_path.as_deref(), Some("/poster.jpg"));
        assert_eq!(meta.vote_average, Some(8.369));
        assert_eq!(meta.genre.as_deref(), Some("动作 / 科幻"));
    }

    #[test]
    fn meta_from_tv_payload_uses_name_and_first_air_date() {
        let raw: Value = serde_json::from_str(
            r#"{
              "id": 1396,
              "name": "绝命毒师",
              "first_air_date": "2008-01-20",
              "genres": [{"id": 18, "name": "剧情"}]
            }"#,
        )
        .unwrap();
        let meta = TmdbMeta::from_value(1396, "tv", &raw);
        assert_eq!(meta.title, "绝命毒师");
        assert_eq!(meta.year, Some(2008));
        assert_eq!(meta.genre.as_deref(), Some("剧情"));
        assert_eq!(meta.poster_path, None);
        assert_eq!(meta.vote_average, None);
    }

    #[test]
    fn meta_tolerates_null_and_missing_fields() {
        let raw: Value = serde_json::from_str(
            r#"{"id": 1, "title": "X", "poster_path": null, "overview": "", "vote_average": 0.0}"#,
        )
        .unwrap();
        let meta = TmdbMeta::from_value(1, "movie", &raw);
        assert_eq!(meta.title, "X");
        assert_eq!(meta.poster_path, None, "空串/null 应归一为 None");
        assert_eq!(meta.overview, None);
        assert_eq!(meta.year, None);
        assert_eq!(meta.genre, None);
    }

    #[test]
    fn search_item_display_title_prefers_title_then_name() {
        let movie: TmdbSearchItem =
            serde_json::from_str(r#"{"id":1,"title":"A"}"#).unwrap();
        assert_eq!(movie.display_title(), "A");
        assert_eq!(movie.date(), None);

        let tv: TmdbSearchItem =
            serde_json::from_str(r#"{"id":2,"name":"B","first_air_date":"2001-01-01"}"#).unwrap();
        assert_eq!(tv.display_title(), "B");
        assert_eq!(tv.date().as_deref(), Some("2001-01-01"));
    }

    #[test]
    fn search_response_parses_results_array() {
        let resp: SearchResponse = serde_json::from_str(
            r#"{"page":1,"results":[{"id":1,"title":"A"},{"id":2,"name":"B"}],"total_results":2}"#,
        )
        .unwrap();
        assert_eq!(resp.results.len(), 2);
        assert_eq!(resp.results[0].id, 1);
    }

    #[test]
    fn season_response_parses_episodes() {
        let resp: SeasonResponse = serde_json::from_str(
            r#"{"season_number":1,"episodes":[
                 {"id":11,"episode_number":1,"name":"试播集","overview":"简介",
                  "still_path":"/s1.jpg","air_date":"2008-01-20"},
                 {"id":12,"episode_number":2}
               ]}"#,
        )
        .unwrap();
        assert_eq!(resp.episodes.len(), 2);
        assert_eq!(resp.episodes[0].episode_number, 1);
        assert_eq!(resp.episodes[0].name.as_deref(), Some("试播集"));
        assert_eq!(resp.episodes[0].still_path.as_deref(), Some("/s1.jpg"));
        assert_eq!(resp.episodes[1].name, None, "缺字段应容错为 None");
    }

    #[test]
    fn season_response_tolerates_missing_episodes_key() {
        let resp: SeasonResponse = serde_json::from_str(r#"{"season_number":1}"#).unwrap();
        assert!(resp.episodes.is_empty());
    }
}
