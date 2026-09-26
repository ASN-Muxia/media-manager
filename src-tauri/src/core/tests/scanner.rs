//! `scanner.rs` 的单元测试。
//!
//! 用 `#[path]` 挂回所属模块（见 `scanner.rs` 末尾），
//! 这样生产代码文件里只剩业务实现，单文件体积能降一大截，
//! 而 `use super::*` 仍能访问到私有成员，可见性一个字没改。

use super::*;
use crate::store::db;

fn temp_dir(tag: &str) -> PathBuf {
    let mut d = std::env::temp_dir();
    let uniq = format!(
        "mm-scan-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    d.push(uniq);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn touch(dir: &Path, rel: &str) {
    let p = dir.join(rel);
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(&p, b"dummy").unwrap();
}

fn memory_db() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
    conn.execute_batch(db::SCHEMA).unwrap();
    conn
}

#[test]
fn collects_only_video_files_and_skips_hidden_dirs() {
    let d = temp_dir("collect");
    touch(&d, "The.Matrix.1999.1080p.mkv");
    touch(&d, "notes.txt");
    touch(&d, "cover.jpg");
    touch(&d, "Season 1/Show.S01E01.mkv");
    touch(&d, "Season 1/Show.S01E02.mkv");
    touch(&d, ".hidden/secret.mkv");
    touch(&d, "subs/Show.S01E01.srt");

    let files = collect_files(&[d.to_string_lossy().to_string()]);
    let names: Vec<_> = files.iter().map(|f| f.name.clone()).collect();
    assert_eq!(files.len(), 3, "实际收集到 {names:?}");
    assert!(names.iter().all(|n| !n.ends_with(".txt")));
    assert!(names.iter().all(|n| !n.ends_with(".srt")));
    assert!(!names.contains(&"secret.mkv".to_string()), "隐藏目录应被跳过");

    std::fs::remove_dir_all(&d).ok();
}

#[test]
fn nonexistent_root_is_ignored() {
    let files = collect_files(&["Z:\\definitely\\not\\here".to_string()]);
    assert!(files.is_empty());
}

#[test]
fn import_classifies_movies_and_groups_series() {
    let d = temp_dir("import");
    touch(&d, "The.Matrix.1999.1080p.BluRay.x264.mkv");
    touch(&d, "Inception.2010.1080p.mkv");
    touch(&d, "Breaking.Bad.S01E01.720p.mkv");
    touch(&d, "Breaking.Bad.S01E02.720p.mkv");
    touch(&d, "Breaking.Bad.S02E01.720p.mkv");

    let mut conn = memory_db();
    let roots = vec![d.to_string_lossy().to_string()];
    let files = collect_files(&roots);
    let stats = import(&mut conn, &files, &roots).expect("导入应成功");

    assert_eq!(stats.scanned, 5);
    assert_eq!(stats.added_movies, 2, "两部电影");
    assert_eq!(stats.added_series, 1, "一个剧集标题");
    assert_eq!(stats.added_episodes, 3, "三集");

    let tv_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM media WHERE type='tv'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(tv_count, 1, "剧集只应有一条 media 主记录（分组）");

    let ep_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM episodes", [], |r| r.get(0))
        .unwrap();
    assert_eq!(ep_count, 3);

    let seasons: Vec<i64> = {
        let mut stmt = conn
            .prepare("SELECT DISTINCT season FROM episodes ORDER BY season")
            .unwrap();
        let rows = stmt.query_map([], |r| r.get(0)).unwrap();
        rows.map(|r| r.unwrap()).collect()
    };
    assert_eq!(seasons, vec![1, 2], "应识别出两季");

    let history: i64 = conn
        .query_row("SELECT COUNT(*) FROM scan_history WHERE status='done'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(history, 1, "应写入一条完成的扫描记录");

    std::fs::remove_dir_all(&d).ok();
}

#[test]
fn rescan_is_idempotent() {
    let d = temp_dir("rescan");
    touch(&d, "The.Matrix.1999.1080p.mkv");
    touch(&d, "Show.S01E01.mkv");

    let mut conn = memory_db();
    let roots = vec![d.to_string_lossy().to_string()];

    let files = collect_files(&roots);
    let first = import(&mut conn, &files, &roots).expect("首次导入应成功");
    assert_eq!(first.added_movies, 1);
    assert_eq!(first.added_episodes, 1);

    let files2 = collect_files(&roots);
    let second = import(&mut conn, &files2, &roots).expect("重复导入应成功");
    assert_eq!(second.added_movies, 0, "重复扫描不应重复入库");
    assert_eq!(second.added_episodes, 0);
    assert_eq!(second.skipped, 2, "两个文件都应被计为已存在");

    let total: i64 = conn
        .query_row("SELECT COUNT(*) FROM media", [], |r| r.get(0))
        .unwrap();
    assert_eq!(total, 2);

    std::fs::remove_dir_all(&d).ok();
}

#[test]
fn empty_roots_returns_error() {
    let mut conn = memory_db();
    assert!(scan_and_import(&mut conn, &[]).is_err());
}

/// 回归：刮削会把显示标题改成 TMDB 标题，此时重扫不能新建重复剧集
/// （分组匹配同时认 original_title）。
#[test]
fn rescan_after_title_change_does_not_duplicate_series() {
    let d = temp_dir("rename");
    touch(&d, "Breaking.Bad.S01E01.mkv");
    let mut conn = memory_db();
    let roots = vec![d.to_string_lossy().to_string()];

    let files = collect_files(&roots);
    import(&mut conn, &files, &roots).expect("首次导入");
    conn.execute("UPDATE media SET title = '绝命毒师' WHERE type = 'tv'", [])
        .unwrap();

    let files2 = collect_files(&roots);
    let second = import(&mut conn, &files2, &roots).expect("二次导入");
    assert_eq!(second.added_series, 0, "标题被改写后重扫不应新建剧集");

    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM media WHERE type='tv'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 1);

    std::fs::remove_dir_all(&d).ok();
}

