//! 观看状态与续播（`playback_state` 表的读写）。
//!
//! ## 这里绝不触碰视频文件
//! 全部状态只落在数据库里，视频文件保持只读。没有「把进度写回文件」这回事，
//! 也没有任何转码 / 重写动作——本模块唯一的 IO 目标是 SQLite。
//!
//! ## 为什么是「集级」而不是「秒级」
//! 本项目调用**外部播放器**（PotPlayer）播放，进程一去不回，拿不到播放位置。
//! 因此：
//! - `position_secs` 字段**预留**给将来能读到进度的场景，本期不自动采集；
//! - 实际的续播语义是「看到第几集」——播完一集就推进到下一集；
//! - 「继续播放」= 播放下一个未看集（全都看完了就从第一集重来）。
//! 这个口径必须写进界面文案，不能让用户按 Jellyfin 的秒级续播去理解。

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use std::collections::BTreeMap;

use crate::util::now_epoch;

/// 一条（整部 / 某一集的）观看状态。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaybackRow {
    pub media_id: i64,
    /// 电影为 `None`（整部一条）；剧集为具体单集 id。
    pub episode_id: Option<i64>,
    pub watched: bool,
    /// 预留字段：本期不自动采集（外置播放器回传不了进度）。
    pub position_secs: Option<i64>,
    pub duration_secs: Option<i64>,
    pub play_count: i64,
    pub last_played_at: Option<i64>,
}

/// 一个条目在界面上要展示的观看摘要。
#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct MediaPlayback {
    pub media_id: i64,
    /// 电影：本身是否看过；剧集：**全部**单集都看过才算。
    pub watched: bool,
    /// 剧集：已看集数 / 总集数（电影为 0/0）。
    pub watched_episodes: i64,
    pub total_episodes: i64,
    /// 整部累计播放次数。
    pub play_count: i64,
    pub last_played_at: Option<i64>,
    /// 「继续播放」要播的单集：下一个未看集；全部看完则为第一集。
    pub resume_episode_id: Option<i64>,
    /// 最近看过的那一集（用于「看到第几集」文案）。
    pub last_episode_id: Option<i64>,
    /// 最近看过的那一集的季/集号（界面展示用）。
    pub last_season: Option<i64>,
    pub last_episode: Option<i64>,
    /// **上次真正播放过**的那一集（只看 `last_played_at`，不看 `watched`）。
    ///
    /// 与 `resume_episode_id` / `last_episode_id` 是三个不同的问题：
    /// - `resume_episode_id` 回答「接下来该看哪集」（第一个未看集）；
    /// - `last_episode_id` 回答「看到第几集」（要求已看，否则文案会显示一个没看过的集）；
    /// - 这个字段回答「上次我点开的是哪个文件」——双击播放要的就是它。
    ///
    /// ⚠️ 必须单列一个字段：`record_play` 只写 `last_played_at`、**不写 `watched`**
    /// （播放≠看完），所以只靠上面两个字段时，「播了但没标记已看」会退化成
    /// 第一集 / 下一个未看集 —— 用户说的「上次记录的那一集」就丢了。
    pub last_played_episode_id: Option<i64>,
}

/// 标记整部 / 某一集的观看状态。
///
/// `episode_id` 为 `None` 表示针对电影（或整部剧集）本身。
pub fn set_watched(
    conn: &Connection,
    media_id: i64,
    episode_id: Option<i64>,
    watched: bool,
) -> Result<(), String> {
    let ts = now_epoch();
    conn.execute(
        "INSERT INTO playback_state \
             (media_id, episode_id, watched, play_count, updated_at) \
         VALUES (?1, ?2, ?3, 0, ?4) \
         ON CONFLICT(media_id, IFNULL(episode_id, 0)) DO UPDATE SET \
             watched = excluded.watched, updated_at = excluded.updated_at",
        params![media_id, episode_id, watched as i64, ts],
    )
    .map_err(|e| format!("写入观看状态失败: {e}"))?;

    // 整部标记必须**级联到每一集**，否则会出现两种自相矛盾的状态：
    // - 剧集的 `watched` 由「已看集数 >= 总集数」推导，整部标记那一行
    //   （`episode_id IS NULL`）压根不参与 → 点了「标记已看」按钮纹丝不动，
    //   看起来就像点击没反应；
    // - 即便把推导改成整部标记优先，进度条仍会停在 `0/25`，
    //   出现「整部已看但进度是 0」的错位。
    // 级联后两者天然一致：标记已看 = 25/25，取消 = 0/25。
    if episode_id.is_none() {
        cascade_episodes(conn, media_id, watched, ts)?;
    }
    Ok(())
}

