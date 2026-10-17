//! 剧集层级（树）的命令层：查询、移动、另存为、撤销。
//!
//! 分工：
//! - 规则（能不能放）在 `core::tree`，纯函数、可单测；
//! - 这里只做「取数 → 套规则 → 写库」，并把**失败回滚**交给事务；
//! - 撤销栈放在进程内（`TreeUndo`），只记「改之前的样子」，不存整库快照。

use rusqlite::{params, Connection};
use serde::Serialize;
use std::sync::Mutex;
use tauri::State;

use crate::core::tree;

/// 撤销栈最多保留多少步。层级操作都很小，20 步足够，也避免无限增长。
const MAX_UNDO: usize = 20;

/* ------------------------------ 数据结构 ------------------------------ */

/// 树上的一个节点（`media` 行 + 层级信息）。
///
/// 前端拿到的是**扁平列表**（`parent_id` 表达关系），由它自己建树渲染；
/// 后端不递归拼嵌套结构，免得一份数据在两端各拼一次、两边不一致。
#[derive(Debug, Clone, Serialize)]
pub struct MediaTreeNode {
    pub id: i64,
    pub parent_id: Option<i64>,
    pub sort_order: i64,
    #[serde(rename = "type")]
    pub media_type: String,
    pub title: String,
    /// 同名剧集带目录名后缀（`剧名 · 目录名`），与海报墙保持一致。
    pub display_title: String,
    pub year: Option<i32>,
    pub poster_path: Option<String>,
    pub episode_count: i64,
    /// 直接子节点数量（>0 时前端显示展开箭头）。
    pub child_count: i64,
    /// 层级（顶层 = 0）。由后端算好，前端不必自己走一遍父子链。
    pub depth: i64,
    pub source: String,
    pub locked: bool,
}

/// 一次可撤销的操作。
#[derive(Debug, Clone)]
pub enum TreeOp {
    /// 改挂 / 排序：记下每条被改动的行的**旧**父节点与旧排序号。
    Reparent { before: Vec<(i64, Option<i64>, i64)> },
    /// 另存为建出来的新行：撤销 = 删掉它们（副本没有子级，删起来无副作用）。
    Created { ids: Vec<i64> },
}

/// 进程内的撤销栈。`App::manage` 按类型管理，这里是独立类型不会撞车。
#[derive(Default)]
pub struct TreeUndo(pub Mutex<Vec<TreeOp>>);

impl TreeUndo {
    /// 只暴露「还剩几步」给界面：入栈 / 出栈都发生在命令处理函数里
    /// （那里已经拿着锁，再套一层方法反而容易自己锁死自己）。
    pub fn depth(&self) -> i64 {
        self.0.lock().map(|s| s.len() as i64).unwrap_or(0)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct TreeOpResult {
    pub message: String,
    /// 撤销栈还剩多少步（前端据此禁用「撤销」按钮）。
    pub undo_depth: i64,
}

/* ------------------------------ 查询 ------------------------------ */

const TREE_COLUMNS: &str = "m.id, m.parent_id, COALESCE(m.sort_order, 0), m.type, m.title, \
     m.year, m.poster_path, COALESCE(m.source, 'scan'), COALESCE(m.locked, 0), m.group_key, \
     (SELECT COUNT(*) FROM episodes e WHERE e.media_id = m.id), \
     (SELECT COUNT(*) FROM media c WHERE c.parent_id = m.id), \
     CASE WHEN m.group_key IS NOT NULL \
            AND (SELECT COUNT(*) FROM media d WHERE d.type = 'tv' AND d.type = m.type \
                 AND d.title = m.title) > 1 \
          THEN m.title || ' · ' || m.group_key ELSE m.title END";

/// 层级视图的数据源。
///
/// 默认只列**剧集**（`media_type` 缺省为 `tv`）——层级是给剧集用的，
/// 电影混进来会让「父子」语义变模糊。传 `all` 可以拿到全部。
#[tauri::command]
pub fn get_media_tree(
    state: State<'_, Mutex<Connection>>,
    media_type: Option<String>,
) -> Result<Vec<MediaTreeNode>, String> {
    let conn = state.lock().map_err(|e| e.to_string())?;
    get_media_tree_sync(&conn, &media_type.unwrap_or_default())
}

pub fn get_media_tree_sync(
    conn: &Connection,
    media_type: &str,
) -> Result<Vec<MediaTreeNode>, String> {
    let filter = if media_type.is_empty() {
        "tv".to_string()
    } else {
        media_type.to_string()
    };

    // 深度要沿父链走，而父节点可能被类型过滤掉了（比如父是电影）；
    // 所以边表**始终取全表**，过滤只作用在最终输出上。
    let edges = load_edges(&conn)?;

    let sql = format!(
        "SELECT {TREE_COLUMNS} FROM media m \
         WHERE (?1 = 'all' OR m.type = ?1) \
         ORDER BY COALESCE(m.sort_order, 0) ASC, m.title COLLATE NOCASE ASC, m.id ASC"
    );
    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![filter], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, Option<i64>>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, Option<i32>>(5)?,
                r.get::<_, Option<String>>(6)?,
                r.get::<_, String>(7)?,
                r.get::<_, bool>(8)?,
                r.get::<_, Option<String>>(9)?,
                r.get::<_, i64>(10)?,
                r.get::<_, i64>(11)?,
                r.get::<_, String>(12)?,
            ))
        })
        .map_err(|e| e.to_string())?;

    let mut out = Vec::new();
    for r in rows {
        let t = r.map_err(|e| e.to_string())?;
        let depth = tree::depth_of(&edges, t.0).unwrap_or(0);
        out.push(MediaTreeNode {
            id: t.0,
            parent_id: t.1,
            sort_order: t.2,
            media_type: t.3,
            title: t.4,
            year: t.5,
            poster_path: t.6,
            source: t.7,
            locked: t.8,
            episode_count: t.10,
            child_count: t.11,
            display_title: t.12,
            depth: depth as i64,
        });
        let _ = t.9; // group_key 只参与 display_title 的拼接
    }
    Ok(out)
}

