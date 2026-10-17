import { beforeEach, describe, expect, it } from "vitest";
import { createPinia, setActivePinia } from "pinia";
import { useMediaStore } from "./mediaStore";
import type { MediaItem } from "../api/commands";

/**
 * Shift 多选 / Ctrl 多选的**接线**用例。
 *
 * 纯语义（区间怎么算、anchor 怎么走、边界）在 utils/selection.spec.ts 里已经钉死；
 * 这里只验证 store 真的把它接上了：
 * - 用的是**当前可见顺序**（visibleItems），不是全库顺序；
 * - 锚点在 store 里，`clearSelection` 会把它清掉；
 * - 普通点击仍然是「切换单张」（不破坏原有行为）。
 */

function item(id: number, over: Partial<MediaItem> = {}): MediaItem {
  return {
    id, type: "movie", title: "T" + id, year: 2020, tmdb_id: null,
    poster_path: null, backdrop_path: null, overview: null, vote_average: null,
    genre: null, file_path: null, file_name: null, episode_count: 0,
    added_at: 1, display_title: "T" + id, sort_order: 0, depth: 0, ...over,
  };
}

/** 直接塞 `items`（跳过 IPC）：这些用例只关心选中集合怎么变。 */
function seed(ids: number[]) {
  const s = useMediaStore();
  s.items = ids.map((i) => item(i));
  return s;
}

describe("mediaStore 多选：Shift / Ctrl", () => {
  beforeEach(() => setActivePinia(createPinia()));

  it("普通点击：切换单张，并记下锚点", () => {
    const s = seed([1, 2, 3]);
    s.selectWithMods(2, {});
    expect(s.selected).toEqual([2]);
    expect(s.selectAnchor).toBe(2);
    s.selectWithMods(2, {});
    expect(s.selected).toEqual([]);
  });

  it("Shift 点击：把上一次点击到这一张之间的整段并入选中", () => {
    const s = seed([1, 2, 3, 4, 5]);
    s.selectWithMods(2, {});
    s.selectWithMods(4, { shift: true });
    expect(s.selected, "2 保持不变，3/4 被并入").toEqual([2, 3, 4]);
  });

  it("连续 Shift：锚点保持不变，区间可以反复调整", () => {
    const s = seed([1, 2, 3, 4, 5]);
    s.selectWithMods(2, {});
    s.selectWithMods(5, { shift: true });
    expect(s.selected).toEqual([2, 3, 4, 5]);
    expect(s.selectAnchor, "连续 Shift 后锚点仍是 2").toBe(2);
    s.selectWithMods(3, { shift: true });
    expect(s.selected).toEqual([2, 3, 4, 5]);
  });

  it("Shift 是并入不是替换：之前选中的其它项保留", () => {
    const s = seed([1, 2, 3, 4, 5]);
    s.selectWithMods(1, {});
    s.selectWithMods(5, {});
    expect(s.selected).toEqual([1, 5]);
    s.selectWithMods(3, { shift: true });
    expect(s.selected).toContain(1);
    expect(s.selected).toContain(5);
    expect(s.selected).toContain(3);
  });

  it("没有锚点时 Shift 退化成普通切换（不吞掉这次点击）", () => {
    const s = seed([1, 2, 3]);
    expect(s.selectAnchor).toBe(null);
    s.selectWithMods(2, { shift: true });
    expect(s.selected).toEqual([2]);
    expect(s.selectAnchor).toBe(2);
  });

  it("Ctrl 点击等价于切换单张", () => {
    const s = seed([1, 2, 3]);
    s.selectWithMods(1, { ctrl: true });
    s.selectWithMods(3, { ctrl: true });
    expect(s.selected).toEqual([1, 3]);
    s.selectWithMods(1, { ctrl: true });
    expect(s.selected).toEqual([3]);
  });

  it("Cmd(meta) 与 Ctrl 同义", () => {
    const s = seed([1, 2, 3]);
    s.selectWithMods(2, { meta: true });
    expect(s.selected).toEqual([2]);
  });

  /** 区间必须按**屏幕上看到的顺序**算 —— 用户划的是看得见的那一段。 */
  it("区间按当前可见顺序算，不是全库顺序", () => {
    const s = useMediaStore();
    // 故意让 display_title 的拼音序与 id 序相反：可见顺序应是 [3,2,1]
    s.items = [item(1, { display_title: "ccc" }), item(2, { display_title: "bbb" }), item(3, { display_title: "aaa" })];
    const visible = s.visibleItems().map((i) => i.id);
    s.selectWithMods(visible[0], {});
    s.selectWithMods(visible[2], { shift: true });
    expect(s.selected, "可见顺序里的首尾 = 全部三项").toEqual(visible);
  });

  it("clearSelection 会把锚点一起清掉", () => {
    const s = seed([1, 2, 3]);
    s.selectWithMods(1, {});
    expect(s.selectAnchor).toBe(1);
    s.clearSelection();
    expect(s.selected).toEqual([]);
    expect(s.selectAnchor, "锚点必须跟着清，否则下次 Shift 从旧起点算").toBe(null);
  });

  it("锚点指向的条目已被过滤掉时：Shift 退化成普通切换（不抛错）", () => {
    const s = seed([1, 2, 3]);
    s.selectWithMods(1, {});
    // 换一批数据（模拟换栏目 / 敲关键字），锚点 1 不在新列表里
    s.items = [item(7), item(8)];
    s.selectWithMods(8, { shift: true });
    expect(s.selected).toContain(8);
  });
});