/// 同名剧集放在不同目录里，必须各自成条，不能并成一条。
#[test]
fn same_title_series_in_different_folders_are_not_merged() {
    let d = temp_dir("dup");
    touch(&d, "Show A/Show.S01E01.mkv");
    touch(&d, "Show A/Show.S01E02.mkv");
    touch(&d, "Show B/Show.S01E01.mkv");

    let mut conn = memory_db();
    let roots = vec![d.to_string_lossy().to_string()];
    let files = collect_files(&roots);
    let stats = import(&mut conn, &files, &roots).expect("导入应成功");

    assert_eq!(stats.added_series, 2, "同名不同目录的两部剧不应合并");
    let tv: i64 = conn
        .query_row("SELECT COUNT(*) FROM media WHERE type='tv'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(tv, 2);

    // 单集各自挂到各自的主记录上：按分组键校验，不依赖插入顺序
    let mut stmt = conn
        .prepare(
            "SELECT m.group_key, COUNT(e.id) FROM media m \
             LEFT JOIN episodes e ON e.media_id = m.id \
             WHERE m.type = 'tv' GROUP BY m.id ORDER BY m.group_key",
        )
        .unwrap();
    let per: Vec<(String, i64)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    assert_eq!(
        per,
        vec![("Show A".to_string(), 2), ("Show B".to_string(), 1)],
        "Show A 两集、Show B 一集，且各自挂在自己的主记录上"
    );

    std::fs::remove_dir_all(&d).ok();
}

/// 分组键取「根目录下的第一层目录名」；文件直接在根目录时退回解析标题。
#[test]
fn group_key_comes_from_first_folder_under_root() {
    let d = temp_dir("gk");
    touch(&d, "Breaking Bad/Season 1/Breaking.Bad.S01E01.mkv");
    touch(&d, "Breaking Bad/Season 2/Breaking.Bad.S02E01.mkv");
    touch(&d, "Loose.Show.S01E01.mkv");

    let roots = vec![d.to_string_lossy().to_string()];
    let files = collect_files(&roots);
    let key_of = |n: &str| {
        files
            .iter()
            .find(|f| f.name == n)
            .unwrap_or_else(|| panic!("未收集到 {n}"))
            .group_key
            .clone()
    };

    assert_eq!(
        key_of("Breaking.Bad.S01E01.mkv").as_deref(),
        Some("Breaking Bad")
    );
    assert_eq!(
        key_of("Breaking.Bad.S02E01.mkv").as_deref(),
        Some("Breaking Bad"),
        "同一部剧的不同季应落到同一个分组键"
    );
    assert_eq!(
        key_of("Loose.Show.S01E01.mkv").as_deref(),
        Some("Loose Show"),
        "根目录下的文件应退回解析出的标题"
    );

    std::fs::remove_dir_all(&d).ok();
}

/// 增强层默认关闭：不开时对扫描结果**零影响**（这是默认行为，必须锁住）。
#[test]
fn llm_enhancement_is_noop_when_disabled() {
    let mut files = vec![ScanFile {
        path: PathBuf::from("/x/00123.mkv"),
        name: "00123.mkv".to_string(),
        size: 1,
        parsed: parse_file_name("00123.mkv"),
        kind: MediaKind::Unknown,
        root_index: 0,
        folder: None,
        group_key: None,
        resolution: None,
        release_source: None,
    }];
    let before = files[0].parsed.clone();
    let improved = apply_llm_enhancement(&mut files, &crate::provider::llm::LlmConfig::default(), &[]);
    assert_eq!(improved, 0);
    assert_eq!(files[0].parsed.title, before.title, "未启用时不应改动解析结果");
    assert!(files[0].resolution.is_none());
    assert!(files[0].release_source.is_none());
}

/// 增强层补出来的分辨率 / 来源要真的落库（否则这两个字段永远是空的）
#[test]
fn resolution_and_source_are_persisted() {
    let mut conn = memory_db();
    let files = vec![ScanFile {
        path: PathBuf::from("/x/Ep01.mkv"),
        name: "My.Show.S01E01.mkv".to_string(),
        size: 1,
        parsed: parse_file_name("My.Show.S01E01.mkv"),
        kind: MediaKind::Tv,
        root_index: 0,
        folder: None,
        group_key: Some("My Show".to_string()),
        resolution: Some("1080p".to_string()),
        release_source: Some("WEB-DL".to_string()),
    }];
    let roots = vec!["/x".to_string()];
    let _ = import(&mut conn, &files, &roots).expect("导入应成功");

    let (res, src): (Option<String>, Option<String>) = conn
        .query_row(
            "SELECT resolution, release_source FROM episodes LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(res.as_deref(), Some("1080p"));
    assert_eq!(src.as_deref(), Some("WEB-DL"));
}

/// 升级前的老行没有 group_key，重扫时应复用并自愈回填，而不是新建重复条目。
#[test]
fn legacy_series_row_is_healed_with_group_key_on_rescan() {
    let d = temp_dir("heal");
    touch(&d, "Breaking.Bad.S01E01.mkv");
    let roots = vec![d.to_string_lossy().to_string()];

    let mut conn = memory_db();
    conn.execute(
        "INSERT INTO media (type, title, year, added_at, updated_at) \
         VALUES ('tv','Breaking Bad',2008,1,1)",
        [],
    )
    .unwrap();
    let legacy_id = conn.last_insert_rowid();
    conn.execute(
        "INSERT INTO episodes (media_id, season, episode, file_path, file_name, added_at) \
         VALUES (?1, 1, 1, 'D:\\\\legacy.mkv', 'Breaking.Bad.S01E01.mkv', 1)",
        params![legacy_id],
    )
    .unwrap();

    let files = collect_files(&roots);
    let stats = import(&mut conn, &files, &roots).expect("导入应成功");

    assert_eq!(stats.added_series, 0, "应复用老行而不是新建");
    let key: Option<String> = conn
        .query_row(
            "SELECT group_key FROM media WHERE id = ?1",
            params![legacy_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(key.as_deref(), Some("Breaking Bad"), "老行应被回填分组键");

    std::fs::remove_dir_all(&d).ok();
}

/// 端到端：按季分目录存放的同一部剧，二次校正后应并成**一个** Series。
#[test]
fn seasons_in_sibling_folders_are_merged_into_one_series() {
    let d = temp_dir("seasons");
    touch(&d, "Show Season 1/Show.S01E01.mkv");
    touch(&d, "Show Season 1/Show.S01E02.mkv");
    touch(&d, "Show Season 2/Show.S02E01.mkv");

    let mut conn = memory_db();
    let roots = vec![d.to_string_lossy().to_string()];
    let files = collect_files(&roots);
    let stats = import(&mut conn, &files, &roots).expect("导入应成功");

    assert_eq!(stats.added_series, 1, "同一部剧不应被拆成多条：{stats:?}");
    assert_eq!(stats.added_episodes, 3);
    let tv: i64 = conn
        .query_row("SELECT COUNT(*) FROM media WHERE type='tv'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(tv, 1);

    std::fs::remove_dir_all(&d).ok();
}

/// 端到端：一个目录里混了两部剧（编号冲突），应拆成两个 Series。
#[test]
fn one_folder_holding_two_shows_is_split() {
    let d = temp_dir("mixed");
    touch(&d, "Mix/Alpha.S01E01.mkv");
    touch(&d, "Mix/Alpha.S01E02.mkv");
    touch(&d, "Mix/Beta.S01E01.mkv");
    touch(&d, "Mix/Beta.S01E02.mkv");

    let mut conn = memory_db();
    let roots = vec![d.to_string_lossy().to_string()];
    let files = collect_files(&roots);
    let stats = import(&mut conn, &files, &roots).expect("导入应成功");

    assert_eq!(stats.added_series, 2, "编号冲突应拆开：{stats:?}");
    let keys: Vec<String> = {
        let mut stmt = conn
            .prepare("SELECT group_key FROM media WHERE type='tv' ORDER BY group_key")
            .unwrap();
        stmt.query_map([], |r| r.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect()
    };
    assert_eq!(keys.len(), 2);
    assert_ne!(keys[0], keys[1], "拆分后的组键不能撞车：{keys:?}");

    std::fs::remove_dir_all(&d).ok();
}

/* ---------------- 聚合回归用例（Phase F） ----------------
 *
 * 这三个用例对应「一部剧被拆成多个条目」的三类成因：
 *   A. 不同年份版本 / 特辑 / 命名差异（神秘博士）
 *   B. 无标记的纯编号文件（胜利正义）
 *   C. 集数极多的动画，文件名是纯数字或带季标记（哆啦A梦）
 */

fn tv_rows(conn: &Connection) -> Vec<(String, i64)> {
    let mut stmt = conn
        .prepare(
            "SELECT m.group_key, COUNT(e.id) FROM media m \
             LEFT JOIN episodes e ON e.media_id = m.id \
             WHERE m.type = 'tv' GROUP BY m.id ORDER BY m.group_key",
        )
        .unwrap();
    stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))
        .unwrap()
        .map(|r| r.unwrap())
        .collect()
}

fn episodes_of(conn: &Connection) -> Vec<(i64, i64)> {
    let mut stmt = conn
        .prepare("SELECT season, episode FROM episodes ORDER BY season, episode")
        .unwrap();
    stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))
        .unwrap()
        .map(|r| r.unwrap())
        .collect()
}

/// A. 神秘博士：2005 版 + 2023 特辑 + 按季分目录，必须并成**一部剧**。
#[test]
fn doctor_who_year_variants_and_specials_stay_one_series() {
    let d = temp_dir("doctorwho");
    touch(&d, "神秘博士 (2005)/神秘博士.S01E01.1080p.mkv");
    touch(&d, "神秘博士 (2005)/神秘博士.S01E02.1080p.mkv");
    touch(&d, "神秘博士 (2023)/神秘博士.2023.特辑.1080p.mkv");
    touch(&d, "神秘博士 (2023)/神秘博士.2023.特辑2.1080p.mkv");
    touch(&d, "神秘博士 第二季/神秘博士.S02E01.1080p.mkv");

    let mut conn = memory_db();
    let roots = vec![d.to_string_lossy().to_string()];
    let files = collect_files(&roots);
    let stats = import(&mut conn, &files, &roots).expect("导入应成功");

    let rows = tv_rows(&conn);
    assert_eq!(
        rows.len(),
        1,
        "神秘博士的所有版本/特辑应并成一条：{rows:?}（stats={stats:?}）"
    );
    assert_eq!(rows[0].1, 5, "5 个文件 = 5 集：{rows:?}");
    let eps = episodes_of(&conn);
    assert_eq!(
        eps,
        vec![(0, 1), (0, 2), (1, 1), (1, 2), (2, 1)],
        "两份特辑归第 0 季并补成 1、2 集，正片按原季集号：{eps:?}"
    );

    std::fs::remove_dir_all(&d).ok();
}

/// 二、Doctor Who 圣诞特辑（**D 盘真实文件名与目录布局**）。
///
/// 老数据在库里把它们拆成了七八条独立条目（其中 4 条甚至是电影），
/// 因为英文的 `Christmas Special` 既不是「特辑标记」也不是噪声词。
#[test]
fn real_doctor_who_christmas_specials_join_the_main_series() {
    let d = temp_dir("dw-christmas");
    // 正剧：按发行包分目录
    touch(&d, "神秘博士/Doctor.Who.2005.S01.1080p.BluRay.x264-SHORTBREHD[rartv]/doctor.who.2005.s01e01.1080p.bluray.x264-shortbrehd.mkv");
    touch(&d, "神秘博士/Doctor.Who.2005.S01.1080p.BluRay.x264-SHORTBREHD[rartv]/doctor.who.2005.s01e02.1080p.bluray.x264-shortbrehd.mkv");
    touch(&d, "神秘博士/Doctor.Who.2005.S07.1080p.BluRay.x264-MIXED[rartv]/Doctor.Who.2005.S07E01.1080p.BluRay.x264-BiA.mkv");
    touch(&d, "神秘博士/Doctor.Who.2005.S08.1080p.BluRay.x264-MIXED[rartv]/Doctor.Who.2005.S08E01.1080p.BluRay.x264-RRH.mkv");
    touch(&d, "神秘博士/Doctor.Who.2005.S08.1080p.BluRay.x264-MIXED[rartv]/Doctor.Who.2005.S08E02.1080p.BluRay.x264-RRH.mkv");
    // 圣诞特辑：带年份、带副标题、带 Season 标记的三种写法
    touch(&d, "神秘博士/Doctor.Who.2005.S02.1080p.BluRay.x264-SHORTBREHD[rartv]/doctor.who.2005.christmas.special.2005.1080p.bluray.x264-shortbrehd.mkv");
    touch(&d, "神秘博士/Doctor.Who.2005.S04.1080p.BluRay.x264-SHORTBREHD[rartv]/doctor.who.2005.christmas.special.the.next.doctor.2008.1080p.bluray.x264-shortbrehd.mkv");
    touch(&d, "神秘博士/Doctor.Who.2005.S04.1080p.BluRay.x264-SHORTBREHD[rartv]/doctor.who.2005.christmas.special.voyage.of.the.damned.2007.1080p.bluray.x264-shortbrehd.mkv");
    touch(&d, "神秘博士/Doctor.Who.2005.S06.1080p.BluRay.x264-MIXED[rartv]/doctor.who.2005.a.christmas.carol.2010.special.1080p.bluray.x264-shortbrehd.mkv");
    touch(&d, "神秘博士/Doctor.Who.2005.S08.1080p.BluRay.x264-MIXED[rartv]/Doctor.Who.2005.S08.Christmas.Special.1080p.BluRay.x264-RRH.mkv");
    touch(&d, "神秘博士/Doctor.Who.2005.S09.1080p.BluRay.x264-MIXED[rartv]/Doctor.Who.2005.Christmas.Special.2014.Last.Christmas.1080p.WEB-DL.DD5.1.H.mkv");
    touch(&d, "神秘博士/Doctor.Who.2005.S09.1080p.BluRay.x264-MIXED[rartv]/Doctor.Who.2005.Christmas.Special.2015.1080p.BluRay.x264-SHORTBREHD.mkv");
    touch(&d, "神秘博士/Doctor.Who.2005.S09.1080p.BluRay.x264-MIXED[rartv]/Doctor.Who.2005.Christmas.Special.2015.Extras.1080p.BluRay.x264-SHORTBREHD.mkv");
    // 装在 `Specials` 子目录里、连「Christmas」都不写的那几个
    touch(&d, "神秘博士/Doctor.Who.2005.S07.1080p.BluRay.x264-MIXED[rartv]/Specials/Doctor.Who.The.Time.of.the.Doctor.2013.1080p.BluRay.x264-SADPANDA.mkv");
    touch(&d, "神秘博士/Doctor.Who.2005.S07.1080p.BluRay.x264-MIXED[rartv]/Specials/The.Day.Of.The.Doctor.2013.1080p.BluRay.x264.DTS-WiKi.mkv");
    touch(&d, "神秘博士/Doctor.Who.2005.S07.1080p.BluRay.x264-MIXED[rartv]/Specials/doctor.who.2005.christmas.special.the.doctor.the.widow.and.the.wardrobe.2011.1080p.bluray.x264-shortbrehd.mkv");
    touch(&d, "神秘博士/Doctor.Who.2005.S07.1080p.BluRay.x264-MIXED[rartv]/Specials/doctor.who.2005.christmas.special.the.snowmen.2012.1080p.bluray.x264-shortbrehd.mkv");
    // 发行组前缀 + `chrimbo` 缩写的那种
    touch(&d, "神秘博士/Doctor.Who.2005.2012.Christmas.Special.720p.BluRay.X264-TRiPS[rarbg]/trips-doctor.who.s07.chrimbo.mkv");

    let total = 18;
    let mut conn = memory_db();
    let roots = vec![d.to_string_lossy().to_string()];
    let files = collect_files(&roots);
    let stats = import(&mut conn, &files, &roots).expect("导入应成功");

    let rows = tv_rows(&conn);
    assert_eq!(
        rows.len(),
        1,
        "全部正剧 + 特辑应并成一条 Doctor Who：{rows:?}（stats={stats:?}）"
    );
    assert_eq!(rows[0].1, total, "{total} 个文件应全是这一部剧的集数：{rows:?}");
    assert_eq!(stats.added_movies, 0, "这里没有电影，全都是剧集：{stats:?}");
    assert!(
        stats.unknown == 0,
        "不该有待确认条目（老数据里 4 条特辑被误判成电影）：{stats:?}"
    );

    let eps = episodes_of(&conn);
    // 第 0 季 = 特辑（含 `Specials` 目录里的那些），连续编号；
    // 1/7/8 季按文件名里写明的季集号保留。
    #[rustfmt::skip]
    let expected: Vec<(i64, i64)> = vec![
        (0, 1), (0, 2), (0, 3), (0, 4), (0, 5), (0, 6), (0, 7), (0, 8), (0, 9), (0, 10),
        (0, 11),
        (1, 1), (1, 2),
        (7, 1), (7, 2),
        (8, 1), (8, 2), (8, 3),
    ];
    assert_eq!(
        eps, expected,
        "特辑归第 0 季并连续编号，正片季集号不变：{eps:?}"
    );

    std::fs::remove_dir_all(&d).ok();
}

/// 一、作品目录优先：扫描根就是**作品目录本身**时（`D:\电视剧\Legal High`），
/// 用它自己当聚合单位，而不是把里面的 `Season 1` 当成作品。
#[test]
fn work_folder_is_the_aggregation_unit_when_root_is_the_show_dir() {
    let d = temp_dir("workdir-root");
    let show = d.join("Legal High");
    touch(&show, "Season 1/Legal.High.S01E01.1080p.mkv");
    touch(&show, "Season 1/Legal.High.S01E02.1080p.mkv");
    touch(&show, "Season 2/Legal.High.S02E01.1080p.mkv");
    touch(&show, "Specials/Legal.High.SP.1080p.mkv");

    let mut conn = memory_db();
    let roots = vec![show.to_string_lossy().to_string()];
    let files = collect_files(&roots);
    let stats = import(&mut conn, &files, &roots).expect("导入应成功");

    let rows = tv_rows(&conn);
    assert_eq!(rows.len(), 1, "一部剧不应被季目录拆开：{rows:?}（stats={stats:?}）");
    assert_eq!(rows[0].0, "Legal High", "组键应取作品目录名：{rows:?}");
    assert_eq!(rows[0].1, 4, "4 个文件 = 4 集：{rows:?}");

    std::fs::remove_dir_all(&d).ok();
}

/// B. 胜利正义：文件名只有编号、没有明确标记，必须识别成**同一部剧的连续集数**。
#[test]
fn unmarked_numbered_files_become_continuous_episodes() {
    let d = temp_dir("victory");
    touch(&d, "胜利正义/胜利正义 01.mp4");
    touch(&d, "胜利正义/胜利正义 02.mp4");
    touch(&d, "胜利正义/胜利正义 03.mp4");
    touch(&d, "胜利正义/胜利正义 04.mp4");
    touch(&d, "胜利正义/胜利正义 EP05.mp4");
    touch(&d, "胜利正义/胜利正义 - 06.mp4");
    touch(&d, "胜利正义/胜利正义 0007.mp4");

    let mut conn = memory_db();
    let roots = vec![d.to_string_lossy().to_string()];
    let files = collect_files(&roots);
    let stats = import(&mut conn, &files, &roots).expect("导入应成功");

    let rows = tv_rows(&conn);
    assert_eq!(
        rows.len(),
        1,
        "同一部剧不应被拆开：{rows:?}（stats={stats:?}）"
    );
    let eps = episodes_of(&conn);
    assert_eq!(
        eps,
        vec![(1, 1), (1, 2), (1, 3), (1, 4), (1, 5), (1, 6), (1, 7)],
        "集号应是连续的 1..7：{eps:?}"
    );

    std::fs::remove_dir_all(&d).ok();
}

/// C. 哆啦A梦：集数极多、文件名是纯数字或带季标记，必须聚成一条且集号连续。
#[test]
fn long_running_anime_with_pure_numbers_stays_one_series() {
    let d = temp_dir("doraemon");
    touch(&d, "哆啦A梦/0001.mp4");
    touch(&d, "哆啦A梦/0002.mp4");
    touch(&d, "哆啦A梦/0003.mp4");
    touch(&d, "哆啦A梦/0004.mp4");
    touch(&d, "哆啦A梦/0005.mp4");
    touch(&d, "哆啦A梦/哆啦A梦 0006.mp4");
    touch(&d, "哆啦A梦/哆啦A梦.S02.0007.mkv");

    let mut conn = memory_db();
    let roots = vec![d.to_string_lossy().to_string()];
    let files = collect_files(&roots);
    let stats = import(&mut conn, &files, &roots).expect("导入应成功");

    let rows = tv_rows(&conn);
    assert_eq!(
        rows.len(),
        1,
        "上千集的动画不能被切成一堆碎条目：{rows:?}（stats={stats:?}）"
    );
    let eps = episodes_of(&conn);
    assert_eq!(
        eps,
        vec![(1, 1), (1, 2), (1, 3), (1, 4), (1, 5), (1, 6), (2, 7)],
        "集号应连续且不撞车：{eps:?}"
    );

    std::fs::remove_dir_all(&d).ok();
}

/// 进度回报必须节流（每个文件都回调会把界面刷爆），且回调返回 false 能中止。
#[test]
fn collect_reports_throttled_progress_and_can_be_cancelled() {
    let d = temp_dir("tick");
    for i in 0..60 {
        touch(&d, &format!("Movie{i}.2020.mkv"));
    }
    let roots = vec![d.to_string_lossy().to_string()];

    let mut seen: Vec<usize> = Vec::new();
    let files = collect_files_reported(&roots, &mut |phase, n, cur| {
        assert_eq!(phase, PHASE_COLLECT);
        assert!(!cur.is_empty(), "应当带上当前目录，界面要用它做提示");
        seen.push(n);
        true
    });
    assert_eq!(files.len(), 60);
    assert!(!seen.is_empty(), "遍历过程中应回报过进度");
    assert!(seen.len() < 60, "节流后不应每个文件都回报：{seen:?}");
    assert_eq!(
        *seen.last().unwrap(),
        seen.iter().copied().max().unwrap(),
        "回报的条数应单调递增"
    );

    // 取消：回调返回 false 后必须立刻收工，而不是把整棵树走完。
    let mut calls = 0usize;
    let stopped = collect_files_reported(&roots, &mut |_, _, _| {
        calls += 1;
        false
    });
    assert_eq!(calls, 1, "第一次回调返回 false 后不应继续遍历");
    assert!(
        stopped.len() <= REPORT_EVERY,
        "中止时最多只有当前这一批：{}",
        stopped.len()
    );

    std::fs::remove_dir_all(&d).ok();
}

#[test]
fn import_reported_reports_progress() {
    let d = temp_dir("itick");
    for i in 0..30 {
        touch(&d, &format!("Movie{i}.2020.mkv"));
    }
    let roots = vec![d.to_string_lossy().to_string()];
    let mut conn = memory_db();
    let files = collect_files(&roots);

    let mut seen: Vec<(usize, String)> = Vec::new();
    let stats = import_reported(&mut conn, &files, &roots, &mut |phase, n, name| {
        assert_eq!(phase, PHASE_IMPORT);
        seen.push((n, name.to_string()));
        true
    })
    .expect("导入应成功");

    assert_eq!(stats.added_movies, 30);
    assert!(!seen.is_empty(), "入库过程中应回报过进度：{seen:?}");
    assert!(seen.len() < 30, "入库进度同样要节流：{:?}", seen.len());

    std::fs::remove_dir_all(&d).ok();
}

/// 判定：文件名完全没有集号、但同目录有一串编号文件 → 剧集（老逻辑会全判成电影）。
#[test]
fn numbered_files_in_a_folder_are_classified_as_tv() {
    let d = temp_dir("numbered");
    for i in 1..=4 {
        touch(&d, &format!("剧名/{i:02}.mkv"));
    }
    let roots = vec![d.to_string_lossy().to_string()];
    let mut conn = memory_db();
    let files = collect_files(&roots);
    let stats = import(&mut conn, &files, &roots).expect("导入应成功");

    assert_eq!(stats.added_series, 1, "应归成一部剧：{stats:?}");
    assert_eq!(stats.added_episodes, 4, "四个文件是四集：{stats:?}");
    let eps: Vec<i32> = {
        let mut stmt = conn
            .prepare("SELECT episode FROM episodes ORDER BY episode")
            .unwrap();
        stmt.query_map([], |r| r.get(0)).unwrap().map(|r| r.unwrap()).collect()
    };
    assert_eq!(eps, vec![1, 2, 3, 4], "尾部编号应被当作集号");

    std::fs::remove_dir_all(&d).ok();
}

/// 判定：目录名带季标记（`Season 2`）时，目录里的无标记文件也是剧集。
#[test]
fn season_named_folder_makes_unmarked_files_tv() {
    let d = temp_dir("seasondir");
    touch(&d, "剧名/Season 2/01.mkv");
    let roots = vec![d.to_string_lossy().to_string()];
    let files = collect_files(&roots);
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].kind, MediaKind::Tv, "{:?}", files[0].parsed);
    std::fs::remove_dir_all(&d).ok();
}

