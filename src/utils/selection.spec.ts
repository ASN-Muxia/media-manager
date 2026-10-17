import { describe, expect, it } from "vitest";
import { extendRange, resolveClick, toggleOne, type ClickMods } from "./selection";

/**
 * 多选点击语义的用例：只测纯函数，不牵扯 DOM / Pinia / Tauri。
 *
 * 每个 `it` 对应一条真实交互下的行为（Shift 并入、锚点保持不变、
 * 各种"构不成区间"时退化成普通切换…），语义一改这里就红 ——
 * 这是 `selection.ts` 的可执行说明文档。
 */

/** 一次调用的简写：把 `selected` 当作"点击前的选中集合"。 */
function click(
  order: number[],
  selected: number[],
  anchor: number | null,
  id: number,
  mods: ClickMods = {},
): { selected: number[]; anchor: number | null } {
  return resolveClick({ order, selected, anchor, id, mods });
}

const ORDER = [1, 2, 3, 4, 5, 6];

describe("toggleOne：单张切换", () => {
  it("未选中时加入，并排在已有选中项之后", () => {
    expect(toggleOne([3, 1], 5)).toEqual([3, 1, 5]);
  });

  it("已选中时取消，其余项的相对顺序不变", () => {
    expect(toggleOne([3, 1, 5], 1)).toEqual([3, 5]);
  });

  it("不改动传入的数组，返回的是新数组", () => {
    const selected = [3, 1];
    const next = toggleOne(selected, 5);
    expect(selected).toEqual([3, 1]);
    expect(next).not.toBe(selected);
  });
});

describe("extendRange：区间并入", () => {
  it("把 anchor 到 target 之间（含两端）的整段并入 base", () => {
    expect(extendRange(ORDER, [], 2, 5)).toEqual([2, 3, 4, 5]);
  });

  it("正向与反向取到的是同一段（先点下面再 Shift 点上面也成立）", () => {
    expect(extendRange(ORDER, [], 5, 2)).toEqual([2, 3, 4, 5]);
  });

  it("已经是选中项的不会被重复写入，且先 base 后区间新增", () => {
    // base 里的 3 落在区间内：不能出现两次，也不能打乱 base 的先后
    expect(extendRange(ORDER, [3, 9], 1, 4)).toEqual([3, 9, 1, 2, 4]);
  });

  it("区间只是并入而不替换：区间之外已选中的项全部保留", () => {
    expect(extendRange(ORDER, [6], 1, 2)).toEqual([6, 1, 2]);
  });

  it("base 里存在 order 之外的残留 id 时保留它们（裁剪是调用方的责任）", () => {
    expect(extendRange(ORDER, [99], 1, 2)).toEqual([99, 1, 2]);
  });

  it("anchor 为 null 时退化成 base 的去重副本", () => {
    expect(extendRange(ORDER, [3, 3, 1], null, 5)).toEqual([3, 1]);
  });

  it("anchor 不在 order 里时退化成 base 的去重副本", () => {
    expect(extendRange(ORDER, [1], 77, 5)).toEqual([1]);
  });

  it("target 不在 order 里时退化成 base 的去重副本", () => {
    expect(extendRange(ORDER, [1], 2, 77)).toEqual([1]);
  });

  it("order 为空时返回 base 的副本，而不是原数组", () => {
    const base = [1, 2];
    const out = extendRange([], base, 1, 2);
    expect(out).toEqual([1, 2]);
    expect(out).not.toBe(base);
  });

  it("order 里重复的 id 只认首次出现的下标，不会把区间拉长", () => {
    // 3 在 order 里出现两次；从 1 到 3 只会取到**首次**那个 3（下标 2），
    // 下标 4 的重复项不在区间里，所以区间长度不会被莫名其妙拉长
    expect(extendRange([1, 2, 3, 4, 3, 5], [], 1, 3)).toEqual([1, 2, 3]);
    // 顺序跟着 order 走，不跟 anchor / target 的方向走：
    // 3 的首次下标是 1、2 的下标是 2，故区间是 order[1..2] = [3, 2]
    expect(extendRange([5, 3, 2, 1, 3], [], 3, 2)).toEqual([3, 2]);
  });

  it("不改动传入的 order 与 base", () => {
    const order = [1, 2, 3];
    const base = [3];
    extendRange(order, base, 1, 3);
    expect(order).toEqual([1, 2, 3]);
    expect(base).toEqual([3]);
  });
});