/// 全表的 `(id, parent_id)` 边表（**不过滤类型**：父节点可能是别的类型）。
pub(crate) fn load_edges(conn: &Connection) -> Result<Vec<(i64, Option<i64>)>, String> {
    let mut stmt = conn
        .prepare("SELECT id, parent_id FROM media")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<i64>>(1)?)))
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| e.to_string())?);
    }
    Ok(out)
}

fn title_of(conn: &Connection, id: i64) -> String {
    conn.query_row("SELECT title FROM media WHERE id = ?1", params![id], |r| {
        r.get::<_, String>(0)
    })
    .unwrap_or_else(|_| format!("#{id}"))
}

/// 把 `core::tree` 的判定错误翻成**带剧名**的中文提示。
///
/// 纯规则层只认 id，用户要看的是名字，所以在这一层查标题。
fn describe_drop_error(conn: &Connection, err: tree::DropError) -> String {
    match err {
        tree::DropError::SelfTarget { id } => {
            format!("不能把「{}」放到它自己里面", title_of(conn, id))
        }
        tree::DropError::Cycle { mover, target } => format!(
            "不能把「{}」放进「{}」——那是它自己的子级，会形成循环引用",
            title_of(conn, mover),
            title_of(conn, target)
        ),
        tree::DropError::TooDeep { depth, limit } => {
            format!("层级最多 {limit} 层，这样会到第 {depth} 层")
        }
        tree::DropError::Missing { id } => format!("找不到 id 为 {id} 的条目（可能已被删除）"),
    }
}

/// 给某节点加一个子节点是否合法（新建 / 另存为都会先问一次）。
pub fn check_child_allowed(conn: &Connection, parent_id: i64) -> Result<(), String> {
    let edges = load_edges(conn)?;
    let depth = tree::depth_of(&edges, parent_id)
        .map_err(|e| describe_drop_error(conn, e))?;
    if depth + 1 > tree::MAX_DEPTH - 1 {
        return Err(format!(
            "层级最多 {} 层，「{}」已经在第 {} 层，不能再往下建了",
            tree::MAX_DEPTH,
            title_of(conn, parent_id),
            depth + 1
        ));
    }
    Ok(())
}

/* ------------------------------ 移动 ------------------------------ */

