//! `tree.rs` 的单元测试。
//!
//! 用 `#[path]` 挂回所属模块（见 `tree.rs` 末尾）。

use super::*;
use crate::store::db;
use rusqlite::Connection;

fn memory_db() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
    db::migrate(&conn).unwrap();
    conn
}

/// 建一条剧集，返回 id。`parent` 为 `None` 时建在顶层。
fn tv(conn: &Connection, title: &str, parent: Option<i64>) -> i64 {
    let ts = db::now_epoch();
    let order: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(sort_order), -1) + 1 FROM media WHERE parent_id IS ?1",
            params![parent],
            |r| r.get(0),
        )
        .unwrap_or(0);
    conn.execute(
        "INSERT INTO media (type, title, group_key, source, parent_id, sort_order, added_at, updated_at) \
         VALUES ('tv', ?1, ?1, 'scan', ?2, ?3, ?4, ?4)",
        params![title, parent, order, ts],
    )
    .unwrap();
    conn.last_insert_rowid()
}

fn parent_of(conn: &Connection, id: i64) -> Option<i64> {
    conn.query_row("SELECT parent_id FROM media WHERE id = ?1", params![id], |r| {
        r.get(0)
    })
    .unwrap()
}

fn order_of(conn: &Connection, id: i64) -> i64 {
    conn.query_row(
        "SELECT COALESCE(sort_order,0) FROM media WHERE id = ?1",
        params![id],
        |r| r.get(0),
    )
    .unwrap()
}

/// 某个父节点下的子节点顺序（按排序号升序）。
fn children_of(conn: &Connection, parent: Option<i64>) -> Vec<i64> {
    let mut stmt = conn
        .prepare("SELECT id FROM media WHERE parent_id IS ?1 ORDER BY sort_order, id")
        .unwrap();
    stmt.query_map(params![parent], |r| r.get::<_, i64>(0))
        .unwrap()
        .map(|r| r.unwrap())
        .collect()
}

/* ------------------------------ 查询 ------------------------------ */

#[test]
fn tree_returns_depth_and_child_count() {
    let conn = memory_db();
    let root = tv(&conn, "西游记", None);
    let s1 = tv(&conn, "第一季", Some(root));
    let leaf = tv(&conn, "第一季-特辑", Some(s1));
    tv(&conn, "另一部", None);

    let nodes = get_media_tree_sync(&conn, "tv").unwrap();
    let find = |id: i64| nodes.iter().find(|n| n.id == id).cloned().unwrap();
    assert_eq!(find(root).depth, 0);
    assert_eq!(find(s1).depth, 1);
    assert_eq!(find(root).child_count, 1, "只有「第一季」是它的直接子级");
    assert_eq!(find(s1).child_count, 1, "特辑是它的子级，不能算到 root 头上");
    assert_eq!(find(leaf).depth, 2);
    assert_eq!(find(leaf).child_count, 0);
}

#[test]
fn tree_defaults_to_tv_only() {
    let conn = memory_db();
    tv(&conn, "剧", None);
    conn.execute(
        "INSERT INTO media (type,title,added_at,updated_at) VALUES ('movie','电影',1,1)",
        [],
    )
    .unwrap();
    assert_eq!(get_media_tree_sync(&conn, "tv").unwrap().len(), 1);
    assert_eq!(get_media_tree_sync(&conn, "all").unwrap().len(), 2);
}

/* ------------------------------ 移动 ------------------------------ */

#[test]
fn move_into_another_series_reparents() {
    let mut conn = memory_db();
    let a = tv(&conn, "A", None);
    let b = tv(&conn, "B", None);
    let leaf = tv(&conn, "叶", Some(a));
    let mut undo = Vec::new();

    let res = move_nodes_sync(&mut conn, vec![leaf], Some(b), None, &mut undo).unwrap();
    assert_eq!(parent_of(&conn, leaf), Some(b));
    assert_eq!(children_of(&conn, Some(b)), vec![leaf]);
    assert!(res.message.contains("B"), "提示语要说清放到哪儿：{}", res.message);
    assert_eq!(res.undo_depth, 1);
}