describe("resolveClick：一次点击的完整决策", () => {
  it("普通点击就是切换单张，并把锚点移到被点的那张", () => {
    expect(click(ORDER, [1], 5, 3)).toEqual({ selected: [1, 3], anchor: 3 });
    expect(click(ORDER, [1, 3], 3, 3)).toEqual({ selected: [1], anchor: 3 });
  });

  it("Ctrl+点击等价于 toggleOne，同样更新锚点", () => {
    expect(click(ORDER, [1], null, 4, { ctrl: true })).toEqual({
      selected: [1, 4],
      anchor: 4,
    });
    expect(click(ORDER, [1, 4], 4, 1, { ctrl: true })).toEqual({
      selected: [4],
      anchor: 1,
    });
  });

  it("Cmd（meta）+点击与 Ctrl+点击同义", () => {
    expect(click(ORDER, [1], null, 4, { meta: true })).toEqual({
      selected: [1, 4],
      anchor: 4,
    });
  });

  it("Shift+点击把区间并入已选，且锚点保持不变", () => {
    expect(click(ORDER, [1], 1, 4, { shift: true })).toEqual({
      selected: [1, 2, 3, 4],
      anchor: 1,
    });
  });

  it("连续 Shift 始终从同一个锚点重算区间，锚点不漂移", () => {
    const first = click(ORDER, [1], 1, 4, { shift: true });
    const second = click(ORDER, first.selected, first.anchor, 6, { shift: true });
    // 第二次仍以 1 为起点（而不是以 4 为起点再往后扩）
    expect(second).toEqual({ selected: [1, 2, 3, 4, 5, 6], anchor: 1 });
  });

  it("Shift 的区间与已选重合时不产生重复项", () => {
    expect(click(ORDER, [2, 3], 1, 5, { shift: true })).toEqual({
      selected: [2, 3, 1, 4, 5],
      anchor: 1,
    });
  });

  it("anchor 为 null 时 Shift 退化成普通切换，并把锚点设为被点的那张", () => {
    expect(click(ORDER, [1], null, 4, { shift: true })).toEqual({
      selected: [1, 4],
      anchor: 4,
    });
  });

  it("anchor 不在 order 里时 Shift 退化成普通切换，并重置锚点", () => {
    expect(click(ORDER, [1], 99, 4, { shift: true })).toEqual({
      selected: [1, 4],
      anchor: 4,
    });
  });

  it("被点的 id 不在 order 里时退化成普通切换，不抛错也不返回 undefined", () => {
    const out = click(ORDER, [1], 2, 99, { shift: true });
    expect(out).toEqual({ selected: [1, 99], anchor: 99 });
    expect(out.selected).toBeDefined();
  });

  it("被点的 id 已在选中里且不在 order 里时，Shift 退化成取消它", () => {
    expect(click(ORDER, [1, 99], 2, 99, { shift: true })).toEqual({
      selected: [1],
      anchor: 99,
    });
  });

  it("order 为空时任何修饰键都不炸，按普通切换处理", () => {
    expect(click([], [7], null, 7)).toEqual({ selected: [], anchor: 7 });
    expect(click([], [], null, 7, { shift: true })).toEqual({
      selected: [7],
      anchor: 7,
    });
    expect(click([], [7], 7, 7, { ctrl: true })).toEqual({
      selected: [],
      anchor: 7,
    });
  });

  it("shift 与 ctrl 同时按下时 shift 优先，走区间而不是单张切换", () => {
    expect(click(ORDER, [], 2, 4, { shift: true, ctrl: true })).toEqual({
      selected: [2, 3, 4],
      anchor: 2,
    });
  });

  it("不改动传入的 order / selected（纯函数）", () => {
    const order = [1, 2, 3, 4];
    const selected = [1];
    const out = click(order, selected, 1, 3, { shift: true });
    expect(order).toEqual([1, 2, 3, 4]);
    expect(selected).toEqual([1]);
    expect(out.selected).not.toBe(selected);
  });
});
