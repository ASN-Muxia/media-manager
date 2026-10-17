//! 表结构与增量迁移（`store` 的「怎么建、怎么迁」部分）。
//!
//! 从 `db.rs` 拆出来的原因：`db.rs` 一度同时承担「连接入口」和「全部建表/迁移
//! 语句」两件事，512 行里 400 行是 SQL 常量，改迁移要在 SQL 海里找位置。
//! 拆开后 `db.rs` 只管怎么打开连接，这里只管结构长什么样。
//!
//! 铁律（B6）：
//! - 一律 `CREATE TABLE/INDEX/TRIGGER IF NOT EXISTS` 或 `ensure_column`；
//! - **禁止** `DROP TABLE` / 重建库 / 要求用户重新导入；
//! - 结构变更后抬 `SCHEMA_VERSION`，并在真实老库上验证一次。

use rusqlite::{params, Connection};

/// 建表语句（幂等，可重复执行）。
pub const SCHEMA: &str = r#"
PRAGMA journal_mode = WAL;
PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS settings (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS media (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    type           TEXT    NOT NULL DEFAULT 'movie',
    title          TEXT    NOT NULL,
    original_title TEXT,
    group_key      TEXT,
    year           INTEGER,
    tmdb_id        INTEGER,
    poster_path    TEXT,
    backdrop_path  TEXT,
    overview       TEXT,
    vote_average   REAL,
    genre          TEXT,
    file_path      TEXT,
    file_name      TEXT,
    file_size      INTEGER,
    duration       INTEGER,
    added_at       INTEGER NOT NULL,
    updated_at     INTEGER NOT NULL,
    -- source：'scan'（扫描归纳出来的）/ 'manual'（用户手动新建的 Series）
    source         TEXT    NOT NULL DEFAULT 'scan',
    -- locked：用户确认过分组后锁住，二次校正不再调整
    locked         INTEGER NOT NULL DEFAULT 0,
    tvdb_id        INTEGER,
    resolution     TEXT,
    release_source TEXT
);

CREATE TABLE IF NOT EXISTS episodes (
    id        INTEGER PRIMARY KEY AUTOINCREMENT,
    media_id  INTEGER NOT NULL REFERENCES media(id) ON DELETE CASCADE,
    season    INTEGER NOT NULL DEFAULT 1,
    episode   INTEGER NOT NULL DEFAULT 1,
    title     TEXT,
    file_path TEXT,
    file_name TEXT,
    file_size INTEGER,
    duration  INTEGER,
    added_at  INTEGER NOT NULL,
    tmdb_id    INTEGER,
    overview   TEXT,
    still_path TEXT,
    air_date   TEXT,
    tvdb_id    INTEGER,
    resolution TEXT,
    release_source TEXT
);

CREATE TABLE IF NOT EXISTS scan_history (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    path        TEXT    NOT NULL,
    started_at  INTEGER NOT NULL,
    finished_at INTEGER,
    status      TEXT    NOT NULL DEFAULT 'running',
    found       INTEGER NOT NULL DEFAULT 0,
    added       INTEGER NOT NULL DEFAULT 0,
    message     TEXT
);

CREATE INDEX IF NOT EXISTS idx_media_type        ON media(type);
CREATE INDEX IF NOT EXISTS idx_media_tmdb        ON media(tmdb_id);
-- file_path 用普通唯一索引：SQLite 中 NULL 互不冲突，因此剧集主记录（file_path 为 NULL）可多条共存，
-- 同时让 `ON CONFLICT(file_path)` upsert 生效。
CREATE UNIQUE INDEX IF NOT EXISTS idx_media_path ON media(file_path);
CREATE INDEX IF NOT EXISTS idx_episodes_media    ON episodes(media_id);
CREATE UNIQUE INDEX IF NOT EXISTS idx_episodes_path ON episodes(file_path);
CREATE INDEX IF NOT EXISTS idx_scan_path         ON scan_history(path);

-- 分类：左侧导航的大栏目。kind 区分「用户手动建的」和「自动识别补出来的」，
-- origin 记录来源（manual / tmdb / folder / rule），全部是数据，不含任何硬编码枚举。
CREATE TABLE IF NOT EXISTS categories (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    name       TEXT    NOT NULL,
    kind       TEXT    NOT NULL DEFAULT 'manual',
    origin     TEXT    NOT NULL DEFAULT 'manual',
    sort_order INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL
);
-- 同名分类只保留一条（大小写不敏感），否则自动同步会反复造重复的栏目。
CREATE UNIQUE INDEX IF NOT EXISTS idx_categories_name ON categories(name COLLATE NOCASE);

CREATE TABLE IF NOT EXISTS media_categories (
    media_id    INTEGER NOT NULL REFERENCES media(id) ON DELETE CASCADE,
    category_id INTEGER NOT NULL REFERENCES categories(id) ON DELETE CASCADE,
    created_at  INTEGER NOT NULL,
    PRIMARY KEY (media_id, category_id)
);
CREATE INDEX IF NOT EXISTS idx_media_categories_cat ON media_categories(category_id);

CREATE TABLE IF NOT EXISTS tags (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    name       TEXT    NOT NULL,
    created_at INTEGER NOT NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_tags_name ON tags(name COLLATE NOCASE);

CREATE TABLE IF NOT EXISTS media_tags (
    media_id   INTEGER NOT NULL REFERENCES media(id) ON DELETE CASCADE,
    tag_id     INTEGER NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
    created_at INTEGER NOT NULL,
    PRIMARY KEY (media_id, tag_id)
);
CREATE INDEX IF NOT EXISTS idx_media_tags_tag ON media_tags(tag_id);
"#;

/// 当前结构版本。
/// v1 = 初始 4 表；v2 = episodes 增加 tmdb_id/overview/still_path/air_date；
/// v3 = media 增加 group_key（剧集分组键，同名剧集按目录区分）；
/// v4 = 分类 / 标签四张表 + 实体扩展字段（source / locked / tvdb_id / 分辨率 / 来源）；
/// v5 = 剧集层级（parent_id / sort_order）；
/// v6 = 观看状态 / 元数据缓存 / 检索索引 / 增量指纹 / 标签加厚（见 `migrate_v6`）；
/// v7 = 资源目录库 resource_assets + 每次检测的汇总 resource_snapshots（见 `migrate_v7`）。
pub const SCHEMA_VERSION: i32 = 7;

/// 建表 + 增量迁移。对**已存在的老库**用 `ALTER TABLE` 补列，保证升级不丢数据。
pub fn migrate(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(SCHEMA)
        .map_err(|e| format!("初始化表结构失败: {e}"))?;

    // v1 -> v2：为单集补充 TMDB 元数据列
    ensure_column(conn, "episodes", "tmdb_id", "INTEGER")?;
    ensure_column(conn, "episodes", "overview", "TEXT")?;
    ensure_column(conn, "episodes", "still_path", "TEXT")?;
    ensure_column(conn, "episodes", "air_date", "TEXT")?;

    // v2 -> v3：剧集分组键（同名剧集按所在目录区分，避免被错误合并）
    ensure_column(conn, "media", "group_key", "TEXT")?;
    // 索引必须在补列之后建立，否则老库会因列不存在而失败
    conn.execute_batch("CREATE INDEX IF NOT EXISTS idx_media_group ON media(type, group_key);")
        .map_err(|e| format!("创建分组索引失败: {e}"))?;

    // v3 -> v4：Series 实体扩展字段
    // source：区分「扫描归纳出来的」和「用户手动新建的」，手动建的不会被重扫改写。
    ensure_column(conn, "media", "source", "TEXT NOT NULL DEFAULT 'scan'")?;
    // locked：用户手动确认过分组后锁住，二次校正不再动它。
    ensure_column(conn, "media", "locked", "INTEGER NOT NULL DEFAULT 0")?;
    ensure_column(conn, "media", "tvdb_id", "INTEGER")?;
    // 分辨率 / 发布来源：由 LLM 脏文件名解析（可选增强层）回填，正则解析不产出。
    ensure_column(conn, "media", "resolution", "TEXT")?;
    ensure_column(conn, "media", "release_source", "TEXT")?;
    ensure_column(conn, "episodes", "tvdb_id", "INTEGER")?;
    ensure_column(conn, "episodes", "resolution", "TEXT")?;
    ensure_column(conn, "episodes", "release_source", "TEXT")?;

    // v4 -> v5：剧集层级（树形管理）
    // parent_id：父节点，NULL = 顶层。没有外键约束是刻意的——层级由
    // `core/tree.rs` 在写入前校验（禁止自身 / 子孙 / 超限），而 SQLite 的外键
    // 在事务里对「先插子再插父」这类顺序敏感，交给应用层判断更好排查。
    ensure_column(conn, "media", "parent_id", "INTEGER")?;
    // sort_order：同级排序。默认 0，同一父节点下用它升序排，再按标题兜底。
    ensure_column(conn, "media", "sort_order", "INTEGER NOT NULL DEFAULT 0")?;
    conn.execute_batch("CREATE INDEX IF NOT EXISTS idx_media_parent ON media(parent_id, sort_order);")
        .map_err(|e| format!("创建层级索引失败: {e}"))?;

    // v5 -> v6：观看状态 / 元数据缓存 / 检索索引 / 增量指纹 / 标签加厚
    migrate_v6(conn)?;

    // v6 -> v7：资源目录库（磁盘文件 ↔ 库条目的对应关系）+ 每次检测的汇总
    migrate_v7(conn)?;

    // 外键级联依赖 PRAGMA foreign_keys，每次连接都要开（SCHEMA 里也写了，
    // 但老库升级路径下 SCHEMA 已执行过，这里再兜一次）。
    conn.execute_batch("PRAGMA foreign_keys = ON;")
        .map_err(|e| format!("开启外键约束失败: {e}"))?;

    // 悬挂的 `parent_id` 自愈（幂等，无副作用）：见 `heal_orphan_parents`。
    heal_orphan_parents(conn)?;

    conn.pragma_update(None, "user_version", SCHEMA_VERSION)
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// 把指向**已不存在**条目的 `parent_id` 清成顶层，返回修好的行数。
///
/// 为什么需要：`media.parent_id` 刻意没有外键约束（层级规则在应用层校验），
/// 所以历史上任何一次裸删都会在库里留下一串孤儿——它们的父节点已经没了，
/// 于是封面墙上永远不显示，详情页却还挂着「脱离聚合」按钮、显示 `#<已删id>`，
/// 用户既看不见也改不了。
///
/// 为什么放在这里：删除路径已经补齐了「删前过继」，但那管不了**存量**——
/// 老库里的孤儿不会因为代码改好了就自己消失。迁移入口每次开库都跑一次，
/// 是唯一确定会执行到的地方；语句本身是幂等的（第二次必然改 0 行），
/// 所以可以放心常驻。
pub fn heal_orphan_parents(conn: &Connection) -> Result<usize, String> {
    let n = conn
        .execute(
            "UPDATE media SET parent_id = NULL \
              WHERE parent_id IS NOT NULL \
                AND parent_id NOT IN (SELECT id FROM media)",
            [],
        )
        .map_err(|e| format!("清理悬挂的父级引用失败: {e}"))?;
    Ok(n)
}

/// v5 → v6 的全部增量变更。
///
/// 每一段都是独立的、可失败的（FTS5 缺失时整段跳过），任何一段失败都不影响
/// 其余结构，也不影响用户已有数据。
pub fn migrate_v6(conn: &Connection) -> Result<(), String> {
    // ── 增量扫描指纹：记录入库时的 mtime，下次扫描据此判断「变了没有」 ──
    ensure_column(conn, "media", "file_mtime", "INTEGER")?;
    ensure_column(conn, "episodes", "file_mtime", "INTEGER")?;

    // ── 观看状态 / 续播 ──
    // 一张表同时覆盖「整部电影」和「某一集」：电影记 episode_id = NULL。
    // 唯一索引用 IFNULL 把 NULL 折成 0，否则 SQLite 允许重复的 (media_id, NULL)。
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS playback_state (
             id            INTEGER PRIMARY KEY AUTOINCREMENT,
             media_id      INTEGER NOT NULL REFERENCES media(id) ON DELETE CASCADE,
             episode_id    INTEGER REFERENCES episodes(id) ON DELETE CASCADE,
             -- watched：整部/整集看完（集级进度，不是秒级）
             watched       INTEGER NOT NULL DEFAULT 0,
             -- position_secs：预留给将来能读到播放器进度的场景，本期不自动采集
             position_secs INTEGER,
             duration_secs INTEGER,
             play_count    INTEGER NOT NULL DEFAULT 0,
             last_played_at INTEGER,
             updated_at    INTEGER NOT NULL
         );
         CREATE UNIQUE INDEX IF NOT EXISTS idx_playback_target
             ON playback_state(media_id, IFNULL(episode_id, 0));
         CREATE INDEX IF NOT EXISTS idx_playback_recent ON playback_state(last_played_at DESC);",
    )
    .map_err(|e| format!("创建观看状态表失败: {e}"))?;

    // ── 元数据缓存 ──
    // 只缓存**成功**响应；失败不进正缓存，避免一次网络抖动把错误记住几分钟。
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS provider_cache (
             id         INTEGER PRIMARY KEY AUTOINCREMENT,
             provider   TEXT    NOT NULL,
             cache_key  TEXT    NOT NULL,
             payload    TEXT    NOT NULL,
             created_at INTEGER NOT NULL,
             expires_at INTEGER NOT NULL
         );
         CREATE UNIQUE INDEX IF NOT EXISTS idx_provider_cache_key
             ON provider_cache(provider, cache_key);
         CREATE INDEX IF NOT EXISTS idx_provider_cache_exp ON provider_cache(expires_at);",
    )
    .map_err(|e| format!("创建元数据缓存表失败: {e}"))?;

    // ── 检索索引（FTS5）──
    // 外部内容表：正文只存一份（media 表），FTS 只建倒排，不复制数据。
    // 建不起来（SQLite 未编 FTS5）就整段跳过，查询侧回落到 LIKE，用户无感。
    if fts5_available(conn) {
        create_fts_index(conn)?;
    }

    // ── 标签加厚（P1）──
    ensure_column(conn, "tags", "color", "TEXT")?;
    ensure_column(conn, "tags", "alias", "TEXT")?;
    ensure_column(conn, "tags", "parent_id", "INTEGER")?;

    // ── 技术参数（可选 ffprobe 实测，默认不参与判定）──
    ensure_column(conn, "media", "tech_json", "TEXT")?;
    ensure_column(conn, "episodes", "tech_json", "TEXT")?;
    // 类型改判审计：谁把这条从什么改成什么，可追溯（R1 接线后必用）。
    ensure_column(conn, "media", "type_audit", "TEXT")?;

    Ok(())
}

/// v6 → v7 的全部增量变更：资源目录库。
///
/// ## 这张表要解决什么问题
/// `media` / `episodes` 记的是「用户看到的作品」，它们的 `file_path` 会被重新识别、
/// 归并、改挂改写——一旦文件从盘上消失，那条记录也就跟着没了，**再也没有地方
/// 记得它原来在哪**。资源目录库反过来：它以**绝对路径**为身份，只记「盘上见过什么」，
/// 与作品条目解耦。于是「上次检测时这个文件在哪、多大、什么时候不见的」全部留得下来，
/// 这就是「定位旧资源」的落点。
///
/// ## 两条刻意的设计
/// 1. **`status='missing'` 的行永不删除**，只在文件重新出现时置回 `present`。
///    删掉它就等于把「它上次在哪」这条唯一的线索扔了；留着的成本只是几行文本。
/// 2. **同一路径只有一行**，靠 `UNIQUE(path)` 的 upsert 保证幂等：
///    重复检测只会刷新 `last_seen`，不会造出第二行来。
///
/// `media_id` 与 `media.parent_id` 同策略——**刻意不加外键**：
/// 条目会被删、会被重新识别成另一个 id，而资源行必须活下来；
/// 挂了 `ON DELETE CASCADE` 就会把刚攒下的历史一起带走。
pub fn migrate_v7(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS resource_assets (
             id          INTEGER PRIMARY KEY AUTOINCREMENT,
             -- 媒体根目录：多根目录时用来区分「哪块盘上的」
             root_path   TEXT    NOT NULL,
             -- 文件所在目录的绝对路径（按目录聚合展示、判断同目录兄弟）
             dir_path    TEXT    NOT NULL,
             -- 相对根目录的目录（'' = 直接躺在根目录下）
             rel_dir     TEXT    NOT NULL DEFAULT '',
             file_name   TEXT    NOT NULL,
             -- 绝对路径：稳定身份。文件改名 = 新行 + 旧行标 missing，不做路径猜测
             path        TEXT    NOT NULL,
             size        INTEGER NOT NULL DEFAULT 0,
             -- Unix 秒；取不到为 NULL（此时一律保守当成「变了」）
             mtime       INTEGER,
             ext         TEXT    NOT NULL DEFAULT '',
             -- present / missing（取值与 core::registry 的两个常量一致）
             status      TEXT    NOT NULL DEFAULT 'present',
             first_seen  INTEGER NOT NULL,
             last_seen   INTEGER NOT NULL,
             -- 第一次被判为消失的时间；留空表示「还没消失过」
             missing_at  INTEGER,
             -- 关联的 media.id（可为 NULL；无外键，与 parent_id 同策略）
             media_id    INTEGER,
             -- 归一化标题：同名的不同文件能对上「这是同一部作品」
             content_key TEXT
         );
         -- 绝对路径唯一：upsert 的落点，也保证同一路径不会有两行
         CREATE UNIQUE INDEX IF NOT EXISTS idx_resource_assets_path ON resource_assets(path);
         CREATE INDEX IF NOT EXISTS idx_resource_assets_dir    ON resource_assets(dir_path);
         CREATE INDEX IF NOT EXISTS idx_resource_assets_status ON resource_assets(status);
         CREATE INDEX IF NOT EXISTS idx_resource_assets_media  ON resource_assets(media_id);",
    )
    .map_err(|e| format!("创建资源目录表失败: {e}"))?;

    // 每次检测 / 增量对比的汇总：让「每一次检测」都能和上几次对比，
    // 而不是只看到眼前这一份快照。历史本身不清理（每次一行，量极小）。
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS resource_snapshots (
             id          INTEGER PRIMARY KEY AUTOINCREMENT,
             started_at  INTEGER NOT NULL,
             finished_at INTEGER,
             -- 本次遍历的根目录（与 scan_history.path 同口径的可读文本）
             roots       TEXT    NOT NULL DEFAULT '',
             scanned     INTEGER NOT NULL DEFAULT 0,
             added       INTEGER NOT NULL DEFAULT 0,
             updated     INTEGER NOT NULL DEFAULT 0,
             unchanged   INTEGER NOT NULL DEFAULT 0,
             missing     INTEGER NOT NULL DEFAULT 0,
             restored    INTEGER NOT NULL DEFAULT 0,
             message     TEXT
         );
         CREATE INDEX IF NOT EXISTS idx_resource_snapshots_time
             ON resource_snapshots(started_at DESC);",
    )
    .map_err(|e| format!("创建资源快照表失败: {e}"))?;

    Ok(())
}