#[test]
fn move_keeps_order_and_renumbers_siblings() {
    let mut conn = memory_db();
    let p = tv(&conn, "父", None);
    let x = tv(&conn, "X", Some(p));
    let y = tv(&conn, "Y", Some(p));
    let z = tv(&conn, "Z", Some(p));
    assert_eq!(children_of(&conn, Some(p)), vec![x, y, z]);
    let mut undo = Vec::new();

    // 把 Z 挪到最前
    move_nodes_sync(&mut conn, vec![z], Some(p), Some(0), &mut undo).unwrap();
    assert_eq!(children_of(&conn, Some(p)), vec![z, x, y]);
    assert_eq!(order_of(&conn, z), 0, "排序号必须重排成连续的 0..n-1");
    assert_eq!(order_of(&conn, y), 2);
}

#[test]
fn moving_a_subtree_takes_descendants_along() {
    let mut conn = memory_db();
    let a = tv(&conn, "A", None);
    let b = tv(&conn, "B", None);
    let mid = tv(&conn, "中", Some(a));
    let leaf = tv(&conn, "叶", Some(mid));
    let mut undo = Vec::new();

    move_nodes_sync(&mut conn, vec![mid], Some(b), None, &mut undo).unwrap();
    assert_eq!(parent_of(&conn, mid), Some(b));
    assert_eq!(parent_of(&conn, leaf), Some(mid), "子孙要跟着走，不能被落下");
}

#[test]
fn selecting_parent_and_child_moves_them_as_one_unit() {
    let mut conn = memory_db();
    let a = tv(&conn, "A", None);
    let b = tv(&conn, "B", None);
    let mid = tv(&conn, "中", Some(a));
    let leaf = tv(&conn, "叶", Some(mid));
    let mut undo = Vec::new();

    move_nodes_sync(&mut conn, vec![mid, leaf], Some(b), None, &mut undo).unwrap();
    assert_eq!(parent_of(&conn, mid), Some(b));
    assert_eq!(parent_of(&conn, leaf), Some(mid), "子级不能被拆出来另算");
}

#[test]
fn dropping_into_own_descendant_is_rejected() {
    let mut conn = memory_db();
    let a = tv(&conn, "A", None);
    let mid = tv(&conn, "中", Some(a));
    let leaf = tv(&conn, "叶", Some(mid));
    let mut undo = Vec::new();

    let err = move_nodes_sync(&mut conn, vec![a], Some(leaf), None, &mut undo).unwrap_err();
    assert!(err.contains("循环引用"), "要说明是循环引用：{err}");
    assert_eq!(parent_of(&conn, a), None, "被拒绝时不能留下半套改动");
    assert!(undo.is_empty(), "失败不该进撤销栈");
}

#[test]
fn dropping_onto_self_is_rejected() {
    let mut conn = memory_db();
    let a = tv(&conn, "A", None);
    let mut undo = Vec::new();
    let err = move_nodes_sync(&mut conn, vec![a], Some(a), None, &mut undo).unwrap_err();
    assert!(err.contains("自己"), "要说明不能放自己里：{err}");
}

#[test]
fn exceeding_depth_limit_is_rejected_with_the_level_number() {
    let mut conn = memory_db();
    // 造到 MAX_DEPTH 层：l0 → l1 → l2 → l3
    let l0 = tv(&conn, "l0", None);
    let l1 = tv(&conn, "l1", Some(l0));
    let l2 = tv(&conn, "l2", Some(l1));
    let l3 = tv(&conn, "l3", Some(l2));
    let extra = tv(&conn, "extra", None);
    let mut undo = Vec::new();

    let err = move_nodes_sync(&mut conn, vec![extra], Some(l3), None, &mut undo).unwrap_err();
    assert!(err.contains("层级最多"), "要报层级超限：{err}");
    assert_eq!(parent_of(&conn, extra), None);
}

#[test]
fn moving_within_the_limit_is_allowed() {
    let mut conn = memory_db();
    let l0 = tv(&conn, "l0", None);
    let l1 = tv(&conn, "l1", Some(l0));
    let l2 = tv(&conn, "l2", Some(l1));
    let extra = tv(&conn, "extra", None);
    let mut undo = Vec::new();
    assert!(move_nodes_sync(&mut conn, vec![extra], Some(l2), None, &mut undo).is_ok());
    assert_eq!(parent_of(&conn, extra), Some(l2));
}

