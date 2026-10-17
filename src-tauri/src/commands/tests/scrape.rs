//! `scrape.rs` 的单元测试。
//!
//! 用 `#[path]` 挂回所属模块（见 `scrape.rs` 末尾），
//! 这样生产代码文件里只剩业务实现，单文件体积能降一大截，
//! 而 `use super::*` 仍能访问到私有成员，可见性一个字没改。

use super::*;
fn new_control() -> Arc<ScrapeControl> {
    Arc::new(ScrapeControl::default())
}

/* --------------------- 元数据缓存（接线验证） --------------------- */
/*
   怎么在不联网的前提下证明「缓存真的被用上了」：
   客户端故意用一个**空 API Key** 构造 —— 这种客户端一旦真去发请求必然失败。
   所以「调用成功且拿到预置的值」只能说明一件事：数据是**从缓存里读出来的**。
*/

/// 造一个不可能联网成功的客户端（空 Key）。
fn offline_client() -> TmdbClient {
    TmdbClient::with_hosts_and_language("", &[], "zh-CN")
}

#[test]
fn cached_detail_serves_from_cache_without_network() {
    let conn = memory_db();
    let meta = crate::provider::tmdb::TmdbMeta {
        tmdb_id: 7,
        title: "缓存片".to_string(),
        ..Default::default()
    };
    let key = cache_key(&["detail", "movie", "7"]);
    cache::put(
        &conn,
        PROVIDER_TMDB,
        &key,
        &serde_json::to_string(&meta).unwrap(),
        3600,
    )
    .unwrap();

    let got = cached_detail(&conn, &offline_client(), "movie", 7).unwrap();
    assert_eq!(got.title, "缓存片", "命中缓存时不该联网");
}

#[test]
fn cached_search_serves_from_cache_without_network() {
    let conn = memory_db();
    let items = vec![crate::provider::tmdb::TmdbSearchItem {
        id: 42,
        title: Some("卧虎藏龙".to_string()),
        ..Default::default()
    }];
    let key = cache_key(&["search", "movie", "卧虎藏龙", ""]);
    cache::put(
        &conn,
        PROVIDER_TMDB,
        &key,
        &serde_json::to_string(&items).unwrap(),
        3600,
    )
    .unwrap();

    let got = cached_search(&conn, &offline_client(), "movie", "卧虎藏龙", None).unwrap();
    assert_eq!(got[0].id, 42);
}

/// 缓存键必须带年份：同名的 1994 版与 2019 版不能互相顶掉。
#[test]
fn cache_key_includes_year() {
    let a = cache_key(&["search", "movie", "狮子王", "1994"]);
    let b = cache_key(&["search", "movie", "狮子王", "2019"]);
    assert_ne!(a, b, "不同年份必须落在不同 key 上");
}

/// 缓存里的脏数据（结构升级过、被手工改过）必须当成 miss，
/// 而不是把整个刮削流程弄崩 —— 这条路径上一次网络都没有，所以返回 None。
#[test]
fn corrupt_cache_entry_is_a_miss_not_a_panic() {
    let conn = memory_db();
    let key = cache_key(&["search", "movie", "坏数据", ""]);
    cache::put(&conn, PROVIDER_TMDB, &key, "{not json", 3600).unwrap();

    let got = cached_search(&conn, &offline_client(), "movie", "坏数据", None);
    assert!(got.is_none(), "脏缓存按 miss 处理，随后联网失败 → None");
}

/// 过期条目不再算命中（`expires_at` 由 `cache::put` 按 TTL 算）。
#[test]
fn expired_cache_entry_is_not_served() {
    let conn = memory_db();
    let items = vec![crate::provider::tmdb::TmdbSearchItem { id: 1, ..Default::default() }];
    let key = cache_key(&["search", "movie", "过期", ""]);
    // TTL = 0 → 立即过期
    cache::put(
        &conn,
        PROVIDER_TMDB,
        &key,
        &serde_json::to_string(&items).unwrap(),
        0,
    )
    .unwrap();

    assert!(cached_search(&conn, &offline_client(), "movie", "过期", None).is_none());
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
    assert!(p.douban.is_none(), "豆瓣默认关闭，升级上来的老库不能悄悄开启");
    assert!(!p.llm.usable(), "大模型默认必须是关闭的");
    assert!(!p.has_tvdb());
}