/// FTS5 索引名。
pub const FTS_TABLE: &str = "media_fts";

/// 这个 SQLite 有没有编进 FTS5。
///
/// 不查 `compile_options`（不同版本返回格式不一），直接**试着建一张探针表**——
/// 能建就说明有，建完立刻删掉。这是唯一可靠的判断方式。
pub fn fts5_available(conn: &Connection) -> bool {
    let ok = conn
        .execute_batch(
            "CREATE VIRTUAL TABLE IF NOT EXISTS __fts5_probe USING fts5(x); \
             DROP TABLE __fts5_probe;",
        )
        .is_ok();
    ok
}

/// 建 FTS5 外部内容表 + 三个同步触发器（全部 IF NOT EXISTS，幂等）。
pub fn create_fts_index(conn: &Connection) -> Result<(), String> {
    let ddl = format!(
        "CREATE VIRTUAL TABLE IF NOT EXISTS {t} USING fts5(
             title, original_title, group_key, genre, file_name,
             content='media', content_rowid='id', tokenize='unicode61'
         );
         CREATE TRIGGER IF NOT EXISTS media_fts_ai AFTER INSERT ON media BEGIN
             INSERT INTO {t}(rowid, title, original_title, group_key, genre, file_name)
             VALUES (new.id, new.title, new.original_title, new.group_key, new.genre, new.file_name);
         END;
         CREATE TRIGGER IF NOT EXISTS media_fts_ad AFTER DELETE ON media BEGIN
             INSERT INTO {t}({t}, rowid, title, original_title, group_key, genre, file_name)
             VALUES ('delete', old.id, old.title, old.original_title, old.group_key, old.genre, old.file_name);
         END;
         CREATE TRIGGER IF NOT EXISTS media_fts_au AFTER UPDATE ON media BEGIN
             INSERT INTO {t}({t}, rowid, title, original_title, group_key, genre, file_name)
             VALUES ('delete', old.id, old.title, old.original_title, old.group_key, old.genre, old.file_name);
             INSERT INTO {t}(rowid, title, original_title, group_key, genre, file_name)
             VALUES (new.id, new.title, new.original_title, new.group_key, new.genre, new.file_name);
         END;",
        t = FTS_TABLE
    );
    conn.execute_batch(&ddl)
        .map_err(|e| format!("创建检索索引失败: {e}"))
}

