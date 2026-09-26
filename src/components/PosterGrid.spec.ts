import { describe, expect, it } from "vitest";
import { mount } from "@vue/test-utils";
import PosterGrid from "./PosterGrid.vue";
import type { MediaItem } from "../api/commands";

function item(id: number, over: Partial<MediaItem> = {}): MediaItem {
  return {
    id,
    type: "movie",
    title: `Movie ${id}`,
    year: 2020,
    tmdb_id: null,
    poster_path: null,
    backdrop_path: null,
    overview: null,
    vote_average: null,
    genre: null,
    file_path: null,
    file_name: null,
    episode_count: 0,
    added_at: 1,
    display_title: `Movie ${id}`,
    ...over,
  };
}

describe("PosterGrid", () => {
  it("每个条目渲染一张卡片", () => {
    const w = mount(PosterGrid, { props: { items: [item(1), item(2), item(3)] } });
    expect(w.findAll(".card")).toHaveLength(3);
  });

  /** 空库要给出可操作的引导，而不是一片空白。 */
  it("空列表显示引导文案且不渲染网格", () => {
    const w = mount(PosterGrid, { props: { items: [] } });
    expect(w.find(".grid").exists()).toBe(false);
    expect(w.text()).toContain("媒体库还是空的");
    expect(w.text()).toContain("扫描媒体库");
  });

  it("子卡片的点击事件向上冒泡（带的是条目 id）", async () => {
    const w = mount(PosterGrid, { props: { items: [item(11), item(12)] } });
    await w.findAll(".card")[1].trigger("click");
    expect(w.emitted("open")).toEqual([[12]]);
  });
});