/// 边界：特别篇 / OVA 归到第 0 季，不占正片集号。
#[test]
fn specials_land_in_season_zero() {
    let d = temp_dir("special");
    touch(&d, "某动画 OVA 第01话.mkv");
    let roots = vec![d.to_string_lossy().to_string()];
    let mut conn = memory_db();
    let files = collect_files(&roots);
    import(&mut conn, &files, &roots).expect("导入应成功");
    let season: i32 = conn
        .query_row("SELECT season FROM episodes LIMIT 1", [], |r| r.get(0))
        .unwrap();
    assert_eq!(season, 0, "特别篇应落在第 0 季");
    std::fs::remove_dir_all(&d).ok();
}

/// 边界：证据不足的文件不能静默归成电影——要落成「待确认」。
#[test]
fn undecidable_file_becomes_unknown_entry() {
    let d = temp_dir("unknown");
    touch(&d, "Dune.mkv");
    let roots = vec![d.to_string_lossy().to_string()];
    let mut conn = memory_db();
    let files = collect_files(&roots);
    let stats = import(&mut conn, &files, &roots).expect("导入应成功");

    assert_eq!(stats.added_movies, 0, "不该被当成电影：{stats:?}");
    assert_eq!(stats.added_unknown, 1, "应标成待确认：{stats:?}");
    let kind: String = conn
        .query_row("SELECT type FROM media LIMIT 1", [], |r| r.get(0))
        .unwrap();
    assert_eq!(kind, "unknown");
    std::fs::remove_dir_all(&d).ok();
}

