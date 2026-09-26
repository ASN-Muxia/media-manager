//! TMDB v3 客户端（基于 ureq，同步阻塞，在命令线程中调用）。
//!
//! 只依赖公开的三个端点：
//! - `GET /search/movie`、`GET /search/tv`
//! - `GET /movie/{id}`、`GET /tv/{id}`

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::io;
use std::net::{IpAddr, SocketAddr, ToSocketAddrs};

const API_BASE: &str = "https://api.themoviedb.org/3";

/// 元数据语言默认值（设置项为空时用它）。
pub const DEFAULT_LANGUAGE: &str = "zh-CN";

/// 默认端口（netloc 不带端口时用它）。本项目只走 HTTPS。
const DEFAULT_PORT: u16 = 443;

/// 拆分 `host:port` / `[::1]:443` / `host` 形式的 netloc。
fn split_netloc(netloc: &str) -> (String, u16) {
    if let Some(rest) = netloc.strip_prefix('[') {
        // IPv6 字面量：`[::1]:443`
        let (host, port_str) = match rest.split_once(']') {
            Some((h, tail)) => (h, tail.strip_prefix(':').unwrap_or("")),
            None => (rest, ""),
        };
        return (
            host.to_string(),
            port_str.parse::<u16>().unwrap_or(DEFAULT_PORT),
        );
    }
    match netloc.rsplit_once(':') {
        Some((h, p)) if p.chars().all(|c| c.is_ascii_digit()) => (
            h.to_string(),
            p.parse::<u16>().unwrap_or(DEFAULT_PORT),
        ),
        _ => (netloc.to_string(), DEFAULT_PORT),
    }
}

/// 解析一行 hosts 配置，返回若干 `(域名, IP)` 映射。
///
/// 兼容两种写法（顺序自动识别，与系统 hosts 文件一致时写「IP 域名」）：
/// - `13.224.161.90 api.themoviedb.org`
/// - `api.themoviedb.org 13.224.161.90`
/// - 一行可写多个域名：`1.2.3.4 a.com b.com`
/// `#` 之后视为注释；无法识别的行直接忽略。
pub fn parse_hosts_line(line: &str) -> Vec<(String, String)> {
    let body = line.split('#').next().unwrap_or("").trim();
    if body.is_empty() {
        return Vec::new();
    }
    let tokens: Vec<&str> = body.split_whitespace().collect();
    if tokens.len() < 2 {
        return Vec::new();
    }
    let first_is_ip = tokens[0].parse::<IpAddr>().is_ok();
    let last_is_ip = tokens[tokens.len() - 1].parse::<IpAddr>().is_ok();
    let (ip, hosts) = if first_is_ip {
        (tokens[0].to_string(), &tokens[1..])
    } else if last_is_ip {
        (
            tokens[tokens.len() - 1].to_string(),
            &tokens[..tokens.len() - 1],
        )
    } else {
        return Vec::new(); // 既没有可识别的 IP，忽略
    };
    hosts
        .iter()
        .map(|h| (normalize_host(h), ip.clone()))
        .collect()
}

/// 域名归一：去尾部点 + 转小写。
fn normalize_host(host: &str) -> String {
    host.trim()
        .trim_end_matches('.')
        .trim_start_matches("*.")
        .to_ascii_lowercase()
}

/// 自定义域名 → IP 映射（等价一个只作用于本进程的小型 hosts 文件）。
///
/// 之所以做成 `ureq::Resolver` 而不是「把 URL 里的域名换成 IP」：
/// 后者会破坏 HTTPS 的 SNI 与证书校验（证书签的是域名，不是 IP）。
/// 只改解析结果、保留原 URL，TLS 握手与 `Host` 头都不受影响。
#[derive(Debug, Default, Clone)]
pub struct HostsResolver {
    map: HashMap<String, IpAddr>,
}

impl HostsResolver {
    /// 按行构造；重复域名以**后出现的为准**（便于后面的配置覆盖前面的）。
    pub fn from_lines(lines: &[String]) -> Self {
        let mut map = HashMap::new();
        for line in lines {
            for (host, ip) in parse_hosts_line(line) {
                if host.is_empty() {
                    continue;
                }
                if let Ok(addr) = ip.parse::<IpAddr>() {
                    map.insert(host, addr);
                }
            }
        }
        Self { map }
    }

    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.map.len()
    }

    /// 命中自定义映射时返回映射后的地址，否则走系统 DNS。
    fn resolve(&self, netloc: &str) -> io::Result<Vec<SocketAddr>> {
        let (host, port) = split_netloc(netloc);
        if let Some(ip) = self.map.get(&host) {
            return Ok(vec![SocketAddr::new(*ip, port)]);
        }
        netloc.to_socket_addrs().map(|it| it.collect())
    }
}

impl ureq::Resolver for HostsResolver {
    fn resolve(&self, netloc: &str) -> io::Result<Vec<SocketAddr>> {
        HostsResolver::resolve(self, netloc)
    }
}

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
    /// 元数据语言（`language` 参数）。留空会回落 `DEFAULT_LANGUAGE`。
    language: String,
}

impl TmdbClient {
    /// 不带自定义 hosts（仅走系统 DNS）。
    #[allow(dead_code)]
    pub fn new(api_key: impl Into<String>) -> Self {
        Self::with_hosts(api_key, &[])
    }

    /// 带自定义域名解析。`hosts` 为 hosts 文件格式的若干行（见 `parse_hosts_line`）。
    pub fn with_hosts(api_key: impl Into<String>, hosts: &[String]) -> Self {
        Self::with_hosts_and_language(api_key, hosts, DEFAULT_LANGUAGE)
    }

