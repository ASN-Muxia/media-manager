import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { mount, type VueWrapper } from "@vue/test-utils";
import { nextTick } from "vue";
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

/* ==================== 滑动多选 ==================== */
/* 纯状态机与触发判定的边界在 `utils/dragSelect.spec.ts`；
   这里验证的是"它真的接上了 DOM"：鼠标即时拖、触摸长按、
   手指先动要当滚动放弃、以及**松手时才统一提交一次**。 */

/** `elementFromPoint` 的替身：jsdom 没有布局引擎，几何命中只能靠桩。 */
const hit = { el: null as Element | null };
let wrappers: VueWrapper[] = [];

function mountGrid(items: MediaItem[], selected: number[] = []): VueWrapper {
  const w = mount(PosterGrid, {
    props: { items, selected },
    attachTo: document.body,
  });
  wrappers.push(w);
  return w;
}

/**
 * 造一个指针事件。
 *
 * jsdom 没有 PointerEvent，且 `bubbles` / `cancelable` 在 Event 上是只读访问器
 * —— 所以只能把这两个交给构造函数，其余字段贴上去。
 */
function pointer(
  target: EventTarget,
  type: string,
  extra: Record<string, unknown> = {},
): void {
  const init: PointerEventInit = {
    bubbles: true,
    cancelable: true,
    button: 0,
    pointerType: "mouse",
    clientX: 0,
    clientY: 0,
    ...(extra as PointerEventInit),
  };
  let ev: Event;
  if (typeof PointerEvent === "function") {
    ev = new PointerEvent(type, init);
  } else {
    const { bubbles, cancelable, ...rest } = init;
    ev = Object.assign(new Event(type, { bubbles, cancelable }), rest);
  }
  target.dispatchEvent(ev);
}

/** 派发并等一次渲染（Vue 的 DOM 更新是异步的）。 */
async function fire(
  target: EventTarget,
  type: string,
  extra: Record<string, unknown> = {},
): Promise<void> {
  pointer(target, type, extra);
  await nextTick();
}

function cards(w: VueWrapper) {
  return w.findAll(".card");
}

function pickedIds(w: VueWrapper): number[] {
  return w.findAll(".card.picked").map((n) => Number(n.attributes("data-id")));
}

beforeEach(() => {
  hit.el = null;
  Object.defineProperty(document, "elementFromPoint", {
    configurable: true,
    writable: true,
    value: () => hit.el,
  });
});

afterEach(() => {
  for (const w of wrappers) w.unmount();
  wrappers = [];
  document.body.innerHTML = "";
});

