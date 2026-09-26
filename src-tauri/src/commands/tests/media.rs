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
    let sql = format!(
        "SELECT {ITEM_COLUMNS} FROM media m {LIST_WHERE} \
         ORDER BY m.title COLLATE NOCASE ASC, m.year ASC"
    );
    let mut stmt = conn.prepare(&sql).unwrap();
    let rows = stmt
        .query_map(
            params![filter, None::<i64>, None::<i64>, None::<String>, None::<i64>],
            map_item,
        )
        .unwrap();
    rows.map(|r| r.unwrap()).collect()
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

/* ------------------------- 系列电影合集 ------------------------- */

fn plan_from_rows(conn: &Connection) -> Vec<MovieCollection> {
    let mut stmt = conn
        .prepare("SELECT m.id, m.title, m.year, m.poster_path, m.file_path, m.file_size, m.tmdb_id \
                  FROM media m WHERE m.type = 'movie'")
        .unwrap();
    let rows = stmt
        .query_map([], |row| {
            Ok(collection::MovieEntry {
                id: row.get(0)?,
                title: row.get(1)?,
                year: row.get(2)?,
                poster_path: row.get(3)?,
                tmdb_id: row.get(6)?,
                file_size: row.get(5)?,
            })
        })
        .unwrap();
    let mut entries = Vec::new();
    for r in rows {
        entries.push(r.unwrap());
    }
    let plan = collection::plan_collections(&entries, &collection::CollectionConfig::default());
    let by_id: BTreeMap<i64, &collection::MovieEntry> =
        entries.iter().map(|e| (e.id, e)).collect();
    plan.collections
        .into_iter()
        .map(|c| {
            let dup = c.duplicates.len();
            MovieCollection {
                key: c.key,
                name: c.name,
                movies: c
                    .members
                    .iter()
                    .filter_map(|id| {
                        by_id.get(id).map(|e| CollectionMovie {
                            id: e.id,
                            title: e.title.clone(),
                            year: e.year,
                            poster_path: e.poster_path.clone(),
                            file_path: None,
                        })
                    })
                    .collect(),
                duplicate_count: dup,
                start_year: c.start_year,
                end_year: c.end_year,
            }
        })
        .collect()
}

/// 同一个系列（含续作标识）的电影归到一起，且按上映时间排序。
#[test]
fn movie_collections_group_a_franchise_by_release_year() {
    let conn = memory_db();
    insert_media(&conn, "movie", "沙丘 2", "D:\\影视\\Dune2.mkv", &[("year", "2024")]);
    insert_media(&conn, "movie", "沙丘", "D:\\影视\\Dune.mkv", &[("year", "2021")]);

    let got = plan_from_rows(&conn);
    assert_eq!(got.len(), 1, "两部沙丘应聚成一个合集");
    assert_eq!(got[0].movies.iter().map(|m| m.year).collect::<Vec<_>>(), vec![Some(2021), Some(2024)]);
    assert_eq!(got[0].start_year, Some(2021));
    assert_eq!(got[0].end_year, Some(2024));
}

/// 毫不相干的电影不能被并进同一个合集。
#[test]
fn movie_collections_keep_unrelated_movies_apart() {
    let conn = memory_db();
    insert_media(&conn, "movie", "沙丘", "D:\\影视\\Dune.mkv", &[("year", "2021")]);
    insert_media(&conn, "movie", "沙丘 2", "D:\\影视\\Dune2.mkv", &[("year", "2024")]);
    insert_media(&conn, "movie", "1917", "D:\\影视\\1917.mkv", &[("year", "2019")]);

    let got = plan_from_rows(&conn);
    assert_eq!(got.len(), 1, "1917 不该自成合集，也不该并进沙丘");
    assert_eq!(got[0].movies.len(), 2);
}
