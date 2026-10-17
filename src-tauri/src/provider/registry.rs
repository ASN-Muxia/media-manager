//! provider 注册表：内置数据源的顺序与开关。
//!
//! ## 本期做到哪一步
//! **做**：统一的注册表（顺序 + 是否启用）+ 配置开关。
//! **不做**：动态库加载 / 应用内在线安装插件——Rust 的 ABI 不稳定，
//! 做动态加载的工程量约等于前面所有 P0 之和，而当前痛点一个都不需要插件。
//! 注册表是插件化的**先决条件**，先把它做对。
//!
//! ⚠️ 优先级顺序**写死在注册表里且不可随意改**：TMDb 权威 → TheTVDB → Fanart →
//! OMDb → 豆瓣。注册表只回答「哪些源开着」，不改「谁说了算」（B5）。
//! 大模型**不进注册表**——它只补脏文件名，绝不替代元数据 API。

/// 数据源标识。
pub const TMDB: &str = "tmdb";
pub const TVDB: &str = "tvdb";
pub const FANART: &str = "fanart";
pub const OMDB: &str = "omdb";
pub const DOUBAN: &str = "douban";

/// 内置数据源的**权威顺序**（下标越小越权威）。
pub const ORDER: &[&str] = &[TMDB, TVDB, FANART, OMDB, DOUBAN];

/// 一个数据源在注册表里的条目。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub id: &'static str,
    /// 展示名（设置页用）。
    pub label: &'static str,
    /// 默认是否启用。
    pub default_on: bool,
    /// 这个源提供什么（设置页说明文字）。
    pub provides: &'static str,
}

/// 全部内置数据源（顺序即权威顺序）。
pub fn entries() -> Vec<Entry> {
    vec![
        Entry { id: TMDB, label: "TMDb", default_on: true, provides: "权威元数据：片名 / 年份 / 简介 / 海报" },
        Entry { id: TVDB, label: "TheTVDB", default_on: true, provides: "剧集的季 / 集结构与单集信息" },
        Entry { id: FANART, label: "Fanart.tv", default_on: true, provides: "背景图 / 高清美术资源" },
        Entry { id: OMDB, label: "OMDb", default_on: true, provides: "评分（IMDb 系）" },
        Entry { id: DOUBAN, label: "豆瓣", default_on: true, provides: "中文元数据兜底" },
    ]
}

/// 按「设置里关掉了哪些」过滤出本次真正要用的源，**保持权威顺序不变**。
///
/// `disabled` 里是不区分大小写的源 id；关掉一个源不会让后面的源提前。
pub fn enabled_order(disabled: &[String]) -> Vec<&'static str> {
    let off: Vec<String> = disabled.iter().map(|s| s.trim().to_ascii_lowercase()).collect();
    ORDER
        .iter()
        .copied()
        .filter(|id| !off.contains(&id.to_string()))
        .collect()
}

/// 源 id 是否合法（防止设置里塞进不存在的源）。
pub fn is_known(id: &str) -> bool {
    ORDER.contains(&id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn order_is_fixed_and_tmdb_is_authoritative() {
        assert_eq!(ORDER[0], TMDB, "TMDb 必须排第一（权威源）");
        assert_eq!(entries().len(), ORDER.len(), "注册表与顺序表必须一一对应");
    }

    #[test]
    fn disabling_a_source_does_not_promote_others() {
        let off = vec![TMDB.to_string()];
        let on = enabled_order(&off);
        assert_eq!(on, vec![TVDB, FANART, OMDB, DOUBAN]);
        assert!(!on.contains(&TMDB));
    }

    #[test]
    fn disabling_a_middle_source_keeps_the_rest_in_order() {
        let on = enabled_order(&[FANART.to_string()]);
        assert_eq!(on, vec![TMDB, TVDB, OMDB, DOUBAN]);
    }

    #[test]
    fn unknown_ids_are_rejected() {
        assert!(is_known(TMDB));
        assert!(!is_known("llm"), "大模型不进注册表");
        assert!(!is_known("whatever"));
    }

    /// 全部关闭时返回空，调用方应有兜底（不能因为关完源就崩）。
    #[test]
    fn all_disabled_yields_empty() {
        let all: Vec<String> = ORDER.iter().map(|s| s.to_string()).collect();
        assert!(enabled_order(&all).is_empty());
    }

    #[test]
    fn disabling_is_case_insensitive() {
        assert!(!enabled_order(&["TMDB".to_string()]).contains(&TMDB));
    }
}