/// 边界：动画剧场版是电影，不能被当成剧集。
#[test]
fn theatrical_anime_is_imported_as_movie() {
    let d = temp_dir("theatrical");
    touch(&d, "名侦探柯南 剧场版 2023.mkv");
    let roots = vec![d.to_string_lossy().to_string()];
    let mut conn = memory_db();
    let files = collect_files(&roots);
    let stats = import(&mut conn, &files, &roots).expect("导入应成功");
    assert_eq!(stats.added_movies, 1, "{stats:?}");
    assert_eq!(stats.added_series, 0, "{stats:?}");
    std::fs::remove_dir_all(&d).ok();
}

/// 扫描历史应记录真实目录，而不是「N 个目录」这种无信息量的标签。
#[test]
fn scan_history_stores_real_root_paths() {
    let d = temp_dir("hist");
    touch(&d, "Movie.2020.1080p.mkv");
    let roots = vec![d.to_string_lossy().to_string()];

    let mut conn = memory_db();
    let files = collect_files(&roots);
    import(&mut conn, &files, &roots).expect("导入应成功");

    let label: String = conn
        .query_row(
            "SELECT path FROM scan_history ORDER BY id DESC LIMIT 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(
        label.contains("mm-scan-hist"),
        "扫描历史应记录真实目录（实际为 {label}）"
    );

    std::fs::remove_dir_all(&d).ok();
}
