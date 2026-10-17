import { describe, expect, it } from "vitest";
import { mount } from "@vue/test-utils";
import { nextTick } from "vue";
import PosterGrid from "./PosterGrid.vue";
import type { MediaItem } from "../api/commands";

/**
 * 展开态**跨卸载留存**的回归用例。
 *
 * 背景（用户报告）：「对这个聚合的展开与收缩进行一个记忆，回到主页的时候留存我最后一次的
 * 操作，而不是直接全展开或者全收缩起来」。
 *
 * 根因：封面墙挂在 `App.vue` 的 `v-if/v-else` 链上，点开详情页会把它**卸载**，
 * 组件内的 `expanded` / `collapsedByUser` 随之销毁；返回主页时组件重建，
 * 「新分组自动展开」那段 watch 又把一切摊开 —— 于是每次回来都是全展开。
 *
 * 修法：折叠名单由 `settingsStore.collapsedGroups` 持有（落库），通过
 * `collapsedGroups` prop 传进组件、`collapsed-change` 事件回写。
 * 这里用「卸载 → 用同一份名单重新挂载」来模拟「离开再回到主页」——
 * 这正是原先丢状态的那一步。
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

/** A(1) 有 2 个子；B(2) 有 2 个子。 */
const ITEMS: MediaItem[] = [
  item(1, { child_count: 2 }),
  item(11, { parent_id: 1, depth: 1 }),
  item(12, { parent_id: 1, depth: 1 }),
  item(2, { child_count: 2 }),
  item(21, { parent_id: 2, depth: 1 }),
  item(22, { parent_id: 2, depth: 1 }),
];

function mountGrid(collapsedGroups?: number[]) {
  return mount(PosterGrid, { props: { items: ITEMS, collapsedGroups }, attachTo: document.body });
}

/** 当前渲染出来的卡片 id 顺序（用以判断哪些子项可见）。 */
function ids(w: ReturnType<typeof mountGrid>): number[] {
  return w.findAll(".cell").map((c) => Number(c.attributes("data-id")));
}

/** 点某个卡片上的展开 / 折叠按钮。 */
async function toggle(w: ReturnType<typeof mountGrid>, id: number) {
  const cell = w.findAll(".cell").find((c) => Number(c.attributes("data-id")) === id)!;
  await cell.findAll("button.op")[0].trigger("click");
  await nextTick();
}


/** 最近一次上报的折叠名单（emitted 的最后一组参数）。
 *
 * 不用数组的 at 方法：本项目的 tsc lib 是 ES2020，它会直接编译报错
 * （之前踩过，见 MEMORY 的测试约定）。
 */
function lastReport(w: ReturnType<typeof mountGrid>): number[] {
  const list = w.emitted("collapsed-change");
  if (!list || list.length === 0) return [];
  return list[list.length - 1][0] as number[];
}

describe("PosterGrid 展开态记忆（跨卸载留存）", () => {
  it("没有传 collapsedGroups（老库）：默认全部展开，子项都渲染出来", () => {
    const w = mountGrid();
    expect(ids(w)).toEqual([1, 11, 12, 2, 21, 22]);
  });

  it("传了 collapsedGroups=[1]：挂载时那一组就是收着的", () => {
    const w = mountGrid([1]);
    // 分组 1 收着 → 它的两个子项不出现；分组 2 未在名单里 → 照常展开
    expect(ids(w)).toEqual([1, 2, 21, 22]);
  });

  it("传了 collapsedGroups=[1,2]：两组都收着（一键收起后的状态）", () => {
    const w = mountGrid([1, 2]);
    expect(ids(w)).toEqual([1, 2]);
  });

  /*
   * 本组核心：模拟「收起来 → 离开主页 → 回来」。
   * 卸载再用记录下来的名单重新挂载，就是用户来回切页面时真实发生的事。
   */
  it("折叠 A 后离开再回来：A 仍然是收着的（不会被自动展开冲掉）", async () => {
    const w1 = mountGrid();
    await toggle(w1, 1);
    // 组件把自己算出的折叠名单报了上去 —— 这就是要落库的那份
    const reported = lastReport(w1);
    expect(reported).toEqual([1]);
    w1.unmount();

    // 「回到主页」：用同一份名单重建
    const w2 = mountGrid(reported);
    expect(ids(w2)).toEqual([1, 2, 21, 22]);
  });

  it("一键收起后离开再回来：仍然全是收着的（而不是全展开）", async () => {
    const w1 = mountGrid();
    (w1.vm as unknown as { collapseAll: () => void }).collapseAll();
    await nextTick();
    const reported = lastReport(w1);
    expect(reported).toEqual([1, 2]);
    w1.unmount();

    const w2 = mountGrid(reported);
    expect(ids(w2)).toEqual([1, 2]);
  });

  it("一键展开会清空折叠名单（用户明确要求全开）", async () => {
    const w = mountGrid([1, 2]);
    (w.vm as unknown as { expandAll: () => void }).expandAll();
    await nextTick();
    expect(lastReport(w)).toEqual([]);
    expect(ids(w)).toEqual([1, 11, 12, 2, 21, 22]);
  });

  it("外部名单后到（设置异步加载）：覆盖默认的全展开", async () => {
    const w = mountGrid();
    expect(ids(w)).toEqual([1, 11, 12, 2, 21, 22]);
    // 设置读完了，props 才变成 [1]
    await w.setProps({ collapsedGroups: [1] });
    await nextTick();
    expect(ids(w)).toEqual([1, 2, 21, 22]);
  });

  /*
   * 回归：用户已经亲手调过展开态之后，后到的（更旧的）库值不许覆盖他。
   * 没有这道闸，用户在设置读完前折叠的分组会被库里的旧值重新打开。
   */
  it("用户已动过手：后到的旧名单不覆盖当前展开态", async () => {
    const w = mountGrid();
    await toggle(w, 1); // 用户收起 1
    await w.setProps({ collapsedGroups: [] }); // 库值说"什么都没收"（旧的）
    await nextTick();
    expect(ids(w)).toEqual([1, 2, 21, 22]); // 仍然收着，不被旧值打开
  });

  /*
   * 折叠名单里可能留着**已经被删掉**（或本次筛选下不可见）的分组 id。
   * 上报时只报"当下真实存在且收着的"，名单才不会越积越脏。
   */
  it("上报的折叠名单只含当下存在的分组（不累积过期 id）", async () => {
    const w = mountGrid([999, 1]); // 999 不存在
    await toggle(w, 2); // 再收一个，触发上报
    const reported = lastReport(w);
    expect(reported).toEqual([1, 2]); // 不含 999
  });

  it("展开 / 收起切换会触发 collapsed-change", async () => {
    const w = mountGrid();
    await toggle(w, 2);
    expect(lastReport(w)).toEqual([2]);
    await toggle(w, 2);
    expect(lastReport(w)).toEqual([]);
  });
});
