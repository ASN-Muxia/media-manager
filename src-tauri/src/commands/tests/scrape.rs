//! `scrape.rs` 的单元测试。
//!
//! 用 `#[path]` 挂回所属模块（见 `scrape.rs` 末尾），
//! 这样生产代码文件里只剩业务实现，单文件体积能降一大截，
//! 而 `use super::*` 仍能访问到私有成员，可见性一个字没改。

use super::*;
fn new_control() -> Arc<ScrapeControl> {
    Arc::new(ScrapeControl::default())
}

#[test]
fn control_begin_resets_cancel_flag() {
    let c = new_control();
    c.request_cancel();
    assert!(c.cancelled());
    c.begin();
    assert!(!c.cancelled(), "新一轮开始时应复位取消标志");
    assert!(c.running());
    c.finish();
    assert!(!c.running());
}

#[test]
fn cancel_is_visible_to_worker_thread() {
    let c = new_control();
    c.begin();
    let worker = c.clone();
    assert!(!worker.cancelled());
    c.request_cancel();
    assert!(worker.cancelled(), "后台线程应能看到取消标志");
}

#[test]
fn progress_tick_reports_counts() {
    let p = ScrapeProgress::tick(10, 3, 2, 1, "某部电影".to_string());
    assert!(p.running);
    assert!(!p.finished);
    assert_eq!((p.total, p.done, p.ok, p.failed), (10, 3, 2, 1));
    assert_eq!(p.current, "某部电影");
}

#[test]
fn progress_done_message_differs_for_cancel() {
    let normal = ScrapeProgress::done(10, 8, 2, false);
    assert!(normal.finished && !normal.running && !normal.cancelled);
    assert!(normal.message.contains("成功 8"));

    let cancelled = ScrapeProgress::done(10, 3, 1, true);
    assert!(cancelled.cancelled);
    assert!(cancelled.message.contains("已取消"));
}

#[test]
fn progress_aborted_carries_reason() {
    let p = ScrapeProgress::aborted("打开数据库失败".to_string());
    assert!(p.finished && !p.running);
    assert_eq!(p.message, "打开数据库失败");
}

/// 被取消后应立刻收敛：不再处理后续条目。
#[test]
fn cancelled_batch_stops_before_new_items() {
    let c = new_control();
    c.begin();
    let ids = vec![1, 2, 3, 4];
    let mut processed = 0;
    for _ in &ids {
        if c.cancelled() {
            break;
        }
        processed += 1;
        c.request_cancel(); // 模拟处理完第一条后用户点了取消
    }
    assert_eq!(processed, 1, "取消后不应继续处理后续条目");
    assert!(ScrapeProgress::done(ids.len() as i64, 1, 0, true).cancelled);
}

/// 取消时 `done` 只统计真正处理过的条数，未处理的不算失败。
#[test]
fn cancelled_progress_does_not_count_skipped_as_failed() {
    let p = ScrapeProgress::done(10, 2, 0, true);
    assert_eq!(p.done, 2);
    assert_eq!(p.failed, 0);
    assert!(p.message.contains("跳过 8"));
}

/* --------------------- 可选 provider 的规则 --------------------- */

fn memory_db() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
    crate::store::db::migrate(&conn).unwrap();
    conn
}

