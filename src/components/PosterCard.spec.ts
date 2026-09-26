import { describe, expect, it } from "vitest";
import { mount } from "@vue/test-utils";
import PosterCard from "./PosterCard.vue";
import type { MediaItem } from "../api/commands";

function item(over: Partial<MediaItem> = {}): MediaItem {
  return {
    id: 7,
    type: "movie",
    title: "Inception",
    year: 2010,
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
    display_title: "Inception",
    ...over,
  };
}

describe("PosterCard", () => {
  it("显示展示名、年份与评分", () => {
    const w = mount(PosterCard, {
      props: { item: item({ year: 2010, vote_average: 8.369 }) },
    });
    expect(w.text()).toContain("Inception");
    expect(w.text()).toContain("2010");
    expect(w.text()).toContain("8.4"); // 评分保留一位
  });

  it("电影与剧集的类型角标不同", () => {
    expect(mount(PosterCard, { props: { item: item() } }).text()).toContain("电影");
    expect(
      mount(PosterCard, { props: { item: item({ type: "tv" }) } }).text(),
    ).toContain("剧集");
  });

  /** 判定不出来的条目不能再冒充电影——这是老版本最大的误判来源。 */
  it("待确认的条目有独立的角标", () => {
    const w = mount(PosterCard, { props: { item: item({ type: "unknown" }) } });
    expect(w.text()).toContain("待确认");
    expect(w.text()).not.toContain("电影");
  });

  it("剧集有单集数时显示集数", () => {
    const w = mount(PosterCard, {
      props: { item: item({ type: "tv", episode_count: 12 }) },
    });
    expect(w.text()).toContain("12 集");
  });

  /** 未刮削的条目要靠这个角标提示用户去批量刮削。 */
  it("没有海报时显示未刮削标记", () => {
    const w = mount(PosterCard, { props: { item: item({ poster_path: null }) } });
    expect(w.find(".unscraped").exists()).toBe(true);
  });

  it("有海报时用图片，不再显示未刮削标记", () => {
    const w = mount(PosterCard, {
      props: { item: item({ poster_path: "/p.jpg" }) },
    });
    expect(w.find(".unscraped").exists()).toBe(false);
    expect(w.find("img").attributes("src")).toBe(
      "https://image.tmdb.org/t/p/w342/p.jpg",
    );
  });

  it("点击卡片时把 id 抛出去", async () => {
    const w = mount(PosterCard, { props: { item: item({ id: 42 }) } });
    await w.trigger("click");
    expect(w.emitted("open")).toEqual([[42]]);
  });
});