/// 把整部标记同步到该剧集下的每一集。
///
/// 用 `INSERT ... SELECT` 一次写完，避免先查 id 列表再逐条 execute。
/// 已是该状态的集也会被 UPDATE（幂等），所以重复调用不会产生副作用。
fn cascade_episodes(
    conn: &Connection,
    media_id: i64,
    watched: bool,
    ts: i64,
) -> Result<(), String> {
    conn.execute(
        "INSERT INTO playback_state (media_id, episode_id, watched, play_count, updated_at) \
         SELECT ?1, e.id, ?2, 0, ?3 FROM episodes e WHERE e.media_id = ?1 \
         ON CONFLICT(media_id, IFNULL(episode_id, 0)) DO UPDATE SET \
             watched = excluded.watched, updated_at = excluded.updated_at",
        params![media_id, watched as i64, ts],
    )
    .map_err(|e| format!("同步单集观看状态失败: {e}"))?;
    Ok(())
}

/// 记录一次播放（累加次数 + 刷新最近播放时间），并把该集推进为「已看」。
///
/// 只在播放**成功启动**后调用，因此不存在「点了没播成却算看过」的情况。
pub fn record_play(
    conn: &Connection,
    media_id: i64,
    episode_id: Option<i64>,
) -> Result<(), String> {
    let ts = now_epoch();
    conn.execute(
        "INSERT INTO playback_state \
             (media_id, episode_id, watched, play_count, last_played_at, updated_at) \
         VALUES (?1, ?2, 0, 1, ?3, ?3) \
         ON CONFLICT(media_id, IFNULL(episode_id, 0)) DO UPDATE SET \
             play_count = play_count + 1, \
             last_played_at = excluded.last_played_at, \
             updated_at = excluded.updated_at",
        params![media_id, episode_id, ts],
    )
    .map_err(|e| format!("记录播放失败: {e}"))?;
    Ok(())
}

/// 手动填写 / 修正播放位置（秒）。
///
/// 本期不自动采集，但允许用户在详情页手动改——成本极低，却能覆盖自动推进
/// 偶尔判错的情况（比如播放器没起来、或者看了半集就关了）。
pub fn set_position(
    conn: &Connection,
    media_id: i64,
    episode_id: Option<i64>,
    position_secs: Option<i64>,
    duration_secs: Option<i64>,
) -> Result<(), String> {
    let ts = now_epoch();
    conn.execute(
        "INSERT INTO playback_state \
             (media_id, episode_id, watched, play_count, position_secs, duration_secs, updated_at) \
         VALUES (?1, ?2, 0, 0, ?3, ?4, ?5) \
         ON CONFLICT(media_id, IFNULL(episode_id, 0)) DO UPDATE SET \
             position_secs = excluded.position_secs, \
             duration_secs = excluded.duration_secs, \
             updated_at = excluded.updated_at",
        params![media_id, episode_id, position_secs, duration_secs, ts],
    )
    .map_err(|e| format!("写入播放位置失败: {e}"))?;
    Ok(())
}

/// 读一条原始状态。
pub fn get_row(
    conn: &Connection,
    media_id: i64,
    episode_id: Option<i64>,
) -> Result<Option<PlaybackRow>, String> {
    conn.query_row(
        "SELECT media_id, episode_id, watched, position_secs, duration_secs, play_count, last_played_at \
         FROM playback_state WHERE media_id = ?1 AND IFNULL(episode_id, 0) = IFNULL(?2, 0)",
        params![media_id, episode_id],
        |r| {
            Ok(PlaybackRow {
                media_id: r.get(0)?,
                episode_id: r.get(1)?,
                watched: r.get::<_, i64>(2)? != 0,
                position_secs: r.get(3)?,
                duration_secs: r.get(4)?,
                play_count: r.get(5)?,
                last_played_at: r.get(6)?,
            })
        },
    )
    .optional()
    .map_err(|e| e.to_string())
}