/// 检索索引表是否存在。
pub fn fts_index_exists(conn: &Connection) -> bool {
    conn.query_row(
        "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1",
        params![FTS_TABLE],
        |_| Ok(()),
    )
    .is_ok()
}

/// 重建检索索引（**只动索引，不碰 `media` 表的任何一行**）。
///
/// 什么时候需要：索引的**内部结构**坏了。这个症状很有迷惑性——
/// `PRAGMA integrity_check` 报 ok，fts5 自己的 `integrity-check` 也过，
/// 但只要真的去删一行 media（`media_fts_ad` 触发器要从索引里删对应条目），
/// 就抛 `database disk image is malformed`。于是「清空媒体库」和「批量删除」
/// 一起失败，而列表查询照常工作，看起来就像代码坏了。
///
/// 在真实库上验证过的顺序：
/// `DELETE FROM media` 失败 → `INSERT INTO media_fts(media_fts) VALUES('rebuild')` → 再删就成功。
pub fn rebuild_fts_index(conn: &Connection) -> Result<(), String> {
    if !fts_index_exists(conn) {
        return Ok(());
    }
    conn.execute_batch(&format!(
        "INSERT INTO {t}({t}) VALUES('rebuild');",
        t = FTS_TABLE
    ))
    .map_err(|e| format!("重建检索索引失败: {e}"))
}

