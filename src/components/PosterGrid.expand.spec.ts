import { describe, expect, it } from "vitest";
import { mount } from "@vue/test-utils";
import { nextTick } from "vue";
import PosterGrid from "./PosterGrid.vue";
import type { MediaItem } from "../api/commands";

/**
 * 展开 / 收起 与「闭合分割线」的回归用例。
 *
 * 背景（用户报告）：「在展开一个项目的时候会把后面紧紧跟着的那个项目一起展开」。
 *
 * 实测结论：expanded 集合**没有**被判错（点 A 只加 A），真正的成因在**布局** ——
 * 网格是多列 auto-fill，.cell 与 .groupbar 都只是普通 grid item，于是 A 的子项
 * 会和紧跟其后的顶层项目 B **挤在同一行**，看起来就像 B 也被展开了。
 *
 * 真实浏览器（Edge + CDP）量到的坐标：
 *   修复前：cell id=11 y=334 x=0 / cell id=12 y=334 x=185 / cell id=2 y=334 x=370  ← B 混进来
 *   修复后：A 的子项独占一行；cell id=2 被挤到新的一行
 *
 * jsdom 没有布局引擎、量不到坐标，所以这里钉的是**能决定布局的那个结构**：
 * 每个已展开分组的最后一个后代之后必须有一个跨整行的 .groupend。
 * 一旦有人删掉它，下一个项目就会重新混进子项那一行 —— 这条用例就会红。
 */

function item(id: number, over: Partial<MediaItem> = {}): MediaItem {
  return {
    id, type: "movie", title: "T" + id, year: 2020, tmdb_id: null,
    poster_path: null, backdrop_path: null, overview: null, vote_average: null,
    genre: null, file_path: null, file_name: null, episode_count: 0,
    added_at: 1, display_title: "T" + id, parent_id: null, child_count: 0,
    sort_order: 0, depth: 0, ...over,
  };
}

/** A(1) 有 2 个子。B(2) 紧跟其后，也有 2 个子。C(3)/D(4) 无子。 */
const ITEMS: MediaItem[] = [
  item(1, { child_count: 2 }),
  item(11, { parent_id: 1, depth: 1 }),
  item(12, { parent_id: 1, depth: 1 }),
  item(2, { child_count: 2 }),
  item(21, { parent_id: 2, depth: 1 }),
  item(22, { parent_id: 2, depth: 1 }),
  item(3),
  item(4),
];

function mountGrid(items: MediaItem[] = ITEMS) {
  return mount(PosterGrid, { props: { items }, attachTo: document.body });
}

/** 渲染出的网格直接子元素顺序（含分组条与分割线），用 token 标记。 */
function layout(w: ReturnType<typeof mountGrid>): string[] {
  return Array.from((w.find(".grid").element as HTMLElement).children).map((el) => {
    const e = el as HTMLElement;
    if (e.classList.contains("groupend")) return "END";
    if (e.classList.contains("groupbar")) return "BAR";
    return "cell:" + e.dataset.id;
  });
}

/** 点某个卡片上的展开 / 折叠按钮。 */
async function toggle(w: ReturnType<typeof mountGrid>, id: number) {
  const cell = w.findAll(".cell").find((c) => Number(c.attributes("data-id")) === id)!;
  await cell.findAll("button.op")[0].trigger("click");
  await nextTick();
}