#[test]
fn moving_to_root_is_allowed() {
    let mut conn = memory_db();
    let a = tv(&conn, "A", None);
    let child = tv(&conn, "子", Some(a));
    let mut undo = Vec::new();
    let res = move_nodes_sync(&mut conn, vec![child], None, None, &mut undo).unwrap();
    assert_eq!(parent_of(&conn, child), None);
    assert!(res.message.contains("顶层"), "提示语要说清是顶层：{}", res.message);
}

#[test]
fn unknown_ids_are_reported() {
    let mut conn = memory_db();
    let mut undo = Vec::new();
    let err = move_nodes_sync(&mut conn, vec![999], None, None, &mut undo).unwrap_err();
    assert!(err.contains("999"), "要把找不到的 id 说出来：{err}");
}

/* ------------------------------ 另存为 ------------------------------ */

#[test]
fn save_as_creates_a_copy_under_the_target() {
    let mut conn = memory_db();
    let a = tv(&conn, "A", None);
    let b = tv(&conn, "B", None);
    let cid: i64 = {
        conn.execute(
            "INSERT INTO categories (name, kind, origin, created_at) VALUES ('测试栏目','manual','manual',1)",
            [],
        )
        .unwrap();
        conn.last_insert_rowid()
    };
    conn.execute(
        "INSERT INTO media_categories (media_id, category_id, created_at) VALUES (?1, ?2, 1)",
        params![a, cid],
    )
    .unwrap();
    let mut undo = Vec::new();

    let res = copy_node_sync(&mut conn, a, Some(b), None, &mut undo).unwrap();
    assert!(res.message.contains("副本"), "提示语要说明是副本：{}", res.message);
    let kids = children_of(&conn, Some(b));
    assert_eq!(kids.len(), 1);
    let copy = kids[0];
    let title: String = conn
        .query_row("SELECT title FROM media WHERE id = ?1", params![copy], |r| r.get(0))
        .unwrap();
    assert_eq!(title, "A 副本");
    let cats: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM media_categories WHERE media_id = ?1",
            params![copy],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(cats, 1, "分类要跟着复制");
}

