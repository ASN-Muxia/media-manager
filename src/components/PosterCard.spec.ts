import { describe, expect, it, vi } from "vitest";
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

/**
 * 派发一次**真实鼠标**点击。
 *
 * ⚠️ 不能用 `w.trigger("click")`：test-utils 造出来的 `MouseEvent` 是
 * `detail: 0`，与浏览器里的**键盘**点击同形，会走「立即打开」那条分支 ——
 * 而我们要测的是鼠标路径（延迟 / 双击取消）。所以这里自己 new 一个
 * `detail: 1`（单击）或 `2`（双击的第二下）的事件。
 */
async function mouseClick(w: ReturnType<typeof mount>, detail = 1): Promise<void> {
  w.element.dispatchEvent(
    new MouseEvent("click", { bubbles: true, cancelable: true, detail }),
  );
  await w.vm.$nextTick();
}

async function mouseDblClick(w: ReturnType<typeof mount>): Promise<void> {
  w.element.dispatchEvent(
    new MouseEvent("dblclick", { bubbles: true, cancelable: true, detail: 2 }),
  );
  await w.vm.$nextTick();
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
    vi.useFakeTimers();
    try {
      const w = mount(PosterCard, { props: { item: item({ id: 42 }) } });
      await mouseClick(w);
      // ⚠️ 单击被**延迟**了（见下面「双击播放」一组）：要等过双击窗口
      await vi.advanceTimersByTimeAsync(300);
      expect(w.emitted("open")).toEqual([[42]]);
    } finally {
      vi.useRealTimers();
    }
  });

  /* ---------------- 双击直接播放 ---------------- */
  /*
   * 为什么这里全是假定时器：单击必须**延后**到双击窗口结束才开详情。
   * 否则第一下 click 立刻 `emit("open")` → App 里 `MediaDetail` 是
   * `v-if`、封面墙是 `v-else`，网格当场卸载，第二下落在详情页上，
   * `dblclick` 永远等不到。这组用例钉的就是这条时序。
   */

  it("单击会延迟到双击窗口之后才打开详情", async () => {
    vi.useFakeTimers();
    try {
      const w = mount(PosterCard, { props: { item: item({ id: 42 }) } });
      await mouseClick(w);
      // 窗口还没过：详情不该开（否则网格卸载、双击落空）
      expect(w.emitted("open")).toBeUndefined();

      await vi.advanceTimersByTimeAsync(300);
      expect(w.emitted("open")).toEqual([[42]]);
    } finally {
      vi.useRealTimers();
    }
  });

  it("双击发 play，并且取消那次挂起的打开详情", async () => {
    vi.useFakeTimers();
    try {
      const w = mount(PosterCard, { props: { item: item({ id: 42 }) } });
      await mouseClick(w, 1);
      await mouseClick(w, 2); // 双击的第二下
      await mouseDblClick(w);

      expect(w.emitted("play")).toEqual([[42]]);

      // 时间推过窗口：挂起的 open 必须已经被取消，不能"播完又弹详情"
      await vi.advanceTimersByTimeAsync(500);
      expect(w.emitted("open")).toBeUndefined();
    } finally {
      vi.useRealTimers();
    }
  });

  it("多选模式下双击不播放（那时的语义是勾选）", async () => {
    vi.useFakeTimers();
    try {
      const w = mount(PosterCard, {
        props: { item: item({ id: 42 }), selectable: true },
      });
      await mouseClick(w, 1);
      await mouseClick(w, 2);
      await mouseDblClick(w);

      expect(w.emitted("play")).toBeUndefined();
      // 多选模式仍然是**即时**勾选，不受双击窗口影响
      expect(w.emitted("select")).toHaveLength(2);
    } finally {
      vi.useRealTimers();
    }
  });

  /** 键盘触发（`detail === 0`）不可能是双击，不该让用户等窗口。 */
  it("键盘触发的 click 立即打开，不延迟", async () => {
    const w = mount(PosterCard, { props: { item: item({ id: 42 }) } });
    const ev = new MouseEvent("click", { bubbles: true, cancelable: true, detail: 0 });
    w.element.dispatchEvent(ev);
    await w.vm.$nextTick();
    expect(w.emitted("open")).toEqual([[42]]);
  });

  /** 卸载时把挂起的定时器清掉：否则卡片没了还会 emit 一次 open。 */
  it("卸载时清掉挂起的打开，不再发事件", async () => {
    vi.useFakeTimers();
    try {
      const w = mount(PosterCard, { props: { item: item({ id: 42 }) } });
      await mouseClick(w);
      w.unmount();
      await vi.advanceTimersByTimeAsync(500);
      expect(w.emitted("open")).toBeUndefined();
    } finally {
      vi.useRealTimers();
    }
  });

  /* ---------------- 观看角标（v6） ---------------- */

  it("没传观看状态时不画角标（不能画成「没看过」这种假信息）", () => {
    const w = mount(PosterCard, { props: { item: item() } });
    expect(w.find(".seen").exists()).toBe(false);
  });

  it("看过的电影显示「看过」", () => {
    const w = mount(PosterCard, { props: { item: item(), watched: true } });
    expect(w.find(".seen").text()).toBe("看过");
  });

  it("剧集显示集级进度而非「看过」（口径是集，不是秒）", () => {
    const w = mount(PosterCard, {
      props: { item: item({ type: "tv", episode_count: 25 }), progress: "3/25", watched: false },
    });
    expect(w.find(".seen").text()).toBe("3/25");
    expect(w.find(".seen").text()).not.toMatch(/分|秒/);
  });

  it("多选模式下不画观看角标（左下角让给类型标签）", () => {
    const w = mount(PosterCard, {
      props: { item: item(), selectable: true, watched: true },
    });
    expect(w.find(".seen").exists()).toBe(false);
  });
});