/// 这条错误是不是「索引 / 文件结构坏了」。
///
/// SQLite 的原话是 `database disk image is malformed`。这种情况**直接重试没有用**，
/// 必须先修索引再重试，所以调用方得能认出它。
pub fn looks_malformed(err: &str) -> bool {
    err.to_ascii_lowercase().contains("malformed")
}

/// 若 `err` 是「检索索引损坏」，就重建索引并返回 `true`（意思是**值得重试一次**）。
///
/// 拆成两个函数是为了可测：`looks_malformed` 判是不是这类错，
/// `rebuild_fts_index` 负责修；组合逻辑简单到一眼能看对，就不必靠集成用例去凑了
/// ——真实库那种内部不一致的形态依赖具体 SQLite 版本，测试环境构造不出来。
pub fn heal_fts_if_malformed(conn: &Connection, err: &str) -> bool {
    looks_malformed(err) && rebuild_fts_index(conn).is_ok()
}

/// 若列不存在则添加（幂等）。
pub fn ensure_column(
    conn: &Connection,
    table: &str,
    column: &str,
    decl: &str,
) -> Result<(), String> {
    if column_exists(conn, table, column)? {
        return Ok(());
    }
    conn.execute(&format!("ALTER TABLE {table} ADD COLUMN {column} {decl}"), [])
        .map_err(|e| format!("为 {table} 添加列 {column} 失败: {e}"))?;
    Ok(())
}

