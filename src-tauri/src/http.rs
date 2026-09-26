//! 各元数据供应方共用的 HTTP 小工具。
//!
//! 为什么单独抽一层：TMDb / TheTVDB / Fanart.tv / OMDb / 大模型接口
//! 要的都是同一件事——「带自定义域名解析、带超时、拿回一个 JSON」。
//! 每个模块各抄一遍 ureq 样板既啰嗦又容易在超时、错误处理上走偏。
//!
//! 注意：这里**不**把 URL 里的域名换成 IP。换成 IP 会破坏 HTTPS 的 SNI 与证书
//! 校验（证书签的是域名），所以只改解析结果、保留原 URL（见 `tmdb::HostsResolver`）。

use serde::de::DeserializeOwned;
use serde_json::Value;
use std::time::Duration;

/// 建一个带自定义解析与超时的 ureq agent。
pub fn build_agent(hosts: &[String], timeout_secs: u64) -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(timeout_secs))
        .user_agent("media-manager/0.1 (+tauri)")
        .resolver(crate::tmdb::HostsResolver::from_lines(hosts))
        .build()
}

/// GET 一个 JSON。错误里带上供应方名字，界面上才能看出是谁挂了。
pub fn get_json<T: DeserializeOwned>(
    agent: &ureq::Agent,
    who: &str,
    url: &str,
    headers: &[(&str, &str)],
) -> Result<T, String> {
    let mut req = agent.get(url);
    for (k, v) in headers {
        req = req.set(k, v);
    }
    let resp = req.call().map_err(|e| match e {
        ureq::Error::Status(401, _) => format!("{who} 鉴权失败（401）：请检查设置里的 Key"),
        ureq::Error::Status(403, _) => format!("{who} 没有访问权限（403）：Key 可能无效"),
        ureq::Error::Status(404, _) => format!("{who} 未找到对应资源（404）"),
        ureq::Error::Status(429, _) => format!("{who} 触发限流（429）：稍后再试"),
        ureq::Error::Status(code, _) => format!("{who} 请求失败（HTTP {code}）"),
        other => format!("{who} 网络请求失败：{other}"),
    })?;
    resp.into_json::<T>()
        .map_err(|e| format!("{who} 响应解析失败：{e}"))
}

/// POST 一个 JSON 再拿回 JSON。
pub fn post_json<T: DeserializeOwned>(
    agent: &ureq::Agent,
    who: &str,
    url: &str,
    headers: &[(&str, &str)],
    body: &Value,
) -> Result<T, String> {
    let mut req = agent.post(url);
    for (k, v) in headers {
        req = req.set(k, v);
    }
    let resp = req.send_json(body.clone()).map_err(|e| match e {
        ureq::Error::Status(401, _) => format!("{who} 鉴权失败（401）：请检查设置里的 Key"),
        ureq::Error::Status(403, _) => format!("{who} 没有访问权限（403）：Key 可能无效"),
        ureq::Error::Status(429, _) => format!("{who} 触发限流（429）：稍后再试"),
        ureq::Error::Status(code, _) => format!("{who} 请求失败（HTTP {code}）"),
        other => format!("{who} 网络请求失败：{other}"),
    })?;
    resp.into_json::<T>()
        .map_err(|e| format!("{who} 响应解析失败：{e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_is_built_without_panic() {
        // 只验证构造不炸、且能带自定义解析（真正的网络调用不进单测）
        let a = build_agent(&["93.184.216.34 example.com".to_string()], 5);
        let _ = a;
    }
}
