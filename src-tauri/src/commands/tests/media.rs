//! `media.rs` 的单元测试。
//!
//! 用 `#[path]` 挂回所属模块（见 `media.rs` 末尾），
//! 这样生产代码文件里只剩业务实现，单文件体积能降一大截，
//! 而 `use super::*` 仍能访问到私有成员，可见性一个字没改。

use super::*;
use crate::store::db;

fn memory_db() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
    // 走 migrate 而不是裸 SCHEMA：分类 / 标签表和实体扩展字段都要在
    db::migrate(&conn).unwrap();
    conn
}

fn seed(conn: &Connection) {
    conn.execute(
        "INSERT INTO media (type,title,year,file_path,file_name,added_at,updated_at) \
         VALUES ('movie','Dune',2021,'D:\\\\Dune.mkv','Dune.mkv',1,1)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO media (type,title,year,added_at,updated_at) VALUES ('tv','Breaking Bad',2008,1,1)",
        [],
    )
    .unwrap();
    let sid = conn.last_insert_rowid();
    for (s, e) in [(1, 1), (1, 2), (2, 1)] {
        conn.execute(
            "INSERT INTO episodes (media_id,season,episode,file_path,file_name,added_at) \
             VALUES (?1,?2,?3,?4,?5,1)",
            params![
                sid,
                s,
                e,
                format!("D:\\\\BB.S{s:02}E{e:02}.mkv"),
                format!("BB.S{s:02}E{e:02}.mkv")
            ],
        )
        .unwrap();
    }
}

fn query_items(conn: &Connection, filter: &str) -> Vec<MediaListItem> {
    // 走 `list_where_with_fts(0, None)`：关键词子句要先展开占位符，
    // 裸 `LIST_WHERE` 不是合法 SQL；`None` 表示这次不接 FTS 候选。
    let sql = format!(
        "SELECT {ITEM_COLUMNS} FROM media m {} \
         ORDER BY m.title COLLATE NOCASE ASC, m.year ASC",
        list_where_with_fts(0, None)
    );
    let mut stmt = conn.prepare(&sql).unwrap();
    let rows = stmt
        .query_map(
            params![filter, None::<i64>, None::<i64>, None::<String>, None::<i64>],
            map_item,
        )
        .unwrap();
    let mut out: Vec<MediaListItem> = rows.map(|r| r.unwrap()).collect();
    // 与 `get_media_list` 同一条收尾：层级不在 SQL 里算，要过 `attach_depth`
    attach_depth(conn, &mut out).unwrap();
    out
}

/// 与 `get_media_detail` 同形地取单条（详情路径同样要真实层级）。
fn query_item(conn: &Connection, id: i64) -> MediaListItem {
    let sql = format!("SELECT {ITEM_COLUMNS} FROM media m WHERE m.id = ?1");
    let mut item = conn.query_row(&sql, params![id], map_item).unwrap();
    attach_depth(conn, std::slice::from_mut(&mut item)).unwrap();
    item
}

#[test]
fn list_filters_by_type() {
    let conn = memory_db();
    seed(&conn);
    assert_eq!(query_items(&conn, "all").len(), 2);
    let movies = query_items(&conn, "movie");
    assert_eq!(movies.len(), 1);
    assert_eq!(movies[0].media_type, "movie");
    let tv = query_items(&conn, "tv");
    assert_eq!(tv.len(), 1);
    assert_eq!(tv[0].media_type, "tv");
}

/// 回归：前端默认给 `unclassified` 传 `false`，它必须是「不过滤」而不是
/// 「只看已归类」——否则「全部 / 电影 / 剧集」三个栏目会整体空白。
#[test]
fn unclassified_false_means_no_filter() {
    assert_eq!(normalize_unclassified(None), None);
    assert_eq!(normalize_unclassified(Some(false)), None);
    assert_eq!(normalize_unclassified(Some(true)), Some(1));

    // 种子数据没有任何分类关联；不过滤时应全部可见。
    let conn = memory_db();
    seed(&conn);
    assert_eq!(query_items(&conn, "all").len(), 2);
}

/* ---------------- 关键词检索 ---------------- */

/// 与 `get_media_list` 相同的 SQL，仅把关键词换成测试入参。
/// 注意：命令层会对关键词先 `escape_like`，测试同样照做——
/// 直接传原始串等于绕过命令层的转义。
fn query_keyword(conn: &Connection, kw: Option<&str>) -> Vec<String> {
    // 与命令层同形：按词项数展开占位符，并对每个词项做 `escape_like`。
    let terms: Vec<String> = match kw {
        Some(k) => search::split_terms(k).iter().map(|t| search::escape_like(t)).collect(),
        None => Vec::new(),
    };
    let sql = format!(
        "SELECT {ITEM_COLUMNS} FROM media m {} \
         ORDER BY m.title COLLATE NOCASE ASC, m.year ASC",
        list_where_with_fts(terms.len(), None)
    );
    let gate = kw.map(|k| k.to_string());
    let mut stmt = conn.prepare(&sql).unwrap();
    let mut bound: Vec<Box<dyn rusqlite::ToSql>> = vec![
        Box::new("all".to_string()),
        Box::new(None::<i64>),
        Box::new(None::<i64>),
        Box::new(gate),
        Box::new(None::<i64>),
    ];
    for t in &terms {
        bound.push(Box::new(t.clone()));
    }
    let rows = stmt
        .query_map(
            rusqlite::params_from_iter(bound.iter().map(|b| b.as_ref())),
            map_item,
        )
        .unwrap();
    rows.map(|r| r.unwrap().title).collect()
}

fn seed_search() -> Connection {
    let conn = memory_db();
    for title in ["100% 好剧", "1000部合集", "A_B 档案", "AXB 档案"] {
        conn.execute(
            "INSERT INTO media (type,title,added_at,updated_at) \
             VALUES ('movie',?1,1,1)",
            params![title],
        )
        .unwrap();
    }
    conn
}

/// LIKE 元字符必须按字面转义；先转义 `\` 自身，顺序不能换。
///
/// 实现在 `store::search`（命令层不再自己维护一份，避免两处口径漂移）。
#[test]
fn escape_like_quotes_wildcards() {
    assert_eq!(search::escape_like("100%"), "100\\%");
    assert_eq!(search::escape_like("A_B"), "A\\_B");
    assert_eq!(search::escape_like("a\\b"), "a\\\\b");
    assert_eq!(search::escape_like("50%_"), "50\\%\\_");
    assert_eq!(search::escape_like("普通词"), "普通词");
    assert_eq!(search::escape_like(""), "");
}

/// 修复核心：`100%` / `A_B` 里的 `%` `_` 不能被当成通配符——
/// 否则「100%」会连带命中「1000部合集」（多匹配），「A_B」会命中「AXB」。
#[test]
fn keyword_search_treats_wildcards_as_literals() {
    let conn = seed_search();
    assert_eq!(
        query_keyword(&conn, Some("100%")),
        vec!["100% 好剧".to_string()],
        "百分号必须按字面匹配"
    );
    assert_eq!(
        query_keyword(&conn, Some("A_B")),
        vec!["A_B 档案".to_string()],
        "下划线必须按字面匹配"
    );
}

/// 空关键词不过滤；普通关键词的多结果匹配必须完整（一个都不能漏）。
#[test]
fn keyword_search_empty_and_multi_results() {
    let conn = seed_search();
    assert_eq!(query_keyword(&conn, None).len(), 4, "无关键词 = 不过滤");
    assert_eq!(
        query_keyword(&conn, Some("")).len(),
        4,
        "空串被 SQL 的 `?4 = ''` 短路，同样不过滤"
    );
    assert_eq!(
        query_keyword(&conn, Some("档案")),
        // NOCASE 把大写折叠成小写比较：'_'(0x5F) < 'x'(0x78)
        vec!["A_B 档案".to_string(), "AXB 档案".to_string()],
        "两个结果都要命中：{:#?}",
        query_keyword(&conn, Some("档案"))
    );
    // 大小写：SQLite LIKE 对 ASCII 默认不区分大小写
    assert_eq!(query_keyword(&conn, Some("a_b")), vec!["A_B 档案".to_string()]);
}

/// 原名（original_title）与目录名（group_key）都要能命中——
/// 前端列表检索已经把它们纳入 haystack，服务端口径必须一致。
#[test]
fn keyword_search_matches_original_title_and_group_key() {
    let conn = memory_db();
    conn.execute(
        "INSERT INTO media (type,title,original_title,group_key,added_at,updated_at) \
         VALUES ('movie','流浪地球','The Wandering Earth','流浪地球 (2019)',1,1)",
        [],
    )
    .unwrap();
    assert_eq!(query_keyword(&conn, Some("wandering")), vec!["流浪地球".to_string()]);
    assert_eq!(query_keyword(&conn, Some("2019)")), vec!["流浪地球".to_string()]);
}

/// 批量删除只应删掉名单里的条目，其他条目一动不动。
#[test]
fn delete_media_batch_removes_only_requested_rows() {
    let conn = memory_db();
    seed(&conn);
    let ids: Vec<i64> = conn
        .prepare("SELECT id FROM media ORDER BY id")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    assert_eq!(ids.len(), 2);

    let n = delete_rows(&conn, "media", &[ids[0]]).unwrap();
    assert_eq!(n, 1);
    let left: i64 = conn
        .query_row("SELECT COUNT(*) FROM media", [], |r| r.get(0))
        .unwrap();
    assert_eq!(left, 1);
    let alive: i64 = conn
        .query_row("SELECT COUNT(*) FROM media WHERE id = ?1", params![ids[1]], |r| r.get(0))
        .unwrap();
    assert_eq!(alive, 1, "没被选中的条目必须还在");

    assert_eq!(delete_rows(&conn, "media", &[]).unwrap(), 0, "空名单是空操作");
    assert_eq!(delete_rows(&conn, "media", &[9999]).unwrap(), 0);
}

