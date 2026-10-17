//! `registry.rs` 的单元测试。
//!
//! 用 `#[path]` 挂回所属模块（见 `registry.rs` 末尾），照 `commands/tests/tree.rs` 的写法。
//!
//! 重点覆盖三件最容易出错的事：
//! 1. 连续两次检测：第二次全是「未变」，且**不会重复插入**；
//! 2. 文件消失：行**不删**，只标 `missing`（「定位旧资源」全靠它）；
//! 3. 文件回来：置回 `present` 并算「恢复」，`first_seen` 不变。
//!
//! 这里刻意**不碰真实磁盘**：入库函数 `apply_catalog_sync` 只认一份已经收集好的
//! 文件列表，测试自己造 `ScannedAsset` 喂进去，遍历那一步（`walk_media_files`）不参与。

use super::*;
use crate::store::db;
use rusqlite::Connection;

fn memory_db() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    db::migrate(&conn).unwrap();
    conn
}

/// 造一个「磁盘上看到的视频文件」。
fn asset(root: &str, rel: &str, name: &str, size: u64, mtime: Option<i64>) -> ScannedAsset {
    let dir = if rel.is_empty() {
        root.to_string()
    } else {
        format!("{root}\\{rel}")
    };
    let ext = name
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default();
    ScannedAsset {
        root_path: root.to_string(),
        path: format!("{dir}\\{name}"),
        dir_path: dir,
        rel_dir: rel.to_string(),
        file_name: name.to_string(),
        ext,
        size,
        mtime,
        content_key: content_key_of(name),
    }
}

/// 「本次走通了前 n 个根目录」。
fn walked(n: usize) -> BTreeSet<usize> {
    (0..n).collect()
}

fn row_count(conn: &Connection, sql: &str) -> i64 {
    conn.query_row(sql, [], |r| r.get(0)).unwrap()
}

/// 某个路径那一行的 `status`（不存在时为 `None`）。
fn status_of(conn: &Connection, path: &str) -> Option<String> {
    conn.query_row(
        "SELECT status FROM resource_assets WHERE path = ?1",
        params![path],
        |r| r.get::<_, String>(0),
    )
    .optional()
    .unwrap()
}

/// 某个路径那一行的 `first_seen`。
fn first_seen_of(conn: &Connection, path: &str) -> i64 {
    conn.query_row(
        "SELECT first_seen FROM resource_assets WHERE path = ?1",
        params![path],
        |r| r.get::<_, i64>(0),
    )
    .unwrap()
}

/// 某个路径那一行的 `missing_at`。
fn missing_at_of(conn: &Connection, path: &str) -> Option<i64> {
    conn.query_row(
        "SELECT missing_at FROM resource_assets WHERE path = ?1",
        params![path],
        |r| r.get::<_, Option<i64>>(0),
    )
    .unwrap()
}

const ROOT: &str = "D:\\media";

fn one_file() -> Vec<ScannedAsset> {
    vec![asset(ROOT, "片", "片.mkv", 100, Some(10))]
}

/* ------------------------------ 幂等 ------------------------------ */

/// 第一次检测是新增；第二次一模一样 → 全部「未变」，且**不重复插入**。
#[test]
fn second_sync_is_unchanged_and_does_not_duplicate_rows() {
    let mut conn = memory_db();
    let roots = vec![ROOT.to_string()];
    let files = one_file();

    let first = apply_catalog_sync(&mut conn, &roots, &files, &walked(1), 1000).unwrap();
    assert_eq!((first.added, first.updated, first.unchanged), (1, 0, 0));
    assert_eq!(first.scanned, 1);
    assert_eq!(first.missing, 0);
    let first_seen = first_seen_of(&conn, &files[0].path);

    let second = apply_catalog_sync(&mut conn, &roots, &files, &walked(1), 2000).unwrap();
    assert_eq!(
        (second.added, second.updated, second.unchanged, second.missing),
        (0, 0, 1, 0),
        "同一个文件第二次检测必须落在「未变」"
    );

    assert_eq!(
        row_count(&conn, "SELECT COUNT(*) FROM resource_assets"),
        1,
        "重复检测绝不能造出第二行（靠 UNIQUE(path) + upsert）"
    );
    assert_eq!(
        first_seen_of(&conn, &files[0].path),
        first_seen,
        "first_seen 记录「第一次见到它」，任何一次重扫都不能覆盖"
    );
}