/// 汇总一个条目（含其单集）的观看摘要。
pub fn summarize(conn: &Connection, media_id: i64) -> Result<MediaPlayback, String> {
    let mut out = MediaPlayback {
        media_id,
        ..Default::default()
    };

    // 整部级别的状态（电影就是它本身；剧集也可能有整部标记）
    let own = get_row(conn, media_id, None)?;
    if let Some(o) = &own {
        out.play_count = o.play_count;
        out.last_played_at = o.last_played_at;
    }

    // 剧集：按季集顺序取单集列表 + 各自的已看标记
    let mut stmt = conn
        .prepare(
            "SELECT e.id, e.season, e.episode, IFNULL(p.watched, 0), IFNULL(p.play_count, 0), p.last_played_at
             FROM episodes e
             LEFT JOIN playback_state p
               ON p.media_id = e.media_id AND p.episode_id = e.id
             WHERE e.media_id = ?1
             ORDER BY e.season, e.episode, e.id",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![media_id], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, i64>(3)? != 0,
                r.get::<_, i64>(4)?,
                r.get::<_, Option<i64>>(5)?,
            ))
        })
        .map_err(|e| e.to_string())?;

    let mut episodes: Vec<(i64, i64, i64, bool, i64, Option<i64>)> = Vec::new();
    for r in rows {
        episodes.push(r.map_err(|e| e.to_string())?);
    }
    if episodes.is_empty() {
        // 电影 / 待确认：直接看整部标记
        out.watched = own.as_ref().map(|o| o.watched).unwrap_or(false);
        return Ok(out);
    }

    out.total_episodes = episodes.len() as i64;
    out.watched_episodes = episodes.iter().filter(|e| e.3).count() as i64;
    // 「整部标记」与「集集都看过」是**或**的关系，缺一不可：
    // - 只看集数 → 整部标记那一行不参与，剧集点「标记已看」毫无反应（历史 bug）；
    // - 只看整部标记 → 逐集看完但没点整部的条目会被显示成没看过。
    let own_watched = own.as_ref().map(|o| o.watched).unwrap_or(false);
    out.watched =
        own_watched || (out.total_episodes > 0 && out.watched_episodes >= out.total_episodes);
    // 整部累计播放次数也包括各集的
    out.play_count += episodes.iter().map(|e| e.4).sum::<i64>();

    // 最近播放时间取「整部」与「各集」里最大的那个
    let latest = episodes
        .iter()
        .filter_map(|e| e.5)
        .max()
        .or(own.as_ref().and_then(|o| o.last_played_at));
    if latest.is_some() {
        out.last_played_at = latest;
    }

    // 「继续播放」：第一个未看集；全看完了则回到第一集（重刷）
    out.resume_episode_id = episodes
        .iter()
        .find(|e| !e.3)
        .map(|e| e.0)
        .or_else(|| episodes.first().map(|e| e.0));

    // 最近看过的那一集：优先按「最近播放时间」；没有时间戳时（比如用户是手动
    // 勾的已看，而不是真播放过）退回集号最大的已看集——否则「看到第几集」永远空白。
    let last = episodes
        .iter()
        .filter(|e| e.3 && e.5.is_some())
        .max_by_key(|e| e.5.unwrap_or(0))
        .map(|e| (e.0, e.1, e.2))
        .or_else(|| {
            episodes
                .iter()
                .filter(|e| e.3)
                .max_by_key(|e| (e.1, e.2))
                .map(|e| (e.0, e.1, e.2))
        });
    if let Some((id, s, ep)) = last {
        out.last_episode_id = Some(id);
        out.last_season = Some(s);
        out.last_episode = Some(ep);
    }

    // 上次**真正播放**过的那一集：只看 `last_played_at`，与是否已看无关。
    //
    // 必须排在 `last_episode_id` 之后单独算，理由见字段注释：`record_play`
    // 不写 `watched`，而上面那段要求 `e.3`（已看）为真 —— 只播不标记时它会空。
    // 这里再退一步：连播放时间都没有（用户是手勾的已看），就沿用「最近看过」那一集，
    // 让双击播放永远有一个确定目标。
    out.last_played_episode_id = episodes
        .iter()
        .filter(|e| e.5.is_some())
        .max_by_key(|e| (e.5.unwrap_or(0), e.1, e.2))
        .map(|e| e.0)
        .or(out.last_episode_id);
    Ok(out)
}

/// 批量汇总（列表页一次拿全，避免 N+1 查询）。
pub fn summarize_many(
    conn: &Connection,
    media_ids: &[i64],
) -> Result<BTreeMap<i64, MediaPlayback>, String> {
    let mut out = BTreeMap::new();
    for id in media_ids {
        out.insert(*id, summarize(conn, *id)?);
    }
    Ok(out)
}

