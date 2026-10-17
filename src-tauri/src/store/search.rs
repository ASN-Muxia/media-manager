//! 关键词检索：词项拆分、SQL 子句构造，以及可选的 FTS5 加速。
//!
//! ## 为什么「词项 AND」比「整串 LIKE」强
//! 原来的检索是 `title LIKE '%关键词%'`，用户输「卧虎 2000」时，
//! 数据库要找一个**同时包含这整串文字**的标题——几乎必然落空。
//! 改成按空白拆词、每个词各自匹配、词与词之间 AND 之后，
//! 同一个输入就能命中《卧虎藏龙》(2000)。这是本次检索升级最直观的收益。
//!
//! ## 为什么要「并集」而不是直接切到 FTS
//! FTS5 未必可用（SQLite 编译时可能没带上），而且它的分词口径与 LIKE 不完全
//! 一致。所以查询侧取 **LIKE 结果 ∪ FTS 结果**：
//! - FTS 可用 → 结果只增不减（并集），性能更好；
//! - FTS 不可用 / 查询过短 / 查询出错 → 原样回落到 LIKE，用户无感。
//! 这条「一键回落」的设计是硬要求——检索变快可以，检索变少不行。
//!
//! ## 对视频文件零写入
//! 本模块只读数据库。索引由 SQLite 触发器维护，不产生任何媒体目录内的文件。

use rusqlite::Connection;

use crate::store::schema::{fts5_available, FTS_TABLE};

/// 把用户输入拆成检索词项。
///
/// 按空白切分；引号包裹的片段视为一个整体（用户想精确搜「变形金刚 1」时可用）。
/// 空串返回空数组。
pub fn split_terms(input: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut buf = String::new();
    let mut in_quote = false;
    for ch in input.trim().chars() {
        match ch {
            '"' | '\'' | '“' | '”' => {
                in_quote = !in_quote;
            }
            c if c.is_whitespace() && !in_quote => {
                if !buf.is_empty() {
                    out.push(std::mem::take(&mut buf));
                }
            }
            c => buf.push(c),
        }
    }
    if !buf.is_empty() {
        out.push(buf);
    }
    out
}

/// LIKE 元字符转义（配合 SQL 里的 `ESCAPE '\'`）。
///
/// **顺序有讲究**：必须先转义 `\` 自身，否则刚加进去的转义符会被后面的
/// 替换再转一次（`\` → `\\` → `\\\\`）。
pub fn escape_like(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

/// 参与检索的字段（必须与前端 `visibleItems()` 的字段口径一致）。
const SEARCH_FIELDS: &[&str] = &[
    "m.title",
    "m.original_title",
    "COALESCE(m.group_key, '')",
    "m.genre",
    "CAST(m.year AS TEXT)",
];

/// 按词项数生成关键词匹配的 SQL 子句。
///
/// 参数从 `?6` 开始编号（?1 类型 / ?2 分类 / ?3 标签 / ?4 关键词原串 / ?5 未归类）。
/// 词项之间取 **AND**：每个词只要在任一字段里出现即可（字段之间 OR）。
pub fn keyword_clause(term_count: usize) -> String {
    if term_count == 0 {
        return "0".to_string();
    }
    let mut parts: Vec<String> = Vec::new();
    for i in 0..term_count {
        let p = 6 + i; // ?6, ?7, ...
        let field_tests: Vec<String> = SEARCH_FIELDS
            .iter()
            .map(|f| format!("{f} LIKE '%' || ?{p} || '%' ESCAPE '\\'"))
            .collect();
        parts.push(format!("({})", field_tests.join(" OR ")));
    }
    parts.join(" AND ")
}

/// 当前库能不能用 FTS5 索引（表存在且 SQLite 支持）。
pub fn fts_ready(conn: &Connection) -> bool {
    if !fts5_available(conn) {
        return false;
    }
    conn.query_row(
        &format!("SELECT COUNT(*) FROM {FTS_TABLE} LIMIT 1"),
        [],
        |_| Ok(()),
    )
    .is_ok()
}

/// 构造 FTS5 的 MATCH 表达式。
///
/// 每个词项加双引号按短语匹配，项之间 AND；引号内已有的 `"` 加倍转义。
/// 返回 `None` 表示这个词不适合走 FTS（比如只有标点）。
pub fn build_match_expr(terms: &[String]) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    for t in terms {
        let t = t.trim();
        if t.is_empty() {
            continue;
        }
        // 只有元字符的词（如 `%`、`_`）交给 LIKE 处理，FTS 侧直接跳过
        if t.chars().all(|c| "%_\\".contains(c)) {
            continue;
        }
        parts.push(format!("\"{}\"", t.replace('"', "\"\"")));
    }
    if parts.is_empty() {
        return None;
    }
    Some(parts.join(" AND "))
}

