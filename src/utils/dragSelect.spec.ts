import { describe, expect, it } from "vitest";
import {
  applyDragTo,
  beginDragSelection,
  finishDragSelection,
  toggleOne,
} from "./dragSelect";

const ORDER = [10, 11, 12, 13, 14, 15];

/** 模拟「从 startId 一路划到 dragged」，返回最终选中列表。 */
function dragOver(selected: number[], startId: number, dragged: number[]): number[] {
  const st = beginDragSelection(selected, startId);
  for (const id of dragged) applyDragTo(st, id);
  return finishDragSelection(st, ORDER);
}

describe("拖动连续多选", () => {
  it("从空白处起手 → 整段都是选中", () => {
    expect(dragOver([], 11, [12, 13])).toEqual([11, 12, 13]);
  });

  it("从已选中处起手 → 整段都是取消（用来抹掉一片）", () => {
    expect(dragOver([11, 12, 13], 12, [13])).toEqual([11]);
  });

  it("起点本身立刻生效（按下即改变那一行）", () => {
    const st = beginDragSelection([], 10);
    expect(st.ids.has(10)).toBe(true);
    const st2 = beginDragSelection([10], 10);
    expect(st2.ids.has(10)).toBe(false);
  });

  it("同一行反复划过是幂等的（手抖不会来回翻转）", () => {
    const st = beginDragSelection([], 10);
    expect(applyDragTo(st, 11)).toBe(true);
    expect(applyDragTo(st, 11)).toBe(false);
    expect(applyDragTo(st, 11)).toBe(false);
    expect(finishDragSelection(st, ORDER)).toEqual([10, 11]);
  });

  it("拖出去再拖回来不会把已取消的又选上", () => {
    // 从已选中的 12 起手（方向 = 取消）
    const st = beginDragSelection([11, 12, 13], 12);
    applyDragTo(st, 13);
    applyDragTo(st, 999); // 拖到了不存在的行（比如移出列表）
    applyDragTo(st, 13); // 又划回来
    expect(finishDragSelection(st, ORDER)).toEqual([11]);
  });

  it("结果保留选择先后顺序（已选的在前，新划的按拖动顺序在后）", () => {
    // 先选 14，再从 10 起手划到 12
    const st = beginDragSelection([14], 10);
    applyDragTo(st, 11);
    applyDragTo(st, 12);
    expect(finishDragSelection(st, ORDER)).toEqual([14, 10, 11, 12]);
  });

  it("结束拖动后再划过不再生效", () => {
    const st = beginDragSelection([], 10);
    const ended: typeof st = { ...st, active: false };
    expect(applyDragTo(ended, 11)).toBe(false);
    expect(finishDragSelection(ended, ORDER)).toEqual([10]);
  });

  it("全选后再整段划过 → 清空", () => {
    expect(dragOver([10, 11, 12, 13, 14, 15], 10, [11, 12, 13, 14, 15])).toEqual([]);
  });

  it("不碰任何行时原样返回", () => {
    expect(dragOver([12, 13], 12, [])).toEqual([13]);
    // 起手就落在唯一选中的那一行 → 取消它
    expect(dragOver([12], 12, [])).toEqual([]);
  });

  it("过滤掉已不在当前列表里的行（防止删到看不见的条目）", () => {
    const st = beginDragSelection([], 10);
    applyDragTo(st, 11);
    st.ids.add(8888); // 手工塞一个不在当前列表里的 id
    expect(finishDragSelection(st, ORDER)).toEqual([10, 11]);
  });

  it("列表为空时结果必然为空（没有可见的行就没什么可选）", () => {
    expect(finishDragSelection(beginDragSelection([], 10), [])).toEqual([]);
    expect(finishDragSelection(beginDragSelection([10], 10), [])).toEqual([]);
  });
});

describe("单点切换（复选框路径）", () => {
  it("未选中 → 追加到末尾；已选中 → 移除，其余顺序不变", () => {
    expect(toggleOne([], 5)).toEqual([5]);
    expect(toggleOne([1, 2, 3], 4)).toEqual([1, 2, 3, 4]);
    expect(toggleOne([1, 2, 3], 2)).toEqual([1, 3]);
    expect(toggleOne([7], 7)).toEqual([]);
  });

  it("不修改传入数组（纯函数）", () => {
    const src = [1, 2];
    toggleOne(src, 3);
    expect(src).toEqual([1, 2]);
  });
});
