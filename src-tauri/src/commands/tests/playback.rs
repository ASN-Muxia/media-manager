//! `playback.rs` 的单元测试。

use super::*;
use crate::store::db;

fn db() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
    db::migrate(&conn).unwrap();
    conn
}

/// 造一个剧集：3 集，路径按季集顺序。
fn seed_episodes(conn: &Connection) -> (i64, Vec<i64>) {
    conn.execute(
        "INSERT INTO media (type,title,added_at,updated_at) VALUES ('tv','剧',0,0)",
        [],
    )
    .unwrap();
    let mid = conn.last_insert_rowid();
    let mut ids = Vec::new();
    for (s, e) in [(1, 1), (1, 2), (2, 1)] {
        conn.execute(
            "INSERT INTO episodes (media_id,season,episode,file_path,file_name,added_at) \
             VALUES (?1,?2,?3,?4,?5,0)",
            rusqlite::params![mid, s, e, format!("D:/剧/S{s:02}E{e:02}.mkv"), format!("S{s:02}E{e:02}.mkv")],
        )
        .unwrap();
        ids.push(conn.last_insert_rowid());
    }
    (mid, ids)
}

/// 连播的文件顺序必须是（季, 集）升序——从中间某集开始时尤其容易错。
#[test]
fn episode_paths_from_follows_season_episode_order() {
    let conn = db();
    let (mid, ids) = seed_episodes(&conn);
    let paths = episode_paths_from(&conn, mid, ids[0]).unwrap();
    assert_eq!(paths.len(), 3);
    assert!(paths[0].ends_with("S01E01.mkv"));
    assert!(paths[1].ends_with("S01E02.mkv"));
    assert!(paths[2].ends_with("S02E01.mkv"));
}

/// 从第 2 集开始连播，不应该把第 1 集也带上。
#[test]
fn episode_paths_from_starts_at_requested_episode() {
    let conn = db();
    let (mid, ids) = seed_episodes(&conn);
    let paths = episode_paths_from(&conn, mid, ids[1]).unwrap();
    assert_eq!(paths.len(), 2);
    assert!(paths[0].ends_with("S01E02.mkv"));
}

#[test]
fn episode_path_reads_the_row() {
    let conn = db();
    let (_mid, ids) = seed_episodes(&conn);
    let p = episode_path(&conn, ids[0]).unwrap();
    assert!(p.ends_with("S01E01.mkv"));
}

/// 没有文件路径的单集要报错而不是 panic。
#[test]
fn episode_path_errors_when_path_missing() {
    let conn = db();
    conn.execute(
        "INSERT INTO media (type,title,added_at,updated_at) VALUES ('tv','剧',0,0)",
        [],
    )
    .unwrap();
    let mid = conn.last_insert_rowid();
    conn.execute(
        "INSERT INTO episodes (media_id,season,episode,added_at) VALUES (?1,1,1,0)",
        rusqlite::params![mid],
    )
    .unwrap();
    let ep = conn.last_insert_rowid();
    assert!(episode_path(&conn, ep).is_err());
}

/* ==================== 双击播放：目标选择 ==================== */
/*
 * 「双击封面直接播」的目标是**上次真正播放过的那一集**，不是第一个未看集。
 * 两者的差别只在一种真实场景里看得出来：看了几集、但没点「标记已看」
 * （`record_play` 只写 `last_played_at`，不写 `watched`）。
 */

/// 播过第 2 集（未标记已看）→ 双击要回到第 2 集，而不是第 1 集从头再来。
#[test]
fn last_played_episode_survives_without_watched_flag() {
    use crate::store::playback;

    let conn = db();
    let (mid, ids) = seed_episodes(&conn);
    // 只「播过」：与 play_episode 成功后做的是同一件事
    playback::record_play(&conn, mid, Some(ids[1])).unwrap();

    let s = playback::summarize(&conn, mid).unwrap();
    assert_eq!(
        s.last_played_episode_id,
        Some(ids[1]),
        "上次播放的那一集必须能单独取到，不能依赖 watched"
    );
    assert!(!s.watched, "只播一次不等于看完了");
    assert_eq!(
        s.resume_episode_id,
        Some(ids[0]),
        "续播点仍是第一个未看集（与双击播放是两个不同的问题）"
    );
}

/// 播过第 3 集之后又播回第 1 集：双击要播**最近**那次（第 1 集）。
#[test]
fn last_played_episode_is_the_most_recent_one() {
    use crate::store::playback;

    let conn = db();
    let (mid, ids) = seed_episodes(&conn);
    playback::record_play(&conn, mid, Some(ids[2])).unwrap();
    // 时间戳是秒级，连续两次可能落在同一秒 → 显式把前一行推早，避免 flaky
    conn.execute(
        "UPDATE playback_state SET last_played_at = last_played_at - 100 WHERE episode_id = ?1",
        rusqlite::params![ids[2]],
    )
    .unwrap();
    playback::record_play(&conn, mid, Some(ids[0])).unwrap();

    let s = playback::summarize(&conn, mid).unwrap();
    assert_eq!(s.last_played_episode_id, Some(ids[0]), "要取最近播放的那一集");
}