/// 用 FTS5 取候选 id。
///
/// 返回 `None` 表示「这次不用 FTS」（不可用 / 词项不适合），调用方应回落到 LIKE；
/// 返回 `Some(ids)` 时调用方应取 **LIKE 与它的并集**，保证结果只增不减。
pub fn matching_ids(conn: &Connection, keyword: &str) -> Option<Vec<i64>> {
    let terms = split_terms(keyword);
    if terms.is_empty() || !fts_ready(conn) {
        return None;
    }
    let expr = build_match_expr(&terms)?;
    let sql = format!("SELECT rowid FROM {FTS_TABLE} WHERE {FTS_TABLE} MATCH ?1");
    let mut stmt = conn.prepare(&sql).ok()?;
    let rows = stmt.query_map([&expr], |r| r.get::<_, i64>(0)).ok()?;
    let mut out = Vec::new();
    for r in rows {
        if let Ok(id) = r {
            out.push(id);
        }
    }
    Some(out)
}

/// 把候选 id 变成 SQL 的 `IN (...)` 片段。
///
/// 空集合返回 `0`（恒假），这样并集就退化为纯 LIKE——正是「回落」的形状。
pub fn in_clause(ids: &[i64]) -> String {
    if ids.is_empty() {
        return "0".to_string();
    }
    let list: Vec<String> = ids.iter().map(|i| i.to_string()).collect();
    format!("m.id IN ({})", list.join(","))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::schema::migrate;

    fn db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        conn
    }

    #[test]
    fn splits_on_whitespace_and_keeps_quoted_phrase() {
        assert_eq!(split_terms("卧虎 2000"), vec!["卧虎", "2000"]);
        assert_eq!(split_terms("  "), Vec::<String>::new());
        assert_eq!(split_terms("\"变形金刚 1\""), vec!["变形金刚 1"]);
    }

    /// 顺序陷阱：`\` 必须先转义，否则会被二次转义。
    #[test]
    fn escape_like_escapes_backslash_first() {
        assert_eq!(escape_like("100%"), "100\\%");
        assert_eq!(escape_like("A_B"), "A\\_B");
        assert_eq!(escape_like("a\\b"), "a\\\\b");
    }

    #[test]
    fn keyword_clause_ands_terms_and_ors_fields() {
        let c = keyword_clause(2);
        assert!(c.contains("?6") && c.contains("?7"), "两个词项要各占一个参数：{c}");
        assert!(c.contains(" AND "), "词项之间是 AND：{c}");
        assert!(c.contains("m.original_title"), "原名也要参与检索：{c}");
        assert_eq!(keyword_clause(0), "0");
    }

    #[test]
    fn build_match_expr_quotes_each_term() {
        assert_eq!(
            build_match_expr(&["卧虎".to_string(), "2000".to_string()]).as_deref(),
            Some("\"卧虎\" AND \"2000\"")
        );
        // 纯元字符的词不进 FTS
        assert_eq!(build_match_expr(&["%".to_string()]), None);
        assert_eq!(build_match_expr(&[]), None);
    }

    #[test]
    fn in_clause_degenerates_to_false_on_empty() {
        assert_eq!(in_clause(&[]), "0");
        assert_eq!(in_clause(&[1, 2]), "m.id IN (1,2)");
    }

    /// 端到端：多词 AND 检索（这是原来 LIKE 整串匹配做不到的）。
    #[test]
    fn multi_term_and_search_finds_the_row() {
        let conn = db();
        conn.execute(
            "INSERT INTO media (type,title,year,genre,added_at,updated_at) \
             VALUES ('movie','卧虎藏龙',2000,'剧情 / 武侠',0,0)",
            [],
        )
        .unwrap();
        let terms: Vec<String> = split_terms("卧虎 2000")
            .iter()
            .map(|t| escape_like(t))
            .collect();
        let clause = keyword_clause(terms.len());
        let sql = format!("SELECT COUNT(*) FROM media m WHERE {clause}");
        let mut stmt = conn.prepare(&sql).unwrap();
        // 关键词子句的参数从 ?6 起，前面 5 个是完整列表 SQL 的其它入参——
        // 单独跑子句时必须把 1..5 也绑上（SQLite 按编号绑定，不是按顺序）。
        let mut all: Vec<String> = Vec::new();
        for _ in 0..5 {
            all.push(String::new());
        }
        all.extend(terms.iter().cloned());
        let n: i64 = stmt
            .query(rusqlite::params_from_iter(all.iter()))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .get(0)
            .unwrap();
        assert_eq!(n, 1, "「卧虎 2000」应命中《卧虎藏龙》(2000)");
    }

    /// FTS 不可用时 `matching_ids` 必须返回 None，让调用方回落到 LIKE。
    #[test]
    fn matching_ids_returns_none_when_fts_unusable() {
        let conn = Connection::open_in_memory().unwrap();
        // 不跑 migrate：没有 FTS 表
        assert!(matching_ids(&conn, "卧虎").is_none());
    }

    /// FTS 可用时，插入的数据要能被查到（整词命中）。
    #[test]
    fn matching_ids_finds_indexed_row_when_fts_ready() {
        let conn = db();
        if !fts_ready(&conn) {
            return; // 本机 SQLite 没带 FTS5，跳过
        }
        conn.execute(
            "INSERT INTO media (type,title,added_at,updated_at) VALUES ('movie','卧虎藏龙',0,0)",
            [],
        )
        .unwrap();
        let ids = matching_ids(&conn, "卧虎藏龙").unwrap_or_default();
        assert_eq!(ids.len(), 1, "整词应能命中");
    }
}