/// 删单集：只动指定的单集，同剧目其余单集和其他剧目都不受影响。
#[test]
fn delete_episodes_touches_only_requested_rows() {
    let conn = memory_db();
    seed(&conn);
    let sid: i64 = conn
        .query_row("SELECT id FROM media WHERE type='tv'", [], |r| r.get(0))
        .unwrap();
    // 再来一部剧，确保删 A 不影响 B
    conn.execute(
        "INSERT INTO media (type,title,added_at,updated_at) VALUES ('tv','Other',1,1)",
        [],
    )
    .unwrap();
    let other = conn.last_insert_rowid();
    conn.execute(
        "INSERT INTO episodes (media_id,season,episode,file_path,file_name,added_at) \
         VALUES (?1,1,1,'D:\\\\o.mkv','o.mkv',1)",
        params![other],
    )
    .unwrap();

    let ep: i64 = conn
        .query_row(
            "SELECT id FROM episodes WHERE media_id = ?1 ORDER BY episode LIMIT 1",
            params![sid],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(delete_rows(&conn, "episodes", &[ep]).unwrap(), 1);

    let mine: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM episodes WHERE media_id = ?1",
            params![sid],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(mine, 2, "同一剧目剩下的单集应保留");
    let theirs: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM episodes WHERE media_id = ?1",
            params![other],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(theirs, 1, "其他剧目的资源不能被牵连");
}

/// 从文件夹导入：新文件入库、重复文件自动跳过、并挂到指定栏目。
#[test]
fn folder_import_dedupes_and_links_category() {
    let dir = std::env::temp_dir().join(format!("mm-import-{}-{}", std::process::id(), now_ns()));
    std::fs::create_dir_all(&dir).unwrap();
    let touch = |rel: &str| {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, b"dummy").unwrap();
    };
    touch("Movie.2020.1080p.mkv");
    touch("Show/Show.S01E01.mkv");

    let mut conn = memory_db();
    conn.execute(
        "INSERT INTO categories (name, kind, origin, sort_order, created_at) \
         VALUES ('我的栏目','manual','manual',0,1)",
        [],
    )
    .unwrap();
    let cid: i64 = conn
        .query_row("SELECT id FROM categories LIMIT 1", [], |r| r.get(0))
        .unwrap();

    let root = dir.to_string_lossy().to_string();
    let first = import_folder_sync(&mut conn, &root, Some(cid)).unwrap();
    assert_eq!(first.added, 2, "首次导入应新增两个文件：{first:?}");
    assert_eq!(first.skipped, 0);
    assert_eq!(first.linked, 2, "电影与剧集都应挂到栏目下：{first:?}");
    assert!(first.message.contains("新增 2"), "{}", first.message);

    let second = import_folder_sync(&mut conn, &root, Some(cid)).unwrap();
    assert_eq!(second.added, 0, "重复导入不应重复入库：{second:?}");
    assert_eq!(second.skipped, 2, "两个文件都应被识别为已存在：{second:?}");
    assert!(second.message.contains("跳过 2"), "{}", second.message);

    let media_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM media", [], |r| r.get(0))
        .unwrap();
    assert_eq!(media_count, 2, "重复项不应产生第二条记录");

    assert!(import_folder_sync(&mut conn, "Z:\\not\\here", None).is_err());
    std::fs::remove_dir_all(&dir).ok();
}

/**
 * 判定不出来的文件同样要算「导入成功」。
 *
 * 回归：`added` 只累加 `added_movies + added_episodes` 时，导入一个既不像
 * 电影也不像剧集的文件会报「新增 0 个」，而库里其实多了一条待确认记录。
 */