/// 把一批节点搬到 `target_parent` 下的指定位置（`None` = 顶层）。
///
/// 几条有意为之的行为：
/// - **子孙跟着走**：搬的是整棵子树，不是单独一行；
/// - **同时选中父子时只搬父**（`roots_of`），否则索引会错乱、父子会被拆开；
/// - **同级整体重排号**：排序号一律重排成 0..n-1，避免留下空洞或重复；
/// - **整批在一个事务里**：中途任何一步失败自动回滚，不会留下半套层级。
pub fn move_nodes_sync(
    conn: &mut Connection,
    ids: Vec<i64>,
    target_parent: Option<i64>,
    index: Option<i64>,
    undo: &mut Vec<TreeOp>,
) -> Result<TreeOpResult, String> {
    if ids.is_empty() {
        return Err("请先选中要移动的剧集".to_string());
    }
    let edges = load_edges(conn)?;
    let known: Vec<i64> = edges.iter().map(|(id, _)| *id).collect();
    for id in &ids {
        if !known.contains(id) {
            return Err(format!("找不到 id 为 {id} 的条目（可能已被删除）"));
        }
    }
    if let Some(p) = target_parent {
        if !known.contains(&p) {
            return Err(format!("找不到 id 为 {p} 的条目（可能已被删除）"));
        }
    }

    // 只搬「祖先不在本批内」的节点，其余作为子树跟着走
    let movers = tree::roots_of(&edges, &ids);
    // 全被筛掉 = 这一批 id 互相构成环（老数据被外部改脏时会出现）：
    // `roots_of` 认为每个 id 都有「祖先在本批内」，于是谁都不搬。
    // 不能就这么往下走 —— 那样会把目标同级**整体重排一遍**，再用空名单
    // 拼出一句「已把「」移到…」的假成功。宁可明确报错。
    if movers.is_empty() {
        return Err(
            "选中的条目之间存在循环的父子关系（老数据被改脏了），无法移动；\
             请先在详情页把其中一条挂回顶层"
                .to_string(),
        );
    }
    tree::validate_drop(&edges, &movers, target_parent)
        .map_err(|e| describe_drop_error(conn, e))?;

    // 目标位置的同级列表（排除待搬节点本身）
    let mut siblings: Vec<i64> = edges
        .iter()
        .filter(|(_, p)| *p == target_parent)
        .map(|(id, _)| *id)
        .filter(|id| !movers.contains(id))
        .collect();
    // 排序号可能重复（老数据升级上来的全都是 0），按 (sort_order, id) 排序才稳定
    let order_of = |id: i64| -> (i64, i64) {
        conn.query_row(
            "SELECT COALESCE(sort_order,0), id FROM media WHERE id = ?1",
            params![id],
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)),
        )
        .unwrap_or((0, id))
    };
    siblings.sort_by_key(|id| order_of(*id));

    let mut at = index.unwrap_or(siblings.len() as i64) as usize;
    if at > siblings.len() {
        at = siblings.len();
    }
    let mut final_order: Vec<i64> = siblings[..at].to_vec();
    final_order.extend(movers.iter().copied());
    final_order.extend(siblings[at..].iter().copied());

    // 落地：整批一个事务
    let ts = crate::store::db::now_epoch();
    let mut before: Vec<(i64, Option<i64>, i64)> = Vec::new();
    {
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        for (i, id) in final_order.iter().enumerate() {
            let (old_parent, old_order) = tx
                .query_row(
                    "SELECT parent_id, COALESCE(sort_order,0) FROM media WHERE id = ?1",
                    params![id],
                    |r| Ok((r.get::<_, Option<i64>>(0)?, r.get::<_, i64>(1)?)),
                )
                .map_err(|e| e.to_string())?;
            let new_order = i as i64;
            if old_parent == target_parent && old_order == new_order {
                continue;
            }
            before.push((*id, old_parent, old_order));
            tx.execute(
                "UPDATE media SET parent_id = ?2, sort_order = ?3, updated_at = ?4 WHERE id = ?1",
                params![id, target_parent, new_order, ts],
            )
            .map_err(|e| e.to_string())?;
        }
        tx.commit().map_err(|e| e.to_string())?;
    }

    let where_ = match target_parent {
        Some(p) => format!("「{}」内", title_of(conn, p)),
        None => "顶层".to_string(),
    };
    let names = movers
        .iter()
        .map(|id| title_of(conn, *id))
        .collect::<Vec<_>>()
        .join("、");

    if before.is_empty() {
        return Ok(TreeOpResult {
            message: format!("「{names}」已经在{where_}的这个位置，没有改动"),
            undo_depth: undo.len() as i64,
        });
    }
    undo.push(TreeOp::Reparent { before });
    if undo.len() > MAX_UNDO {
        undo.remove(0);
    }
    Ok(TreeOpResult {
        message: format!("已把「{names}」移到{where_}"),
        undo_depth: undo.len() as i64,
    })
}