    /// 带自定义域名解析 + 指定语言（设置项 `tmdbLanguage`）。
    pub fn with_hosts_and_language(
        api_key: impl Into<String>,
        hosts: &[String],
        language: &str,
    ) -> Self {
        // 复用共享的 agent 构造：超时、UA、自定义解析与其余供应方保持一致。
        //（原先这里自己拼一遍 `AgentBuilder`，参数与 `http::build_agent` 逐项相同。）
        let agent = crate::provider::http::build_agent(hosts, 20);
        Self {
            agent,
            api_key: api_key.into(),
            language: language.to_string(),
        }
    }

    /// 实际使用的语言：设置项为空时回落默认值。
    fn language(&self) -> &str {
        if self.language.trim().is_empty() {
            DEFAULT_LANGUAGE
        } else {
            self.language.trim()
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
        // 走共享实现（`http::get_json`），顺带补齐原先缺失的 403 / 429 错误文案。
        crate::provider::http::get_json::<T>(&self.agent, "TMDB", &url, &[])
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
            ("language", self.language().to_string()),
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
        let raw: Value = self.get_json(&path, &[("language", self.language().to_string())])?;
        Ok(TmdbMeta::from_value(id, kind, &raw))
    }

    /// 取某一季的单集列表（单集级刮削用）。
    pub fn season_episodes(&self, tv_id: i64, season: i64) -> Result<Vec<TmdbEpisodeItem>, String> {
        let path = format!("/tv/{tv_id}/season/{season}");
        let resp: SeasonResponse = self.get_json(&path, &[("language", self.language().to_string())])?;
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

    #[test]
    fn split_netloc_handles_host_port_ipv6_and_bare_host() {
        assert_eq!(split_netloc("api.themoviedb.org:443"), ("api.themoviedb.org".to_string(), 443));
        assert_eq!(split_netloc("example.com"), ("example.com".to_string(), 443));
        assert_eq!(split_netloc("[::1]:8080"), ("::1".to_string(), 8080));
    }

    #[test]
    fn parse_hosts_line_supports_ip_first_and_host_first() {
        let a = parse_hosts_line("13.224.161.90 api.themoviedb.org");
        assert_eq!(a, vec![("api.themoviedb.org".to_string(), "13.224.161.90".to_string())]);

        let b = parse_hosts_line("image.tmdb.org  104.16.61.155");
        assert_eq!(b, vec![("image.tmdb.org".to_string(), "104.16.61.155".to_string())]);
    }

    #[test]
    fn parse_hosts_line_supports_multiple_hosts_comments_and_junk() {
        let many = parse_hosts_line("1.2.3.4 a.com b.com");
        assert_eq!(many.len(), 2);
        assert_eq!(many[0].0, "a.com");
        assert_eq!(many[1].0, "b.com");

        assert!(parse_hosts_line("# 整行注释").is_empty());
        assert!(parse_hosts_line("").is_empty());
        assert!(parse_hosts_line("只有一个词").is_empty());
        assert!(parse_hosts_line("不是IP 也不是IP").is_empty());

        // 行尾注释要被剥掉
        let c = parse_hosts_line("5.6.7.8 c.com # 备注");
        assert_eq!(c, vec![("c.com".to_string(), "5.6.7.8".to_string())]);
    }

    #[test]
    fn hosts_resolver_overrides_mapped_host_and_keeps_port() {
        let r = HostsResolver::from_lines(&[
            "13.224.161.90 api.themoviedb.org".to_string(),
            "127.0.0.1 localhost".to_string(),
        ]);
        assert_eq!(r.len(), 2);

        let got = HostsResolver::resolve(&r, "api.themoviedb.org:443").unwrap();
        assert_eq!(got, vec!["13.224.161.90:443".parse::<SocketAddr>().unwrap()]);

        // 未配置的域名仍走系统解析（localhost 必然能解析）
        let other = HostsResolver::resolve(&r, "localhost:12345").unwrap();
        assert!(other.iter().any(|a| a.ip().is_loopback()));
    }

    #[test]
    fn hosts_resolver_ignores_bad_ip_and_later_line_wins() {
        let r = HostsResolver::from_lines(&[
            "999.999.999.999 nope.com".to_string(),
            "1.1.1.1 dup.com".to_string(),
            "2.2.2.2 dup.com".to_string(),
        ]);
        assert_eq!(r.len(), 1, "非法 IP 应被丢弃，重复域名以后出现的为准");
        let got = HostsResolver::resolve(&r, "dup.com:443").unwrap();
        assert_eq!(got, vec!["2.2.2.2:443".parse::<SocketAddr>().unwrap()]);
    }

    #[test]
    fn client_language_falls_back_to_default_when_blank() {
        assert_eq!(
            TmdbClient::with_hosts_and_language("k", &[], "").language(),
            DEFAULT_LANGUAGE
        );
        assert_eq!(
            TmdbClient::with_hosts_and_language("k", &[], "  ").language(),
            DEFAULT_LANGUAGE,
            "全空白应回落默认语言"
        );
        assert_eq!(
            TmdbClient::with_hosts_and_language("k", &[], " en-US ").language(),
            "en-US",
            "首尾空格应被去掉"
        );
        assert_eq!(TmdbClient::with_hosts("k", &[]).language(), DEFAULT_LANGUAGE);
    }

    #[test]
    fn hosts_resolver_empty_falls_back_to_system_dns() {
        let r = HostsResolver::from_lines(&[]);
        assert!(r.is_empty());
        let got = HostsResolver::resolve(&r, "localhost:443").unwrap();
        assert!(!got.is_empty());
    }
}