/// 最近播放过的条目（「继续观看」列表用）。
pub fn recent(conn: &Connection, limit: i64) -> Result<Vec<MediaPlayback>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT DISTINCT media_id FROM playback_state
             WHERE last_played_at IS NOT NULL
             ORDER BY last_played_at DESC LIMIT ?1",
        )
        .map_err(|e| e.to_string())?;
    let ids = stmt
        .query_map(params![limit], |r| r.get::<_, i64>(0))
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for id in ids {
        let id = id.map_err(|e| e.to_string())?;
        out.push(summarize(conn, id)?);
    }
    Ok(out)
}

/// 清掉某条目（及其单集）的全部观看状态。
pub fn clear(conn: &Connection, media_id: i64) -> Result<usize, String> {
    let n = conn
        .execute("DELETE FROM playback_state WHERE media_id = ?1", params![media_id])
        .map_err(|e| e.to_string())?;
    Ok(n)
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

    fn add_movie(conn: &Connection, title: &str) -> i64 {
        conn.execute(
            "INSERT INTO media (type, title, added_at, updated_at) VALUES ('movie',?1,0,0)",
            params![title],
        )
        .unwrap();
        conn.last_insert_rowid()
    }

    fn add_series_with_episodes(conn: &Connection, n: i64) -> (i64, Vec<i64>) {
        conn.execute(
            "INSERT INTO media (type, title, added_at, updated_at) VALUES ('tv','剧',0,0)",
            [],
        )
        .unwrap();
        let mid = conn.last_insert_rowid();
        let mut ids = Vec::new();
        for i in 1..=n {
            conn.execute(
                "INSERT INTO episodes (media_id, season, episode, added_at) VALUES (?1,1,?2,0)",
                params![mid, i],
            )
            .unwrap();
            ids.push(conn.last_insert_rowid());
        }
        (mid, ids)
    }

    /// 某条目在 `playback_state` 里的行数。
    ///
    /// 用来确认级联既没造出重复行（撞唯一索引），也没漏掉该建的集级行。
    fn count_playback_rows(conn: &Connection, media_id: i64) -> i64 {
        conn.query_row(
            "SELECT COUNT(*) FROM playback_state WHERE media_id = ?1",
            params![media_id],
            |r| r.get(0),
        )
        .unwrap()
    }

    #[test]
    fn movie_watched_flag_round_trips() {
        let conn = db();
        let id = add_movie(&conn, "卧虎藏龙");
        assert!(!summarize(&conn, id).unwrap().watched);
        set_watched(&conn, id, None, true).unwrap();
        assert!(summarize(&conn, id).unwrap().watched);
        set_watched(&conn, id, None, false).unwrap();
        assert!(!summarize(&conn, id).unwrap().watched);
    }

    #[test]
    fn series_only_watched_when_all_episodes_watched() {
        let conn = db();
        let (mid, eps) = add_series_with_episodes(&conn, 3);
        set_watched(&conn, mid, Some(eps[0]), true).unwrap();
        let s = summarize(&conn, mid).unwrap();
        assert_eq!(s.watched_episodes, 1);
        assert_eq!(s.total_episodes, 3);
        assert!(!s.watched, "只看了 1 集不能算整部看完");

        set_watched(&conn, mid, Some(eps[1]), true).unwrap();
        set_watched(&conn, mid, Some(eps[2]), true).unwrap();
        let s = summarize(&conn, mid).unwrap();
        assert!(s.watched);
    }

    /// 「继续播放」要指向**第一个未看集**，这是续播的核心语义。
    #[test]
    fn resume_points_to_first_unwatched_episode() {
        let conn = db();
        let (mid, eps) = add_series_with_episodes(&conn, 4);
        set_watched(&conn, mid, Some(eps[0]), true).unwrap();
        set_watched(&conn, mid, Some(eps[1]), true).unwrap();

        let s = summarize(&conn, mid).unwrap();
        assert_eq!(s.resume_episode_id, Some(eps[2]));
        assert_eq!(s.last_episode_id, Some(eps[1]), "最近看过的是第 2 集");
        assert_eq!(s.last_episode, Some(2));
    }

    /// 全部看完后仍要给一个可播目标（回到第一集重刷），不能返回 None 让按钮消失。
    #[test]
    fn resume_falls_back_to_first_episode_when_all_watched() {
        let conn = db();
        let (mid, eps) = add_series_with_episodes(&conn, 2);
        for e in &eps {
            set_watched(&conn, mid, Some(*e), true).unwrap();
        }
        let s = summarize(&conn, mid).unwrap();
        assert_eq!(s.resume_episode_id, Some(eps[0]));
        assert!(s.watched);
    }

    #[test]
    fn record_play_accumulates_count_and_time() {
        let conn = db();
        let id = add_movie(&conn, "电影");
        record_play(&conn, id, None).unwrap();
        record_play(&conn, id, None).unwrap();
        let s = summarize(&conn, id).unwrap();
        assert_eq!(s.play_count, 2);
        assert!(s.last_played_at.is_some());
    }

    /// 同一集重复标记不能撞唯一索引（upsert 语义）。
    #[test]
    fn set_watched_twice_is_an_upsert() {
        let conn = db();
        let id = add_movie(&conn, "电影");
        set_watched(&conn, id, None, true).unwrap();
        set_watched(&conn, id, None, true).unwrap();
        assert!(get_row(&conn, id, None).unwrap().unwrap().watched);
    }

    #[test]
    fn clear_removes_everything_for_that_media() {
        let conn = db();
        let (mid, eps) = add_series_with_episodes(&conn, 2);
        set_watched(&conn, mid, Some(eps[0]), true).unwrap();
        // 整部标记会级联到每一集：整部 1 行 + 2 集各 1 行（第 1 集是 UPDATE）＝ 3 行。
        set_watched(&conn, mid, None, true).unwrap();
        assert_eq!(clear(&conn, mid).unwrap(), 3, "整部 + 每一集共 3 行都应被清掉");
        assert_eq!(summarize(&conn, mid).unwrap().watched_episodes, 0);
        assert!(!summarize(&conn, mid).unwrap().watched);
    }

    /// 删掉剧集时观看状态必须跟着走（外键级联），否则会留下一堆孤儿行。
    #[test]
    fn playback_rows_cascade_on_media_delete() {
        let conn = db();
        let (mid, eps) = add_series_with_episodes(&conn, 1);
        set_watched(&conn, mid, Some(eps[0]), true).unwrap();
        conn.execute("DELETE FROM media WHERE id = ?1", params![mid])
            .unwrap();
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM playback_state", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0, "外键级联没生效，播放状态成了孤儿");
    }

    #[test]
    fn recent_orders_by_last_played_desc() {
        let conn = db();
        let a = add_movie(&conn, "A");
        let b = add_movie(&conn, "B");
        record_play(&conn, a, None).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1100));
        record_play(&conn, b, None).unwrap();
        let r = recent(&conn, 10).unwrap();
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].media_id, b, "最近播放的应排在最前");
    }

    /// 手动修正位置：预留字段要能读写（本期不自动采集，但用户可填）。
    #[test]
    fn manual_position_can_be_set_and_read() {
        let conn = db();
        let id = add_movie(&conn, "电影");
        set_position(&conn, id, None, Some(600), Some(7200)).unwrap();
        let row = get_row(&conn, id, None).unwrap().unwrap();
        assert_eq!(row.position_secs, Some(600));
        assert_eq!(row.duration_secs, Some(7200));
    }

    /* ---------------- 整部标记 ↔ 单集级联（「看过按钮点了没反应」的回归） ---------------- */

    /// 整部标记必须级联到每一集。
    ///
    /// 只让 `summarize` 认整部标记是不够的：进度条会停在 `0/25`，
    /// 出现「整部已看但进度是 0」的错位。级联后标记已看 = 25/25。
    #[test]
    fn series_whole_mark_cascades_progress_to_every_episode() {
        let conn = db();
        let (mid, _eps) = add_series_with_episodes(&conn, 25);
        set_watched(&conn, mid, None, true).unwrap();

        let s = summarize(&conn, mid).unwrap();
        assert!(s.watched, "整部标记必须让剧集显示为已看（按钮要有反应）");
        assert_eq!(s.total_episodes, 25);
        assert_eq!(s.watched_episodes, 25, "标记已看后进度必须是 25/25，而不是 0/25");
        assert!(s.resume_episode_id.is_some(), "全部看完也仍要给一个可播目标");
    }

    /// 取消整部标记：整部行 + 每一集都要回到未看，进度归零。
    #[test]
    fn series_whole_unmark_resets_progress_to_zero() {
        let conn = db();
        let (mid, eps) = add_series_with_episodes(&conn, 4);
        set_watched(&conn, mid, None, true).unwrap();
        set_watched(&conn, mid, None, false).unwrap();

        let s = summarize(&conn, mid).unwrap();
        assert!(!s.watched, "取消整部标记后必须显示为未看");
        assert_eq!(s.watched_episodes, 0, "取消后进度必须归零，而不是停在 4/4");
        assert_eq!(s.total_episodes, 4);
        // 每一集的行都要真的被写成未看，而不只是改了整部那一行
        for e in &eps {
            let row = get_row(&conn, mid, Some(*e)).unwrap().unwrap();
            assert!(!row.watched, "第 {e} 集没有被级联取消");
        }
        assert!(!get_row(&conn, mid, None).unwrap().unwrap().watched);
    }

    /// 整部标记重复调用必须幂等：标记两次、取消两次都不出错，值也正确。
    #[test]
    fn series_whole_mark_toggle_is_idempotent() {
        let conn = db();
        let (mid, _eps) = add_series_with_episodes(&conn, 3);

        set_watched(&conn, mid, None, true).unwrap();
        set_watched(&conn, mid, None, true).unwrap();
        let s = summarize(&conn, mid).unwrap();
        assert!(s.watched);
        assert_eq!(s.watched_episodes, 3);

        set_watched(&conn, mid, None, false).unwrap();
        set_watched(&conn, mid, None, false).unwrap();
        let s = summarize(&conn, mid).unwrap();
        assert!(!s.watched);
        assert_eq!(s.watched_episodes, 0);

        // 反复切换后行数不能膨胀（唯一索引 + upsert 生效）
        assert_eq!(
            count_playback_rows(&conn, mid),
            4,
            "整部 1 行 + 3 集 3 行，反复切换不应产生重复行"
        );
    }

    /// 逐集看完但**没点过整部标记**：仍要算看过（或关系的另一半）。
    #[test]
    fn series_watched_when_every_episode_marked_individually() {
        let conn = db();
        let (mid, eps) = add_series_with_episodes(&conn, 3);
        for e in &eps {
            set_watched(&conn, mid, Some(*e), true).unwrap();
        }
        let s = summarize(&conn, mid).unwrap();
        assert!(s.watched, "每一集都手动标记过就应当算看过");
        assert_eq!(s.watched_episodes, 3);
        // 没点过整部标记 → 不该凭空多出一行整部状态
        assert!(get_row(&conn, mid, None).unwrap().is_none());
    }

    /// 电影（没有 episodes）走整部标记：级联是空操作，不能凭空造出集级行。
    #[test]
    fn movie_whole_mark_creates_no_episode_rows() {
        let conn = db();
        let id = add_movie(&conn, "教父");
        set_watched(&conn, id, None, true).unwrap();
        let s = summarize(&conn, id).unwrap();
        assert!(s.watched);
        assert_eq!(s.total_episodes, 0);
        assert_eq!(s.watched_episodes, 0);
        assert_eq!(
            count_playback_rows(&conn, id),
            1,
            "电影只有整部一行，级联不应写入任何集级行"
        );

        set_watched(&conn, id, None, false).unwrap();
        assert!(!summarize(&conn, id).unwrap().watched);
        assert_eq!(count_playback_rows(&conn, id), 1, "取消也不该新增行");
    }

    /// 单集标记**不级联**：标记第 1 集不能把第 2、3 集也带上。
    #[test]
    fn episode_mark_does_not_cascade_to_siblings() {
        let conn = db();
        let (mid, eps) = add_series_with_episodes(&conn, 3);
        set_watched(&conn, mid, Some(eps[0]), true).unwrap();

        let s = summarize(&conn, mid).unwrap();
        assert_eq!(s.watched_episodes, 1, "只应标记第 1 集");
        assert_eq!(s.total_episodes, 3);
        assert!(!s.watched, "单集标记不应把整部判成已看");
        assert!(
            get_row(&conn, mid, Some(eps[1])).unwrap().is_none(),
            "第 2 集被误标了：单集标记不该级联"
        );
        assert!(
            get_row(&conn, mid, Some(eps[2])).unwrap().is_none(),
            "第 3 集被误标了：单集标记不该级联"
        );
        assert!(
            get_row(&conn, mid, None).unwrap().is_none(),
            "单集标记不应产生整部行"
        );

        // 取消单集同样只影响这一集
        set_watched(&conn, mid, Some(eps[0]), false).unwrap();
        assert_eq!(summarize(&conn, mid).unwrap().watched_episodes, 0);
        assert!(get_row(&conn, mid, None).unwrap().is_none());
    }

    /// 级联要给**还没有 playback_state 行**的集 INSERT 新行（不只是 UPDATE 已有的）。
    #[test]
    fn cascade_inserts_rows_for_episodes_without_state() {
        let conn = db();
        let (mid, eps) = add_series_with_episodes(&conn, 3);
        // 只给第 1 集建行，第 2/3 集是「还没有状态行」的集
        set_watched(&conn, mid, Some(eps[0]), true).unwrap();

        set_watched(&conn, mid, None, true).unwrap();
        let s = summarize(&conn, mid).unwrap();
        assert_eq!(s.watched_episodes, 3, "级联必须给没有状态行的集补上行");
        assert_eq!(count_playback_rows(&conn, mid), 4, "整部 1 行 + 3 集 3 行");
        for e in &eps {
            assert!(
                get_row(&conn, mid, Some(*e)).unwrap().unwrap().watched,
                "第 {e} 集没有被级联标记"
            );
        }
    }

    /// 整部标记之后**新入库**的集：再标记一次要能覆盖到它们。
    ///
    /// 这里顺带钉死一个必须知道的行为：`watched` 是「整部标记 **OR** 集集都看过」，
    /// 所以整部标记是**粘的**——新扫进来的集不会把整部打回未看。这是刻意的取舍：
    /// 换成 AND 的话，「逐集看完但没点过整部」就会被误判成没看过（回到原 bug）。
    /// 粘性的代价由界面补上——进度条如实显示 2/3，续播指向新集，用户想重算就再点一次。
    #[test]
    fn cascade_covers_episodes_added_later() {
        let conn = db();
        let (mid, _eps) = add_series_with_episodes(&conn, 2);
        set_watched(&conn, mid, None, true).unwrap();
        assert_eq!(summarize(&conn, mid).unwrap().watched_episodes, 2);

        // 新扫进来第 3 集：它还没有状态行
        conn.execute(
            "INSERT INTO episodes (media_id, season, episode, added_at) VALUES (?1,1,3,0)",
            params![mid],
        )
        .unwrap();
        let new_ep = conn.last_insert_rowid();
        let s = summarize(&conn, mid).unwrap();
        assert_eq!(s.total_episodes, 3);
        assert_eq!(s.watched_episodes, 2, "新集未看，进度要如实显示 2/3");
        assert_eq!(s.resume_episode_id, Some(new_ep), "续播要指向新扫进来的那一集");

        // 再点一次「已看过」→ 覆盖到新集，变成 3/3
        set_watched(&conn, mid, None, true).unwrap();
        let s = summarize(&conn, mid).unwrap();
        assert_eq!(s.watched_episodes, 3, "再次整部标记必须覆盖到新入库的集");
        assert!(s.watched);
    }

    /// 级联只改 `watched`，不能把播放次数 / 最近播放时间 / 手动位置抹掉。
    #[test]
    fn cascade_preserves_play_count_and_manual_position() {
        let conn = db();
        let (mid, eps) = add_series_with_episodes(&conn, 2);
        record_play(&conn, mid, Some(eps[0])).unwrap();
        set_position(&conn, mid, Some(eps[0]), Some(300), Some(1200)).unwrap();

        set_watched(&conn, mid, None, true).unwrap();
        let row = get_row(&conn, mid, Some(eps[0])).unwrap().unwrap();
        assert!(row.watched, "级联要把这一集标成已看");
        assert_eq!(row.play_count, 1, "级联不能把播放次数清零");
        assert!(row.last_played_at.is_some(), "级联不能抹掉最近播放时间");
        assert_eq!(row.position_secs, Some(300), "级联不能抹掉手动填写的进度");
        assert_eq!(row.duration_secs, Some(1200));

        // 取消整部也不该动这些字段
        set_watched(&conn, mid, None, false).unwrap();
        let row = get_row(&conn, mid, Some(eps[0])).unwrap().unwrap();
        assert!(!row.watched);
        assert_eq!(row.play_count, 1);
        assert_eq!(row.position_secs, Some(300));
    }
}
