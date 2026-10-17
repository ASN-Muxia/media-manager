import { describe, expect, it } from "vitest";
import type { MediaTreeNode } from "../api/commands";
import {
  buildTree, canDrop, dropPosition, flattenTree, isAncestor, subtreeHeight,
  subtreeIds,
} from "./tree";

function node(
  id: number,
  parent_id: number | null,
  title = `n${id}`,
  sort_order = id,
  depth = 0,
): MediaTreeNode {
  return {
    id,
    parent_id,
    sort_order,
    type: "tv",
    title,
    display_title: title,
    year: null,
    poster_path: null,
    episode_count: 0,
    child_count: 0,
    depth,
    source: "scan",
    locked: false,
  };
}

/** 1 → 2 → 4，1 → 3，另有顶层 5 */
function sample(): MediaTreeNode[] {
  return [
    node(1, null, "A", 0, 0),
    node(2, 1, "B", 0, 1),
    node(3, 1, "C", 1, 1),
    node(4, 2, "D", 0, 2),
    node(5, null, "E", 1, 0),
  ];
}

describe("buildTree", () => {
  it("按父子关系嵌套，并按排序号排列", () => {
    const roots = buildTree(sample());
    expect(roots.map((r) => r.node.id)).toEqual([1, 5]);
    expect(roots[0].children.map((c) => c.node.id)).toEqual([2, 3]);
    expect(roots[0].children[0].children.map((c) => c.node.id)).toEqual([4]);
  });

  it("父节点不在列表里时按顶层处理，不丢节点", () => {
    const roots = buildTree([node(9, 77, "孤儿")]);
    expect(roots).toHaveLength(1);
    expect(roots[0].node.id).toBe(9);
  });

  it("排序号相同时按标题兜底", () => {
    const roots = buildTree([
      node(1, null, "乙", 0),
      node(2, null, "甲", 0),
    ]);
    expect(roots.map((r) => r.node.title)).toEqual(["甲", "乙"]);
  });

  /**
   * 同级次序只认 `sort_order`，不认数组先后。
   *
   * 这条是给上游（封面墙 / 详情页成员）立的契约：数组先后是**展示排序**
   * （用户选的，默认拼音名），`sort_order` 才是同级内手工调出来的次序。
   * 两者必须分开——若建树偷懒拿数组下标当次序，「上移 / 下移」算出的落点
   * 就和后端（按库内 `sort_order` 算）不是同一个序。
   */
  it("同级次序只认 sort_order：数组先后与它相反时也不受影响", () => {
    // 数组里 c 在前、a 在后；sort_order 却要求 a < b < c
    const roots = buildTree([
      node(3, null, "c", 2),
      node(2, null, "b", 1),
      node(1, null, "a", 0),
    ]);
    expect(roots.map((r) => r.node.id)).toEqual([1, 2, 3]);
  });

  /** 老数据 `sort_order` 全是 0 / 缺失时不能抛错，退回标题 + id 的稳定序。 */
  it("sort_order 全为 0 或缺失时退回标题 / id 稳定序，不抛错", () => {
    const allZero = buildTree([
      node(2, null, "同一名", 0),
      node(1, null, "同一名", 0),
    ]);
    expect(allZero.map((r) => r.node.id)).toEqual([1, 2]);

    // 老数据可能连字段都没有：undefined 参与减法得 NaN，
    // 排序比较器必须把它当「相等」落到标题 / id 兜底，而不是抛出或乱序
    const dangling = (id: number, title: string): MediaTreeNode => {
      const n = node(id, null, title);
      delete (n as { sort_order?: number }).sort_order;
      return n;
    };
    const missing = buildTree([dangling(9, "B"), dangling(8, "A")]);
    expect(missing.map((r) => r.node.id)).toEqual([8, 9]);
  });
});

describe("flattenTree", () => {
  it("折叠的子级不出现在结果里", () => {
    const rows = flattenTree(buildTree(sample()), new Set());
    expect(rows.map((r) => r.node.id)).toEqual([1, 5]);
  });

  it("展开后按深度展开，并记录同级序号", () => {
    const rows = flattenTree(buildTree(sample()), new Set([1, 2]));
    expect(rows.map((r) => [r.node.id, r.depth, r.index])).toEqual([
      [1, 0, 0],
      [2, 1, 0],
      [4, 2, 0],
      [3, 1, 1],
      [5, 0, 1],
    ]);
  });
});

describe("层级关系", () => {
  it("subtreeIds 含自己与所有后代", () => {
    expect(subtreeIds(sample(), 1).sort()).toEqual([1, 2, 3, 4]);
    expect(subtreeIds(sample(), 4)).toEqual([4]);
  });

  it("isAncestor 只认向上这条链", () => {
    expect(isAncestor(sample(), 1, 4)).toBe(true);
    expect(isAncestor(sample(), 2, 4)).toBe(true);
    expect(isAncestor(sample(), 4, 1)).toBe(false);
  });

  it("subtreeHeight 量的是最高的那条枝", () => {
    expect(subtreeHeight(sample(), 1)).toBe(2);
    expect(subtreeHeight(sample(), 2)).toBe(1);
    expect(subtreeHeight(sample(), 5)).toBe(0);
  });
});

describe("canDrop", () => {
  it("顶层永远可以放", () => {
    expect(canDrop(sample(), [4], null, "inside").ok).toBe(true);
  });

  it("禁止放到自己里", () => {
    const r = canDrop(sample(), [2], 2, "inside");
    expect(r.ok).toBe(false);
    expect(r.reason).toContain("自己");
  });

  it("禁止放进自己的子孙里", () => {
    const r = canDrop(sample(), [1], 4, "inside");
    expect(r.ok).toBe(false);
    expect(r.reason).toContain("循环引用");
  });

  it("插到子孙前后同样被挡住（父节点还是自己）", () => {
    const r = canDrop(sample(), [1], 4, "before");
    expect(r.ok).toBe(false);
    expect(r.reason).toContain("循环引用");
  });

  it("兄弟之间互换位置是允许的", () => {
    expect(canDrop(sample(), [2], 3, "after").ok).toBe(true);
  });

  it("超出层数上限时给出具体层数", () => {
    // 1(d0) → 2(d1) → 4(d2)，把高度为 1 的子树挂到 4 下会到第 5 层
    const nodes = sample();
    const withSub = [...nodes, node(6, null, "F", 2, 0), node(7, 6, "G", 0, 1)];
    const r = canDrop(withSub, [6], 4, "inside");
    expect(r.ok).toBe(false);
    expect(r.reason).toContain("第 5 层");
  });
});

describe("dropPosition", () => {
  it("上下缘判前后，中间判内部", () => {
    expect(dropPosition(5, 40)).toBe("before");
    expect(dropPosition(20, 40)).toBe("inside");
    expect(dropPosition(36, 40)).toBe("after");
  });

  it("高度异常时不炸，按内部处理", () => {
    expect(dropPosition(0, 0)).toBe("inside");
  });
});