#[test]
fn providers_appear_once_keys_are_configured() {
    let conn = memory_db();
    put_setting(&conn, "tvdbKey", "tv-1");
    put_setting(&conn, "fanartKey", "fa-1");
    put_setting(&conn, "omdbKey", "om-1");
    put_setting(&conn, "doubanEnabled", "1");
    put_setting(&conn, "llmEnabled", "1");
    put_setting(&conn, "llmBaseUrl", crate::provider::llm::RECOMMENDED_BASE_URL);
    let p = Providers::load(&conn, "tmdb-key", vec![], "zh-CN");
    assert!(p.tvdb.is_some() && p.fanart.is_some() && p.omdb.is_some());
    assert!(p.douban.is_some(), "开关打开后豆瓣应就位");
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
    assert!(p.douban.is_none(), "豆瓣没开时不应有客户端");
}

/* --------------------- 豆瓣兜底的规则（全离线） --------------------- */

/// 只开豆瓣、不配 TMDb 也是一种合法配置：豆瓣走「整条接管」。
#[test]
fn douban_only_configuration_is_valid() {
    let conn = memory_db();
    put_setting(&conn, "doubanEnabled", "1");
    let p = Providers::load(&conn, "", vec![], "zh-CN");
    assert!(p.douban.is_some(), "开了豆瓣就该就位");
    assert!(!p.tmdb_on, "没配 Key 时 TMDb 不可用");
    assert!(p.tvdb.is_none() && p.fanart.is_none() && p.omdb.is_none());
}