/// 列是否存在。
pub fn column_exists(conn: &Connection, table: &str, column: &str) -> Result<bool, String> {
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info({table})"))
        .map_err(|e| e.to_string())?;
    let names = stmt
        .query_map([], |r| r.get::<_, String>(1))
        .map_err(|e| e.to_string())?;
    for n in names {
        if n.map_err(|e| e.to_string())? == column {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_creates_all_tables() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(SCHEMA).unwrap();
        let names = crate::store::db::tables(&conn).unwrap();
        for t in [
            "episodes",
            "media",
            "scan_history",
            "settings",
            "categories",
            "media_categories",
            "tags",
            "media_tags",
        ] {
            assert!(names.contains(&t.to_string()), "缺少表 {t}，实际为 {names:?}");
        }
        assert_eq!(names.len(), 8, "期望 8 张表，实际为 {names:?}");
    }

    /// 开好库的连接：`parent_id` / `sort_order` 是 v5 迁移补上去的列，
    /// 只建初始 `SCHEMA` 会得到「no such column」。
    fn migrated_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::store::db::migrate(&conn).unwrap();
        conn
    }

    /// 悬挂的 `parent_id` 必须在开库时被清掉——老库里的孤儿不会因为
    /// 「删除路径改好了」就自己消失。
    #[test]
    fn heal_orphan_parents_drops_dangling_refs() {
        let conn = migrated_db();
        let ts = crate::store::db::now_epoch();
        conn.execute(
            "INSERT INTO media (id, type, title, added_at, updated_at) VALUES (1, 'movie', '父', ?1, ?1)",
            params![ts],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO media (id, type, title, parent_id, added_at, updated_at) \
             VALUES (2, 'movie', '子', 1, ?1, ?1)",
            params![ts],
        )
        .unwrap();
        // 模拟历史裸删：父行没了，子行还指着 1
        conn.execute("DELETE FROM media WHERE id = 1", []).unwrap();
        let dangling: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM media WHERE parent_id = 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(dangling, 1, "前置条件：确实留了一条悬挂引用");

        assert_eq!(heal_orphan_parents(&conn).unwrap(), 1);
        let left: Option<i64> = conn
            .query_row("SELECT parent_id FROM media WHERE id = 2", [], |r| r.get(0))
            .unwrap();
        assert_eq!(left, None, "悬挂引用必须被清成顶层");

        // 幂等：第二次必然改 0 行（这是它可以常驻在 migrate 里的前提）
        assert_eq!(heal_orphan_parents(&conn).unwrap(), 0);
    }

    /// 正常层级不能被自愈误伤：父节点**还在**的行一个都不许动。
    #[test]
    fn heal_orphan_parents_keeps_healthy_links() {
        let conn = migrated_db();
        let ts = crate::store::db::now_epoch();
        conn.execute(
            "INSERT INTO media (id, type, title, added_at, updated_at) VALUES (1, 'movie', '父', ?1, ?1)",
            params![ts],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO media (id, type, title, parent_id, added_at, updated_at) \
             VALUES (2, 'movie', '子', 1, ?1, ?1)",
            params![ts],
        )
        .unwrap();
        assert_eq!(heal_orphan_parents(&conn).unwrap(), 0);
        let left: Option<i64> = conn
            .query_row("SELECT parent_id FROM media WHERE id = 2", [], |r| r.get(0))
            .unwrap();
        assert_eq!(left, Some(1), "父节点还在，层级必须原样保留");
    }

    #[test]
    fn schema_is_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(SCHEMA).unwrap();
        conn.execute_batch(SCHEMA).unwrap();
        assert_eq!(crate::store::db::tables(&conn).unwrap().len(), 8);
    }

    #[test]
    fn fresh_schema_has_episode_metadata_columns() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        for col in ["tmdb_id", "overview", "still_path", "air_date"] {
            assert!(column_exists(&conn, "episodes", col).unwrap(), "缺少列 {col}");
        }
        assert!(column_exists(&conn, "media", "group_key").unwrap());
        let v: i32 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, SCHEMA_VERSION);
    }

    /// 老库（media 无 group_key）升级：补列 + 建索引都不能失败。
    #[test]
    fn migrate_adds_group_key_and_index_to_legacy_db() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE media (
                 id INTEGER PRIMARY KEY AUTOINCREMENT, type TEXT NOT NULL DEFAULT 'movie',
                 title TEXT NOT NULL, original_title TEXT, year INTEGER, tmdb_id INTEGER,
                 poster_path TEXT, backdrop_path TEXT, overview TEXT, vote_average REAL,
                 genre TEXT, file_path TEXT, file_name TEXT, file_size INTEGER,
                 duration INTEGER, added_at INTEGER NOT NULL, updated_at INTEGER NOT NULL);
             INSERT INTO media (type, title, added_at, updated_at) VALUES ('tv','老剧',0,0);",
        )
        .unwrap();

        migrate(&conn).unwrap();

        assert!(column_exists(&conn, "media", "group_key").unwrap());
        let title: String = conn
            .query_row("SELECT title FROM media WHERE id = 1", [], |r| r.get(0))
            .unwrap();
        assert_eq!(title, "老剧", "迁移不应丢失原有数据");
        // 再跑一次不应报错（幂等）
        migrate(&conn).unwrap();
        let idx: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name='idx_media_group'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(idx, 1);
    }

    /// v5：层级两列必须补上，且**已有数据要落在顶层**。
    #[test]
    fn migrate_adds_tree_columns_and_keeps_old_rows_at_root() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE media (
                 id INTEGER PRIMARY KEY AUTOINCREMENT, type TEXT NOT NULL DEFAULT 'movie',
                 title TEXT NOT NULL, original_title TEXT, group_key TEXT, year INTEGER,
                 tmdb_id INTEGER, poster_path TEXT, backdrop_path TEXT, overview TEXT,
                 vote_average REAL, genre TEXT, file_path TEXT, file_name TEXT,
                 file_size INTEGER, duration INTEGER, added_at INTEGER NOT NULL,
                 updated_at INTEGER NOT NULL);
             CREATE TABLE episodes (
                 id INTEGER PRIMARY KEY AUTOINCREMENT, media_id INTEGER NOT NULL,
                 season INTEGER NOT NULL DEFAULT 1, episode INTEGER NOT NULL DEFAULT 1,
                 title TEXT, file_path TEXT, file_name TEXT, file_size INTEGER,
                 duration INTEGER, added_at INTEGER NOT NULL, tmdb_id INTEGER,
                 overview TEXT, still_path TEXT, air_date TEXT);
             INSERT INTO media (type, title, added_at, updated_at) VALUES ('tv','老剧',0,0);
             INSERT INTO episodes (media_id, season, episode, added_at) VALUES (1,1,1,0);",
        )
        .unwrap();

        migrate(&conn).unwrap();

        for (table, col) in [("media", "parent_id"), ("media", "sort_order")] {
            assert!(column_exists(&conn, table, col).unwrap(), "缺少 {table}.{col}");
        }
        let (parent, order): (Option<i64>, i64) = conn
            .query_row(
                "SELECT parent_id, sort_order FROM media WHERE id = 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(parent, None, "老数据必须留在顶层");
        assert_eq!(order, 0);
        migrate(&conn).unwrap();
    }

    #[test]
    fn migrate_upgrades_legacy_episodes_table_without_data_loss() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE episodes (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 media_id INTEGER NOT NULL,
                 season INTEGER NOT NULL DEFAULT 1,
                 episode INTEGER NOT NULL DEFAULT 1,
                 title TEXT, file_path TEXT, file_name TEXT,
                 file_size INTEGER, duration INTEGER, added_at INTEGER NOT NULL);
             INSERT INTO episodes (media_id, season, episode, title, added_at)
             VALUES (1, 1, 1, '老数据', 0);",
        )
        .unwrap();
        assert!(!column_exists(&conn, "episodes", "still_path").unwrap());

        migrate(&conn).unwrap();

        for col in ["tmdb_id", "overview", "still_path", "air_date"] {
            assert!(column_exists(&conn, "episodes", col).unwrap(), "缺少列 {col}");
        }
        let title: String = conn
            .query_row("SELECT title FROM episodes WHERE id = 1", [], |r| r.get(0))
            .unwrap();
        assert_eq!(title, "老数据");
    }

    #[test]
    fn migrate_adds_v4_columns_and_catalog_tables_without_data_loss() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE media (
                 id INTEGER PRIMARY KEY AUTOINCREMENT, type TEXT NOT NULL DEFAULT 'movie',
                 title TEXT NOT NULL, original_title TEXT, group_key TEXT, year INTEGER,
                 tmdb_id INTEGER, poster_path TEXT, backdrop_path TEXT, overview TEXT,
                 vote_average REAL, genre TEXT, file_path TEXT, file_name TEXT,
                 file_size INTEGER, duration INTEGER, added_at INTEGER NOT NULL,
                 updated_at INTEGER NOT NULL);
             CREATE TABLE episodes (
                 id INTEGER PRIMARY KEY AUTOINCREMENT, media_id INTEGER NOT NULL,
                 season INTEGER NOT NULL DEFAULT 1, episode INTEGER NOT NULL DEFAULT 1,
                 title TEXT, file_path TEXT, file_name TEXT, file_size INTEGER,
                 duration INTEGER, added_at INTEGER NOT NULL, tmdb_id INTEGER,
                 overview TEXT, still_path TEXT, air_date TEXT);
             INSERT INTO media (type, title, genre, added_at, updated_at)
                 VALUES ('tv','老剧','剧情 / 悬疑',0,0);
             INSERT INTO episodes (media_id, season, episode, added_at) VALUES (1,1,1,0);",
        )
        .unwrap();

        migrate(&conn).unwrap();

        for (table, col) in [
            ("media", "source"),
            ("media", "locked"),
            ("media", "tvdb_id"),
            ("media", "resolution"),
            ("media", "release_source"),
            ("episodes", "tvdb_id"),
            ("episodes", "resolution"),
            ("episodes", "release_source"),
        ] {
            assert!(column_exists(&conn, table, col).unwrap(), "缺少 {table}.{col}");
        }
        let title: String = conn
            .query_row("SELECT title FROM media WHERE id = 1", [], |r| r.get(0))
            .unwrap();
        assert_eq!(title, "老剧");
        let names = crate::store::db::tables(&conn).unwrap();
        for t in ["categories", "media_categories", "tags", "media_tags"] {
            assert!(names.contains(&t.to_string()), "缺少表 {t}");
        }
        migrate(&conn).unwrap();
    }

    #[test]
    fn category_name_is_unique_case_insensitively() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        conn.execute(
            "INSERT INTO categories (name, kind, origin, created_at) VALUES ('科幻','manual','manual',0)",
            [],
        )
        .unwrap();
        let dup = conn.execute(
            "INSERT INTO categories (name, kind, origin, created_at) VALUES ('科幻','auto','tmdb',0)",
            [],
        );
        assert!(dup.is_err(), "同名分类应被唯一索引挡住");
    }

    #[test]
    fn ensure_column_is_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE t (a INTEGER);").unwrap();
        ensure_column(&conn, "t", "b", "TEXT").unwrap();
        ensure_column(&conn, "t", "b", "TEXT").unwrap();
        ensure_column(&conn, "t", "b", "INTEGER").unwrap();
        assert!(column_exists(&conn, "t", "b").unwrap());
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM pragma_table_info('t')", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 2, "不应重复加列");
    }

    /* ---------------- v6 专项 ---------------- */

    /// v6 必须把这些对象补齐，否则后面所有 P0 都无从落地。
    #[test]
    fn v6_creates_playback_cache_and_fingerprint_columns() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();

        for (table, col) in [
            ("media", "file_mtime"),
            ("episodes", "file_mtime"),
            ("media", "tech_json"),
            ("media", "type_audit"),
            ("tags", "color"),
            ("tags", "alias"),
            ("tags", "parent_id"),
        ] {
            assert!(column_exists(&conn, table, col).unwrap(), "缺少 {table}.{col}");
        }
        let names = crate::store::db::tables(&conn).unwrap();
        assert!(names.contains(&"playback_state".to_string()), "缺少 playback_state");
        assert!(names.contains(&"provider_cache".to_string()), "缺少 provider_cache");
    }

    /// 老库（v5，有层级两列但没有 v6 的东西）升级后数据必须还在。
    #[test]
    fn v6_upgrade_keeps_existing_rows() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE media (
                 id INTEGER PRIMARY KEY AUTOINCREMENT, type TEXT NOT NULL DEFAULT 'movie',
                 title TEXT NOT NULL, original_title TEXT, group_key TEXT, year INTEGER,
                 tmdb_id INTEGER, poster_path TEXT, backdrop_path TEXT, overview TEXT,
                 vote_average REAL, genre TEXT, file_path TEXT, file_name TEXT,
                 file_size INTEGER, duration INTEGER, added_at INTEGER NOT NULL,
                 updated_at INTEGER NOT NULL, parent_id INTEGER,
                 sort_order INTEGER NOT NULL DEFAULT 0);
             INSERT INTO media (type, title, added_at, updated_at)
                 VALUES ('tv','老剧',0,0);",
        )
        .unwrap();

        migrate(&conn).unwrap();

        let title: String = conn
            .query_row("SELECT title FROM media WHERE id = 1", [], |r| r.get(0))
            .unwrap();
        assert_eq!(title, "老剧", "v6 迁移不能丢数据");
        assert!(column_exists(&conn, "media", "file_mtime").unwrap());
        // 幂等
        migrate(&conn).unwrap();
    }

    /// 观看状态唯一索引要能挡住「同一集两条记录」。
    #[test]
    fn playback_state_rejects_duplicate_target() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        conn.execute(
            "INSERT INTO media (type, title, added_at, updated_at) VALUES ('tv','剧',0,0)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO playback_state (media_id, episode_id, watched, updated_at) VALUES (1,NULL,1,0)",
            [],
        )
        .unwrap();
        let dup = conn.execute(
            "INSERT INTO playback_state (media_id, episode_id, watched, updated_at) VALUES (1,NULL,0,0)",
            [],
        );
        assert!(dup.is_err(), "(media_id, NULL 集) 重复应被唯一索引挡住");
    }

    /// FTS5 可用时索引与触发器都要建起来，且新行会被自动同步进索引。
    ///
    /// 注意这里**不能用 `MATCH '卧虎'`**：`unicode61` 把连续汉字当成一个整词，
    /// 「卧虎藏龙」是一个 token，子串查不到——这正是中文必须做 bigram 的原因
    /// （分词逻辑与对应用例在 `store/search.rs`）。本用例只验证「触发器同步」。
    #[test]
    fn fts_index_syncs_on_insert_when_available() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        if !fts5_available(&conn) {
            return; // 这个 SQLite 没编 FTS5，跳过（生产侧有 LIKE 回落）
        }
        conn.execute(
            "INSERT INTO media (type, title, genre, file_name, added_at, updated_at)
             VALUES ('movie','卧虎藏龙','剧情','卧虎藏龙.mkv',0,0)",
            [],
        )
        .unwrap();
        // ① 触发器把新行同步进来了
        let n: i64 = conn
            .query_row(&format!("SELECT COUNT(*) FROM {FTS_TABLE}"), [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1, "FTS 触发器应已把新行同步进索引");
        // ② 整词能命中（子串命中由 bigram 查询负责）
        let hit: i64 = conn
            .query_row(
                &format!("SELECT COUNT(*) FROM {FTS_TABLE} WHERE {FTS_TABLE} MATCH '卧虎藏龙'"),
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(hit, 1, "整词应能命中");
        // ③ 删掉后也要同步移除
        conn.execute("DELETE FROM media WHERE id = 1", []).unwrap();
        let after: i64 = conn
            .query_row(&format!("SELECT COUNT(*) FROM {FTS_TABLE}"), [], |r| r.get(0))
            .unwrap();
        assert_eq!(after, 0, "删除触发器没有同步清理索引");
    }

    /* ---------------- v7 专项 ---------------- */

    /// v7 必须建出资源目录库 + 快照表，且四类索引齐全。
    #[test]
    fn v7_creates_resource_registry_tables_and_indexes() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();

        let names = crate::store::db::tables(&conn).unwrap();
        for t in ["resource_assets", "resource_snapshots"] {
            assert!(names.contains(&t.to_string()), "缺少表 {t}，实际为 {names:?}");
        }
        for col in [
            "id",
            "root_path",
            "dir_path",
            "rel_dir",
            "file_name",
            "path",
            "size",
            "mtime",
            "ext",
            "status",
            "first_seen",
            "last_seen",
            "missing_at",
            "media_id",
            "content_key",
        ] {
            assert!(
                column_exists(&conn, "resource_assets", col).unwrap(),
                "缺少 resource_assets.{col}"
            );
        }
        for idx in [
            "idx_resource_assets_path",
            "idx_resource_assets_dir",
            "idx_resource_assets_status",
            "idx_resource_assets_media",
        ] {
            let n: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name=?1",
                    params![idx],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(n, 1, "缺少索引 {idx}");
        }
        let v: i32 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, SCHEMA_VERSION);
    }

    /// 同一路径只能有一行——这是 upsert 幂等的前提，靠 UNIQUE(path) 挡住。
    #[test]
    fn v7_resource_assets_path_is_unique() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        conn.execute(
            "INSERT INTO resource_assets (root_path, dir_path, file_name, path, first_seen, last_seen)
             VALUES ('D:\\m', 'D:\\m', 'a.mkv', 'D:\\m\\a.mkv', 1, 1)",
            [],
        )
        .unwrap();
        let dup = conn.execute(
            "INSERT INTO resource_assets (root_path, dir_path, file_name, path, first_seen, last_seen)
             VALUES ('D:\\m', 'D:\\m', 'a.mkv', 'D:\\m\\a.mkv', 2, 2)",
            [],
        );
        assert!(dup.is_err(), "同一绝对路径重复插入应被唯一索引挡住");
    }

    /// resource_assets.media_id **刻意没有外键**：条目被删后资源行必须活下来。
    ///
    /// 挂上 ON DELETE CASCADE 会把刚攒下的「它上次在哪」一起带走，
    /// 正是这张表要避免的事。
    #[test]
    fn v7_resource_assets_survives_media_deletion() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        let ts = crate::store::db::now_epoch();
        conn.execute(
            "INSERT INTO media (id, type, title, added_at, updated_at) VALUES (1,'movie','片',?1,?1)",
            params![ts],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO resource_assets
                 (root_path, dir_path, file_name, path, status, first_seen, last_seen, media_id)
             VALUES ('D:\\m','D:\\m','a.mkv','D:\\m\\a.mkv','missing',1,1,1)",
            [],
        )
        .unwrap();

        conn.execute("DELETE FROM media WHERE id = 1", []).unwrap();

        let left: i64 = conn
            .query_row("SELECT COUNT(*) FROM resource_assets", [], |r| r.get(0))
            .unwrap();
        assert_eq!(left, 1, "media 行删掉后资源行必须还在（定位旧资源全靠它）");
    }

    /// 老库（v6，什么都没有）升级到 v7 不能丢数据，且可以反复跑。
    #[test]
    fn v7_upgrade_keeps_existing_rows_and_is_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE media (
                 id INTEGER PRIMARY KEY AUTOINCREMENT, type TEXT NOT NULL DEFAULT 'movie',
                 title TEXT NOT NULL, original_title TEXT, group_key TEXT, year INTEGER,
                 tmdb_id INTEGER, poster_path TEXT, backdrop_path TEXT, overview TEXT,
                 vote_average REAL, genre TEXT, file_path TEXT, file_name TEXT,
                 file_size INTEGER, duration INTEGER, added_at INTEGER NOT NULL,
                 updated_at INTEGER NOT NULL, parent_id INTEGER,
                 sort_order INTEGER NOT NULL DEFAULT 0);
             INSERT INTO media (type, title, added_at, updated_at)
                 VALUES ('tv','老剧',0,0);",
        )
        .unwrap();

        migrate(&conn).unwrap();
        migrate(&conn).unwrap();

        let title: String = conn
            .query_row("SELECT title FROM media WHERE id = 1", [], |r| r.get(0))
            .unwrap();
        assert_eq!(title, "老剧", "v7 迁移不能丢数据");
        let names = crate::store::db::tables(&conn).unwrap();
        assert!(names.contains(&"resource_assets".to_string()));
        assert!(names.contains(&"resource_snapshots".to_string()));
    }

    /// FTS5 不可用时 `migrate` 也必须成功（这是「一键回落」的前提）。
    #[test]
    fn migrate_succeeds_without_fts5() {
        // 直接验证 migrate_v6 的前半段不依赖 FTS：在不支持的库上整段仍然可跑。
        // 这里用一个没有 FTS5 的假连接不可行，改为验证 fts5_available 本身不会崩。
        let conn = Connection::open_in_memory().unwrap();
        let _ = fts5_available(&conn);
        migrate(&conn).unwrap();
        let v: i32 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, SCHEMA_VERSION);
    }
}
