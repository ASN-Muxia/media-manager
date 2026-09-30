import { describe, expect, it } from "vitest";
import {
  applyDragTo,
  beginDragSelection,
  dragTrigger,
  DRAG_THRESHOLD_PX,
  EDGE_MAX_SPEED,
  EDGE_ZONE_PX,
  edgeScrollSpeed,
  finishDragSelection,
  LONG_PRESS_MS,
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

/* ==================== 触发判定 ==================== */
/* 这一组是「滑动多选」最容易和页面滚动打架的地方，必须单独钉住。 */

describe("dragTrigger 触发判定", () => {
  it("鼠标：超过位移阈值就即时进入拖动", () => {
    expect(dragTrigger("mouse", true, 0)).toBe("drag");
    expect(dragTrigger("pen", true, 10)).toBe("drag");
  });

  it("位移阈值是个正数且不会大到吞掉正常拖动", () => {
    expect(DRAG_THRESHOLD_PX).toBeGreaterThan(0);
    expect(DRAG_THRESHOLD_PX).toBeLessThan(20);
  });

  it("鼠标：没超过阈值就是普通点击，不算拖动", () => {
    expect(dragTrigger("mouse", false, 0)).toBe("wait");
    // 按得再久也不该变成拖动 —— 桌面端不该有"长按"这个隐藏操作
    expect(dragTrigger("mouse", false, 5000)).toBe("wait");
  });

  /** 触摸下"手指先动"就是想滚页面，绝不能抢成框选。 */
  it("触摸：长按成立前先移动 → 判定为滚动，放弃", () => {
    expect(dragTrigger("touch", true, 10)).toBe("abort");
    expect(dragTrigger("touch", true, LONG_PRESS_MS - 1)).toBe("abort");
  });

  it("触摸：按住到时长且没移动 → 进入拖动", () => {
    expect(dragTrigger("touch", false, LONG_PRESS_MS)).toBe("drag");
    expect(dragTrigger("touch", false, LONG_PRESS_MS + 1)).toBe("drag");
  });

  it("触摸：还没到时长就继续等", () => {
    expect(dragTrigger("touch", false, 0)).toBe("wait");
    expect(dragTrigger("touch", false, LONG_PRESS_MS - 1)).toBe("wait");
  });
});

describe("edgeScrollSpeed 贴边自动滚动", () => {
  const TOP = 0;
  const BOTTOM = 600;

  it("不在边缘区时不滚", () => {
    expect(edgeScrollSpeed(300, TOP, BOTTOM)).toBe(0);
    expect(edgeScrollSpeed(EDGE_ZONE_PX, TOP, BOTTOM)).toBe(0);
  });

  it("靠近上边缘向上滚（负速度）", () => {
    expect(edgeScrollSpeed(10, TOP, BOTTOM)).toBeLessThan(0);
  });

  it("靠近下边缘向下滚（正速度）", () => {
    expect(edgeScrollSpeed(BOTTOM - 10, TOP, BOTTOM)).toBeGreaterThan(0);
  });

  it("越贴边越快", () => {
    const near = Math.abs(edgeScrollSpeed(2, TOP, BOTTOM));
    const far = Math.abs(edgeScrollSpeed(EDGE_ZONE_PX - 2, TOP, BOTTOM));
    expect(near).toBeGreaterThan(far);
  });

  /** 拖出容器外面（距离 ≤ 0）应该取最大速度，否则选不到视野外的项。 */
  it("指针跑出容器外时取最大速度", () => {
    expect(Math.abs(edgeScrollSpeed(-20, TOP, BOTTOM))).toBe(EDGE_MAX_SPEED);
    expect(Math.abs(edgeScrollSpeed(BOTTOM + 20, TOP, BOTTOM))).toBe(EDGE_MAX_SPEED);
  });

  it("上下同时不成立时优先上边缘（不会一次给出两个方向）", () => {
    // 容器很矮，指针在中间：两个边缘区重叠，取上边缘（先判上）
    expect(edgeScrollSpeed(5, 0, 10)).toBeLessThanOrEqual(0);
  });
});