/// 一集都没播过（库是新的）→ 没有「上次」，双击退回第一个未看集。
#[test]
fn last_played_episode_is_none_before_any_play() {
    use crate::store::playback;

    let conn = db();
    let (mid, ids) = seed_episodes(&conn);
    let s = playback::summarize(&conn, mid).unwrap();
    assert_eq!(s.last_played_episode_id, None);
    // 双击时的兜底仍然是可播的：`last_played.or(resume)`
    assert_eq!(s.last_played_episode_id.or(s.resume_episode_id), Some(ids[0]));
}

/// 手动勾过「已看」（有 watched、没有播放时间）→ 仍要有确定目标，不能是 None。
#[test]
fn last_played_episode_falls_back_to_last_watched() {
    use crate::store::playback;

    let conn = db();
    let (mid, ids) = seed_episodes(&conn);
    playback::set_watched(&conn, mid, Some(ids[1]), true).unwrap();
    let s = playback::summarize(&conn, mid).unwrap();
    assert_eq!(
        s.last_played_episode_id,
        Some(ids[1]),
        "只有已看标记、没有播放时间时，沿用「最近看过」那一集"
    );
}

/// 电影的路径来自 `media.file_path`；没有文件时给一句能看懂的话，不是 panic。
#[test]
fn movie_path_reads_media_row() {
    let conn = db();
    conn.execute(
        "INSERT INTO media (type,title,file_path,added_at,updated_at) \
         VALUES ('movie','片','D:/片/片.mkv',0,0)",
        [],
    )
    .unwrap();
    let mid = conn.last_insert_rowid();
    assert_eq!(movie_path(&conn, mid).unwrap(), "D:/片/片.mkv");

    conn.execute(
        "INSERT INTO media (type,title,added_at,updated_at) VALUES ('movie','空',0,0)",
        [],
    )
    .unwrap();
    let empty = conn.last_insert_rowid();
    let err = movie_path(&conn, empty).unwrap_err();
    assert!(err.contains("没有关联的本地文件"), "要说清原因：{err}");

    assert!(movie_path(&conn, 999_999).is_err(), "不存在的 id 不能 panic");
}

/// 送进播放器之前要能报出「这是第几季第几集」，且与 `episodes` 表一致。
#[test]
fn season_episode_is_read_for_the_narration() {
    let conn = db();
    let (_mid, ids) = seed_episodes(&conn);
    // seed 里的顺序是 (1,1) (1,2) (2,1)
    assert_eq!(episode_season_episode(&conn, ids[0]).unwrap(), (1, 1));
    assert_eq!(episode_season_episode(&conn, ids[1]).unwrap(), (1, 2));
    assert_eq!(episode_season_episode(&conn, ids[2]).unwrap(), (2, 1));
}

/**
 * 双击时的目标选择规则，抽成一条断言：**上次播放 > 第一个未看集**。
 *
 * `play_episode` 里那行 `s.last_played_episode_id.or(s.resume_episode_id)`
 * 是整条「双击播上次那集」的核心。把它当纯函数钉住，避免以后有人把顺序反过来
 * （那样"看了第 2 集但没标记已看"会每次双击都从第 1 集重来）。
 */
#[test]
fn double_click_target_prefers_last_played_over_first_unwatched() {
    use crate::store::playback;

    let conn = db();
    let (mid, ids) = seed_episodes(&conn);
    playback::record_play(&conn, mid, Some(ids[1])).unwrap();

    let s = playback::summarize(&conn, mid).unwrap();
    let target = s.last_played_episode_id.or(s.resume_episode_id);
    assert_eq!(target, Some(ids[1]), "有播放记录 → 播上次那一集");

    // 反向判据：真的一集没播过时，才轮到「第一个未看集」
    let (mid2, ids2) = {
        conn.execute(
            "INSERT INTO media (type,title,added_at,updated_at) VALUES ('tv','另一部',0,0)",
            [],
        )
        .unwrap();
        let m = conn.last_insert_rowid();
        let mut v = Vec::new();
        for (s, e) in [(1, 1), (1, 2)] {
            conn.execute(
                "INSERT INTO episodes (media_id,season,episode,file_path,file_name,added_at) \
                 VALUES (?1,?2,?3,?4,?5,0)",
                rusqlite::params![m, s, e, format!("D:/另一部/S0{s}E0{e}.mkv"), format!("S0{s}E0{e}.mkv")],
            )
            .unwrap();
            v.push(conn.last_insert_rowid());
        }
        (m, v)
    };
    let s2 = playback::summarize(&conn, mid2).unwrap();
    assert_eq!(s2.last_played_episode_id, None);
    assert_eq!(
        s2.last_played_episode_id.or(s2.resume_episode_id),
        Some(ids2[0]),
        "没有播放记录 → 退回第一个未看集"
    );
}