/// 内容变了（大小 / mtime）→ 算「变动」。
#[test]
fn modified_file_is_counted_as_updated() {
    let mut conn = memory_db();
    let roots = vec![ROOT.to_string()];
    let files = one_file();
    apply_catalog_sync(&mut conn, &roots, &files, &walked(1), 1000).unwrap();

    let bigger = vec![asset(ROOT, "片", "片.mkv", 999, Some(10))];
    let r = apply_catalog_sync(&mut conn, &roots, &bigger, &walked(1), 2000).unwrap();
    assert_eq!((r.added, r.updated, r.unchanged), (0, 1, 0));
    assert_eq!(
        row_count(&conn, "SELECT COUNT(*) FROM resource_assets"),
        1,
        "改动是更新同一行，不是新增"
    );
}

/// 旧库里 `mtime` 是空的（老数据 / 取不到）→ 保守算「变动」。
///
/// 「宁可多写一次，不能漏掉真改动」与 `core::diff` 是同一条原则。
#[test]
fn missing_mtime_is_treated_as_modified() {
    let mut conn = memory_db();
    let roots = vec![ROOT.to_string()];
    let files = vec![asset(ROOT, "片", "片.mkv", 100, None)];
    apply_catalog_sync(&mut conn, &roots, &files, &walked(1), 1000).unwrap();
    let r = apply_catalog_sync(&mut conn, &roots, &files, &walked(1), 2000).unwrap();
    assert_eq!(r.updated, 1, "mtime 取不到时不许判「未变」");
    assert_eq!(r.unchanged, 0);
}

/* ------------------------------ missing / restored ------------------------------ */

