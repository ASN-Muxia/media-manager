import { describe, expect, it } from "vitest";
import {
  applyDragRange,
  applyDragTo,
  beginDragSelection,
  clampIndex,
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

/* ==================== 快速滑动 / 首尾项 ==================== */
/* 指针事件是稀疏的，一次 pointermove 可能跨过好几项；只写"指针正下方那一项"
   会把中间整段漏掉（真机上表现为"快速划一下只选中了头尾"）。这一组把
   "补齐"与"夹到端点"钉死在纯计算层，DOM 层只管把下标喂进来。 */

describe("clampIndex 下标夹取", () => {
  it("越界时夹到首项 / 末项，不报错也不返回 -1", () => {
    expect(clampIndex(-5, 4)).toBe(0);
    expect(clampIndex(99, 4)).toBe(3);
  });

  it("范围内的下标原样返回", () => {
    expect(clampIndex(0, 4)).toBe(0);
    expect(clampIndex(2, 4)).toBe(2);
  });

  /** 空列表没有可夹的项 —— 返回 -1，调用方据此跳过，而不是当第 0 项写进去。 */
  it("空列表返回 -1", () => {
    expect(clampIndex(0, 0)).toBe(-1);
    expect(clampIndex(-1, 0)).toBe(-1);
  });
});

describe("applyDragRange 补齐被跳过的项", () => {
  it("从第 1 项跳到第 4 项：中间的项全部补上", () => {
    const st = beginDragSelection([], 10);
    expect(applyDragRange(st, ORDER, 0, 3)).toBe(true);
    expect(finishDragSelection(st, ORDER)).toEqual([10, 11, 12, 13]);
  });

  /**
   * 兜底：先断言"旧路径做不到"，再断言"新路径做得到"。
   * 旧路径 = 只 applyDragTo 指针正下方那一项。
   */
  it("旧路径（只写命中那一项）会漏掉中间项 —— 新路径不会", () => {
    const old = beginDragSelection([], 10);
    applyDragTo(old, ORDER[3]); // 指针直接落在第 4 项
    expect(finishDragSelection(old, ORDER)).toEqual([10, 13]); // ← 漏了 11、12

    const now = beginDragSelection([], 10);
    applyDragRange(now, ORDER, 0, 3);
    expect(finishDragSelection(now, ORDER)).toEqual([10, 11, 12, 13]);
  });

  it("反向（从后往前抹）同样补齐", () => {
    const st = beginDragSelection(ORDER, ORDER[4]); // 起点已选中 → 整段取消
    expect(applyDragRange(st, ORDER, 4, 1)).toBe(true);
    // 只抹到下标 1；下标 5 那一项（15）在范围外，保持不动
    expect(finishDragSelection(st, ORDER)).toEqual([10, 15]);
  });

  it("反向一路抹到首项 → 只剩下范围外的尾项", () => {
    const st = beginDragSelection(ORDER, ORDER[5]); // 从最后一项起手
    applyDragRange(st, ORDER, 5, 0);
    expect(finishDragSelection(st, ORDER)).toEqual([]);
  });

  /** 首尾项：下标越界要被夹住，端点不能漏也不能报错。 */
  it("下标越界时夹到端点，端点项不漏", () => {
    const st = beginDragSelection([], 10);
    expect(applyDragRange(st, ORDER, -99, 99)).toBe(true);
    expect(finishDragSelection(st, ORDER)).toEqual([10, 11, 12, 13, 14, 15]);
  });

  it("起止相同 = 只写那一项（幂等由 applyDragTo 保证）", () => {
    const st = beginDragSelection([], 11);
    expect(applyDragRange(st, ORDER, 1, 1)).toBe(false); // 起点那一项已经写过了
    expect(finishDragSelection(st, ORDER)).toEqual([11]);
  });

  it("重复划同一段不会来回翻转（幂等）", () => {
    const st = beginDragSelection([], 10);
    expect(applyDragRange(st, ORDER, 0, 3)).toBe(true);
    expect(applyDragRange(st, ORDER, 0, 3)).toBe(false);
    expect(finishDragSelection(st, ORDER)).toEqual([10, 11, 12, 13]);
  });

  it("拖动结束后不再生效", () => {
    const st = beginDragSelection([], 10);
    const ended = { ...st, active: false };
    expect(applyDragRange(ended, ORDER, 0, 3)).toBe(false);
    expect(finishDragSelection(ended, ORDER)).toEqual([10]);
  });

  it("空列表不会写出任何东西", () => {
    const st = beginDragSelection([], 10);
    expect(applyDragRange(st, [], 0, 3)).toBe(false);
  });

  /* ---------- 下面几条是复核补的：覆盖「只有 1 项 / 只有 0 项 / 重叠 range /
     order 里有重复 id / 反向且两端同时越界」这些他没写到的组合 ---------- */

  /** 只有 1 项的列表：下标再怎么越界都只能落到那一项上。 */
  it("列表只有 1 项时，越界下标全部夹到它身上", () => {
    const st = beginDragSelection([], 7);
    expect(applyDragRange(st, [7], -99, 99)).toBe(false); // 起手那项已写过 → 无变化
    expect(finishDragSelection(st, [7])).toEqual([7]);

    const st2 = beginDragSelection([7], 7); // 起点已选中 → 方向是取消
    expect(applyDragRange(st2, [7], 5, -5)).toBe(false);
    expect(finishDragSelection(st2, [7])).toEqual([]);
  });

  /** 连续两次 range 有重叠：重叠段不能被"第二次经过"翻转掉。 */
  it("两次 range 有重叠时重叠段不翻转", () => {
    const st = beginDragSelection([], 10);
    expect(applyDragRange(st, ORDER, 0, 2)).toBe(true); // 10 11 12
    expect(applyDragRange(st, ORDER, 1, 4)).toBe(true); // 再补 13 14，重叠的 11 12 不动
    expect(finishDragSelection(st, ORDER)).toEqual([10, 11, 12, 13, 14]);

    // 反向再抹一次 1..3：起点（下标 0）在起手那一下就已经被取消了，
    // 之后只有下标 1~3 被抹掉，下标 4（14）在范围外保持不动
    const st2 = beginDragSelection([10, 11, 12, 13, 14], 10);
    expect(applyDragRange(st2, ORDER, 3, 1)).toBe(true);
    expect(finishDragSelection(st2, ORDER)).toEqual([14]);
  });

  /** `order` 里出现重复 id（渲染层理论上不该，但状态机不能崩、不能重复输出）。 */
  it("order 里有重复 id 时不崩、结果不重复、不翻转", () => {
    const dup = [10, 11, 10];
    const st = beginDragSelection([], 10);
    expect(applyDragRange(st, dup, 0, 2)).toBe(true); // 只新增了 11
    expect(finishDragSelection(st, dup)).toEqual([10, 11]);

    // 反方向：从已选中的 10 起手 → 整段都是取消，重复出现的 10 也只会被删一次
    const st2 = beginDragSelection([10, 11], 10); // 方向 = 取消
    expect(applyDragRange(st2, dup, 0, 2)).toBe(true);
    expect(finishDragSelection(st2, dup)).toEqual([]);
  });

  /** 反向 + 两端同时越界：要夹成「全表反向」，不是空转也不是死循环。 */
  it("反向且两端同时越界 → 夹成整段反向，不死循环", () => {
    const st = beginDragSelection(ORDER, ORDER[5]); // 从末项起手，方向 = 取消
    expect(applyDragRange(st, ORDER, 99, -99)).toBe(true);
    expect(finishDragSelection(st, ORDER)).toEqual([]);
  });

  /** 单项列表 + 空列表的 `clampIndex` 边界（长度 1 时上下界重合）。 */
  it("clampIndex：长度为 1 时任意下标都返回 0", () => {
    expect(clampIndex(-1, 1)).toBe(0);
    expect(clampIndex(0, 1)).toBe(0);
    expect(clampIndex(99, 1)).toBe(0);
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
