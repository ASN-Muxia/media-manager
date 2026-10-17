/**
 * 剧集层级（树）的**纯函数**。
 *
 * 刻意与 Vue 组件分开：建树、扁平化、放置合法性都是可单测的纯逻辑，
 * 组件只负责把结果画出来并转发事件（与 `utils/dragSelect.ts` 同一个思路）。
 *
 * ⚠️ 这里的校验只是**提前反馈**（画红线、给提示），真正的拦截在 Rust
 * `core::tree`——前端规则永远不能当成安全边界。
 */

import { TREE_MAX_DEPTH, type MediaTreeNode } from "../api/commands";

/** 建好的树节点（包一层，不动原始数据）。 */
export interface TreeNode {
  node: MediaTreeNode;
  children: TreeNode[];
}

/** 用于渲染的一行（树被摊平后的结果）。 */
export interface FlatRow {
  node: MediaTreeNode;
  /** 缩进层级（顶层 = 0） */
  depth: number;
  hasChildren: boolean;
  expanded: boolean;
  /** 在同级中的位置（拖拽计算目标序号用） */
  index: number;
}

/** 拖拽落点的三种形态。 */
export type DropPosition = "before" | "inside" | "after";

/**
 * 把扁平列表建成树。
 *
 * 排序按 `sort_order` 升序，相同则按标题——与后端 SQL 的排序保持一致，
 * 否则「拖到第 2 位」在前后端看到的位置会不一样。
 */
export function buildTree(nodes: MediaTreeNode[]): TreeNode[] {
  const byId = new Map<number, TreeNode>();
  for (const n of nodes) {
    byId.set(n.id, { node: n, children: [] });
  }
  const roots: TreeNode[] = [];
  for (const n of nodes) {
    const self = byId.get(n.id)!;
    // 父节点不在当前列表里（比如被类型过滤掉了）时按顶层处理，
    // 否则这些节点会整片消失在树视图里。
    const parent = n.parent_id !== null ? byId.get(n.parent_id) : undefined;
    if (parent && parent !== self) {
      parent.children.push(self);
    } else {
      roots.push(self);
    }
  }
  const sort = (list: TreeNode[]): void => {
    list.sort(
      (a, b) =>
        a.node.sort_order - b.node.sort_order ||
        a.node.title.localeCompare(b.node.title, "zh") ||
        a.node.id - b.node.id,
    );
    for (const t of list) sort(t.children);
  };
  sort(roots);
  return roots;
}

/** 摊平成可渲染的行；折叠的节点其子级不出现在结果里。 */
export function flattenTree(roots: TreeNode[], expanded: Set<number>): FlatRow[] {
  const out: FlatRow[] = [];
  const walk = (list: TreeNode[], depth: number): void => {
    list.forEach((t, index) => {
      const open = expanded.has(t.node.id);
      out.push({
        node: t.node,
        depth,
        hasChildren: t.children.length > 0,
        expanded: open,
        index,
      });
      if (open) walk(t.children, depth + 1);
    });
  };
  walk(roots, 0);
  return out;
}

/** 某节点的整棵子树 id（含自己）。 */
export function subtreeIds(nodes: MediaTreeNode[], id: number): number[] {
  const childrenOf = new Map<number, number[]>();
  for (const n of nodes) {
    if (n.parent_id === null) continue;
    const list = childrenOf.get(n.parent_id) ?? [];
    list.push(n.id);
    childrenOf.set(n.parent_id, list);
  }
  const out: number[] = [];
  const stack = [id];
  const seen = new Set<number>([id]);
  while (stack.length) {
    const cur = stack.pop()!;
    out.push(cur);
    for (const c of childrenOf.get(cur) ?? []) {
      if (seen.has(c)) continue; // 数据成环时兜住，别死循环
      seen.add(c);
      stack.push(c);
    }
  }
  return out;
}

/** `ancestor` 是否是 `node` 的祖先。 */
export function isAncestor(
  nodes: MediaTreeNode[],
  ancestor: number,
  node: number,
): boolean {
  const parentOf = new Map<number, number | null>();
  for (const n of nodes) parentOf.set(n.id, n.parent_id);
  let cur = parentOf.get(node) ?? null;
  const seen = new Set<number>();
  while (cur !== null && cur !== undefined) {
    if (cur === ancestor) return true;
    if (seen.has(cur)) return false;
    seen.add(cur);
    cur = parentOf.get(cur) ?? null;
  }
  return false;
}

/** 子树高度（叶子 = 0）。 */
export function subtreeHeight(nodes: MediaTreeNode[], id: number): number {
  const depthOf = (n: number): number => {
    const parentOf = new Map<number, number | null>();
    for (const x of nodes) parentOf.set(x.id, x.parent_id);
    let d = 0;
    let cur = parentOf.get(n) ?? null;
    const seen = new Set<number>();
    while (cur !== null && cur !== undefined && !seen.has(cur)) {
      seen.add(cur);
      d += 1;
      cur = parentOf.get(cur) ?? null;
    }
    return d;
  };
  const base = depthOf(id);
  let best = base;
  for (const n of subtreeIds(nodes, id)) {
    best = Math.max(best, depthOf(n));
  }
  return best - base;
}

export interface DropCheck {
  ok: boolean;
  /** 不合法时给用户的说明（合法时为空串）。 */
  reason: string;
}

/**
 * 这次放置能不能做。**只用于界面反馈**，真正的拦截在后端。
 *
 * `mode` 为 `inside` 时目标是「成为 target 的子级」，否则是「插到 target 前后」，
 * 两者的父节点不同（前者父 = target，后者父 = target 的父），校验要用对。
 */
export function canDrop(
  nodes: MediaTreeNode[],
  dragIds: number[],
  targetId: number | null,
  mode: DropPosition,
): DropCheck {
  const ok: DropCheck = { ok: true, reason: "" };
  if (!dragIds.length) return { ok: false, reason: "没有选中要拖动的剧集" };
  if (targetId === null) return ok;
  if (dragIds.includes(targetId)) {
    return { ok: false, reason: "不能放到自己里面" };
  }
  const target = nodes.find((n) => n.id === targetId);
  if (!target) return { ok: false, reason: "目标条目不存在" };

  const parentId =
    mode === "inside" ? targetId : target.parent_id ?? null;
  for (const id of dragIds) {
    if (parentId !== null && isAncestor(nodes, id, parentId)) {
      return { ok: false, reason: "不能放进自己的子级里（会形成循环引用）" };
    }
  }
  const base = parentId === null ? 0 : (nodes.find((n) => n.id === parentId)?.depth ?? 0) + 1;
  for (const id of dragIds) {
    const deepest = base + subtreeHeight(nodes, id);
    if (deepest > TREE_MAX_DEPTH - 1) {
      return {
        ok: false,
        reason: `层级最多 ${TREE_MAX_DEPTH} 层，这样会到第 ${deepest + 1} 层`,
      };
    }
  }
  return ok;
}

/**
 * 由指针在行内的相对高度决定落点形态。
 *
 * 上下各留 28% 判「插到前后」，中间判「放进内部」——中间区域太小会导致用户
 * 想嵌套时老是插成兄弟，太大又会让「排序」变得难做。
 */
export function dropPosition(offsetY: number, height: number): DropPosition {
  if (height <= 0) return "inside";
  const ratio = offsetY / height;
  if (ratio < 0.28) return "before";
  if (ratio > 0.72) return "after";
  return "inside";
}