/// 文件消失 → 那一行**不删**，只标 `missing`（下次还能答出它上次在哪、多大）。
#[test]
fn disappeared_file_is_marked_missing_not_deleted() {
    let mut conn = memory_db();
    let roots = vec![ROOT.to_string()];
    let files = one_file();
    apply_catalog_sync(&mut conn, &roots, &files, &walked(1), 1000).unwrap();

    // 盘上什么都没了
    let r = apply_catalog_sync(&mut conn, &roots, &[], &walked(1), 2000).unwrap();
    assert_eq!((r.missing, r.added, r.updated), (1, 0, 0));
    assert_eq!(r.scanned, 0);

    assert_eq!(
        row_count(&conn, "SELECT COUNT(*) FROM resource_assets"),
        1,
        "missing 的行必须留在库里——删了就没有「旧资源」可定位了"
    );
    assert_eq!(
        status_of(&conn, &files[0].path).as_deref(),
        Some(registry::STATUS_MISSING)
    );
    assert_eq!(
        missing_at_of(&conn, &files[0].path),
        Some(2000),
        "missing_at 记下「什么时候消失的」"
    );
    // 磁盘事实（大小 / 目录）也原样留着
    let (size, rel): (i64, String) = conn
        .query_row(
            "SELECT size, rel_dir FROM resource_assets WHERE path = ?1",
            params![files[0].path],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(size, 100);
    assert_eq!(rel, "片");
}

/// 连续两次都没见到：`missing_at` 只记「第一次消失」的时间，不被刷新。
///
/// 否则每次检测都会把它冲成「刚刚」，历史就废了。
#[test]
fn missing_at_is_not_refreshed_by_later_syncs() {
    let mut conn = memory_db();
    let roots = vec![ROOT.to_string()];
    let files = one_file();
    apply_catalog_sync(&mut conn, &roots, &files, &walked(1), 1000).unwrap();
    apply_catalog_sync(&mut conn, &roots, &[], &walked(1), 2000).unwrap();
    let third = apply_catalog_sync(&mut conn, &roots, &[], &walked(1), 3000).unwrap();

    assert_eq!(third.missing, 0, "已经 missing 的行不该被重复标记");
    assert_eq!(missing_at_of(&conn, &files[0].path), Some(2000));
}

/// 文件重新出现 → 算「恢复」，置回 `present` 并清掉 `missing_at`，`first_seen` 不变。
#[test]
fn reappearing_file_is_restored() {
    let mut conn = memory_db();
    let roots = vec![ROOT.to_string()];
    let files = one_file();
    apply_catalog_sync(&mut conn, &roots, &files, &walked(1), 1000).unwrap();
    let first_seen = first_seen_of(&conn, &files[0].path);
    apply_catalog_sync(&mut conn, &roots, &[], &walked(1), 2000).unwrap();

    let r = apply_catalog_sync(&mut conn, &roots, &files, &walked(1), 3000).unwrap();
    assert_eq!((r.restored, r.added, r.updated, r.missing), (1, 0, 0, 0));
    assert_eq!(
        status_of(&conn, &files[0].path).as_deref(),
        Some(registry::STATUS_PRESENT)
    );
    assert_eq!(missing_at_of(&conn, &files[0].path), None, "回来了就要清掉消失时间");
    assert_eq!(
        first_seen_of(&conn, &files[0].path),
        first_seen,
        "恢复不算新增，first_seen 必须还是第一次见到它的时间"
    );
    assert_eq!(row_count(&conn, "SELECT COUNT(*) FROM resource_assets"), 1);
}

/// **关键边界**：只走通了第一个根目录时，第二个盘上的资源不能被误判成消失。
///
/// 这是 `walked` 存在的唯一理由——拔掉一块移动硬盘不该把它的内容判成「没了」。
#[test]
fn roots_that_were_not_walked_are_never_marked_missing() {
    let mut conn = memory_db();
    let e_root = "E:\\kv";
    let roots = vec![ROOT.to_string(), e_root.to_string()];
    let files = vec![
        asset(ROOT, "片", "片.mkv", 100, Some(10)),
        asset(e_root, "", "另一块盘.mkv", 200, Some(20)),
    ];
    apply_catalog_sync(&mut conn, &roots, &files, &walked(2), 1000).unwrap();
    assert_eq!(row_count(&conn, "SELECT COUNT(*) FROM resource_assets"), 2);

    // 第二次：只有第一块盘走通了（E 盘没挂上）
    let r = apply_catalog_sync(&mut conn, &roots, &files[..1], &walked(1), 2000).unwrap();
    assert_eq!(r.missing, 0, "没走通的根目录下的资源一条都不许标消失");
    assert_eq!(
        status_of(&conn, &files[1].path).as_deref(),
        Some(registry::STATUS_PRESENT),
        "E 盘那一行必须原样留在 present"
    );
    // 走通的那个根目录下真的消失的文件，照样要标
    let r2 = apply_catalog_sync(&mut conn, &roots, &[], &walked(1), 3000).unwrap();
    assert_eq!(r2.missing, 1);
    assert_eq!(
        status_of(&conn, &files[1].path).as_deref(),
        Some(registry::STATUS_PRESENT),
        "E 盘仍然没走通，依旧不许动它"
    );
}

/// 用户把媒体目录**改窄**（`D:\\media` → `D:\\media\\子`）后，老行依然要被 reconcile 到。
///
/// 这是按路径前缀而不是「库里存的 root_path」判范围的直接理由：只比字符串的话，
/// 改成子目录之后老行永远匹配不上，会一直停在 present，等于定位旧资源的线断了。
#[test]
fn narrowing_the_root_setting_still_reconciles_old_rows() {
    let mut conn = memory_db();
    let inner = format!("{ROOT}\\子");
    let old_roots = vec![ROOT.to_string()];
    let file = asset(ROOT, "子", "片.mkv", 100, Some(10));
    apply_catalog_sync(&mut conn, &old_roots, &[file.clone()], &walked(1), 1000).unwrap();

    // 用户改成只扫子目录；这个文件已经不在盘上了
    let new_roots = vec![inner];
    let r = apply_catalog_sync(&mut conn, &new_roots, &[], &walked(1), 2000).unwrap();
    assert_eq!(r.missing, 1, "新根目录下确实见不到它，必须被标消失");
    assert_eq!(
        status_of(&conn, &file.path).as_deref(),
        Some(registry::STATUS_MISSING)
    );
}

/// 前缀判定必须卡在分隔符边界上：`D:\\media2` **不属于** `D:\\media`。
///
/// 只看字符串前缀的话，扫描 `D:\\media` 会把 `D:\\media2` 整片资源误判成消失。
#[test]
fn path_prefix_respects_directory_boundaries() {
    assert!(path_under("D:\\media\\a.mkv", "D:\\media"));
    assert!(path_under("D:\\media\\子\\b.mkv", "D:\\media\\"));
    assert!(!path_under("D:\\media2\\a.mkv", "D:\\media"));
    assert!(!path_under("D:\\media", "D:\\media"), "目录本身不是它下面的文件");
    assert!(!path_under("E:\\media\\a.mkv", "D:\\media"));
    assert!(!path_under("D:\\media\\a.mkv", ""), "空目录不该匹配任何东西");
}

/* ------------------------------ 快照 ------------------------------ */

/// 每次检测都留一条 `resource_snapshots`，数字与返回结果一致。
#[test]
fn every_sync_writes_one_snapshot() {
    let mut conn = memory_db();
    let roots = vec![ROOT.to_string()];
    let files = one_file();
    let first = apply_catalog_sync(&mut conn, &roots, &files, &walked(1), 1000).unwrap();
    apply_catalog_sync(&mut conn, &roots, &[], &walked(1), 2000).unwrap();

    assert_eq!(row_count(&conn, "SELECT COUNT(*) FROM resource_snapshots"), 2);
    let (added, scanned, missing, roots_text): (i64, i64, i64, String) = conn
        .query_row(
            "SELECT added, scanned, missing, roots FROM resource_snapshots ORDER BY id ASC LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .unwrap();
    assert_eq!(added, first.added as i64);
    assert_eq!(scanned, first.scanned as i64);
    assert_eq!(missing, first.missing as i64);
    assert_eq!(roots_text, ROOT, "快照里要留下这次扫的是哪个目录");
}

/// 汇总提示语是中文，且「没有变化」时也要说清楚（不能是一句空话）。
#[test]
fn summary_message_is_honest_about_no_changes() {
    let mut conn = memory_db();
    let roots = vec![ROOT.to_string()];
    let files = one_file();
    apply_catalog_sync(&mut conn, &roots, &files, &walked(1), 1000).unwrap();
    let r = apply_catalog_sync(&mut conn, &roots, &files, &walked(1), 2000).unwrap();
    assert!(r.message.contains("没有变化"), "实际: {}", r.message);
    assert!(r.message.contains('1'), "应当说清有几个文件: {}", r.message);
}

/* ------------------------------ 查询 ------------------------------ */

/// `status` 过滤 + LEFT JOIN 媒体标题。
#[test]
fn catalog_query_filters_status_and_joins_media_title() {
    let mut conn = memory_db();
    let roots = vec![ROOT.to_string()];
    let files = one_file();
    apply_catalog_sync(&mut conn, &roots, &files, &walked(1), 1000).unwrap();

    // 手动把这一行挂到一条剧集条目上（真实流程里由扫描写入 media 时建立关联）
    let ts = db::now_epoch();
    conn.execute(
        "INSERT INTO media (id, type, title, group_key, file_path, added_at, updated_at)
         VALUES (1, 'tv', '神秘博士', 'S07', ?1, ?2, ?2)",
        params![files[0].path, ts],
    )
    .unwrap();
    conn.execute(
        "UPDATE resource_assets SET media_id = 1 WHERE path = ?1",
        params![files[0].path],
    )
    .unwrap();

    let all = get_resource_catalog_sync(&conn, 50, None, None).unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].display_title.as_deref(), Some("神秘博士 · S07"));
    assert_eq!(all[0].media_type.as_deref(), Some("tv"));
    assert_eq!(all[0].media_id, Some(1));
    assert_eq!(all[0].file_name, "片.mkv");
    assert_eq!(all[0].rel_dir, "片");
    assert_eq!(all[0].root_path, ROOT);
    assert_eq!(all[0].size, 100);

    // 状态过滤：现在全是 present，查 missing 应当为空
    assert!(get_resource_catalog_sync(&conn, 50, Some("missing"), None)
        .unwrap()
        .is_empty());
    assert_eq!(
        get_resource_catalog_sync(&conn, 50, Some("present"), None)
            .unwrap()
            .len(),
        1
    );
}