describe("PosterGrid 滑动多选", () => {
  it("鼠标按下拖动，划过的卡片连续选中，松手时统一提交", async () => {
    const w = mountGrid([item(1), item(2), item(3), item(4)]);
    const c = cards(w);

    await fire(c[0].element, "pointerdown", { clientX: 0, clientY: 0 });
    hit.el = c[1].element;
    await fire(c[1].element, "pointermove", { clientX: 60, clientY: 0 });
    hit.el = c[2].element;
    await fire(c[2].element, "pointermove", { clientX: 120, clientY: 0 });

    // 拖动期间：实时高亮，但**一次都没有提交**
    expect(pickedIds(w)).toEqual([1, 2, 3]);
    expect(w.emitted("select-many")).toBeUndefined();
    expect(w.emitted("select")).toBeUndefined();

    await fire(c[2].element, "pointerup", { clientX: 120, clientY: 0 });
    expect(w.emitted("select-many")).toEqual([[[1, 2, 3]]]);
  });

  /** 反向划过已选项要能取消 —— 一次拖动既能框选一片也能抹掉一片。 */
  it("起点是已选中项时，整段拖动都是取消", async () => {
    const w = mountGrid([item(1), item(2), item(3), item(4)], [1, 2, 4]);
    const c = cards(w);
    expect(pickedIds(w)).toEqual([1, 2, 4]);

    await fire(c[0].element, "pointerdown", { clientX: 0, clientY: 0 });
    hit.el = c[1].element;
    await fire(c[1].element, "pointermove", { clientX: 60, clientY: 0 });
    // 划过已选项要立刻取消高亮
    expect(pickedIds(w)).toEqual([4]);

    await fire(c[1].element, "pointerup", { clientX: 60, clientY: 0 });
    expect(w.emitted("select-many")).toEqual([[[4]]]);
  });

  it("原地按下不移动 = 普通点击，不产生滑动结果", async () => {
    const w = mountGrid([item(1), item(2)]);
    const c = cards(w);

    await fire(c[0].element, "pointerdown", { clientX: 10, clientY: 10 });
    await fire(c[0].element, "pointermove", { clientX: 12, clientY: 11 }); // 手抖 2px
    await fire(c[0].element, "pointerup", { clientX: 12, clientY: 11 });

    expect(w.emitted("select-many")).toBeUndefined();
    // 原有的点击逻辑没被破坏
    await c[0].trigger("click");
    expect(w.emitted("open")).toEqual([[1]]);
  });

  /** 拖完一松手浏览器会紧接着派发 click，必须吞掉，否则会误开详情。 */
  it("拖动结束后紧随的那次 click 被吞掉", async () => {
    const w = mountGrid([item(1), item(2), item(3)]);
    const c = cards(w);

    await fire(c[0].element, "pointerdown", { clientX: 0, clientY: 0 });
    hit.el = c[1].element;
    await fire(c[1].element, "pointermove", { clientX: 60, clientY: 0 });
    await fire(c[1].element, "pointerup", { clientX: 60, clientY: 0 });

    await c[1].trigger("click");
    expect(w.emitted("open")).toBeUndefined();
    expect(w.emitted("select")).toBeUndefined();
  });

  it("拖动期间给网格加标记（禁掉文字选择）", async () => {
    const w = mountGrid([item(1), item(2)]);
    const c = cards(w);
    expect(w.find(".grid").classes()).not.toContain("dragging");

    await fire(c[0].element, "pointerdown", { clientX: 0, clientY: 0 });
    hit.el = c[1].element;
    await fire(c[1].element, "pointermove", { clientX: 60, clientY: 0 });
    expect(w.find(".grid").classes()).toContain("dragging");

    await fire(c[1].element, "pointerup", { clientX: 60, clientY: 0 });
    expect(w.find(".grid").classes()).not.toContain("dragging");
  });

  /** 触摸：长按成立后才能拖（真实计时器 + 假时间推进）。 */
  it("触摸需要长按才进入多选", async () => {
    vi.useFakeTimers();
    try {
      const w = mountGrid([item(1), item(2), item(3)]);
      const c = cards(w);

      await fire(c[0].element, "pointerdown", { pointerType: "touch", clientX: 0, clientY: 0 });
      // 还没到长按时长：不算拖动
      hit.el = c[1].element;
      await fire(c[1].element, "pointermove", {
        pointerType: "touch",
        clientX: 2,
        clientY: 1,
      });
      expect(w.find(".grid").classes()).not.toContain("dragging");

      vi.advanceTimersByTime(400);
      await nextTick();
      expect(w.find(".grid").classes()).toContain("dragging");
      expect(pickedIds(w)).toEqual([1]);

      hit.el = c[2].element;
      await fire(c[2].element, "pointermove", {
        pointerType: "touch",
        clientX: 4,
        clientY: 120,
      });
      expect(pickedIds(w)).toEqual([1, 3]);

      await fire(c[2].element, "pointerup", { pointerType: "touch", clientX: 4, clientY: 120 });
      expect(w.emitted("select-many")).toEqual([[[1, 3]]]);
    } finally {
      vi.useRealTimers();
    }
  });

  /** 触摸下"手指先动"就是想滚页面：必须放弃，不能抢成框选。 */
  it("触摸在长按成立前移动 → 判定为滚动，完全不介入", async () => {
    const w = mountGrid([item(1), item(2), item(3)]);
    const c = cards(w);

    await fire(c[0].element, "pointerdown", { pointerType: "touch", clientX: 0, clientY: 0 });
    hit.el = c[1].element;
    await fire(c[1].element, "pointermove", {
      pointerType: "touch",
      clientX: 0,
      clientY: 80,
    });
    await fire(c[1].element, "pointermove", {
      pointerType: "touch",
      clientX: 0,
      clientY: 160,
    });

    expect(w.find(".grid").classes()).not.toContain("dragging");
    expect(pickedIds(w)).toEqual([]);
    await fire(c[1].element, "pointerup", { pointerType: "touch", clientX: 0, clientY: 160 });
    expect(w.emitted("select-many")).toBeUndefined();
  });

  /** 提交结果只保留**当前可见**的项，避免删到界面上看不见的条目。 */
  it("提交时过滤掉已不在列表里的 id", async () => {
    const w = mountGrid([item(1), item(2), item(3)], [9]);
    const c = cards(w);

    await fire(c[0].element, "pointerdown", { clientX: 0, clientY: 0 });
    hit.el = c[1].element;
    await fire(c[1].element, "pointermove", { clientX: 60, clientY: 0 });
    await fire(c[1].element, "pointerup", { clientX: 60, clientY: 0 });

    // 库里残留的 9 不在当前列表里，不能被带进结果
    expect(w.emitted("select-many")).toEqual([[[1, 2]]]);
  });
});