#[test]
fn save_as_with_a_custom_title() {
    let mut conn = memory_db();
    let a = tv(&conn, "A", None);
    let mut undo = Vec::new();
    copy_node_sync(&mut conn, a, None, Some("A 续作".to_string()), &mut undo).unwrap();
    let title: String = conn
        .query_row("SELECT title FROM media WHERE title = 'A 续作'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(title, "A 续作");
}

#[test]
fn save_as_into_own_subtree_is_rejected() {
    let mut conn = memory_db();
    let a = tv(&conn, "A", None);
    let child = tv(&conn, "子", Some(a));
    let mut undo = Vec::new();
    let err = copy_node_sync(&mut conn, a, Some(child), None, &mut undo).unwrap_err();
    assert!(err.contains("自己的子级"), "要说清原因：{err}");
    assert!(undo.is_empty());
}

/* ------------------------------ 撤销 ------------------------------ */

#[test]
fn undo_restores_parent_and_order() {
    let mut conn = memory_db();
    let p = tv(&conn, "父", None);
    let x = tv(&conn, "X", Some(p));
    let y = tv(&conn, "Y", Some(p));
    let mut undo = Vec::new();

    move_nodes_sync(&mut conn, vec![y], Some(p), Some(0), &mut undo).unwrap();
    assert_eq!(children_of(&conn, Some(p)), vec![y, x]);

    let res = undo_tree_op_sync(&mut conn, &mut undo).unwrap();
    assert!(res.message.contains("撤销"), "要有撤销反馈：{}", res.message);
    assert_eq!(children_of(&conn, Some(p)), vec![x, y], "顺序也要回到原样");
    assert_eq!(undo.len(), 0);
}

#[test]
fn undo_of_save_as_deletes_the_copy() {
    let mut conn = memory_db();
    let a = tv(&conn, "A", None);
    let b = tv(&conn, "B", None);
    let mut undo = Vec::new();
    copy_node_sync(&mut conn, a, Some(b), None, &mut undo).unwrap();
    assert_eq!(children_of(&conn, Some(b)).len(), 1);

    undo_tree_op_sync(&mut conn, &mut undo).unwrap();
    assert_eq!(children_of(&conn, Some(b)).len(), 0, "副本要被删掉");
    let total: i64 = conn
        .query_row("SELECT COUNT(*) FROM media", [], |r| r.get(0))
        .unwrap();
    assert_eq!(total, 2, "原条目不能被顺手删掉");
}

#[test]
fn undo_with_nothing_to_undo_says_so() {
    let mut conn = memory_db();
    let mut undo = Vec::new();
    let err = undo_tree_op_sync(&mut conn, &mut undo).unwrap_err();
    assert!(err.contains("没有可撤销"), "空栈时要给出明确提示：{err}");
}

#[test]
fn undo_stack_is_bounded() {
    let mut conn = memory_db();
    let p = tv(&conn, "父", None);
    let x = tv(&conn, "X", Some(p));
    let mut undo = Vec::new();
    for _ in 0..(MAX_UNDO + 5) {
        let _ = move_nodes_sync(&mut conn, vec![x], Some(p), Some(0), &mut undo);
    }
    assert!(undo.len() <= MAX_UNDO, "撤销栈不能无限增长：{}", undo.len());
}

/// 撤销时**被搬的那条已经被删了**：跳过它，不能凭空把它"复活"成一行。
///
/// 现实路径：删除命令、重新识别、扫描收编都会裸删 `media` 行，而撤销栈是进程内
/// 的、删除时并不清栈 —— 于是栈里可能留着已经不在库里的 id。
#[test]
fn undo_skips_rows_that_were_deleted_meanwhile() {
    let mut conn = memory_db();
    let a = tv(&conn, "A", None);
    let b = tv(&conn, "B", None);
    let x = tv(&conn, "X", Some(a));
    let mut undo = Vec::new();

    // 把 X 从 A 挪到 B 下 → 栈里记下「X 的旧父是 A」
    move_nodes_sync(&mut conn, vec![x], Some(b), None, &mut undo).unwrap();
    assert_eq!(parent_of(&conn, x), Some(b));

    // 模拟别处裸删 X：注意**不能**清 undo，正是要复现"栈比库活得久"
    conn.execute("DELETE FROM media WHERE id = ?1", params![x])
        .unwrap();

    let res = undo_tree_op_sync(&mut conn, &mut undo).unwrap();
    assert!(
        res.message.contains("已被删除"),
        "要说清楚有行已经不在了：{}",
        res.message
    );
    let total: i64 = conn
        .query_row("SELECT COUNT(*) FROM media", [], |r| r.get(0))
        .unwrap();
    assert_eq!(total, 2, "撤销不能让已删的行重新出现");
}

/// 撤销时**旧父已经被删了**（X 还在）：退回顶层，不能写出指向已删行的 `parent_id`。
///
/// `media` 刻意没有外键，`UPDATE ... WHERE id = ?` 对已删的父**不会报错**，
/// 所以不校验就会静默造出一条悬挂引用。
#[test]
fn undo_falls_back_to_root_when_the_old_parent_is_gone() {
    let mut conn = memory_db();
    let a = tv(&conn, "A", None);
    let b = tv(&conn, "B", None);
    let x = tv(&conn, "X", Some(a));
    let mut undo = Vec::new();

    move_nodes_sync(&mut conn, vec![x], Some(b), None, &mut undo).unwrap();
    conn.execute("DELETE FROM media WHERE id = ?1", params![a])
        .unwrap();

    undo_tree_op_sync(&mut conn, &mut undo).unwrap();
    assert_eq!(
        parent_of(&conn, x),
        None,
        "旧父已删 → 退回顶层；写回已删的 id 就是悬挂引用"
    );
}

/// 一批 id 在父子链上**互相成环**时 `roots_of` 会一个都不返回。
///
/// 那种情况必须明确报错：否则会带着空名单往下走，把目标同级整体重排一遍、
/// 再用空名单拼出一句"已把「」移到…"的假成功。
#[test]
fn moving_a_batch_that_is_all_cyclic_is_rejected() {
    let mut conn = memory_db();
    let a = tv(&conn, "A", None);
    let b = tv(&conn, "B", None);
    // 直接改库造环（老数据被外部工具改脏就是这个形态）
    conn.execute(
        "UPDATE media SET parent_id = ?2 WHERE id = ?1",
        params![a, b],
    )
    .unwrap();
    conn.execute(
        "UPDATE media SET parent_id = ?2 WHERE id = ?1",
        params![b, a],
    )
    .unwrap();

    let mut undo = Vec::new();
    let err = move_nodes_sync(&mut conn, vec![a, b], None, None, &mut undo).unwrap_err();
    assert!(err.contains("循环"), "成环批要明确报错：{err}");
    assert!(undo.is_empty(), "被拒的操作不能进撤销栈");
}

/* ------------------------------ 删除 ------------------------------ */

#[test]
fn deleting_a_promotes_its_children_to_the_grandparent() {
    let mut conn = memory_db();
    let a = tv(&conn, "A", None);
    let mid = tv(&conn, "中", Some(a));
    let leaf = tv(&conn, "叶", Some(mid));

    let n = delete_nodes_sync(&mut conn, &[mid]).unwrap();
    assert_eq!(n, 1);
    assert_eq!(parent_of(&conn, leaf), Some(a), "子级要过继给祖父，不能成孤儿");
    let total: i64 = conn
        .query_row("SELECT COUNT(*) FROM media", [], |r| r.get(0))
        .unwrap();
    assert_eq!(total, 2);
}

#[test]
fn deleting_parent_and_child_together_finds_a_living_ancestor() {
    let mut conn = memory_db();
    let root = tv(&conn, "根", None);
    let mid = tv(&conn, "中", Some(root));
    let leaf = tv(&conn, "叶", Some(mid));
    let deep = tv(&conn, "深", Some(leaf));

    // 同时删「中」和「叶」：「深」要落到「根」下，而不是留在被删的「叶」下
    delete_nodes_sync(&mut conn, &[mid, leaf]).unwrap();
    assert_eq!(parent_of(&conn, deep), Some(root));
}

/// `adopt_children` 是给「不走 `delete_nodes_sync` 的裸删路径」用的同一个过继口子
/// （扫描收编 / 空壳清理 / 重新识别）。判据必须与批量删除完全一致，
/// 否则孤儿会从「修好的那条路」以外的口子继续漏进来。
#[test]
fn adopt_children_matches_the_batch_delete_rule() {
    let conn = memory_db();
    let a = tv(&conn, "A", None);
    let b = tv(&conn, "B", Some(a));
    let c = tv(&conn, "C", Some(b));

    // 删中间层：子级上提一层
    assert_eq!(adopt_children(&conn, &[b]).unwrap(), 1);
    assert_eq!(parent_of(&conn, c), Some(a), "父被删 → 过继给祖父");

    // 父与祖父一起删：剩下的候选新父全在名单里 → 挂顶层
    assert_eq!(adopt_children(&conn, &[a, b]).unwrap(), 1);
    assert_eq!(parent_of(&conn, c), None, "祖先全被删 → 挂顶层");

    // 叶子 / 空名单：0 行且不报错（调用方都是无脑先调一次的写法）
    assert_eq!(adopt_children(&conn, &[c]).unwrap(), 0);
    assert_eq!(adopt_children(&conn, &[]).unwrap(), 0);
}

/// 父链成环时绝不能挑一个「不在删除名单里的子孙」当新父。
#[test]
fn adopt_children_on_a_cycle_falls_back_to_top_level() {
    let conn = memory_db();
    let a = tv(&conn, "A", None);
    let b = tv(&conn, "B", Some(a));
    // 手工把数据改脏：a 挂回 b 下面，形成 a <-> b
    conn.execute(
        "UPDATE media SET parent_id = ?2 WHERE id = ?1",
        params![a, b],
    )
    .unwrap();

    assert_eq!(adopt_children(&conn, &[a]).unwrap(), 1);
    assert_eq!(parent_of(&conn, b), None, "成环 → 一律挂顶层，绝不自己挂自己");
}