/// 没有对应作品条目时也要显示得出来（LEFT JOIN，不是 INNER JOIN）。
///
/// 「盘上有、库里没有」正是资源目录要暴露的东西，绝不能因为它没有 media 就被藏起来。
#[test]
fn catalog_query_shows_assets_without_media() {
    let mut conn = memory_db();
    let roots = vec![ROOT.to_string()];
    apply_catalog_sync(&mut conn, &roots, &one_file(), &walked(1), 1000).unwrap();
    let rows = get_resource_catalog_sync(&conn, 50, None, None).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].media_id, None);
    assert_eq!(rows[0].display_title, None);
    assert_eq!(rows[0].media_type, None);
}

/// 关键词里的 LIKE 元字符必须被转义。
///
/// 不转义的话，用户输入一个百分号就会把整库都搜出来——那不是「搜得宽」，是搜错了。
#[test]
fn catalog_keyword_escapes_like_wildcards() {
    let mut conn = memory_db();
    let roots = vec![ROOT.to_string()];
    let files = vec![
        asset(ROOT, "甲", "卧虎藏龙.mkv", 1, Some(1)),
        asset(ROOT, "乙", "real 100%.mkv", 2, Some(2)),
    ];
    apply_catalog_sync(&mut conn, &roots, &files, &walked(1), 1000).unwrap();

    let hit = get_resource_catalog_sync(&conn, 50, None, Some("卧虎")).unwrap();
    assert_eq!(hit.len(), 1, "按文件名里的中文搜得中");
    let escaped = get_resource_catalog_sync(&conn, 50, None, Some("%")).unwrap();
    assert_eq!(
        escaped.len(),
        1,
        "百分号必须当成普通字符（只命中文件名里真带它的那条），不能变成通配符"
    );
    assert_eq!(escaped[0].file_name, "real 100%.mkv");

    // 相对目录也能搜（用户记得的往往是「在哪个文件夹里」）
    let by_dir = get_resource_catalog_sync(&conn, 50, None, Some("甲")).unwrap();
    assert_eq!(by_dir.len(), 1);
    assert_eq!(by_dir[0].file_name, "卧虎藏龙.mkv");
}

