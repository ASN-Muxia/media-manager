//! `share.rs` 的单元测试。

use super::*;
use crate::core::mediatype::SourceHint;
use crate::store::db;

fn db() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
    db::migrate(&conn).unwrap();
    conn
}

fn temp_dir(tag: &str) -> PathBuf {
    let mut d = std::env::temp_dir();
    d.push(format!(
        "mm-share-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// 导出电影：写出 movie.nfo，默认不覆盖。
#[test]
fn export_writes_movie_nfo_and_skips_existing() {
    let d = temp_dir("movie");
    let conn = db();
    conn.execute(
        "INSERT INTO media (type,title,year,tmdb_id,file_path,file_name,added_at,updated_at) \
         VALUES ('movie','卧虎藏龙',2000,1390,?1,'卧虎藏龙.mkv',0,0)",
        rusqlite::params![d.join("卧虎藏龙.mkv").to_string_lossy()],
    )
    .unwrap();
    let id = conn.last_insert_rowid();

    let (w, s) = export_one(&conn, id, None, false).unwrap();
    assert_eq!(w, 1, "应写出一个 NFO");
    assert_eq!(s, 0);
    assert!(d.join("movie.nfo").exists());

    // 再来一次：默认跳过
    let (w2, s2) = export_one(&conn, id, None, false).unwrap();
    assert_eq!(w2, 0);
    assert_eq!(s2, 1, "已存在的 NFO 必须被跳过");
    std::fs::remove_dir_all(&d).ok();
}

/// 导出剧集：tvshow.nfo + 每集一份单集 NFO。
#[test]
fn export_writes_tvshow_and_episode_nfos() {
    let d = temp_dir("tv");
    let conn = db();
    conn.execute(
        "INSERT INTO media (type,title,tvdb_id,added_at,updated_at) VALUES ('tv','神秘博士',78804,0,0)",
        [],
    )
    .unwrap();
    let mid = conn.last_insert_rowid();
    for (s, e) in [(1, 1), (1, 2)] {
        conn.execute(
            "INSERT INTO episodes (media_id,season,episode,file_path,file_name,added_at) \
             VALUES (?1,?2,?3,?4,?5,0)",
            rusqlite::params![
                mid,
                s,
                e,
                d.join(format!("S{s:02}E{e:02}.mkv")).to_string_lossy(),
                format!("S{s:02}E{e:02}.mkv")
            ],
        )
        .unwrap();
    }
    let (w, _s) = export_one(&conn, mid, None, false).unwrap();
    assert_eq!(w, 3, "tvshow.nfo + 2 个单集 NFO");
    assert!(d.join("tvshow.nfo").exists());
    assert!(d.join("S01E01.nfo").exists());
    assert!(d.join("S01E02.nfo").exists());
    std::fs::remove_dir_all(&d).ok();
}

/// 导出到指定目录：sidecar 模式下也不会往媒体目录写。
#[test]
fn export_honours_shared_target_dir() {
    let media = temp_dir("m");
    let out = temp_dir("o");
    let conn = db();
    conn.execute(
        "INSERT INTO media (type,title,file_path,file_name,added_at,updated_at) \
         VALUES ('movie','电影',?1,'电影.mkv',0,0)",
        rusqlite::params![media.join("电影.mkv").to_string_lossy()],
    )
    .unwrap();
    let id = conn.last_insert_rowid();
    let (w, _) = export_one(&conn, id, Some(&out), false).unwrap();
    assert_eq!(w, 1);
    assert!(out.join("movie.nfo").exists());
    assert!(!media.join("movie.nfo").exists(), "不应写到媒体目录");
    std::fs::remove_dir_all(&media).ok();
    std::fs::remove_dir_all(&out).ok();
}

/// 导入：回填空字段，但**绝不覆盖**已有值。
#[test]
fn fill_metadata_only_fills_blanks() {
    let conn = db();
    conn.execute(
        "INSERT INTO media (type,title,year,added_at,updated_at) VALUES ('movie','原标题',1999,0,0)",
        [],
    )
    .unwrap();
    let id = conn.last_insert_rowid();
    let info = NfoInfo {
        title: Some("NFO 标题".to_string()),
        year: Some(2000),
        tmdb_id: Some(42),
        kind: Some(NfoKind::Movie),
        ..Default::default()
    };
    fill_metadata(&conn, id, &info).unwrap();
    let (t, y, tmdb): (String, i64, i64) = conn
        .query_row("SELECT title, year, tmdb_id FROM media WHERE id = ?1", rusqlite::params![id], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })
        .unwrap();
    assert_eq!(t, "原标题", "已有标题不能被 NFO 覆盖");
    assert_eq!(y, 1999, "已有年份不能被覆盖");
    assert_eq!(tmdb, 42, "空字段应被补上");
}

/// 导入：NFO 说是剧集，且该条目是待确认 → 过 gate 后改判。
#[test]
fn import_promotes_unknown_to_tv_via_gate() {
    let conn = db();
    conn.execute(
        "INSERT INTO media (type,title,added_at,updated_at) VALUES ('unknown','某剧',0,0)",
        [],
    )
    .unwrap();
    let id = conn.last_insert_rowid();
    let info = NfoInfo {
        kind: Some(NfoKind::TvShow),
        ..Default::default()
    };
    let hint = crate::core::nfo::source_hint_of(&info).unwrap();
    assert!(crate::commands::media::source_hint_allowed(&conn, id, hint).unwrap());
    assert!(crate::commands::media::apply_source_hint(&conn, id, hint).unwrap());
    let t: String = conn
        .query_row("SELECT type FROM media WHERE id = ?1", rusqlite::params![id], |r| r.get(0))
        .unwrap();
    assert_eq!(t, "tv");
}

/// 导入：用户锁过的条目，NFO 说了也不算。
#[test]
fn import_respects_locked_rows() {
    let conn = db();
    conn.execute(
        "INSERT INTO media (type,title,locked,added_at,updated_at) VALUES ('unknown','用户定的',1,0,0)",
        [],
    )
    .unwrap();
    let id = conn.last_insert_rowid();
    let hint = SourceHint::Movie;
    assert!(!crate::commands::media::apply_source_hint(&conn, id, hint).unwrap());
}

/// 端到端：导出 → 改库 → 导入回来，元数据能回来。
#[test]
fn export_then_import_round_trips() {
    let d = temp_dir("round");
    let conn = db();
    conn.execute(
        "INSERT INTO media (type,title,file_path,file_name,added_at,updated_at) \
         VALUES ('movie','卧虎藏龙',?1,'卧虎藏龙.mkv',0,0)",
        rusqlite::params![d.join("卧虎藏龙.mkv").to_string_lossy()],
    )
    .unwrap();
    let id = conn.last_insert_rowid();
    export_one(&conn, id, None, false).unwrap();

    // 「换机器」：把年份清掉
    conn.execute("UPDATE media SET year = NULL WHERE id = ?1", rusqlite::params![id])
        .unwrap();

    let text = std::fs::read_to_string(d.join("movie.nfo")).unwrap();
    let info = nfo::parse(&text);
    assert_eq!(info.title.as_deref(), Some("卧虎藏龙"));
    fill_metadata(&conn, id, &info).unwrap();
    std::fs::remove_dir_all(&d).ok();
}