describe("PosterGrid 展开 / 收起", () => {
  it("展开的分组在其最后一个子项之后画一条闭合分割线", () => {
    const w = mountGrid();
    expect(layout(w)).toEqual([
      "cell:1", "BAR", "cell:11", "cell:12", "END",
      "cell:2", "BAR", "cell:21", "cell:22", "END",
      "cell:3", "cell:4",
    ]);
    w.unmount();
  });

  /** 这条直接对应 bug：展开 A 不能影响 B。 */
  it("折叠 B 之后再展开 A：B 不会被带着一起展开", async () => {
    const w = mountGrid();
    await toggle(w, 2);
    expect(layout(w)).toEqual(["cell:1", "BAR", "cell:11", "cell:12", "END", "cell:2", "cell:3", "cell:4"]);

    await toggle(w, 1);
    expect(layout(w)).toEqual(["cell:1", "cell:2", "cell:3", "cell:4"]);

    await toggle(w, 1);
    const l = layout(w);
    expect(l).toContain("cell:11");
    expect(l, "B 的子项必须仍然收着").not.toContain("cell:21");
    expect(l.indexOf("cell:12")).toBeLessThan(l.indexOf("END"));
    w.unmount();
  });

  it("没有子级的条目之后不画分割线（线只属于分组）", () => {
    const w = mountGrid([item(1), item(2), item(3)]);
    expect(layout(w)).toEqual(["cell:1", "cell:2", "cell:3"]);
    w.unmount();
  });

  it("分组被折叠后：它后面的分割线也一起消失（不留孤线）", async () => {
    const w = mountGrid([item(1, { child_count: 1 }), item(11, { parent_id: 1, depth: 1 }), item(2)]);
    expect(layout(w)).toEqual(["cell:1", "BAR", "cell:11", "END", "cell:2"]);
    await toggle(w, 1);
    expect(layout(w)).toEqual(["cell:1", "cell:2"]);
    w.unmount();
  });
});

describe("PosterGrid 一键展开 / 收起", () => {
  it("expandAll 展开所有分组，分割线数量与分组数一致", async () => {
    const w = mountGrid();
    const vm = w.vm as unknown as { collapseAll: () => void; expandAll: () => void };
    vm.collapseAll();
    await nextTick();
    expect(layout(w)).toEqual(["cell:1", "cell:2", "cell:3", "cell:4"]);

    vm.expandAll();
    await nextTick();
    expect(layout(w).filter((x) => x === "END")).toHaveLength(2);
    expect(layout(w)).toContain("cell:21");
    w.unmount();
  });

  it("collapseAll 把所有分组收起来，一条分割线都不留", async () => {
    const w = mountGrid();
    (w.vm as unknown as { collapseAll: () => void }).collapseAll();
    await nextTick();
    expect(layout(w)).toEqual(["cell:1", "cell:2", "cell:3", "cell:4"]);
    w.unmount();
  });

  /**
   * 关键回归：一键收起之后**列表刷新不能把分组又自动弹开**。
   *
   * 封面墙有一段「新出现的分组自动展开」的 watch（用户拖出一组后墙上不能只剩一张卡）。
   * 若 collapseAll 不把分组记进 collapsedByUser，items 一变这些分组就自己回来了，
   * 用户会觉得「全部收起」这个按钮没用。
   */
  it("collapseAll 之后列表刷新：分组不会自己弹开", async () => {
    const w = mountGrid();
    (w.vm as unknown as { collapseAll: () => void }).collapseAll();
    await nextTick();
    expect(layout(w)).toEqual(["cell:1", "cell:2", "cell:3", "cell:4"]);

    await w.setProps({ items: ITEMS.map((i) => ({ ...i })) });
    await nextTick();
    expect(layout(w), "刷新后仍必须保持收起").toEqual(["cell:1", "cell:2", "cell:3", "cell:4"]);
    w.unmount();
  });

  /** 反向：一键展开之后刷新，也不能被上一次的「手动折叠」记忆收回去。 */
  it("expandAll 之后列表刷新：分组保持展开", async () => {
    const w = mountGrid();
    const vm = w.vm as unknown as { collapseAll: () => void; expandAll: () => void };
    vm.collapseAll();
    await nextTick();
    vm.expandAll();
    await nextTick();

    await w.setProps({ items: ITEMS.map((i) => ({ ...i })) });
    await nextTick();
    expect(layout(w).filter((x) => x === "END")).toHaveLength(2);
    w.unmount();
  });

  it("没有分组时 expandAll / collapseAll 是空操作（不抛错）", async () => {
    const w = mountGrid([item(1), item(2)]);
    const vm = w.vm as unknown as { expandAll: () => void; collapseAll: () => void };
    vm.expandAll();
    vm.collapseAll();
    await nextTick();
    expect(layout(w)).toEqual(["cell:1", "cell:2"]);
    w.unmount();
  });
});