/// `limit` 兜底：传 0 / 负数不该得到「一条都不要」这种谜之空列表。
#[test]
fn catalog_limit_is_clamped_to_a_sane_range() {
    let mut conn = memory_db();
    let roots = vec![ROOT.to_string()];
    apply_catalog_sync(&mut conn, &roots, &one_file(), &walked(1), 1000).unwrap();
    assert_eq!(get_resource_catalog_sync(&conn, 0, None, None).unwrap().len(), 1);
    assert_eq!(get_resource_catalog_sync(&conn, -5, None, None).unwrap().len(), 1);
}

/* ------------------------------ 差异清单 ------------------------------ */

/// 从未检测过：三个清单都空，`last_sync_at` 为 `None`。
#[test]
fn diff_before_any_sync_is_empty() {
    let conn = memory_db();
    let d = diff_resource_catalog_sync(&conn).unwrap();
    assert!(d.added.is_empty() && d.changed.is_empty() && d.missing.is_empty());
    assert_eq!(d.present, 0);
    assert_eq!(d.last_sync_at, None);
}

/// 只检测过一次时：没有「上一次」可对比，两个清单都空（不能把整库都当成新增）。
#[test]
fn diff_after_a_single_sync_has_no_added_or_changed() {
    let mut conn = memory_db();
    let roots = vec![ROOT.to_string()];
    apply_catalog_sync(&mut conn, &roots, &one_file(), &walked(1), 1000).unwrap();
    let d = diff_resource_catalog_sync(&conn).unwrap();
    assert!(d.added.is_empty(), "首次检测谈不上「相对上次新增」");
    assert!(d.changed.is_empty());
    assert_eq!(d.present, 1);
    assert!(d.last_sync_at.is_some());
}