#[tauri::command]
pub fn move_media_nodes(
    state: State<'_, Mutex<Connection>>,
    undo: State<'_, TreeUndo>,
    ids: Vec<i64>,
    target_parent_id: Option<i64>,
    index: Option<i64>,
) -> Result<TreeOpResult, String> {
    let mut conn = state.lock().map_err(|e| e.to_string())?;
    let mut stack = undo.0.lock().map_err(|e| e.to_string())?;
    let res = move_nodes_sync(&mut conn, ids, target_parent_id, index, &mut stack)?;
    drop(stack);
    drop(conn);
    Ok(TreeOpResult {
        message: res.message,
        undo_depth: undo.depth(),
    })
}

/* ------------------------------ 另存为（复制） ------------------------------ */

/// 把一条剧集**复制**成一个新节点，挂到 `target_parent` 下。
///
/// ⚠️ 刻意**不复制单集**：单集绑定本地文件，`episodes.file_path` 有唯一索引，
/// 复制过去只会产生一排指向同一文件的重复行。副本是一个新的容器，
/// 用户再用「添加文件到这部剧」往里填。这个行为必须在提示语里讲清楚。
pub fn copy_node_sync(
    conn: &mut Connection,
    id: i64,
    target_parent: Option<i64>,
    new_title: Option<String>,
    undo: &mut Vec<TreeOp>,
) -> Result<TreeOpResult, String> {
    let edges = load_edges(conn)?;
    let known: Vec<i64> = edges.iter().map(|(n, _)| *n).collect();
    if !known.contains(&id) {
        return Err(format!("找不到 id 为 {id} 的条目（可能已被删除）"));
    }
    if let Some(p) = target_parent {
        if !known.contains(&p) {
            return Err(format!("找不到 id 为 {p} 的条目（可能已被删除）"));
        }
        if p == id || tree::subtree_of(&edges, id).contains(&p) {
            return Err(format!(
                "不能把「{}」复制到它自己的子级里",
                title_of(conn, id)
            ));
        }
        check_child_allowed(conn, p)?;
    }

    let title = match new_title {
        Some(t) if !t.trim().is_empty() => t.trim().to_string(),
        _ => format!("{} 副本", title_of(conn, id)),
    };
    let ts = crate::store::db::now_epoch();

    let new_id: i64 = {
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        let sort_order: i64 = tx
            .query_row(
                "SELECT COALESCE(MAX(sort_order), -1) + 1 FROM media WHERE parent_id IS ?1",
                params![target_parent],
                |r| r.get(0),
            )
            .unwrap_or(0);
        tx.execute(
            "INSERT INTO media (type, title, original_title, group_key, year, genre, overview, \
                    poster_path, backdrop_path, tmdb_id, vote_average, source, locked, \
                    parent_id, sort_order, added_at, updated_at) \
             SELECT type, ?2, ?2, ?2, year, genre, overview, poster_path, backdrop_path, \
                    tmdb_id, vote_average, 'manual', locked, ?3, ?4, ?5, ?5 \
               FROM media WHERE id = ?1",
            params![id, title, target_parent, sort_order, ts],
        )
        .map_err(|e| format!("另存为失败：{e}"))?;
        let new_id = tx.last_insert_rowid();
        // 分类 / 标签跟着复制：副本应该和原条目归在同一个栏目里
        tx.execute(
            "INSERT OR IGNORE INTO media_categories (media_id, category_id, created_at) \
             SELECT ?2, category_id, ?3 FROM media_categories WHERE media_id = ?1",
            params![id, new_id, ts],
        )
        .map_err(|e| e.to_string())?;
        tx.execute(
            "INSERT OR IGNORE INTO media_tags (media_id, tag_id, created_at) \
             SELECT ?2, tag_id, ?3 FROM media_tags WHERE media_id = ?1",
            params![id, new_id, ts],
        )
        .map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;
        new_id
    };

    undo.push(TreeOp::Created { ids: vec![new_id] });
    if undo.len() > MAX_UNDO {
        undo.remove(0);
    }
    let where_ = match target_parent {
        Some(p) => format!("「{}」内", title_of(conn, p)),
        None => "顶层".to_string(),
    };
    Ok(TreeOpResult {
        message: format!(
            "已在{where_}另存为「{title}」（副本不含单集，可再往里添加文件）"
        ),
        undo_depth: undo.len() as i64,
    })
}