/// 设置项键名与 `commands/settings.rs` 保持一致。
fn put_setting(conn: &Connection, key: &str, value: &str) {
    conn.execute(
        "INSERT INTO settings (key, value) VALUES (?1, ?2) \
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )
    .unwrap();
}

#[test]
fn optional_providers_are_absent_without_keys() {
    let conn = memory_db();
    let p = Providers::load(&conn, "tmdb-key", vec![], "zh-CN");
    assert!(p.tvdb.is_none(), "没配 Key 时 TheTVDB 应整段缺席");
    assert!(p.fanart.is_none());
    assert!(p.omdb.is_none());
    assert!(!p.llm.usable(), "大模型默认必须是关闭的");
    assert!(!p.has_tvdb());
}

#[test]
fn providers_appear_once_keys_are_configured() {
    let conn = memory_db();
    put_setting(&conn, "tvdbKey", "tv-1");
    put_setting(&conn, "fanartKey", "fa-1");
    put_setting(&conn, "omdbKey", "om-1");
    put_setting(&conn, "llmEnabled", "1");
    put_setting(&conn, "llmBaseUrl", crate::provider::llm::RECOMMENDED_BASE_URL);
    let p = Providers::load(&conn, "tmdb-key", vec![], "zh-CN");
    assert!(p.tvdb.is_some() && p.fanart.is_some() && p.omdb.is_some());
    assert!(p.has_tvdb());
    assert!(p.llm.usable());
    assert_eq!(p.llm.model(), crate::provider::llm::RECOMMENDED_MODEL, "没填模型时应回落到推荐模型");
}

#[test]
fn art_is_only_filled_when_missing() {
    let art = Art {
        poster: Some("https://f/p.jpg".to_string()),
        backdrop: Some("https://f/b.jpg".to_string()),
    };
    // 全空 → 两张都补
    let (p, b) = merge_art(None, None, &art);
    assert_eq!(p.as_deref(), Some("https://f/p.jpg"));
    assert_eq!(b.as_deref(), Some("https://f/b.jpg"));

    // 空串也算"没有"
    let (p, _) = merge_art(Some("".to_string()), None, &art);
    assert!(p.is_some());

    // 已有 TMDb 的图 → 一律不动
    let (p, b) = merge_art(Some("/tmdb.jpg".to_string()), Some("/bg.jpg".to_string()), &art);
    assert!(p.is_none() && b.is_none(), "已有图不应被 Fanart 覆盖");

    // 只缺背景时只补背景
    let (p, b) = merge_art(Some("/tmdb.jpg".to_string()), None, &art);
    assert!(p.is_none());
    assert!(b.is_some());
}

#[test]
fn rating_is_only_filled_when_missing() {
    assert_eq!(merge_rating(None, Some(8.0)), Some(8.0));
    assert_eq!(merge_rating(Some(0.0), Some(8.0)), Some(8.0), "0 分视为没有评分");
    assert_eq!(merge_rating(Some(7.3), Some(8.0)), None, "已有评分不应被覆盖");
    assert_eq!(merge_rating(None, None), None);
    assert_eq!(merge_rating(None, Some(12.0)), None, "越界值当没查到");
}

/// 没配 OMDb / Fanart 时，补漏步骤应该是零成本的空转，而不是报错
#[test]
fn missing_providers_make_supplements_noop() {
    let conn = memory_db();
    conn.execute(
        "INSERT INTO media (type, title, added_at, updated_at) VALUES ('movie', 'X', 0, 0)",
        [],
    )
    .unwrap();
    let id = conn.last_insert_rowid();
    let p = Providers::load(&conn, "tmdb-key", vec![], "zh-CN");
    assert!(!fill_rating_from_omdb(&p, &conn, id, "X", None));
    assert!(!fill_art_from_fanart(&p, &conn, id, "movie", 1));
    assert_eq!(fill_episodes_from_tvdb(&p, &conn, id, "X"), (0, None));
}

/// 大模型回流：脏文件名被认出来后，分辨率 / 来源要落到现有字段
#[test]
fn llm_refine_is_silent_when_disabled() {
    let conn = memory_db();
    conn.execute(
        "INSERT INTO media (type, title, file_name, added_at, updated_at) \
         VALUES ('movie', '00123', '00123.mkv', 0, 0)",
        [],
    )
    .unwrap();
    let id = conn.last_insert_rowid();
    let p = Providers::load(&conn, "tmdb-key", vec![], "zh-CN");
    let pending = read_pending(&conn, id).unwrap();
    assert_eq!(pending.title, "00123");
    assert_eq!(pending.file_name.as_deref(), Some("00123.mkv"));
    // 未启用 → 静默返回 None，绝不抛错
    assert!(refine_title_with_llm(&p, &conn, id, &pending).is_none());
}

/// 预设要让设置页能「一键填入」，推荐项必须四个字段都齐
#[test]
fn llm_presets_put_recommendation_first() {
    let list = llm_presets();
    assert_eq!(list[0].label, "推荐 · DeepSeek + 通义千问 VL");
    assert_eq!(list[0].model, crate::provider::llm::RECOMMENDED_MODEL);
    assert_eq!(list[0].base_url, crate::provider::llm::RECOMMENDED_BASE_URL);
    assert_eq!(list[0].vision_model, crate::provider::llm::RECOMMENDED_VISION_MODEL);
    assert!(!list[0].vision_base_url.is_empty());
    // 备选至少给出地址，方便用户照着填
    assert!(list.len() > 1);
    assert!(list[1].base_url.starts_with("http"));
}

/// 刮削回填里，剧集的 file_name 要能回退到第一集（脏文件名兜底要用）
#[test]
fn pending_file_name_falls_back_to_first_episode() {
    let conn = memory_db();
    conn.execute(
        "INSERT INTO media (type, title, added_at, updated_at) VALUES ('tv', '剧', 0, 0)",
        [],
    )
    .unwrap();
    let id = conn.last_insert_rowid();
    conn.execute(
        "INSERT INTO episodes (media_id, season, episode, file_name, added_at) \
         VALUES (?1, 2, 1, 'S02E01.mkv', 0)",
        params![id],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO episodes (media_id, season, episode, file_name, added_at) \
         VALUES (?1, 1, 1, 'S01E01.mkv', 0)",
        params![id],
    )
    .unwrap();
    let pending = read_pending(&conn, id).unwrap();
    assert_eq!(pending.file_name.as_deref(), Some("S01E01.mkv"));
}