/// 新增 / 变动 / 消失三类各自归位，且互不重叠。
///
/// 时间用的是「上次检测之后」的假时间戳（真实时钟的未来值）——
/// 这正是真实场景里「文件在两次检测之间被加进来 / 改过 / 删掉」的样子。
#[test]
fn diff_separates_added_changed_and_missing() {
    let mut conn = memory_db();
    let roots = vec![ROOT.to_string()];
    let baseline = db::now_epoch();

    let a = asset(ROOT, "甲", "老片.mkv", 100, Some(baseline - 500));
    let gone = asset(ROOT, "乙", "要删的.mkv", 200, Some(baseline - 500));
    apply_catalog_sync(
        &mut conn,
        &roots,
        &[a.clone(), gone.clone()],
        &walked(1),
        baseline - 400,
    )
    .unwrap();

    // 第二次检测：老片被改写、新增一个文件、要删的没了
    let after = baseline + 3600;
    apply_catalog_sync(
        &mut conn,
        &roots,
        &[
            asset(ROOT, "甲", "老片.mkv", 100, Some(after)),
            asset(ROOT, "丙", "新片.mkv", 300, Some(after)),
        ],
        &walked(1),
        after,
    )
    .unwrap();

    let d = diff_resource_catalog_sync(&conn).unwrap();
    let names = |v: &[CatalogEntry]| -> Vec<String> {
        v.iter().map(|e| e.file_name.clone()).collect()
    };
    assert_eq!(names(&d.added), vec!["新片.mkv"], "只有它在门槛之后才第一次出现");
    assert_eq!(names(&d.changed), vec!["老片.mkv"], "它上次就见过、之后被改过");
    assert_eq!(names(&d.missing), vec!["要删的.mkv"], "它现在不在盘上了");
    assert_eq!(d.present, 2, "仍在盘上的是老片 + 新片");
    assert!(d.last_sync_at.unwrap() >= baseline);
    // 消失的那一行必须还查得到（这是「定位旧资源」的落点）
    assert_eq!(
        status_of(&conn, &gone.path).as_deref(),
        Some(registry::STATUS_MISSING)
    );
}

/// 差异清单里的 `added` / `changed` 只列还在盘上的，`missing` 单独归位。
#[test]
fn diff_lists_never_mix_missing_rows_into_added_or_changed() {
    let mut conn = memory_db();
    let roots = vec![ROOT.to_string()];
    apply_catalog_sync(&mut conn, &roots, &one_file(), &walked(1), 1000).unwrap();
    apply_catalog_sync(&mut conn, &roots, &[], &walked(1), 2000).unwrap();
    let d = diff_resource_catalog_sync(&conn).unwrap();
    assert!(d.added.is_empty());
    assert!(d.changed.is_empty());
    assert_eq!(d.missing.len(), 1);
    assert_eq!(d.missing[0].status, registry::STATUS_MISSING);
    assert_eq!(d.present, 0);
}