#[tauri::command]
pub fn copy_media_node(
    state: State<'_, Mutex<Connection>>,
    undo: State<'_, TreeUndo>,
    id: i64,
    target_parent_id: Option<i64>,
    title: Option<String>,
) -> Result<TreeOpResult, String> {
    let mut conn = state.lock().map_err(|e| e.to_string())?;
    let mut stack = undo.0.lock().map_err(|e| e.to_string())?;
    let res = copy_node_sync(&mut conn, id, target_parent_id, title, &mut stack)?;
    drop(stack);
    drop(conn);
    Ok(TreeOpResult {
        message: res.message,
        undo_depth: undo.depth(),
    })
}

/* ------------------------------ 删除 ------------------------------ */

/// 删除剧集（含批量），并把它名下的子级**过继给祖父**。
///
/// `media.parent_id` 没有外键约束（层级规则在应用层校验），所以直接删父节点
/// 会留下一串指向已删行的孤儿——它们在树视图里永远不显示，等于数据丢了。
/// 这里在**同一个事务**里先把子级上提一层，再删目标行。
///
/// 入参先做一次规范化（去重 + 丢掉非正 id）：
/// - 重复 id 会让调用方拿到的条数与「用户选了几条」对不上；
/// - `id <= 0` 是前端不该传的脏数据，静默丢掉而不是让它进 SQL。
///
/// 返回**实际删除的行数**（不存在的 id 不计入），调用方据此提示用户。
pub fn delete_nodes_sync(conn: &mut Connection, ids: &[i64]) -> Result<i64, String> {
    match delete_nodes_once(conn, ids) {
        // 检索索引（FTS5）的内部结构坏掉时，`DELETE FROM media` 会连带失败——
        // `media_fts_ad` 触发器要从索引里删对应行，而索引一碰就
        // `database disk image is malformed`。重建索引后重试一次即可自愈。
        // 重建自己也失败就抛**原始**错误，别用重建的错误盖掉真正的原因。
        Err(e) if crate::store::schema::heal_fts_if_malformed(conn, &e) => {
            delete_nodes_once(conn, ids)
        }
        other => other,
    }
}

fn delete_nodes_once(conn: &mut Connection, ids: &[i64]) -> Result<i64, String> {
    let wanted = normalize_ids(ids);
    if wanted.is_empty() {
        return Ok(0);
    }
    let tx = conn.transaction().map_err(|e| e.to_string())?;

    // 子级先过继，再删父行。`parent_id` 没有外键约束，不过继就会留下一串
    // 指向已删行的孤儿（墙上不显示、详情页却还挂着「脱离聚合」）。
    adopt_children(&tx, &wanted)?;

    let placeholders: Vec<String> = (1..=wanted.len()).map(|i| format!("?{i}")).collect();
    let sql = format!("DELETE FROM media WHERE id IN ({})", placeholders.join(","));
    let n = tx
        .execute(&sql, rusqlite::params_from_iter(wanted.iter()))
        .map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())?;
    Ok(n as i64)
}

/// 删除若干行**之前**把它们的子级过继出去（在当前连接上直接执行）。
///
/// 判据来自 `core::tree::plan_adoption`，与批量删除走的是同一份规则。
/// 存在的理由：库里还有几条**不走 `delete_nodes_sync`** 的裸删路径（扫描收编、
/// 空壳清理、重新识别），它们以前直接 `DELETE FROM media`，留下的悬挂
/// `parent_id` 正是 `undo_tree_op_sync` 里那段注释抱怨的现象——卡片上还留着
/// 「脱离聚合」按钮、详情页显示 `#<已删id>`。同一条规则不该有两份实现，
/// 所以这几处一律先调它。
///
/// 返回被过继的行数（0 = 这些节点本来就没有需要挪动的子级）。
pub(crate) fn adopt_children(conn: &Connection, doomed: &[i64]) -> Result<usize, String> {
    if doomed.is_empty() {
        return Ok(0);
    }
    let edges = load_edges(conn)?;
    let plan = tree::plan_adoption(&edges, doomed);
    if plan.is_empty() {
        return Ok(0);
    }
    let ts = crate::store::db::now_epoch();
    let mut moved = 0usize;
    for (child, target) in &plan {
        moved += conn
            .execute(
                "UPDATE media SET parent_id = ?2, updated_at = ?3 WHERE id = ?1",
                params![child, target, ts],
            )
            .map_err(|e| e.to_string())?;
    }
    Ok(moved)
}

