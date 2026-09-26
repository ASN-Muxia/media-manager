import { describe, expect, it } from "vitest";
import { mount } from "@vue/test-utils";
import CollectionPanel from "./CollectionPanel.vue";
import type { MovieCollection } from "../api/commands";

function collection(over: Partial<MovieCollection> = {}): MovieCollection {
  return {
    key: "dune",
    name: "沙丘",
    movies: [
      { id: 1, title: "沙丘", year: 2021, poster_path: "/a.jpg", file_path: null },
      { id: 2, title: "沙丘 2", year: 2024, poster_path: null, file_path: null },
    ],
    duplicateCount: 0,
    startYear: 2021,
    endYear: 2024,
    ...over,
  };
}

/**
 * 系列合集是**派生视图**（由电影标题算出来），所以这个组件有三件事要盯住：
 * 顺序（按上映时间）、折叠掉的多版本要说清楚、以及空态要能引导用户。
 */
describe("CollectionPanel", () => {
  it("展示系列名、年份跨度与成员数", () => {
    const w = mount(CollectionPanel, { props: { collections: [collection()] } });
    expect(w.text()).toContain("沙丘");
    expect(w.text()).toContain("2021 – 2024");
    expect(w.text()).toContain("2 部");
  });

  it("成员按后端给的顺序展示（后端已按上映时间排序）", () => {
    const w = mount(CollectionPanel, { props: { collections: [collection()] } });
    const titles = w.findAll(".title").map((n) => n.text());
    expect(titles).toEqual(["沙丘", "沙丘 2"]);
  });

  it("折叠掉的同片多版本要报个数，不能悄悄消失", () => {
    const w = mount(CollectionPanel, {
      props: { collections: [collection({ duplicateCount: 3 })] },
    });
    expect(w.text()).toContain("折叠 3 个重复版本");
  });

  it("只有一个年份时不显示区间", () => {
    const w = mount(CollectionPanel, {
      props: {
        collections: [
          collection({ startYear: 2021, endYear: 2021, movies: [
            { id: 1, title: "沙丘", year: 2021, poster_path: null, file_path: null },
          ] }),
        ],
      },
    });
    expect(w.text()).toContain("2021");
    expect(w.text()).not.toContain("–");
  });

  it("点某一部时把它的 id 抛给父级（进详情）", async () => {
    const w = mount(CollectionPanel, { props: { collections: [collection()] } });
    await w.findAll(".card")[1].trigger("click");
    expect(w.emitted("open")).toEqual([[2]]);
  });

  it("加载中显示提示而不是空态", () => {
    const w = mount(CollectionPanel, {
      props: { collections: [], loading: true },
    });
    expect(w.text()).toContain("正在计算系列合集");
    expect(w.find(".empty").exists()).toBe(false);
  });

  it("没有可聚合的系列时给出空态说明", () => {
    const w = mount(CollectionPanel, { props: { collections: [] } });
    expect(w.text()).toContain("还没有能凑成合集的系列");
  });

  it("多个系列各占一块，互不干扰", () => {
    const w = mount(CollectionPanel, {
      props: {
        collections: [
          collection(),
          collection({
            key: "rocky",
            name: "洛奇",
            movies: [
              { id: 3, title: "洛奇", year: 1976, poster_path: null, file_path: null },
              { id: 4, title: "洛奇 2", year: 1979, poster_path: null, file_path: null },
            ],
            startYear: 1976,
            endYear: 1979,
          }),
        ],
      },
    });
    expect(w.findAll("section.coll")).toHaveLength(2);
    expect(w.text()).toContain("2 个系列 · 4 部电影");
  });
});