#[test]
fn folder_import_counts_unknown_entries() {
    let dir = std::env::temp_dir().join(format!("mm-unk-{}-{}", std::process::id(), now_ns()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("一团乱麻.mkv"), b"dummy").unwrap();

    let mut conn = memory_db();
    let r = import_folder_sync(&mut conn, &dir.to_string_lossy(), None).unwrap();
    assert_eq!(r.added, 1, "待确认的条目也是入库了：{r:?}");
    assert!(r.message.contains("待确认"), "{}", r.message);
    assert_eq!(
        count(&conn, "SELECT COUNT(*) FROM media WHERE type = 'unknown'"),
        1
    );

    // 二次导入同样按重复跳过
    let again = import_folder_sync(&mut conn, &dir.to_string_lossy(), None).unwrap();
    assert_eq!(again.added, 0);
    assert_eq!(again.skipped, 1);
    std::fs::remove_dir_all(&dir).ok();
}

/// 目录下没有视频时的反馈要明确，不能静默成功。
#[test]
fn folder_import_reports_empty_folder() {
    let dir = std::env::temp_dir().join(format!("mm-empty-{}-{}", std::process::id(), now_ns()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("readme.txt"), b"hi").unwrap();
    let mut conn = memory_db();
    let r = import_folder_sync(&mut conn, &dir.to_string_lossy(), None).unwrap();
    assert_eq!(r.added, 0);
    assert!(r.message.contains("没有找到视频文件"), "{}", r.message);
    std::fs::remove_dir_all(&dir).ok();
}

fn now_ns() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

#[test]
fn create_media_supports_both_types_and_validates() {
    let conn = memory_db();
    let mid = insert_manual_media(&conn, "  沙丘  ", "movie", Some(2021), None, None, 1).unwrap();
    let tid = insert_manual_media(&conn, "绝命毒师", "tv", Some(2008), None, None, 1).unwrap();
    assert!(mid > 0 && tid > 0);
    assert_eq!(query_items(&conn, "movie").len(), 1);
    assert_eq!(query_items(&conn, "tv").len(), 1);
    // trim 生效
    let t: String = conn
        .query_row("SELECT title FROM media WHERE id = ?1", params![mid], |r| r.get(0))
        .unwrap();
    assert_eq!(t, "沙丘");
    // 手动建的条目 source 是 manual，且与扫描条目同表
    let src: String = conn
        .query_row("SELECT source FROM media WHERE id = ?1", params![mid], |r| r.get(0))
        .unwrap();
    assert_eq!(src, "manual");

    assert!(insert_manual_media(&conn, "   ", "movie", None, None, None, 1).is_err());
    assert!(insert_manual_media(&conn, "x", "anime", None, None, None, 1).is_err());
}

#[test]
fn update_media_renames_locks_and_mirrors_original_title() {
    let conn = memory_db();
    // 未被刮削改写的条目：original_title == title，改名时跟着走
    let id = insert_manual_media(&conn, "旧名", "movie", Some(2000), None, None, 1).unwrap();
    update_media_fields(
        &conn, id, "新名", "tv", Some(2001),
        Some("剧情".to_string()), Some("简介".to_string()),
    )
    .unwrap();
    let row: (String, String, String, i32, String, i64) = conn
        .query_row(
            "SELECT title, original_title, type, year, genre, locked FROM media WHERE id = ?1",
            params![id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
        )
        .unwrap();
    assert_eq!(row.0, "新名");
    assert_eq!(row.1, "新名", "没被刮削改写的条目 original_title 应跟随改名");
    assert_eq!(row.2, "tv");
    assert_eq!(row.4, "剧情");
    assert_eq!(row.5, 1, "手动编辑后应锁定，防止二次校正改动");

    // 被刮削改写过（original_title != title）的条目：改名不破坏原名
    conn.execute(
        "UPDATE media SET title = 'Scraped', original_title = 'Raw.Name.2020' WHERE id = ?1",
        params![id],
    )
    .unwrap();
    update_media_fields(&conn, id, "用户改的名", "tv", None, None, None).unwrap();
    let orig: String = conn
        .query_row("SELECT original_title FROM media WHERE id = ?1", params![id], |r| r.get(0))
        .unwrap();
    assert_eq!(orig, "Raw.Name.2020", "刮削改写过的 original_title 必须保留");

    assert!(update_media_fields(&conn, id, " ", "tv", None, None, None).is_err());
    assert!(update_media_fields(&conn, id, "x", "anime", None, None, None).is_err());
    assert!(update_media_fields(&conn, 9999, "x", "tv", None, None, None).is_err());
}

#[test]
fn episode_count_is_joined_for_series() {
    let conn = memory_db();
    seed(&conn);
    let tv = query_items(&conn, "tv");
    assert_eq!(tv[0].episode_count, 3);
    let movie = query_items(&conn, "movie");
    assert_eq!(movie[0].episode_count, 0, "电影不应有单集");
}

#[test]
fn episodes_are_ordered_by_season_then_episode() {
    let conn = memory_db();
    seed(&conn);
    let sid: i64 = conn
        .query_row("SELECT id FROM media WHERE type='tv'", [], |r| r.get(0))
        .unwrap();
    let eps = query_episodes(&conn, sid).unwrap();
    let pairs: Vec<(i32, i32)> = eps.iter().map(|e| (e.season, e.episode)).collect();
    assert_eq!(pairs, vec![(1, 1), (1, 2), (2, 1)]);
}

#[test]
fn foreign_key_cascade_removes_episodes() {
    let conn = memory_db();
    seed(&conn);
    let sid: i64 = conn
        .query_row("SELECT id FROM media WHERE type='tv'", [], |r| r.get(0))
        .unwrap();
    conn.execute("DELETE FROM media WHERE id = ?1", params![sid]).unwrap();
    let left: i64 = conn
        .query_row("SELECT COUNT(*) FROM episodes", [], |r| r.get(0))
        .unwrap();
    assert_eq!(left, 0, "删除剧集主记录应级联删除单集");
}

#[test]
fn item_serializes_type_as_type() {
    let conn = memory_db();
    seed(&conn);
    let items = query_items(&conn, "movie");
    let json = serde_json::to_value(&items[0]).unwrap();
    assert!(json.get("type").is_some());
    assert_eq!(json["type"], "movie");
    assert!(json.get("media_type").is_none());
}

/* ---------------- sort_order 贯通（聚合成员 ↑↓ 的判据） ---------------- */

/// `sort_order` 必须真的从库里送到列表项里。
///
/// 详情页「聚合成员」的 ↑ / ↓ 拿它算同级下标；少了这一列，前端只能拿
/// **列表先后**顶替，而列表先后是用户选的展示排序（默认按拼音名）——
/// 两个序不是一回事，后端按库内序插到 `index` 位算出的新序与旧序完全相同，
/// 用户看到的就是「点了按钮没反应」。
///
/// 测试数据刻意让标题升序与 `sort_order` 升序**完全相反**：
/// 漏接列、接错位、拿列表下标冒充，这三种错法都过不去。
#[test]
fn list_item_carries_sort_order_from_db() {
    let conn = memory_db();
    let parent = conn
        .query_row(
            "INSERT INTO media (type,title,parent_id,sort_order,added_at,updated_at) \
             VALUES ('tv','P',NULL,0,1,1) RETURNING id",
            [],
            |r| r.get::<_, i64>(0),
        )
        .unwrap();
    // 成员：标题升序 aaa < bbb < ccc，sort_order 恰好 2 > 1 > 0
    for (title, order) in [("aaa", 2i64), ("bbb", 1), ("ccc", 0)] {
        conn.execute(
            "INSERT INTO media (type,title,parent_id,sort_order,added_at,updated_at) \
             VALUES ('tv',?1,?2,?3,1,1)",
            params![title, parent, order],
        )
        .unwrap();
    }

    let items = query_items(&conn, "all");
    // 列表按标题排（`COLLATE NOCASE` 只折叠 ASCII 大小写：aaa < bbb < ccc < P）
    assert_eq!(
        items.iter().map(|i| i.title.as_str()).collect::<Vec<_>>(),
        vec!["aaa", "bbb", "ccc", "P"]
    );

    // 逐条与库内对账：这是「送到前端的就是库里的值」最硬的判据
    for it in &items {
        let in_db: i64 = conn
            .query_row(
                "SELECT COALESCE(sort_order,0) FROM media WHERE id = ?1",
                params![it.id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(it.sort_order, in_db, "{} 的 sort_order 与库内不一致", it.title);
    }

    // 成员按列表先后读出来的次序是 [2,1,0]，与 sort_order 升序 [0,1,2] 相反：
    // 这就是「列表先后 ≠ 同级次序」，也证明上面的断言真的能区分两者。
    let members: Vec<(String, i64)> = items
        .iter()
        .filter(|i| i.parent_id == Some(parent))
        .map(|i| (i.title.clone(), i.sort_order))
        .collect();
    assert_eq!(
        members,
        vec![("aaa".into(), 2), ("bbb".into(), 1), ("ccc".into(), 0)]
    );

    // 新列追加在末尾，不能把 append 之前的下标顶偏：父子的 parent_id / child_count 都要对
    let p = items.iter().find(|i| i.id == parent).unwrap();
    assert_eq!(p.child_count, 3, "child_count 被读成了别的列（列序错位）");
    assert_eq!(p.parent_id, None);
    for title in ["aaa", "bbb", "ccc"] {
        let m = items.iter().find(|i| i.title == title).unwrap();
        assert_eq!(m.parent_id, Some(parent), "{title} 的 parent_id 错位");
        assert_eq!(m.child_count, 0, "{title} 的 child_count 错位");
    }
}

/// 老库的 `sort_order` 列可能是**可空**的（历史上那一版是 `INTEGER`，没有
/// `NOT NULL`）。这类行读出来必须是 0，前端才不至于把「没有次序」当成
/// `undefined` / `NaN` 再算下标。
///
/// 手搓一份老库再跑 `migrate`：`ensure_column` 见列已存在会跳过，
/// 所以这份可空形态会被原样保留，`COALESCE` 是唯一兜底。
#[test]
fn list_item_treats_null_sort_order_as_zero() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE media (
             id INTEGER PRIMARY KEY AUTOINCREMENT, type TEXT NOT NULL DEFAULT 'movie',
             title TEXT NOT NULL, original_title TEXT, group_key TEXT, year INTEGER,
             tmdb_id INTEGER, poster_path TEXT, backdrop_path TEXT, overview TEXT,
             vote_average REAL, genre TEXT, file_path TEXT, file_name TEXT,
             file_size INTEGER, duration INTEGER, added_at INTEGER NOT NULL,
             updated_at INTEGER NOT NULL, parent_id INTEGER, sort_order INTEGER);
         INSERT INTO media (type,title,sort_order,added_at,updated_at)
             VALUES ('tv','老数据',NULL,1,1);",
    )
    .unwrap();
    db::migrate(&conn).unwrap();

    // 先确认这次真的造出了 NULL（不是被 migrate 顺手补成 0）
    let raw: Option<i64> = conn
        .query_row("SELECT sort_order FROM media WHERE id = 1", [], |r| r.get(0))
        .unwrap();
    assert_eq!(raw, None, "测试前提：库里确实有一行 sort_order IS NULL");

    let items = query_items(&conn, "all");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].sort_order, 0, "NULL 必须兜成 0，不能是 NaN / 缺字段");
}

/// 前端 `MediaItem.sort_order` 直接吃这个 JSON 键名。
///
/// `MediaListItem` **没有** `#[serde(rename_all = "camelCase")]`（只有 `type`
/// 一处 `rename`），所以发出去的是 snake_case：`sort_order` / `parent_id` /
/// `child_count`。这条把契约钉死——谁哪天给结构体加上 camelCase，
/// 前端会静默读到 `undefined`，而不是报错。
#[test]
fn item_serializes_sort_order_in_snake_case() {
    let conn = memory_db();
    conn.execute(
        "INSERT INTO media (type,title,parent_id,sort_order,added_at,updated_at) \
         VALUES ('tv','成员',NULL,7,1,1)",
        [],
    )
    .unwrap();

    let items = query_items(&conn, "all");
    let json = serde_json::to_value(&items[0]).unwrap();
    assert_eq!(json["sort_order"], 7);
    assert!(json.get("sortOrder").is_none(), "没有 camelCase 重命名");
    // 同批 snake_case 字段一起钉住，避免「只把 sort_order 写错」
    assert!(json.get("parent_id").is_some());
    assert!(json.get("child_count").is_some());
}

/* ---------------- depth 贯通（前端放行判定的真相来源） ---------------- */

/// 列表项必须带**真实层级**（顶层 = 0），而且要用**全表**父链算。
///
/// 前端 `canDrop` 拿它做层数上限校验；前端自己数深度时 `byId` 只有当前
/// 筛选到的条目，父节点被筛掉就被当成"走到了根" → 深度偏小 → 界面画
/// 「可放置」并把请求发出去，后端却回「层级最多 4 层」。所以这一列必须由
/// 后端沿全表父链算出来，前端只负责用。
#[test]
fn list_item_carries_real_depth() {
    let conn = memory_db();
    // A(0) → B(1) → C(2) → D(3)
    let mut parent: Option<i64> = None;
    for title in ["A", "B", "C", "D"] {
        parent = Some(
            conn.query_row(
                "INSERT INTO media (type,title,parent_id,added_at,updated_at) \
                 VALUES ('tv',?1,?2,1,1) RETURNING id",
                params![title, parent],
                |r| r.get::<_, i64>(0),
            )
            .unwrap(),
        );
    }

    let items = query_items(&conn, "all");
    let depth_of = |t: &str| items.iter().find(|i| i.title == t).unwrap().depth;
    assert_eq!(
        (depth_of("A"), depth_of("B"), depth_of("C"), depth_of("D")),
        (0, 1, 2, 3),
        "depth 必须沿父链算到根，顶层为 0"
    );

    // 只取一条（父链不在结果集里）时深度**不许变小**：这正是审计复现里
    // 「截断列表说能放、完整列表说不能放」的根因所在。
    let only_d = query_item(&conn, items.iter().find(|i| i.title == "D").unwrap().id);
    assert_eq!(only_d.depth, 3, "只取 D 一条时 depth 仍须是 3（父链在别表行里）");
    // 键名契约：前端读的是 `it.depth`（snake_case，没有 camelCase 重命名）
    assert_eq!(serde_json::to_value(&only_d).unwrap()["depth"], 3);
}

/// 成环的老数据不能把列表 / 详情查询拖死：`depth_of` 靠 `seen` 就地停下。
#[test]
fn list_item_depth_survives_cyclic_parents() {
    let conn = memory_db();
    // 手搓成环：A.parent = B 且 B.parent = A
    let a = conn
        .query_row(
            "INSERT INTO media (type,title,added_at,updated_at) \
             VALUES ('tv','A',1,1) RETURNING id",
            [],
            |r| r.get::<_, i64>(0),
        )
        .unwrap();
    let b = conn
        .query_row(
            "INSERT INTO media (type,title,added_at,updated_at) \
             VALUES ('tv','B',1,1) RETURNING id",
            [],
            |r| r.get::<_, i64>(0),
        )
        .unwrap();
    conn.execute("UPDATE media SET parent_id = ?1 WHERE id = ?2", params![b, a]).unwrap();
    conn.execute("UPDATE media SET parent_id = ?1 WHERE id = ?2", params![a, b]).unwrap();

    let items = query_items(&conn, "all");
    assert_eq!(items.len(), 2);
    // 每条走两条边后被 `seen` 拦住：A→B→(A 已见) = 2
    for it in &items {
        assert_eq!(it.depth, 2, "{} 在环里必须拿到有限深度，且不能死循环", it.title);
    }
    // 详情单条路径同样不能死循环
    assert_eq!(query_item(&conn, a).depth, 2);
}

/// 同名剧集要给用户看得见的区分，否则封面墙上是两个一模一样的名字。
#[test]
fn display_title_disambiguates_same_titled_series() {
    let conn = memory_db();
    for key in ["Show A", "Show B"] {
        conn.execute(
            "INSERT INTO media (type,title,original_title,group_key,added_at,updated_at) \
             VALUES ('tv','Show','Show',?1,1,1)",
            params![key],
        )
        .unwrap();
    }
    let items = query_items(&conn, "tv");
    let titles: Vec<String> = items.iter().map(|i| i.display_title.clone()).collect();
    assert!(titles.contains(&"Show · Show A".to_string()), "{titles:?}");
    assert!(titles.contains(&"Show · Show B".to_string()), "{titles:?}");
}

#[test]
fn scan_control_begin_resets_cancel_flag() {
    let c = ScanControl::default();
    c.request_cancel();
    assert!(c.cancelled());
    c.begin();
    assert!(!c.cancelled(), "新一轮开始时应复位取消标志");
    assert!(c.running());
    c.finish();
    assert!(!c.running());
}

/// 遍历阶段的取消标志必须能被后台线程看到（这是「可取消」的前提）。
#[test]
fn scan_cancel_is_visible_to_worker_thread() {
    let c = std::sync::Arc::new(ScanControl::default());
    c.begin();
    let worker = c.clone();
    assert!(!worker.cancelled());
    c.request_cancel();
    assert!(worker.cancelled());
}

#[test]
fn scan_progress_tick_reports_phase_and_counts() {
    let p = ScanProgress::tick("遍历目录", 120, 0, 0, "D:\\TV\\Breaking Bad".to_string());
    assert!(p.running && !p.finished);
    assert_eq!(p.phase, "遍历目录");
    assert_eq!(p.found, 120);
    assert_eq!(p.current, "D:\\TV\\Breaking Bad");
    assert!(p.stats.is_none(), "进行中不应该带统计");
}

#[test]
fn scan_progress_settled_carries_stats_and_message() {
    let stats = ScanStats {
        scanned: 10,
        added_movies: 2,
        added_series: 1,
        added_episodes: 7,
        ..Default::default()
    };
    let p = ScanProgress::settled("完成", "扫描完成：发现 10 个视频".to_string(), false, Some(stats));
    assert!(p.finished && !p.running && !p.cancelled);
    assert_eq!(p.found, 10);
    assert_eq!(p.imported, 9, "新增电影 + 新增单集");
    assert!(p.stats.is_some());

    let cancelled = ScanProgress::settled("已取消", "已取消".to_string(), true, None);
    assert!(cancelled.cancelled);
    assert!(cancelled.stats.is_none());
}

#[test]
fn display_title_equals_title_when_unique() {
    let conn = memory_db();
    seed(&conn);
    let tv = query_items(&conn, "tv");
    assert_eq!(tv[0].display_title, tv[0].title);
    let movie = query_items(&conn, "movie");
    assert_eq!(movie[0].display_title, movie[0].title);
}

/* ------------------- 存量数据的重新识别（向后兼容） ------------------- */

fn insert_media(
    conn: &Connection,
    media_type: &str,
    title: &str,
    path: &str,
    extra: &[(&str, &str)],
) -> i64 {
    let mut sql = String::from(
        "INSERT INTO media (type, title, file_path, file_name, added_at, updated_at",
    );
    let mut values = vec![
        format!("'{media_type}'"),
        format!("'{title}'"),
        format!("'{path}'"),
        format!("'{}'", file_name_of(path)),
        "1".to_string(),
        "1".to_string(),
    ];
    for (col, val) in extra {
        sql.push_str(&format!(", {col}"));
        values.push(format!("'{val}'"));
    }
    sql.push_str(&format!(") VALUES ({})", values.join(",")));
    conn.execute(&sql, []).unwrap();
    conn.last_insert_rowid()
}

fn file_name_of(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default()
}

fn count(conn: &Connection, sql: &str) -> i64 {
    conn.query_row(sql, [], |r| r.get(0)).unwrap()
}

/// 老版本把带集号的剧集判成了电影：重跑之后它们应该搬进单集表，
/// 并挂到**同一个**剧集主记录下（不能一集一部剧）。
#[test]
fn reclassify_moves_misclassified_episodes_into_one_series() {
    let mut conn = memory_db();
    for n in ["S01E01", "S01E02"] {
        insert_media(
            &conn,
            "movie",
            "Breaking Bad",
            &format!("D:\\TV\\Breaking Bad\\Breaking.Bad.{n}.mkv"),
            &[],
        );
    }

    let r = reclassify_library(&mut conn, None).unwrap();
    assert_eq!(r.to_tv, 2, "两条都该修正成剧集");
    assert_eq!(r.changed, 2);
    assert_eq!(
        count(&conn, "SELECT COUNT(*) FROM media WHERE type = 'movie'"),
        0,
        "原来的电影行应该被搬走"
    );
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM media WHERE type = 'tv'"), 1);
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM episodes"), 2);

    let mut stmt = conn
        .prepare("SELECT season, episode FROM episodes ORDER BY episode")
        .unwrap();
    let got: Vec<(i32, i32)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    assert_eq!(got, vec![(1, 1), (1, 2)], "季集号必须按文件名还原");
}

/// 目录里全是编号文件（`01.mkv` / `02.mkv`）：靠编号共识认出这是剧集，
/// 分组键退回目录名——否则会拆成「01 剧」「02 剧」两部。
#[test]
fn reclassify_groups_numbered_files_by_folder() {
    let mut conn = memory_db();
    for n in ["01", "02"] {
        insert_media(&conn, "movie", n, &format!("D:\\TV\\大明王朝\\{n}.mkv"), &[]);
    }

    let r = reclassify_library(&mut conn, None).unwrap();
    assert_eq!(r.to_tv, 2);
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM media WHERE type = 'tv'"), 1);

    let title: String = conn
        .query_row("SELECT title FROM media WHERE type = 'tv'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(title, "大明王朝", "剧名应取目录名，而不是「01」");
    let eps: Vec<i32> = conn
        .prepare("SELECT episode FROM episodes ORDER BY episode")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    assert_eq!(eps, vec![1, 2]);
}

/// 证据不足时不能再静默归成电影，要标成「待确认」。
#[test]
fn reclassify_marks_undecidable_rows_as_unknown() {
    let mut conn = memory_db();
    insert_media(&conn, "movie", "一团乱麻", "D:\\影视\\一团乱麻.mkv", &[]);

    let r = reclassify_library(&mut conn, None).unwrap();
    assert_eq!(r.to_unknown, 1);
    assert_eq!(
        count(&conn, "SELECT COUNT(*) FROM media WHERE type = 'unknown'"),
        1
    );
    assert!(r.message.contains("待确认"));
}

/// 库里已存着年份（刮削写入的）时，条目仍然算「有年份的电影」，
/// 不会因为有年份就被翻成剧集。
#[test]
fn reclassify_keeps_movies_with_stored_year() {
    let mut conn = memory_db();
    insert_media(
        &conn,
        "unknown",
        "沙丘",
        "D:\\影视\\Dune.mkv",
        &[("year", "2021")],
    );

    let r = reclassify_library(&mut conn, None).unwrap();
    assert_eq!(r.to_movie, 1);
    assert_eq!(
        count(&conn, "SELECT COUNT(*) FROM media WHERE type = 'movie'"),
        1
    );
}

/// 用户锁定过的条目一律不动——手动整理过的分组不能被自动流程改写。
#[test]
fn reclassify_skips_locked_rows() {
    let mut conn = memory_db();
    insert_media(
        &conn,
        "movie",
        "Breaking Bad",
        "D:\\TV\\Breaking Bad\\Breaking.Bad.S01E01.mkv",
        &[("locked", "1")],
    );

    let r = reclassify_library(&mut conn, None).unwrap();
    assert_eq!(r.skipped_locked, 1);
    assert_eq!(r.changed, 0);
    assert_eq!(
        count(&conn, "SELECT COUNT(*) FROM media WHERE type = 'movie'"),
        1,
        "锁定的条目必须原样保留"
    );
}

/// 反向修正：整部「剧」只剩一集、而那一集判出来是电影时，搬回电影行。
#[test]
fn reclassify_turns_single_episode_series_back_into_movie() {
    let mut conn = memory_db();
    let sid = insert_media(&conn, "tv", "The Matrix", "", &[("year", "1999")]);
    conn.execute(
        "INSERT INTO episodes (media_id, season, episode, file_path, file_name, added_at) \
         VALUES (?1, 1, 1, 'D:\\影视\\The.Matrix.1999.mkv', 'The.Matrix.1999.mkv', 1)",
        params![sid],
    )
    .unwrap();

    let r = reclassify_library(&mut conn, None).unwrap();
    assert_eq!(r.to_movie, 1);
    // 空壳剧集与其下唯一的单集都被清掉，换成一条电影行
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM media WHERE type = 'tv'"), 0);
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM episodes"), 0);
    assert_eq!(
        count(&conn, "SELECT COUNT(*) FROM media WHERE type = 'movie'"),
        1
    );
}

/// 只重判指定的 id（界面上的「重新识别所选」）。
#[test]
fn reclassify_only_touches_requested_ids() {
    let mut conn = memory_db();
    let keep = insert_media(&conn, "movie", "沙丘", "D:\\影视\\Dune.2021.mkv", &[]);
    let fix = insert_media(
        &conn,
        "movie",
        "Breaking Bad",
        "D:\\TV\\Breaking Bad\\Breaking.Bad.S01E01.mkv",
        &[],
    );

    let r = reclassify_library(&mut conn, Some(&[fix])).unwrap();
    assert_eq!(r.scanned, 1, "只扫传入的这一条");
    assert_eq!(r.to_tv, 1);

    let alive: i64 = conn
        .query_row("SELECT COUNT(*) FROM media WHERE id = ?1", params![keep], |r| r.get(0))
        .unwrap();
    assert_eq!(alive, 1, "没被选中的条目不受影响");
    let moved: i64 = conn
        .query_row("SELECT COUNT(*) FROM media WHERE id = ?1", params![fix], |r| r.get(0))
        .unwrap();
    assert_eq!(moved, 0, "被修正的那条已搬进单集表");
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM episodes"), 1);
}

/// 幂等：修完再跑一次不应该再动任何东西（否则重扫会反复搬家）。
#[test]
fn reclassify_is_idempotent() {
    let mut conn = memory_db();
    for n in ["S01E01", "S01E02"] {
        insert_media(
            &conn,
            "movie",
            "Breaking Bad",
            &format!("D:\\TV\\Breaking Bad\\Breaking.Bad.{n}.mkv"),
            &[],
        );
    }
    reclassify_library(&mut conn, None).unwrap();
    let again = reclassify_library(&mut conn, None).unwrap();
    assert_eq!(again.changed, 0, "第二次不应该再有变化");
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM episodes"), 2);
}

/* ==================== 手建剧集接不进内容（回归） ==================== */
/* 场景：用户先「新建剧集 西游记」，再导入西游记内容，内容必须并进这一条。
   历史 bug 是它会另建一条剧集，且此后**永远**并不过来 —— 所以这里既要验
   "新导入能并进入"，也要验"已经坏掉的老数据重新导入能自愈"。 */

fn touch_files(dir: &std::path::Path, rels: &[String]) {
    for rel in rels {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, b"dummy").unwrap();
    }
}

fn scene_dir(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("mm-{tag}-{}-{}", std::process::id(), now_ns()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn row(conn: &Connection, id: i64) -> (String, String, Option<String>) {
    conn.query_row(
        "SELECT title, type, group_key FROM media WHERE id = ?1",
        params![id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )
    .unwrap()
}

fn episodes_of(conn: &Connection, media_id: i64) -> i64 {
    conn.query_row(
        "SELECT COUNT(*) FROM episodes WHERE media_id = ?1",
        params![media_id],
        |r| r.get(0),
    )
    .unwrap()
}

/// 核心回归：`西游记.1986.01.mkv` 解析出的标题是「西游记 01」（尾部编号不算集号），
/// 与手建的「西游记」**不完全相等**。归一化后两者相同，必须并入同一条。
#[test]
fn manual_series_receives_content_with_mismatched_title() {
    let dir = scene_dir("xyj-mismatch");
    touch_files(
        &dir,
        &(1..=25)
            .map(|n| format!("西游记/西游记.1986.{n:02}.mkv"))
            .collect::<Vec<_>>(),
    );

    let mut conn = memory_db();
    let ts = db::now_epoch();
    let manual = insert_manual_media(&conn, "西游记", "tv", None, None, None, ts).unwrap();
    assert_eq!(
        row(&conn, manual).2.as_deref(),
        Some("西游记"),
        "手建剧集要带上分组键，否则只能靠标题逐字比对"
    );

    let res = import_folder_sync(&mut conn, &dir.to_string_lossy(), None).unwrap();
    assert_eq!(res.added, 25, "25 个文件都应入库：{res:?}");

    let series: i64 = conn
        .query_row("SELECT COUNT(*) FROM media WHERE type='tv'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(series, 1, "只能有一条剧集，不能另建『西游记 01』");
    assert_eq!(episodes_of(&conn, manual), 25, "25 集都要挂到手建的剧集下");
    assert_eq!(row(&conn, manual).0, "西游记", "用户起的名字不能被改成『西游记 01』");

    std::fs::remove_dir_all(&dir).ok();
}

/// 多种命名形状都要能并进来（第N集 / EP / 季目录 / CCTV 后缀…）。
#[test]
fn manual_series_absorbs_common_naming_shapes() {
    let shapes: &[&str] = &[
        "西游记/西游记.S01E{n:02}.1080p.mkv",
        "西游记/西游记.E{n:02}.mkv",
        "西游记/西游记.第{n:02}集.1080p.mkv",
        "西游记/第一季/西游记.第{n:02}集.mkv",
        "西游记/西游记.{n:02}.mkv",
        "西游记/西游记.EP{n:02}.mkv",
        "西游记/西游记.1986.CCTV.{n:02}.mkv",
        "西游记/西游记.第{n:02}話.mkv",
    ];
    for shape in shapes {
        let dir = scene_dir("xyj-shape");
        touch_files(
            &dir,
            &(1..=25)
                .map(|n| shape.replace("{n:02}", &format!("{n:02}")))
                .collect::<Vec<_>>(),
        );
        let mut conn = memory_db();
        let ts = db::now_epoch();
        let manual = insert_manual_media(&conn, "西游记", "tv", None, None, None, ts).unwrap();
        import_folder_sync(&mut conn, &dir.to_string_lossy(), None).unwrap();

        assert_eq!(
            episodes_of(&conn, manual),
            25,
            "命名形状 {shape} 应全部并入手建剧集"
        );
        let series: i64 = conn
            .query_row("SELECT COUNT(*) FROM media WHERE type='tv'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(series, 1, "命名形状 {shape} 不该另建剧集");
        std::fs::remove_dir_all(&dir).ok();
    }
}

/// 存量修复：库里已经是「空的『西游记』 + 装着 25 集的『西游记 01』」，
/// 重新导入一次就应把单集改挂过去，并清掉那个空壳。
///
/// 构造方式：**先**导入（此时没有手建行，自然生成自动剧集，路径与扫描器一致），
/// **再**补一条 group_key 为空的手建行 —— 这正是老代码下的历史状态。
#[test]
fn broken_manual_series_is_repaired_on_reimport() {
    let dir = scene_dir("xyj-repair");
    touch_files(
        &dir,
        &(1..=25)
            .map(|n| format!("西游记/西游记.1986.{n:02}.mkv"))
            .collect::<Vec<_>>(),
    );

    let mut conn = memory_db();
    let root = dir.to_string_lossy().to_string();
    // 第一次导入：没有手建行，会生成自动剧集「西游记 01」
    import_folder_sync(&mut conn, &root, None).unwrap();
    let orphan: i64 = conn
        .query_row(
            "SELECT id FROM media WHERE type='tv' ORDER BY id LIMIT 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(episodes_of(&conn, orphan), 25, "先造出『单集挂在自动行上』的现场");

    // 再补一条手建行（group_key 为空 = 老代码建出来的样子）
    let ts = db::now_epoch();
    conn.execute(
        "INSERT INTO media (type,title,original_title,source,added_at,updated_at) \
         VALUES ('tv','西游记','西游记','manual',?1,?1)",
        params![ts],
    )
    .unwrap();
    let manual = conn.last_insert_rowid();
    assert_eq!(episodes_of(&conn, manual), 0);
    assert_eq!(episodes_of(&conn, orphan), 25);

    let res = import_folder_sync(&mut conn, &root, None).unwrap();

    assert_eq!(res.added, 0, "文件都已存在，不该重复入库：{res:?}");
    assert_eq!(res.skipped, 25, "25 个都该被识别为已存在：{res:?}");
    assert_eq!(episodes_of(&conn, manual), 25, "单集应改挂到手建剧集");
    assert_eq!(episodes_of(&conn, orphan), 0, "原父行不该再持有单集");
    let left: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM media WHERE id = ?1",
            params![orphan],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(left, 0, "被搬空的重复剧集行应被清掉");
    assert!(
        res.message.contains("改挂"),
        "提示要说清楚发生了什么：{}",
        res.message
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// 空壳清理的守卫：只要那行**有一点东西**（有元数据 / 被锁定 / 是手建的），
/// 就一律保留 —— 宁可留个多余条目，也不能误删用户数据。
#[test]
fn shell_cleanup_is_strictly_guarded() {
    let conn = memory_db();
    let ts = db::now_epoch();
    let mk = |extra_col: &str, extra_val: &str| -> i64 {
        let sql = format!(
            "INSERT INTO media (type,title,group_key,{extra_col},added_at,updated_at) \
             VALUES ('tv','X','gk','{extra_val}',?1,?1)"
        );
        conn.execute(&sql, params![ts]).unwrap();
        conn.last_insert_rowid()
    };
    let _plain = mk("source", "auto");
    let scraped = mk("poster_path", "p.jpg");
    let locked = mk("locked", "1");
    conn.execute(
        "INSERT INTO media (type,title,group_key,source,added_at,updated_at) \
         VALUES ('tv','Y','gk','manual',?1,?1)",
        params![ts],
    )
    .unwrap();
    let manual = conn.last_insert_rowid();
    // 给 plain 之外的三个各挂一集，证明"有单集就不删"
    for id in [scraped, locked, manual] {
        conn.execute(
            "INSERT INTO episodes (media_id,season,episode,file_path,added_at) \
             VALUES (?1,1,1,?2,?3)",
            params![id, format!("E{id}.mkv"), ts],
        )
        .unwrap();
    }

    let dropped = crate::core::scanner::drop_empty_series_shells(&conn, "gk").unwrap();
    assert_eq!(dropped, 1, "只有真正空壳的那一行该被删");
    let survivors: Vec<i64> = conn
        .prepare("SELECT id FROM media ORDER BY id")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    assert_eq!(survivors, vec![scraped, locked, manual], "其余行必须原样保留");
}

/// 年份守卫：手建剧集填了年份时，不同年份的同名内容不应被并进去。
///
/// 注意目录名故意与手建标题不同（`西游记 2010` vs `西游记`），这样
/// `group_key` 精确命中那条规则不生效，年份守卫才是唯一的判据。
/// （目录名**恰好**与手建标题相同时会走 `group_key` 精确命中——
/// 那是更强的归属证据，优先于年份提示。）
#[test]
fn manual_series_year_guard_blocks_conflicting_year() {
    let dir = scene_dir("xyj-year");
    touch_files(&dir, &["西游记 2010/西游记.2010.E01.mkv".to_string()]);

    let mut conn = memory_db();
    let ts = db::now_epoch();
    // 用户建的是 1986 版
    let manual =
        insert_manual_media(&conn, "西游记", "tv", Some(1986), None, None, ts).unwrap();
    import_folder_sync(&mut conn, &dir.to_string_lossy(), None).unwrap();

    assert_eq!(
        episodes_of(&conn, manual),
        0,
        "年份冲突（1986 vs 2010）时不该并进去"
    );
    // 它应该照常建自己的条目
    let others: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM media WHERE id <> ?1",
            params![manual],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(others, 1, "2010 版应另立一条");
    std::fs::remove_dir_all(&dir).ok();
}

/// 手建的电影不该被写分组键（电影没有"同一部剧聚合"的概念）。
#[test]
fn manual_movie_has_no_group_key() {
    let conn = memory_db();
    let id = insert_manual_media(&conn, "沙丘", "movie", Some(2021), None, None, 1).unwrap();
    let (_, kind, gk) = row(&conn, id);
    assert_eq!(kind, "movie");
    assert_eq!(gk, None, "电影不该有分组键");
}

/// 手建一个不相干的剧集，不该把别的剧的内容吸走。
#[test]
fn unrelated_manual_series_does_not_swallow_other_shows() {
    let dir = scene_dir("xyj-unrelated");
    touch_files(&dir, &["大明风华/大明风华.S01E01.mkv".to_string()]);

    let mut conn = memory_db();
    let ts = db::now_epoch();
    let manual = insert_manual_media(&conn, "西游记", "tv", None, None, None, ts).unwrap();
    import_folder_sync(&mut conn, &dir.to_string_lossy(), None).unwrap();

    assert_eq!(episodes_of(&conn, manual), 0, "不相干的剧集不该被吸进手建条目");
    let other: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM media WHERE title LIKE '大明风华%'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(other, 1, "它应该照常建自己的剧集");
    std::fs::remove_dir_all(&dir).ok();
}

/// 文件夹名与手建标题**不一致**时，只能靠 `find_manual_series` 的标题归一化命中
/// （分组键精确相等这条规则在这里失效）。这是用户真实场景里最常见的一种：
/// 文件放在「86版西游记」目录里，但手建的剧集叫「西游记」。
#[test]
fn manual_series_matches_by_title_when_folder_name_differs() {
    let shapes: &[&str] = &[
        "86版西游记/西游记.{n:02}.mkv",
        "86版西游记/西游记.1986.{n:02}.mkv",
        "老剧整理/西游记.第{n:02}集.mkv",
        // 目录名不同 + 文件名夹着电视台标记：噪声词抹掉后才对得上
        "老剧整理/西游记.1986.CCTV.{n:02}.mkv",
    ];
    for shape in shapes {
        let dir = scene_dir("xyj-diffdir");
        touch_files(
            &dir,
            &(1..=25)
                .map(|n| shape.replace("{n:02}", &format!("{n:02}")))
                .collect::<Vec<_>>(),
        );
        let mut conn = memory_db();
        let ts = db::now_epoch();
        let manual = insert_manual_media(&conn, "西游记", "tv", None, None, None, ts).unwrap();
        import_folder_sync(&mut conn, &dir.to_string_lossy(), None).unwrap();

        assert_eq!(
            episodes_of(&conn, manual),
            25,
            "目录名不同时 {shape} 也应并入手建剧集（靠标题归一化命中）"
        );
        let series: i64 = conn
            .query_row("SELECT COUNT(*) FROM media WHERE type='tv'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(series, 1, "{shape} 不该另建剧集");
        std::fs::remove_dir_all(&dir).ok();
    }
}


/* ---------------- 指定归属：推断失败时的兜底入口 ---------------- */

/// `86版西游记.01.mkv` 放在 `86版西游记` 目录里：文件名是缩写/版本前缀，
/// 目录名也不是剧名 → 两条推断规则都失效，正常导入必然另建一条剧集。
/// 用 `import_folder_into_sync` 显式指定归属后必须全部进手建的那条。
#[test]
fn explicit_target_attaches_files_that_cannot_be_inferred() {
    let dir = scene_dir("explicit");
    touch_files(
        &dir,
        &(1..=25)
            .map(|n| format!("86版西游记/86版西游记.{n:02}.mkv"))
            .collect::<Vec<_>>(),
    );

    let mut conn = memory_db();
    let ts = db::now_epoch();
    let manual = insert_manual_media(&conn, "西游记", "tv", None, None, None, ts).unwrap();

    // 先确认推断路径确实做不到（否则这条用例就没有意义了）
    let plain = import_folder_sync(&mut conn, &dir.to_string_lossy(), None).unwrap();
    assert_eq!(
        episodes_of(&conn, manual),
        0,
        "推断路径下这条形状本来就该接不进去（这是本功能存在的理由）"
    );
    assert!(plain.added > 0, "但它确实入库了：{plain:?}");

    // 显式指定后必须全部归位
    let res = import_folder_into_sync(&mut conn, manual, &dir.to_string_lossy(), None).unwrap();
    assert_eq!(
        episodes_of(&conn, manual),
        25,
        "指定归属后 25 集都要挂到手建剧集下：{res:?}"
    );
    assert!(
        res.message.contains("归入「西游记」"),
        "提示语要说清楚归到哪一部了，实际：{}",
        res.message
    );
    let series: i64 = conn
        .query_row("SELECT COUNT(*) FROM media WHERE type='tv'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(series, 1, "不该残留被腾空的剧集：{res:?}");

    std::fs::remove_dir_all(&dir).ok();
}

/// 指定归属只能用于剧集；往电影里塞单集没有意义，要明确报错而不是静默吞掉。
#[test]
fn explicit_target_rejects_non_tv_and_bad_path() {
    let mut conn = memory_db();
    let ts = db::now_epoch();
    let movie = insert_manual_media(&conn, "沙丘", "movie", None, None, None, ts).unwrap();

    let err = import_folder_into_sync(&mut conn, movie, &scene_dir("x").to_string_lossy(), None)
        .unwrap_err();
    assert!(err.contains("不是剧集"), "实际错误：{err}");

    let tv = insert_manual_media(&conn, "西游记", "tv", None, None, None, ts).unwrap();
    let err = import_folder_into_sync(&mut conn, tv, "D://绝不存在//的路径", None).unwrap_err();
    assert!(err.contains("目录不存在"), "实际错误：{err}");

    let err = import_folder_into_sync(&mut conn, 9999, "D://绝不存在//的路径", None).unwrap_err();
    assert!(err.contains("找不到"), "实际错误：{err}");
}

/// 目录里没有视频时要给出明确的空结果，不能报"添加成功 0 集"让人以为坏了。
#[test]
fn explicit_target_reports_empty_folder() {
    let dir = scene_dir("empty");
    std::fs::create_dir_all(dir.join("子目录")).unwrap();
    std::fs::write(dir.join("说明.txt"), b"not a video").unwrap();

    let mut conn = memory_db();
    let ts = db::now_epoch();
    let tv = insert_manual_media(&conn, "西游记", "tv", None, None, None, ts).unwrap();
    let res = import_folder_into_sync(&mut conn, tv, &dir.to_string_lossy(), None).unwrap();
    assert_eq!(res.added, 0);
    assert!(res.message.contains("没有找到视频文件"), "实际：{}", res.message);

    std::fs::remove_dir_all(&dir).ok();
}

/// 用户报告的真实文件名：`西游记1986-2000版.01.猴王初问世.mp4`
///
/// 之前 `.01.` 这种「点分裸集号」压根解析不出集号 → 判成「待确认」的独立条目，
/// 挂在剧集之外。现在要能自动落进手建的「西游记」。
#[test]
fn real_user_filename_lands_in_manual_series() {
    let dir = scene_dir("xyj-real-name");
    touch_files(
        &dir,
        &[
            "西游记/西游记1986-2000版.01.猴王初问世.mp4".to_string(),
            "西游记/西游记1986-2000版.02.官封弼马温.mp4".to_string(),
            "西游记/西游记1986-2000版.03.大闹天宫.mp4".to_string(),
        ],
    );
    let mut conn = memory_db();
    let ts = db::now_epoch();
    let manual = insert_manual_media(&conn, "西游记", "tv", None, None, None, ts).unwrap();
    import_folder_sync(&mut conn, &dir.to_string_lossy(), None).unwrap();

    assert_eq!(episodes_of(&conn, manual), 3, "三集都该并入手建剧集");
    let series: i64 = conn
        .query_row("SELECT COUNT(*) FROM media WHERE type='tv'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(series, 1, "不该另建剧集");
    let orphan: i64 = conn
        .query_row("SELECT COUNT(*) FROM media WHERE type<>'tv'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(orphan, 0, "不该留下散落的待确认条目");
    std::fs::remove_dir_all(&dir).ok();
}

/// 「添加进本剧集」要能收编**完全没有集号**的散落条目。
///
/// 用户已经明确指定了归属，归属不该再由文件名猜：文件名认不出集号时，
/// 原来只会回一句「没有可归入…的单集」，功能等于失效。
#[test]
fn add_to_series_adopts_orphan_entries_without_episode_number() {
    let dir = scene_dir("xyj-adopt");
    touch_files(&dir, &["西游记/猴王初问世.1080p.mkv".to_string()]);
    let mut conn = memory_db();
    let ts = db::now_epoch();
    let manual = insert_manual_media(&conn, "西游记", "tv", None, None, None, ts).unwrap();

    // 先按普通扫描入库：认不出集号 → 一条散落的独立条目
    import_folder_sync(&mut conn, &dir.to_string_lossy(), None).unwrap();
    assert_eq!(episodes_of(&conn, manual), 0, "扫描阶段确实认不出来");
    let orphan: i64 = conn
        .query_row("SELECT COUNT(*) FROM media WHERE type<>'tv'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(orphan, 1, "现场要有一条散落条目");

    let res = import_folder_into_sync(&mut conn, manual, &dir.to_string_lossy(), None).unwrap();
    assert_eq!(res.adopted, 1, "应收编 1 条：{:?}", res.message);
    assert_eq!(episodes_of(&conn, manual), 1, "收编后剧集要有 1 集");
    let orphan: i64 = conn
        .query_row("SELECT COUNT(*) FROM media WHERE type<>'tv'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(orphan, 0, "原来的散落条目要被清掉，不能留双份");
    assert!(res.message.contains("归入"), "提示语要说明结果：{}", res.message);
    std::fs::remove_dir_all(&dir).ok();
}

/* -------------------- R1 SourceHint 接线（M1） -------------------- */

/// 测试用的假路径（带反斜杠，跟真实 Windows 路径同形）。
/// 用 `lazy_static` 式的一次性初始化，避免在字符串字面量里写一堆转义。
static FAKE_PATH: std::sync::LazyLock<String> =
    std::sync::LazyLock::new(|| format!("D:{}剧{}01.mkv", std::path::MAIN_SEPARATOR, std::path::MAIN_SEPARATOR));

fn add_media(conn: &Connection, kind: &str, title: &str) -> i64 {
    conn.execute(
        "INSERT INTO media (type,title,file_path,file_name,added_at,updated_at) \
         VALUES (?1,?2,'D:/x.mkv','x.mkv',1,1)",
        params![kind, title],
    )
    .unwrap();
    conn.last_insert_rowid()
}

fn kind_of(conn: &Connection, id: i64) -> String {
    conn.query_row("SELECT type FROM media WHERE id = ?1", params![id], |r| r.get(0))
        .unwrap()
}

/// gate ③：类型一样就不写库。
#[test]
fn source_hint_is_noop_when_type_already_matches() {
    let conn = memory_db();
    let id = add_media(&conn, "tv", "剧");
    assert!(!apply_source_hint(&conn, id, SourceHint::Tv).unwrap());
}

/// gate ④ 的核心：**禁止「剧集 → 电影」**。这是历史伤疤方向。
#[test]
fn source_hint_never_converts_tv_to_movie() {
    let conn = memory_db();
    let id = add_media(&conn, "tv", "被误判的剧");
    assert!(!apply_source_hint(&conn, id, SourceHint::Movie).unwrap());
    assert_eq!(kind_of(&conn, id), "tv", "剧集绝不能被数据源翻成电影");
}

/// 允许方向一：待确认 → 剧集。
#[test]
fn source_hint_converts_unknown_to_tv() {
    let conn = memory_db();
    let id = add_media(&conn, "unknown", "待确认");
    assert!(apply_source_hint(&conn, id, SourceHint::Tv).unwrap());
    assert_eq!(kind_of(&conn, id), "tv");
}

/// 允许方向二：电影 → 剧集（数据源说是剧集，那就是剧集）。
#[test]
fn source_hint_converts_movie_to_tv() {
    let conn = memory_db();
    let id = add_media(&conn, "movie", "其实是剧");
    assert!(apply_source_hint(&conn, id, SourceHint::Tv).unwrap());
    assert_eq!(kind_of(&conn, id), "tv");
}

/// 允许方向三：待确认 → 电影。
#[test]
fn source_hint_converts_unknown_to_movie() {
    let conn = memory_db();
    let id = add_media(&conn, "unknown", "待确认");
    assert!(apply_source_hint(&conn, id, SourceHint::Movie).unwrap());
    assert_eq!(kind_of(&conn, id), "movie");
}

/// gate ①：用户锁过的条目，数据源说了也不算。
#[test]
fn source_hint_respects_locked() {
    let conn = memory_db();
    let id = add_media(&conn, "unknown", "用户定的");
    conn.execute("UPDATE media SET locked = 1 WHERE id = ?1", params![id])
        .unwrap();
    assert!(!apply_source_hint(&conn, id, SourceHint::Tv).unwrap());
    assert_eq!(kind_of(&conn, id), "unknown");
}

/// gate ②：名下已经有单集的，说明它事实上就是剧集容器，不再改判。
#[test]
fn source_hint_skips_rows_that_already_have_episodes() {
    let conn = memory_db();
    let id = add_media(&conn, "tv", "有单集的剧");
    conn.execute(
        "INSERT INTO episodes (media_id,season,episode,added_at) VALUES (?1,1,1,1)",
        params![id],
    )
    .unwrap();
    assert!(!apply_source_hint(&conn, id, SourceHint::Movie).unwrap());
    assert!(!apply_source_hint(&conn, id, SourceHint::Tv).unwrap());
}

/// gate ⑤：改判要留痕，事后能查到是谁把什么改成了什么。
#[test]
fn source_hint_writes_audit_trail() {
    let conn = memory_db();
    let id = add_media(&conn, "unknown", "待确认");
    apply_source_hint(&conn, id, SourceHint::Tv).unwrap();
    let audit: Option<String> = conn
        .query_row("SELECT type_audit FROM media WHERE id = ?1", params![id], |r| r.get(0))
        .unwrap();
    assert_eq!(audit.as_deref(), Some("unknown → tv（数据源标识）"));
}

/// 产出点：有 tvdb_id 就等于是剧集（TheTVDB 只收剧集）。
#[test]
fn hint_from_row_derives_tv_from_tvdb_id() {
    assert_eq!(hint_from_row(Some(123)), Some(SourceHint::Tv));
    assert_eq!(hint_from_row(None), None, "没有明确来源就不猜");
}

/// 重新识别时要真的用上 R1：一条挂了 tvdb_id 的电影行应被改判成剧集。
///
/// 注意 `convert_to_episode` 会删掉原行、另建剧集容器并挂上单集，
/// 所以这里不能再用原 id 查类型——要验证的是「文件被搬进了某个剧集的单集里」。
#[test]
fn reclassify_uses_tvdb_id_as_source_hint() {
    let conn = &mut memory_db();
    let id = add_media(conn, "movie", "被误判成电影的剧");
    conn.execute(
        "UPDATE media SET tvdb_id = 999, file_path = ?1, file_name = '01.mkv' WHERE id = ?2",
        params![&*FAKE_PATH, id],
    )
    .unwrap();
    let out = reclassify_library(conn, None).unwrap();
    assert!(out.to_tv >= 1, "R1 应把这条改判成剧集：{out:?}");

    let kind: String = conn
        .query_row(
            "SELECT m.type FROM episodes e JOIN media m ON m.id = e.media_id \
             WHERE e.file_path = ?1",
            params![&*FAKE_PATH],
            |r| r.get(0),
        )
        .expect("这条文件应已被转成某剧集的单集");
    assert_eq!(kind, "tv", "承载它的容器必须是剧集");
}

/// `SourceHint::from_str` 的解析口径（认不出来就 None，绝不猜）。
#[test]
fn source_hint_from_str_is_conservative() {
    assert_eq!(SourceHint::from_str("tv"), Some(SourceHint::Tv));
    assert_eq!(SourceHint::from_str("TV"), Some(SourceHint::Tv));
    assert_eq!(SourceHint::from_str(" movie "), Some(SourceHint::Movie));
    assert_eq!(SourceHint::from_str("documentary"), None);
}

/* ====================== 删除链路 ====================== */

/// 造一条带齐关联数据的剧集：3 个单集 + 1 个标签 + 1 个分类 + 1 条观看状态。
fn seed_tv_with_relations(conn: &Connection) -> i64 {
    conn.execute(
        "INSERT INTO media (type,title,added_at,updated_at) VALUES ('tv','BB',1,1)",
        [],
    )
    .unwrap();
    let mid = conn.last_insert_rowid();
    let mut eps = Vec::new();
    for i in 1..=3 {
        conn.execute(
            "INSERT INTO episodes (media_id,season,episode,file_path,file_name,added_at) \
             VALUES (?1,1,?2,?3,?4,1)",
            params![mid, i, format!("D:\\\\e{i}.mkv"), format!("e{i}.mkv")],
        )
        .unwrap();
        eps.push(conn.last_insert_rowid());
    }
    conn.execute("INSERT INTO tags (name,created_at) VALUES ('t',1)", [])
        .unwrap();
    let tid = conn.last_insert_rowid();
    conn.execute(
        "INSERT INTO media_tags (media_id,tag_id,created_at) VALUES (?1,?2,1)",
        params![mid, tid],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO categories (name,kind,origin,sort_order,created_at) \
         VALUES ('c','manual','manual',0,1)",
        [],
    )
    .unwrap();
    let cid = conn.last_insert_rowid();
    conn.execute(
        "INSERT INTO media_categories (media_id,category_id,created_at) VALUES (?1,?2,1)",
        params![mid, cid],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO playback_state (media_id,watched,updated_at) VALUES (?1,1,1)",
        params![mid],
    )
    .unwrap();
    mid
}

/// 批量插一些电影（把检索索引撑大，太小的数据结构损坏复现不出来）。
fn seed_bulk(conn: &Connection, n: i64) {
    for i in 0..n {
        conn.execute(
            "INSERT INTO media (type,title,file_name,added_at,updated_at) \
             VALUES ('movie',?1,?2,1,1)",
            params![format!("Bulk {i}"), format!("bulk{i}.mkv")],
        )
        .unwrap();
    }
}

fn count_rows(conn: &Connection, sql: &str) -> i64 {
    conn.query_row(sql, [], |r| r.get(0)).unwrap()
}

/// 删一条剧集，它名下的**所有**关联数据必须一起清干净。
///
/// 单集 / 分类 / 标签 / 观看状态四张表都靠 `ON DELETE CASCADE` 兜底，
/// 而这依赖 `PRAGMA foreign_keys = ON` —— 这条用例就是盯着这个开关别被关掉。
#[test]
fn delete_media_cascades_to_every_related_table() {
    let mut conn = memory_db();
    let mid = seed_tv_with_relations(&conn);

    assert_eq!(crate::commands::tree::delete_nodes_sync(&mut conn, &[mid]).unwrap(), 1);

    assert_eq!(count_rows(&conn, "SELECT COUNT(*) FROM media WHERE id = 1"), 0);
    assert_eq!(count_rows(&conn, "SELECT COUNT(*) FROM episodes"), 0, "单集必须跟着删");
    assert_eq!(count_rows(&conn, "SELECT COUNT(*) FROM media_tags"), 0, "标签关联不能留孤儿");
    assert_eq!(count_rows(&conn, "SELECT COUNT(*) FROM media_categories"), 0);
    assert_eq!(count_rows(&conn, "SELECT COUNT(*) FROM playback_state"), 0);
    // 标签 / 分类本体是独立存在的，不该被连带删掉
    assert_eq!(count_rows(&conn, "SELECT COUNT(*) FROM tags"), 1);
    assert_eq!(count_rows(&conn, "SELECT COUNT(*) FROM categories"), 1);
}

/// 入参规范化：重复 id 只算一次，`id <= 0` 直接丢掉。
///
/// 界面按「返回条数」提示「已删除 N 条」，重复 id 会让这个数字虚高。
#[test]
fn delete_normalizes_duplicate_and_invalid_ids() {
    let mut conn = memory_db();
    seed(&conn);
    let ids: Vec<i64> = conn
        .prepare("SELECT id FROM media ORDER BY id")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();

    // 同一个 id 传三遍，只该删一行
    assert_eq!(
        crate::commands::tree::delete_nodes_sync(&mut conn, &[ids[0], ids[0], ids[0]]).unwrap(),
        1
    );
    // 混进 0 和负数：脏数据被丢掉，真实的那个照删
    assert_eq!(
        crate::commands::tree::delete_nodes_sync(&mut conn, &[ids[1], 0, -1]).unwrap(),
        1
    );
    assert_eq!(count_rows(&conn, "SELECT COUNT(*) FROM media"), 0);
}

/// 幂等：同一批 id 删第二次返回 0，不报错、也不动别的行。
#[test]
fn delete_is_idempotent() {
    let mut conn = memory_db();
    seed(&conn);
    let ids: Vec<i64> = conn
        .prepare("SELECT id FROM media ORDER BY id")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();

    assert_eq!(crate::commands::tree::delete_nodes_sync(&mut conn, &[ids[0]]).unwrap(), 1);
    assert_eq!(
        crate::commands::tree::delete_nodes_sync(&mut conn, &[ids[0]]).unwrap(),
        0,
        "删第二次必须是 0 而不是报错"
    );
    assert_eq!(crate::commands::tree::delete_nodes_sync(&mut conn, &[]).unwrap(), 0);
    // 没点名要删的那条还在
    assert_eq!(
        count_rows(&conn, &format!("SELECT COUNT(*) FROM media WHERE id = {}", ids[1])),
        1
    );
}

/// 部分 id 已经不在库里：返回**真实**条数，存在的那部分确实被删掉。
///
/// 界面靠这个数字区分「全删了」和「有几条早没了」，所以它必须准。
#[test]
fn delete_reports_real_count_when_some_ids_are_gone() {
    let mut conn = memory_db();
    seed(&conn);
    let ids: Vec<i64> = conn
        .prepare("SELECT id FROM media ORDER BY id")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();

    let n = crate::commands::tree::delete_nodes_sync(&mut conn, &[ids[0], 9999, 8888]).unwrap();
    assert_eq!(n, 1, "只有一条真的存在，就该只报 1");
    assert_eq!(
        count_rows(&conn, &format!("SELECT COUNT(*) FROM media WHERE id = {}", ids[1])),
        1,
        "名单外的条目绝不能被动"
    );
}

/// 父子一起删：孙子上提到顶层，不会变成孤儿。
#[test]
fn delete_parent_and_child_together_promotes_grandchild() {
    let mut conn = memory_db();
    let p = conn
        .query_row(
            "INSERT INTO media (type,title,parent_id,sort_order,added_at,updated_at) \
             VALUES ('tv','P',NULL,0,1,1) RETURNING id",
            [],
            |r| r.get::<_, i64>(0),
        )
        .unwrap();
    let c = conn
        .query_row(
            "INSERT INTO media (type,title,parent_id,sort_order,added_at,updated_at) \
             VALUES ('tv','C',?1,0,1,1) RETURNING id",
            params![p],
            |r| r.get::<_, i64>(0),
        )
        .unwrap();
    let g = conn
        .query_row(
            "INSERT INTO media (type,title,parent_id,sort_order,added_at,updated_at) \
             VALUES ('tv','G',?1,0,1,1) RETURNING id",
            params![c],
            |r| r.get::<_, i64>(0),
        )
        .unwrap();

    assert_eq!(crate::commands::tree::delete_nodes_sync(&mut conn, &[p, c]).unwrap(), 2);
    assert_eq!(count_rows(&conn, "SELECT COUNT(*) FROM media"), 1);
    let gp: Option<i64> = conn
        .query_row("SELECT parent_id FROM media WHERE id = ?1", params![g], |r| r.get(0))
        .unwrap();
    assert_eq!(gp, None, "孙辈要被上提到顶层，而不是指向已删的父节点");
}

/// `parent_id` 没有外键约束，存量库里万一有环（A 挂 B、B 又挂 A），
/// 找祖先那步不能转不出来 —— 这是个同步调用，卡住就是整个界面假死。
#[test]
fn delete_survives_parent_cycle() {
    let mut conn = memory_db();
    let a = conn
        .query_row(
            "INSERT INTO media (type,title,parent_id,added_at,updated_at) \
             VALUES ('tv','A',NULL,1,1) RETURNING id",
            [],
            |r| r.get::<_, i64>(0),
        )
        .unwrap();
    let b = conn
        .query_row(
            "INSERT INTO media (type,title,parent_id,added_at,updated_at) \
             VALUES ('tv','B',?1,1,1) RETURNING id",
            params![a],
            |r| r.get::<_, i64>(0),
        )
        .unwrap();
    // 手动造环：A 挂到 B 下
    conn.execute("UPDATE media SET parent_id = ?2 WHERE id = ?1", params![a, b])
        .unwrap();
    // 自己挂自己
    let c = conn
        .query_row(
            "INSERT INTO media (type,title,parent_id,added_at,updated_at) \
             VALUES ('tv','C',NULL,1,1) RETURNING id",
            [],
            |r| r.get::<_, i64>(0),
        )
        .unwrap();
    conn.execute("UPDATE media SET parent_id = ?2 WHERE id = ?1", params![c, c])
        .unwrap();

    let n = crate::commands::tree::delete_nodes_sync(&mut conn, &[a]).unwrap();
    assert_eq!(n, 1, "有环也必须正常删掉，不能卡住");
    assert_eq!(count_rows(&conn, "SELECT COUNT(*) FROM media"), 2);
    // A 被删后，B 的子级（如果有）要挂到顶层而不是留在环里
    let bp: Option<i64> = conn
        .query_row("SELECT parent_id FROM media WHERE id = ?1", params![b], |r| r.get(0))
        .unwrap();
    assert_eq!(bp, None, "撞上环时按「挂到顶层」处理");

    assert_eq!(crate::commands::tree::delete_nodes_sync(&mut conn, &[c]).unwrap(), 1);
}

/// 检索索引坏掉时，删除必须能**自愈**。
///
/// 真实故障（2026-10-05 用户库）：`media_fts` 内部结构损坏 →
/// `DELETE FROM media` 触发 `media_fts_ad` 从索引里删行 →
/// `database disk image is malformed` → 清空媒体库和批量删除一起失败，
/// 而查询照常工作，看起来就像代码坏了。
///
/// 最坑的是**查不出来**：`PRAGMA integrity_check` 报 ok，fts5 自己的
/// `integrity-check` 也过，只有真的删一行才暴露。所以只能靠
/// 「失败 → 重建索引 → 重试」这条自愈路径兜住。
///
/// ⚠️ 真实库那种内部不一致的形态依赖具体 SQLite 版本，**测试里凑不出来**
/// （在 3.53 上 `DELETE FROM media_fts_idx` 能凑出同样症状，rusqlite 编的
/// 这个版本打不坏）。所以这条用例两种情形都接住：真复现了就要求自愈成功，
/// 没复现就至少保证正常删除不受影响。判定与重建本身由下面两条单独钉住。
#[test]
fn delete_survives_a_corrupted_fts_index() {
    let mut conn = memory_db();
    // 没编 fts5 的 SQLite 根本没有索引表，那种库不存在这个故障
    if !crate::store::schema::fts_index_exists(&conn) {
        return;
    }
    seed(&conn);
    seed_bulk(&conn, 60);
    let ids: Vec<i64> = conn
        .prepare("SELECT id FROM media ORDER BY id")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();

    // 试着打坏 fts5 的 idx shadow 表
    conn.execute_batch("DELETE FROM media_fts_idx;").unwrap();
    let broken = conn.execute("DELETE FROM media WHERE id = ?1", params![ids[0]]).is_err();
    // 索引没打坏时，上面这句探测本身已经删掉了 ids[0]，正式验证换一条
    let mut removed: i64 = if broken { 0 } else { 1 };
    let target = if broken { ids[0] } else { ids[1] };

    let n = crate::commands::tree::delete_nodes_sync(&mut conn, &[target])
        .expect("无论索引坏没坏，删除都必须成功");
    assert_eq!(n, 1);
    removed += 1;
    assert_eq!(
        count_rows(&conn, &format!("SELECT COUNT(*) FROM media WHERE id = {target}")),
        0
    );
    // 没点名要删的一律还在
    assert_eq!(count_rows(&conn, "SELECT COUNT(*) FROM media"), ids.len() as i64 - removed);
}

/// 清空媒体库走同一条自愈路径。
#[test]
fn clear_library_survives_a_corrupted_fts_index() {
    let conn = memory_db();
    if !crate::store::schema::fts_index_exists(&conn) {
        return;
    }
    seed(&conn);
    seed_bulk(&conn, 60);
    conn.execute_batch("DELETE FROM media_fts_idx;").unwrap();

    clear_rows_healing(&conn).expect("无论索引坏没坏，清空都必须成功");
    assert_eq!(count_rows(&conn, "SELECT COUNT(*) FROM media"), 0);
    assert_eq!(count_rows(&conn, "SELECT COUNT(*) FROM episodes"), 0);
    assert_eq!(count_rows(&conn, "SELECT COUNT(*) FROM scan_history"), 0);
}

/// 自愈的两个零件分别钉住：认得准、修得动。
#[test]
fn heal_fts_only_fires_on_malformed_and_really_rebuilds() {
    let conn = memory_db();
    if !crate::store::schema::fts_index_exists(&conn) {
        return;
    }
    seed(&conn);
    seed_bulk(&conn, 20);
    // 先破坏索引，再确认 heal 会把它修回来（rebuild 后能正常删）
    conn.execute_batch("DELETE FROM media_fts_idx;").unwrap();
    assert!(
        crate::store::schema::heal_fts_if_malformed(&conn, "database disk image is malformed"),
        "结构损坏必须触发重建，且重建要成功"
    );
    let id: i64 = conn
        .query_row("SELECT id FROM media ORDER BY id LIMIT 1", [], |r| r.get(0))
        .unwrap();
    assert!(conn.execute("DELETE FROM media WHERE id = ?1", params![id]).is_ok());

    // 别的错误不能误触发重建
    assert!(
        !crate::store::schema::heal_fts_if_malformed(&conn, "database is locked"),
        "锁冲突不是结构损坏，不该去重建索引"
    );
}

/// 好库不该被自愈逻辑打扰：正常删除走一次就成，不触发重建。
#[test]
fn healthy_library_does_not_need_healing() {
    let conn = memory_db();
    if !crate::store::schema::fts_index_exists(&conn) {
        return;
    }
    seed(&conn);
    let id: i64 = conn
        .query_row("SELECT id FROM media ORDER BY id LIMIT 1", [], |r| r.get(0))
        .unwrap();

    // 索引是好的，直接删就成功（不会走到自愈分支）
    assert!(conn.execute("DELETE FROM media WHERE id = ?1", params![id]).is_ok());
    // rebuild 本身也必须是安全的空操作
    crate::store::schema::rebuild_fts_index(&conn).unwrap();
    assert_eq!(count_rows(&conn, "SELECT COUNT(*) FROM media_fts"), 1);
}

#[test]
fn looks_malformed_only_matches_structure_errors() {
    assert!(crate::store::schema::looks_malformed("database disk image is malformed"));
    assert!(crate::store::schema::looks_malformed("Database disk image is MALFORMED"));
    assert!(!crate::store::schema::looks_malformed("database is locked"));
    assert!(!crate::store::schema::looks_malformed("找不到 id 为 9 的资源"));
}

/// 删单集同样要规范化入参，且只动指定的单集。
#[test]
fn delete_episodes_normalizes_and_only_touches_requested_rows() {
    let conn = memory_db();
    seed(&conn);
    let sid: i64 = conn
        .query_row("SELECT id FROM media WHERE type='tv'", [], |r| r.get(0))
        .unwrap();
    let eps: Vec<i64> = conn
        .prepare("SELECT id FROM episodes WHERE media_id=?1 ORDER BY id")
        .unwrap()
        .query_map(params![sid], |r| r.get(0))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    assert_eq!(eps.len(), 3);

    // 重复 id 只删一条
    assert_eq!(delete_rows(&conn, "episodes", &[eps[0], eps[0]]).unwrap(), 1);
    // 混进脏 id
    assert_eq!(delete_rows(&conn, "episodes", &[eps[1], -5]).unwrap(), 1);
    let left: i64 = conn
        .query_row("SELECT COUNT(*) FROM episodes WHERE media_id=?1", params![sid], |r| r.get(0))
        .unwrap();
    assert_eq!(left, 1, "只删了点名的两条，第三条还在");
    // 幂等
    assert_eq!(delete_rows(&conn, "episodes", &[eps[0]]).unwrap(), 0);
}