/// 把调用方传来的 id 列表规范化：**去重 + 丢弃非正值，保持原有顺序**。
///
/// 保序是为了让错误信息里的 id 顺序和用户勾选的顺序一致，方便对照。
pub fn normalize_ids(ids: &[i64]) -> Vec<i64> {
    let mut seen: std::collections::HashSet<i64> = std::collections::HashSet::new();
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        if *id > 0 && seen.insert(*id) {
            out.push(*id);
        }
    }
    out
}

/* ------------------------------ 撤销 ------------------------------ */

/// 撤销最后一次层级操作。
///
/// 回滚只针对**层级字段**（父节点与排序号），不动标题、海报、单集——
/// 那是别的入口写的数据，撤销层级时不该被顺手改回去。
pub fn undo_tree_op_sync(
    conn: &mut Connection,
    undo: &mut Vec<TreeOp>,
) -> Result<TreeOpResult, String> {
    let op = match undo.pop() {
        Some(o) => o,
        None => return Err("没有可撤销的层级操作".to_string()),
    };
    let ts = crate::store::db::now_epoch();
    let message = {
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        let msg = match &op {
            TreeOp::Reparent { before } => {
                // ⚠️ 撤销栈是**进程内**的，而条目会被别处删掉（批量删除、重新识别、
                // 扫描收编都会裸删 media 行，不走「子级过继给祖父」那条路）。
                // 这里必须逐行确认目标还在：`media` 刻意没有外键，
                // `UPDATE ... WHERE id = ?` 对已删的行**静默影响 0 行**，
                // 但 `parent_id` 指向已删行的「旧父」会被照样写回去 ——
                // 那就凭空造出一条悬挂引用（卡上还留着「解除分组」按钮，
                // 详情页显示 `#<已删id>`）。跳过不存在的行，并把颗数说实话。
                let mut restored = 0usize;
                let mut skipped = 0usize;
                for (id, old_parent, order) in before {
                    let alive: bool = tx
                        .query_row("SELECT 1 FROM media WHERE id = ?1", params![id], |_| {
                            Ok(true)
                        })
                        .unwrap_or(false);
                    if !alive {
                        skipped += 1;
                        continue;
                    }
                    // 旧父也可能是已删的中间层：那样写回去同样是悬挂引用，
                    // 这种情况退回顶层（与 `delete_nodes_sync` 过继时的兜底一致）。
                    let parent: Option<i64> = match old_parent {
                        Some(p) => tx
                            .query_row("SELECT 1 FROM media WHERE id = ?1", params![p], |_| {
                                Ok(true)
                            })
                            .ok()
                            .map(|_| Some(*p))
                            .unwrap_or(None),
                        None => None,
                    };
                    tx.execute(
                        "UPDATE media SET parent_id = ?2, sort_order = ?3, updated_at = ?4 \
                          WHERE id = ?1",
                        params![id, parent, order, ts],
                    )
                    .map_err(|e| e.to_string())?;
                    restored += 1;
                }
                if restored == 0 {
                    format!("没有可恢复的层级改动（{skipped} 条已被删除）")
                } else if skipped > 0 {
                    format!("已撤销上一步移动（恢复 {restored} 条，{skipped} 条已被删除）")
                } else {
                    format!("已撤销上一步移动（恢复 {restored} 条）")
                }
            }
            TreeOp::Created { ids } => {
                for id in ids {
                    tx.execute("DELETE FROM media WHERE id = ?1", params![id])
                        .map_err(|e| e.to_string())?;
                }
                format!("已撤销另存为（删除 {} 个副本）", ids.len())
            }
        };
        tx.commit().map_err(|e| e.to_string())?;
        msg
    };
    Ok(TreeOpResult {
        message,
        undo_depth: undo.len() as i64,
    })
}

#[tauri::command]
pub fn undo_tree_op(
    state: State<'_, Mutex<Connection>>,
    undo: State<'_, TreeUndo>,
) -> Result<TreeOpResult, String> {
    let mut conn = state.lock().map_err(|e| e.to_string())?;
    let mut stack = undo.0.lock().map_err(|e| e.to_string())?;
    let res = undo_tree_op_sync(&mut conn, &mut stack)?;
    drop(stack);
    drop(conn);
    Ok(TreeOpResult {
        message: res.message,
        undo_depth: undo.depth(),
    })
}

#[cfg(test)]
#[path = "tests/tree.rs"]
mod tests;