/// 配了 TMDb Key 时 `tmdb_on` 才为真；**被用户关掉**（`disabledProviders`）
/// 同样变成不可用——这两种情况都该让豆瓣接管，而不是直接失败。
#[test]
fn tmdb_on_requires_key_and_not_disabled() {
    let conn = memory_db();

    let p = Providers::load(&conn, "k", vec![], "zh-CN");
    assert!(p.tmdb_on, "有 Key 且没关掉 → 可用");

    put_setting(&conn, "disabledProviders", r#"["tmdb"]"#);
    let p = Providers::load(&conn, "k", vec![], "zh-CN");
    assert!(!p.tmdb_on, "用户关掉了 TMDb → 不可用（Key 还在，但不用）");
}

/// 关掉一个可选源只是让它缺席，**不会**让别的源顶上来。
#[test]
fn disabling_optional_sources_only_removes_them() {
    let conn = memory_db();
    put_setting(&conn, "tvdbKey", "t");
    put_setting(&conn, "fanartKey", "f");
    put_setting(&conn, "omdbKey", "o");
    put_setting(&conn, "doubanEnabled", "1");

    let all = Providers::load(&conn, "k", vec![], "zh-CN");
    assert!(all.tvdb.is_some() && all.fanart.is_some() && all.omdb.is_some());
    assert!(all.douban.is_some());

    put_setting(&conn, "disabledProviders", r#"["fanart"]"#);
    let some = Providers::load(&conn, "k", vec![], "zh-CN");
    assert!(some.fanart.is_none(), "被关掉的源要缺席");
    assert!(some.tvdb.is_some() && some.omdb.is_some() && some.douban.is_some(),
        "关掉一个源不该影响其它源");
    assert!(some.tmdb_on, "关掉 Fanart 更不该影响 TMDb");
}

/// 设置里塞了不存在的源 id 时必须被忽略，不能因为一个错字就让某个源消失。
#[test]
fn unknown_disabled_ids_are_ignored_by_providers_load() {
    let conn = memory_db();
    put_setting(&conn, "doubanEnabled", "1");
    put_setting(&conn, "disabledProviders", r#"["llm","whatever"]"#);

    let p = Providers::load(&conn, "k", vec![], "zh-CN");
    assert!(p.douban.is_some() && p.tmdb_on);
}

/// TMDb 不可用时，提示语要说清是「没启用」而不是「没找到」——
/// 否则用户会去 TMDB 那边白找一圈。
///
/// ⚠️ 这条**不能真去请求豆瓣**——`douban_takeover` 里查不到客户端时会先返回，
/// 所以这里只装「没开豆瓣 + 各种 TMDb 状态」的配置，然后断言提示语分支。
/// 真正的联网成功路径由 `provider/douban.rs` 的纯解析测试覆盖。
#[test]
fn douban_takeover_message_reflects_tmdb_state() {
    let conn = memory_db();
    let id = insert_media(&conn, "movie", "沙丘");

    // ① 没配 TMDb Key → 提示要说明「TMDb 未启用」
    let p = Providers::load(&conn, "", vec![], "zh-CN");
    assert!(p.douban.is_none());
    let pending = read_pending(&conn, id).unwrap();
    let out = douban_takeover(&conn, &p, id, pending, None).unwrap();
    assert!(!out.ok);
    assert_eq!(
        out.message,
        "TMDb 未启用（没配 Key 或已关闭），且未在豆瓣找到匹配条目",
        "TMDb 不可用时不该说「未在 TMDB 找到匹配条目」"
    );

    // ② 配了 TMDb Key → 保持加豆瓣之前的原文案（向后兼容）
    let p = Providers::load(&conn, "tmdb-key", vec![], "zh-CN");
    assert!(p.tmdb_on);
    let pending = read_pending(&conn, id).unwrap();
    let out = douban_takeover(&conn, &p, id, pending, None).unwrap();
    assert_eq!(out.message, "未在 TMDB 找到匹配条目");
}

/// 造一条 media 记录，返回 id。`poster` / `overview` 等按需给。
fn insert_media(conn: &Connection, media_type: &str, title: &str) -> i64 {
    conn.execute(
        "INSERT INTO media (type, title, added_at, updated_at) VALUES (?1, ?2, 0, 0)",
        params![media_type, title],
    )
    .unwrap();
    conn.last_insert_rowid()
}

/// 读回一条记录的关键字段，便于断言"哪些被写了、哪些没被动"。
fn read_fill_state(conn: &Connection, id: i64) -> (String, Option<String>, Option<String>, Option<f64>) {
    conn.query_row(
        "SELECT title, overview, poster_path, vote_average FROM media WHERE id = ?1",
        params![id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
    )
    .unwrap()
}

fn sample_douban_meta() -> DoubanMeta {
    DoubanMeta {
        douban_id: "3001114".to_string(),
        title: Some("沙丘".to_string()),
        original_title: Some("Dune".to_string()),
        year: Some(2021),
        directors: Some("丹尼斯·维伦纽瓦".to_string()),
        actors: Some("提莫西·查拉梅 / 丽贝卡·弗格森".to_string()),
        genre: Some("剧情 / 科幻 / 冒险".to_string()),
        release_date: Some("2021-10-22".to_string()),
        rating: Some(7.7),
        overview: Some("少年保罗·厄崔迪前往最危险的星球。".to_string()),
        poster: Some("https://img9.doubanio.com/p.jpg".to_string()),
        episodes: None,
    }
}

/// 整条接管：TMDb 没命中时，豆瓣把能给的都写进去（含 title / year）
#[test]
fn douban_takeover_writes_everything_when_empty() {
    let conn = memory_db();
    let id = insert_media(&conn, "movie", "沙丘.Dune.2021.1080p");
    let m = sample_douban_meta();
    let fill = apply_douban_takeover(&conn, id, &m).unwrap();

    assert!(fill.took_over);
    let (title, overview, poster, vote) = read_fill_state(&conn, id);
    assert_eq!(title, "沙丘", "接管时片名可以被改写");
    assert_eq!(overview.as_deref(), Some("少年保罗·厄崔迪前往最危险的星球。"));
    assert_eq!(poster.as_deref(), Some("https://img9.doubanio.com/p.jpg"));
    assert_eq!(vote, Some(7.7));

    // 工作人员 / 上映日期挂在 release_source 上
    let line: Option<String> = conn
        .query_row("SELECT release_source FROM media WHERE id = ?1", params![id], |r| r.get(0))
        .unwrap();
    let line = line.unwrap();
    assert!(line.contains("上映：2021-10-22"), "实际：{line}");
    assert!(line.contains("导演：丹尼斯·维伦纽瓦"), "实际：{line}");
    assert!(line.contains("主演：提莫西·查拉梅"), "实际：{line}");

    // 年份也补上了
    let year: Option<i32> = conn
        .query_row("SELECT year FROM media WHERE id = ?1", params![id], |r| r.get(0))
        .unwrap();
    assert_eq!(year, Some(2021));

    // 来源标记成 douban:<id>，方便日后区分
    let source: Option<String> = conn
        .query_row("SELECT source FROM media WHERE id = ?1", params![id], |r| r.get(0))
        .unwrap();
    assert_eq!(source.as_deref(), Some("douban:3001114"));
}

/// 整条接管时，豆瓣没给的字段要保留原值，不能被清空
#[test]
fn douban_takeover_keeps_existing_when_douban_is_empty() {
    let conn = memory_db();
    let id = insert_media(&conn, "movie", "原标题");
    conn.execute(
        "UPDATE media SET overview = '原有简介', poster_path = '/old.jpg', vote_average = 9.1 \
         WHERE id = ?1",
        params![id],
    )
    .unwrap();

    // 豆瓣只给了一个空壳
    let m = DoubanMeta {
        title: None,
        overview: None,
        poster: None,
        rating: None,
        ..Default::default()
    };
    apply_douban_takeover(&conn, id, &m).unwrap();

    let (title, overview, poster, vote) = read_fill_state(&conn, id);
    assert_eq!(title, "原标题", "豆瓣没给片名就不该动");
    assert_eq!(overview.as_deref(), Some("原有简介"));
    assert_eq!(poster.as_deref(), Some("/old.jpg"));
    assert_eq!(vote, Some(9.1));
}

/// 只补缺失：TMDb 已经给过的字段，豆瓣再查也不许覆盖
#[test]
fn douban_fill_never_overwrites_existing() {
    let conn = memory_db();
    let id = insert_media(&conn, "movie", "沙丘");
    // 模拟 TMDb 已经写好的部分字段
    conn.execute(
        "UPDATE media SET overview = 'TMDb 的简介', poster_path = '/tmdb.jpg', \
                vote_average = 8.0, original_title = 'Dune (2021)', \
                release_source = 'BluRay' \
         WHERE id = ?1",
        params![id],
    )
    .unwrap();

    let m = sample_douban_meta();
    let fill = apply_douban_fill(&conn, id, &m).unwrap();

    assert!(!fill.took_over, "TMDb 命中时只能补，不能接管");
    assert!(fill.fields.is_empty(), "所有字段都已存在，应无可补：{:?}", fill.fields);

    let (_, overview, poster, vote) = read_fill_state(&conn, id);
    assert_eq!(overview.as_deref(), Some("TMDb 的简介"), "简介不该被覆盖");
    assert_eq!(poster.as_deref(), Some("/tmdb.jpg"), "海报不该被覆盖");
    assert_eq!(vote, Some(8.0), "评分不该被覆盖");

    let orig: Option<String> = conn
        .query_row("SELECT original_title FROM media WHERE id = ?1", params![id], |r| r.get(0))
        .unwrap();
    assert_eq!(orig.as_deref(), Some("Dune (2021)"));

    let src: Option<String> = conn
        .query_row("SELECT release_source FROM media WHERE id = ?1", params![id], |r| r.get(0))
        .unwrap();
    assert_eq!(src.as_deref(), Some("BluRay"), "release_source 已有值就不该被主创行挤掉");
}

/// 只补缺失：真正缺的部分要补上，并有字段名回执
#[test]
fn douban_fill_adds_only_the_gaps() {
    let conn = memory_db();
    let id = insert_media(&conn, "movie", "沙丘");
    // TMDb 给了简介和评分，但没图、没原名、没主创
    conn.execute(
        "UPDATE media SET overview = 'TMDb 的简介', vote_average = 8.0 WHERE id = ?1",
        params![id],
    )
    .unwrap();

    let m = sample_douban_meta();
    let fill = apply_douban_fill(&conn, id, &m).unwrap();

    // 应该只补「原名 / 海报 / 上映主创」这三类
    assert!(fill.fields.contains(&"原名".to_string()), "实际：{:?}", fill.fields);
    assert!(fill.fields.contains(&"海报".to_string()), "实际：{:?}", fill.fields);
    assert!(fill.fields.contains(&"上映/主创".to_string()), "实际：{:?}", fill.fields);
    assert!(!fill.fields.contains(&"简介".to_string()), "简介已有，不该再补");
    assert!(!fill.fields.contains(&"评分".to_string()), "评分已有，不该再补");

    let (title, overview, poster, vote) = read_fill_state(&conn, id);
    assert_eq!(title, "沙丘", "只补模式**绝不改片名**");
    assert_eq!(overview.as_deref(), Some("TMDb 的简介"), "原有简介保留");
    assert_eq!(poster.as_deref(), Some("https://img9.doubanio.com/p.jpg"), "海报被补上");
    assert_eq!(vote, Some(8.0), "原有评分保留");
}

/// `merge_text`：空值定义要一致（NULL / 空串 / 全空白都算「没有」）
#[test]
fn merge_text_treats_blank_as_missing() {
    assert_eq!(merge_text(None, Some("新")), Some("新".to_string()));
    assert_eq!(merge_text(Some(""), Some("新")), Some("新".to_string()));
    assert_eq!(merge_text(Some("   "), Some("新")), Some("新".to_string()));
    assert_eq!(merge_text(Some("旧"), Some("新")), None, "已有值不该被覆盖");
    assert_eq!(merge_text(None, None), None);
    assert_eq!(merge_text(None, Some("  ")), None, "新值全空白也当没查到");
    // 新值要 trim
    assert_eq!(merge_text(None, Some("  新  ")), Some("新".to_string()));
}

/// 豆瓣一个字段都没给时，`apply_douban_fill` 不该发 UPDATE（零成本空转）
#[test]
fn douban_fill_is_noop_when_nothing_to_add() {
    let conn = memory_db();
    let id = insert_media(&conn, "movie", "沙丘");
    conn.execute(
        "UPDATE media SET original_title = 'Dune', overview = 'x', poster_path = '/p.jpg', \
                vote_average = 8.0, release_source = 'y' \
         WHERE id = ?1",
        params![id],
    )
    .unwrap();
    let empty = DoubanMeta::default();
    let fill = apply_douban_fill(&conn, id, &empty).unwrap();
    assert!(!fill.took_over);
    assert!(fill.fields.is_empty());
}

/// 主创行拼接：三缺一 / 全缺都要正确
#[test]
fn crew_line_handles_partial_data() {
    let mut m = DoubanMeta::default();
    assert_eq!(douban_crew_line(&m), "");

    m.release_date = Some("2021-10-22".to_string());
    assert_eq!(douban_crew_line(&m), "上映：2021-10-22");

    m.directors = Some("张三".to_string());
    assert_eq!(douban_crew_line(&m), "上映：2021-10-22　导演：张三");

    m.actors = Some("李四".to_string());
    assert!(douban_crew_line(&m).ends_with("主演：李四"));

    // 全空白串要当缺失
    let blank = DoubanMeta {
        release_date: Some("  ".to_string()),
        directors: Some("".to_string()),
        actors: Some(" ".to_string()),
        ..Default::default()
    };
    assert_eq!(douban_crew_line(&blank), "");
}

/// 豆瓣整条接管只写在 `providers.douban` 缺席时不做任何事（且不报错）
#[test]
fn douban_takeover_is_noop_without_client() {
    let conn = memory_db();
    let id = insert_media(&conn, "movie", "沙丘.Dune.2021");
    let p = Providers::load(&conn, "tmdb-key", vec![], "zh-CN");
    assert!(p.douban.is_none());

    let pending = read_pending(&conn, id).unwrap();
    let out = douban_takeover(&conn, &p, id, pending, None).unwrap();

    assert!(!out.ok, "没启用豆瓣时应返回匹配失败");
    assert_eq!(out.message, "未在 TMDB 找到匹配条目", "文案要与加豆瓣之前一致");
    // 库里什么都没被动
    let (title, overview, poster, vote) = read_fill_state(&conn, id);
    assert_eq!(title, "沙丘.Dune.2021");
    assert!(overview.is_none() && poster.is_none() && vote.is_none());
}

/// 大模型认出的标题在「TMDb + 豆瓣都没命中」时也要落库，别丢
#[test]
fn douban_takeover_persists_llm_hint_on_total_miss() {
    let conn = memory_db();
    let id = insert_media(&conn, "movie", "00123");
    let p = Providers::load(&conn, "tmdb-key", vec![], "zh-CN");
    let pending = read_pending(&conn, id).unwrap();

    let refined = Some(("沙丘".to_string(), Some(2021)));
    let out = douban_takeover(&conn, &p, id, pending, refined).unwrap();
    assert!(!out.ok);

    let (title, _, _, _) = read_fill_state(&conn, id);
    assert_eq!(title, "沙丘", "大模型认出的标题应被保留");
    let year: Option<i32> = conn
        .query_row("SELECT year FROM media WHERE id = ?1", params![id], |r| r.get(0))
        .unwrap();
    assert_eq!(year, Some(2021));
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

/* --------------------- 扫描 / 导入后的自动刮削 --------------------- */

/// 自动刮削的纯决策：多个开关叠加才生效，任一条件不满足就**不触发**。
#[test]
fn auto_scrape_requires_switch_and_at_least_one_source() {
    let targets = vec![1i64, 2, 3];

    // 开关没开 → 不触发（哪怕源和条目都齐）
    assert_eq!(plan_auto_scrape(false, "k", true, targets.clone()), None);

    // 开了但一个源都没配 → 不触发，别空跑
    assert_eq!(plan_auto_scrape(true, "", false, targets.clone()), None);
    assert_eq!(plan_auto_scrape(true, "   ", false, targets.clone()), None);

    // 没有待刮条目 → 不触发
    assert_eq!(plan_auto_scrape(true, "k", false, Vec::new()), None);

    // 有 TMDb Key 就够
    assert_eq!(
        plan_auto_scrape(true, "k", false, targets.clone()),
        Some(targets.clone())
    );
    // 只开豆瓣也行（没配 TMDb Key 时豆瓣承担主要工作）
    assert_eq!(
        plan_auto_scrape(true, "", true, targets.clone()),
        Some(targets.clone())
    );
    // 两个都配更好
    assert_eq!(plan_auto_scrape(true, "k", true, targets.clone()), Some(targets));
}

/// 自动刮削开关的持久化：默认关闭，存取往返要对得上。
#[test]
fn auto_scrape_setting_defaults_off_and_roundtrips() {
    let conn = memory_db();
    assert!(
        !crate::commands::settings::load_auto_scrape(&conn),
        "默认必须关闭：自动往第三方站点发请求要用户显式同意"
    );
    put_setting(&conn, "autoScrapeAfterScan", "1");
    assert!(crate::commands::settings::load_auto_scrape(&conn));
    put_setting(&conn, "autoScrapeAfterScan", "true");
    assert!(crate::commands::settings::load_auto_scrape(&conn), "true 也要认");
    put_setting(&conn, "autoScrapeAfterScan", "0");
    assert!(!crate::commands::settings::load_auto_scrape(&conn));
}

/// 自动刮削与手动刮削共用同一个开关：已占用时自动触发必须放弃，
/// 且**不能**清掉手动任务的取消标志（否则用户点了取消会被悄悄复活）。
#[test]
fn auto_scrape_yields_to_a_running_manual_task() {
    let control = new_control();
    // 模拟用户手动启动的批量刮削
    assert!(control.try_begin());
    control.request_cancel();

    // 自动触发这时不能插进去
    assert!(!control.try_begin(), "手动任务在跑时必须让路");
    assert!(control.cancelled(), "不能把用户点的取消给清掉");
    assert!(control.running());
}

/// 用户手建 / 锁定过的条目，片名归用户所有：豆瓣接管只能补元数据，**不能改名**。
///
/// 这是「手建剧集接不进内容」那个 bug 的另一条触发路径 ——
/// 片名一旦被改成豆瓣的写法，`find_manual_series` 的归一化匹配就可能失效，
/// 下次导入又会接不进来。
#[test]
fn douban_takeover_keeps_user_owned_title() {
    for (tag, sql) in [
        ("手建", "source = 'manual'"),
        ("锁定", "locked = 1"),
    ] {
        let conn = memory_db();
        let id = insert_media(&conn, "tv", "西游记");
        conn.execute(
            &format!("UPDATE media SET {sql} WHERE id = ?1"),
            params![id],
        )
        .unwrap();

        let m = DoubanMeta {
            title: Some("西游记 1986".to_string()),
            original_title: Some("Journey to the West".to_string()),
            year: Some(1986),
            overview: Some("央视版西游记。".to_string()),
            poster: Some("https://img9.doubanio.com/x.jpg".to_string()),
            rating: Some(9.6),
            genre: Some("奇幻 / 冒险".to_string()),
            ..sample_douban_meta()
        };
        let fill = apply_douban_takeover(&conn, id, &m).unwrap();

        assert!(fill.took_over, "{tag} 条目仍应接管（补元数据）");
        assert!(
            !fill.fields.iter().any(|f| f == "片名"),
            "{tag} 条目的片名不能被汇报为已改写，实际：{:?}",
            fill.fields
        );

        let (title, overview, poster, vote) = read_fill_state(&conn, id);
        assert_eq!(title, "西游记", "{tag} 条目片名必须保持用户起的那个");
        // 其余字段照补
        assert_eq!(overview.as_deref(), Some("央视版西游记。"));
        assert_eq!(poster.as_deref(), Some("https://img9.doubanio.com/x.jpg"));
        assert_eq!(vote, Some(9.6));
    }
}

/// 普通（非手建、未锁定）条目的片名仍然可以被改写 —— 上面那条保护不能误伤。
#[test]
fn douban_takeover_still_renames_ordinary_entries() {
    let conn = memory_db();
    let id = insert_media(&conn, "movie", "沙丘.Dune.2021.1080p");
    let fill = apply_douban_takeover(&conn, id, &sample_douban_meta()).unwrap();
    assert!(fill.fields.iter().any(|f| f == "片名"));
    assert_eq!(read_fill_state(&conn, id).0, "沙丘");
}

/* --------------------- TMDB id 的优先级链 --------------------- */

/// 优先级：显式指定 > 入库时解析的 > 文本里现解析的（文件名 → 标题）。
///
/// 命中任何一级就**不再**走「标题 + 年份检索」的模糊匹配——这是本轮需求的核心：
/// 文本里写明了身份，就没必要再去猜。
#[test]
fn tmdb_id_evidence_priority_explicit_then_stored_then_text() {
    // 1) 显式指定最高：即使其它三处都有值也以它为准
    assert_eq!(
        tmdb_id_from_evidence(Some(1), Some(2), Some("x.tmdb-3.mkv"), "y tmdb-4"),
        Some(1)
    );
    // 2) 入库时解析出来的次之
    assert_eq!(
        tmdb_id_from_evidence(None, Some(2), Some("x.tmdb-3.mkv"), "y tmdb-4"),
        Some(2)
    );
    // 3) 都没有才从文本现解析，且**文件名优先于标题**
    assert_eq!(
        tmdb_id_from_evidence(None, None, Some("x.tmdb-3.mkv"), "y tmdb-4"),
        Some(3)
    );
    // 4) 只有标题里有
    assert_eq!(
        tmdb_id_from_evidence(None, None, Some("没线索.mkv"), "y tmdbid:4"),
        Some(4)
    );
    // 5) 都没有 → None，交给原有流程（标题 + 年份检索）
    assert_eq!(
        tmdb_id_from_evidence(None, None, Some("言叶之庭.2013.mkv"), "言叶之庭"),
        None
    );
    assert_eq!(tmdb_id_from_evidence(None, None, None, "言叶之庭"), None);
}

/// 防误伤：`2012` / `1080p` 这类普通数字不会被当成身份线索。
#[test]
fn tmdb_id_evidence_ignores_plain_numbers() {
    assert_eq!(
        tmdb_id_from_evidence(None, None, Some("2012.2009.1080p.mkv"), "2012"),
        None
    );
    assert_eq!(
        tmdb_id_from_evidence(None, None, Some("Se7en.1995.x264.mkv"), "Se7en"),
        None
    );
}