/* ------------------------------ 序列化契约 ------------------------------ */

/// 前端照着这三个结构调命令（`#[serde(rename_all = "camelCase")]`），
/// 字段名一旦改错，前端拿到的就是 undefined——把契约钉死在用例里。
#[test]
fn serialized_field_names_are_camel_case() {
    let entry = CatalogEntry {
        id: 1,
        path: "D:/m/a.mkv".to_string(),
        file_name: "a.mkv".to_string(),
        rel_dir: "子目录".to_string(),
        root_path: "D:/m".to_string(),
        size: 100,
        mtime: Some(5),
        status: registry::STATUS_PRESENT.to_string(),
        media_id: Some(7),
        display_title: Some("剧 · 目录".to_string()),
        media_type: Some("tv".to_string()),
    };
    let v = serde_json::to_value(&entry).unwrap();
    for key in [
        "id",
        "path",
        "fileName",
        "relDir",
        "rootPath",
        "size",
        "mtime",
        "status",
        "mediaId",
        "displayTitle",
        "mediaType",
    ] {
        assert!(v.get(key).is_some(), "CatalogEntry 缺少字段 {key}: {v}");
    }

    let sync = CatalogSyncResult {
        started_at: 1,
        finished_at: 2,
        roots: 3,
        scanned: 4,
        added: 5,
        updated: 6,
        unchanged: 7,
        missing: 8,
        restored: 9,
        message: "检测完成".to_string(),
    };
    let v = serde_json::to_value(&sync).unwrap();
    for key in [
        "startedAt",
        "finishedAt",
        "roots",
        "scanned",
        "added",
        "updated",
        "unchanged",
        "missing",
        "restored",
        "message",
    ] {
        assert!(v.get(key).is_some(), "CatalogSyncResult 缺少字段 {key}: {v}");
    }

    let diff = CatalogDiff {
        added: Vec::new(),
        changed: Vec::new(),
        missing: Vec::new(),
        present: 10,
        last_sync_at: Some(11),
    };
    let v = serde_json::to_value(&diff).unwrap();
    for key in ["added", "changed", "missing", "present", "lastSyncAt"] {
        assert!(v.get(key).is_some(), "CatalogDiff 缺少字段 {key}: {v}");
    }
}

/* ------------------------------ 纯函数辅助 ------------------------------ */

/// 归一化标题用 `core::grouping` 的口径：同一部作品的不同集要能对上。
#[test]
fn content_key_matches_across_episodes_of_one_series() {
    let a = content_key_of("神秘博士.S07.E01.1080p.mkv");
    let b = content_key_of("神秘博士.S07.E02.1080p.mkv");
    assert_eq!(a, b, "同一部剧的两集，归一化标题必须一致");
    assert!(!a.is_empty());
}

/// 扩展名一律小写（大写与小写的同一个扩展名要归一，否则统计会分成两类）。
#[test]
fn extension_is_lowercased_and_optional() {
    assert_eq!(ext_of("A.MKV"), "mkv");
    assert_eq!(ext_of("b.Mp4"), "mp4");
    assert_eq!(ext_of("noext"), "");
}

/// 根目录末尾的分隔符不影响「同一个根」的判断。
#[test]
fn trim_root_ignores_trailing_separators() {
    assert_eq!(trim_root("D:\\media\\"), "D:\\media");
    assert_eq!(trim_root("D:\\media"), "D:\\media");
    assert_eq!(trim_root("/mnt/media/"), "/mnt/media");
    assert_eq!(trim_root("\\"), "\\", "整串都是分隔符时原样返回，别截成空串");
}
