import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { mount, type VueWrapper } from "@vue/test-utils";
import { defineComponent, h, nextTick, ref } from "vue";
import PosterGrid from "./PosterGrid.vue";
import { pressBlocked, useSwipeSelect } from "../composables/useSwipeSelect";
import { LONG_PRESS_MS } from "../utils/dragSelect";
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
    vi.useFakeTimers();
    try {
      const w = mount(PosterGrid, { props: { items: [item(11), item(12)] } });
      // 单击要过双击窗口才开详情（见 PosterCard 的 `DBLCLICK_MS`）
      w.findAll(".card")[1].element.dispatchEvent(
        new MouseEvent("click", { bubbles: true, cancelable: true, detail: 1 }),
      );
      await vi.advanceTimersByTimeAsync(300);
      expect(w.emitted("open")).toEqual([[12]]);
    } finally {
      vi.useRealTimers();
    }
  });

  /** 双击卡片：把「用户双击了谁」原样交给父级，决定播哪一集是后端的事。 */
  it("双击卡片向上抛 play（带条目 id）", async () => {
    const w = mount(PosterGrid, { props: { items: [item(11), item(12)] } });
    await w.findAll(".card")[1].trigger("dblclick");
    expect(w.emitted("play")).toEqual([[12]]);
  });

  /** 必选模式（多选 / 聚合）下双击是「勾选」，不该冒出播放。 */
  it("多选模式下双击不发 play", async () => {
    const w = mount(PosterGrid, {
      props: { items: [item(11)], selectable: true },
    });
    await w.findAll(".card")[0].trigger("dblclick");
    expect(w.emitted("play")).toBeUndefined();
  });
});

/* ==================== 滑动多选 ==================== */
/* 纯状态机与触发判定的边界在 `utils/dragSelect.spec.ts`；
   这里验证的是"它真的接上了 DOM"：鼠标即时拖、触摸长按、
   手指先动要当滚动放弃、以及**松手时才统一提交一次**。 */

/** `elementFromPoint` 的替身：jsdom 没有布局引擎，几何命中只能靠桩。 */
const hit = { el: null as Element | null };
let wrappers: VueWrapper[] = [];

/**
 * 挂一块网格。
 *
 * ⚠️ 默认带上 `selectable: true`：**滑动多选只在「选择」模式里生效**
 * （与"按住即拖动聚合"是同一个手势，只能靠一个明确的用户意图分辨；
 * 见 `PosterGrid.vue` 的 `selectMode`）。本文件测的就是滑动多选，
 * 所以默认就在那个模式里；要测拖拽聚合的去 `PosterGrid.group.spec.ts`。
 */
function mountGrid(
  items: MediaItem[],
  selected: number[] = [],
  props: Record<string, unknown> = {},
): VueWrapper {
  const w = mount(PosterGrid, {
    props: { items, selected, selectable: true, ...props },
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
  // 跟手 / 落点由组件挂在 window 上；元素上派发不会冒泡到 window，补一次。
  // （`bubbles` 是只读访问器，只能交给构造函数。）
  if (target !== window && type.startsWith("pointer")) {
    const { bubbles: _b, cancelable: _c, ...rest } = init;
    const clone = typeof PointerEvent === "function"
      ? new PointerEvent(type, { ...init, bubbles: false })
      : Object.assign(new Event(type, { bubbles: false, cancelable: true }), rest);
    window.dispatchEvent(clone);
  }
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

/**
 * 给元素装一个假的布局矩形。
 *
 * jsdom 没有布局引擎，`getBoundingClientRect()` 恒为全 0 —— 而"划到列表外要
 * 夹到首/末项"这件事必须依赖矩形，所以只能在用例里把矩形桩上去。
 */
function stubRect(el: Element, top: number, bottom: number): void {
  Object.defineProperty(el, "getBoundingClientRect", {
    configurable: true,
    value: () => ({
      top, bottom, left: 0, right: 400, width: 400, height: bottom - top,
      x: 0, y: top, toJSON: () => ({}),
    }),
  });
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

  /**
   * 模式分流：**默认（非「选择」）模式下，按住卡片划过去不再勾选**，
   * 那次手势归"拖到另一张卡上 = 聚合"（见 `PosterGrid.group.spec.ts`）。
   *
   * 这是本次修复唯一对外收敛的行为，理由是两者本就是同一个手势：
   * 左键按下 → 移动 → 松手，没有任何几何判据能把"我要连续勾选"和
   * "我要把这张搬到那张上"分开。界面上本来就有那个明确的意图开关 —— 顶栏「选择」。
   */
  it("默认模式（未进「选择」）：按住卡片拖动发出 move，不勾选任何东西", async () => {
    const w = mountGrid([item(1), item(2), item(3)], [], { selectable: false });
    const c = cards(w);
    const cell = w.findAll(".cell")[0].element as HTMLElement;
    // 一个可拖元素都不留：拖拽源是"这一次手势"，不是某个 draggable 元素
    expect(cell.draggable).toBe(false);

    hit.el = c[1].element; // 指针落在第 2 张卡上
    await fire(c[0].element, "pointerdown", { clientX: 0, clientY: 0 });
    await fire(c[1].element, "pointermove", { clientX: 60, clientY: 0 });

    // 已经起拖：网格上出现落点反馈用的「放回顶层」区
    expect(w.find(".rootzone").exists()).toBe(true);
    await fire(c[1].element, "pointerup", { clientX: 60, clientY: 0 });

    expect(w.emitted("select-many")).toBeUndefined();
    expect(w.emitted("move")).toEqual([[{ ids: [1], parentId: 2, index: null }]]);
  });

  /**
   * 规则：**静止态**（还没按过任何一次）就必须是不可拖的。
   *
   * 上面那条只管"按下之后"；模板里曾经写死 `draggable="true"`，于是挂载后
   * 整墙天生可拖 —— 用户按住卡片往边上一划，浏览器先起的是原生拖拽、
   * 顺手补一次 `pointercancel`，滑动多选连起手机会都没有。静态属性在 vnode
   * 里恒等、Vue 的幂等 patch 不会再改写它，所以这个初值必须由 `props.grouping`
   * 绑定出来，不能靠"按下时再纠正"。
   */
  it("静止态不可拖：挂载后未按过的卡片不抢滑动多选的手势", () => {
    const w = mountGrid([item(1), item(2)]);
    for (const cell of w.findAll(".cell")) {
      expect((cell.element as HTMLElement).draggable).toBe(false);
    }
  });

  it("原地按下不移动 = 普通点击，不产生滑动结果", async () => {
    const w = mountGrid([item(1), item(2)]);
    const c = cards(w);

    await fire(c[0].element, "pointerdown", { clientX: 10, clientY: 10 });
    await fire(c[0].element, "pointermove", { clientX: 12, clientY: 11 }); // 手抖 2px
    await fire(c[0].element, "pointerup", { clientX: 12, clientY: 11 });

    expect(w.emitted("select-many")).toBeUndefined();
    // 原有的点击逻辑没被破坏：本文件默认在「选择」模式，单击 = 勾选这一张
    // （不在选择模式时的单击 = 打开详情，见下面那条反向用例）
    await c[0].trigger("click");
    // ⚠️ 载荷从 `id` 改成了 `{id, mods}`：多选要区分 Shift 区间 / Ctrl 切换，
    // 而修饰键只有点击事件知道（见 `PosterCard.onClick`）。三个键都为 false
    // 就是普通点击。
    expect(w.emitted("select")).toEqual([
      [{ id: 1, mods: { shift: false, ctrl: false, meta: false } }],
    ]);
  });

  /**
   * 反向判据：**默认（非「选择」）模式**下，按住不动再松手是一次普通点击 ——
   * 不该因为"可能拖拽"就把点击吃掉。
   */
  it("默认模式：按住不动再松手仍是普通点击（没起拖就不吞 click）", async () => {
    const w = mountGrid([item(1), item(2)], [], { selectable: false });
    const c = cards(w);

    await fire(c[0].element, "pointerdown", { clientX: 10, clientY: 10 });
    await fire(c[0].element, "pointermove", { clientX: 12, clientY: 11 }); // 手抖 2px，没到阈值
    await fire(c[0].element, "pointerup", { clientX: 12, clientY: 11 });

    expect(w.emitted("move")).toBeUndefined();
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
      // 一次移动从第 1 张跳到第 3 张：中间那张要被补上（只处理"指针正下方那一项"的话是 [1, 3]）
      expect(pickedIds(w)).toEqual([1, 2, 3]);

      await fire(c[2].element, "pointerup", { pointerType: "touch", clientX: 4, clientY: 120 });
      expect(w.emitted("select-many")).toEqual([[[1, 2, 3]]]);
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

  /**
   * 规则：快速滑动要**补齐被跳过的项**。
   * 指针事件是稀疏的，一次 pointermove 可能跨过好几张卡；只写"指针正下方
   * 那一项"会把中间整段漏掉（旧实现的结果就是 [1, 4]）。
   */
  it("快速滑动：一次移动跨过多张卡，中间的卡也要补上", async () => {
    const w = mountGrid([item(1), item(2), item(3), item(4), item(5)]);
    const c = cards(w);

    await fire(c[0].element, "pointerdown", { clientX: 0, clientY: 0 });
    hit.el = c[3].element; // 只给一次移动，指针已经落到第 4 张卡上
    await fire(c[3].element, "pointermove", { clientX: 300, clientY: 0 });

    expect(pickedIds(w)).toEqual([1, 2, 3, 4]);
    await fire(c[3].element, "pointerup", { clientX: 300, clientY: 0 });
    expect(w.emitted("select-many")).toEqual([[[1, 2, 3, 4]]]);
  });

  /** 规则同上，反向（从后往前抹）也要补齐。 */
  it("快速反向滑动：从后往前抹，中间被跳过的卡同样要补上", async () => {
    const w = mountGrid([item(1), item(2), item(3), item(4), item(5)], [1, 2, 3, 4, 5]);
    const c = cards(w);

    await fire(c[4].element, "pointerdown", { clientX: 400, clientY: 0 });
    hit.el = c[1].element;
    await fire(c[1].element, "pointermove", { clientX: 60, clientY: 0 });
    await fire(c[1].element, "pointerup", { clientX: 60, clientY: 0 });

    expect(w.emitted("select-many")).toEqual([[[1]]]);
  });

  /**
   * 规则：划到**最后一项之外**不能漏掉端点。
   * 兜底顺序：先断言没有布局信息时什么都不做（退化不改旧行为），
   * 再断言装上矩形后末项被带上。
   */
  it("划到最后一张卡之外的空白，仍然带上最后一张（端点不漏）", async () => {
    const before = mountGrid([item(1), item(2), item(3), item(4)]);
    const bc = cards(before);
    await fire(bc[1].element, "pointerdown", { clientX: 0, clientY: 50 });
    hit.el = null;
    await fire(before.find(".grid").element, "pointermove", { clientX: 0, clientY: 400 });
    await fire(before.find(".grid").element, "pointerup", { clientX: 0, clientY: 400 });
    // 没有布局信息（jsdom 的常态）→ 划出网格不改变选择
    expect(before.emitted("select-many")).toEqual([[[2]]]);

    const w = mountGrid([item(1), item(2), item(3), item(4)]);
    stubRect(w.find(".grid").element, 0, 200);
    const c = cards(w);
    await fire(c[1].element, "pointerdown", { clientX: 0, clientY: 50 });
    hit.el = null; // 指针已经划出网格，几何命中取不到任何卡片
    await fire(w.find(".grid").element, "pointermove", { clientX: 0, clientY: 400 });
    await fire(w.find(".grid").element, "pointerup", { clientX: 0, clientY: 400 });
    expect(w.emitted("select-many")).toEqual([[[2, 3, 4]]]);
  });

  /** 规则：划到**第一项之外**同样不能漏掉端点。 */
  it("划到第一张卡之外的空白，仍然带上第一张（端点不漏）", async () => {
    const w = mountGrid([item(1), item(2), item(3), item(4)]);
    stubRect(w.find(".grid").element, 0, 200);
    const c = cards(w);

    await fire(c[2].element, "pointerdown", { clientX: 0, clientY: 150 });
    hit.el = null;
    await fire(w.find(".grid").element, "pointermove", { clientX: 0, clientY: -60 });
    await fire(w.find(".grid").element, "pointerup", { clientX: 0, clientY: -60 });

    // 顺序是"选择发生的先后顺序"（模块里的约定），所以是 3 打头
    expect(w.emitted("select-many")).toEqual([[[3, 2, 1]]]);
  });

  /**
   * 规则：几何命中取不到时要**优雅退化**，不能抛错。
   * jsdom（以及任何没有布局引擎的环境）没有 `elementFromPoint`，
   * 旧实现直接调用会 TypeError，把整条滑动链路一起炸掉；
   * 现在鼠标路径退回 `event.target`，照样能选。
   */
  it("几何命中取不到时鼠标路径退回 event.target，且不抛错", async () => {
    const w = mountGrid([item(1), item(2), item(3)]);
    const c = cards(w);

    // 模拟"环境根本没有 elementFromPoint"
    (document as unknown as Record<string, unknown>).elementFromPoint = undefined;
    expect(typeof document.elementFromPoint).not.toBe("function");

    await fire(c[0].element, "pointerdown", { clientX: 0, clientY: 0 });
    await fire(c[2].element, "pointermove", { clientX: 200, clientY: 0 });
    await fire(c[2].element, "pointerup", { clientX: 200, clientY: 0 });

    expect(w.emitted("select-many")).toEqual([[[1, 2, 3]]]);
  });

  /**
   * 规则：边缘自动滚动**真的会滚**，且每滚一帧重新做一次命中。
   * 只测"有没有启动"是不够的 —— 曾经的版本启动过但没重新命中，
   * 结果视野外的项照样选不上。
   */
  it("贴到网格边缘时启动自动滚动，滚动后重新命中", async () => {
    const host = document.createElement("div");
    host.setAttribute("style", "overflow-y: auto");
    Object.defineProperty(host, "scrollHeight", { configurable: true, value: 1000 });
    Object.defineProperty(host, "clientHeight", { configurable: true, value: 300 });
    Object.defineProperty(host, "scrollTop", { configurable: true, writable: true, value: 0 });
    stubRect(host, 0, 300);
    document.body.appendChild(host);

    const frames: FrameRequestCallback[] = [];
    const raf = vi.fn((cb: FrameRequestCallback): number => frames.push(cb));
    const realRaf = window.requestAnimationFrame;
    window.requestAnimationFrame = raf as unknown as typeof window.requestAnimationFrame;
    (globalThis as { requestAnimationFrame?: unknown }).requestAnimationFrame = raf;

    try {
      const w = mount(
        PosterGrid,
        // 滑动多选只在「选择」模式里生效（见 mountGrid 的说明）
        { props: { items: [item(1), item(2), item(3)], selectable: true }, attachTo: host },
      );
      wrappers.push(w);
      const c = cards(w);

      await fire(c[0].element, "pointerdown", { clientX: 0, clientY: 100 });
      hit.el = c[1].element;
      await fire(c[1].element, "pointermove", { clientX: 60, clientY: 296 }); // 贴近下边缘
      expect(raf).toHaveBeenCalled(); // 滚动循环起来了

      hit.el = c[2].element; // 滚了一帧后指针落到第 3 张卡上
      const frame = frames[frames.length - 1];
      frame(0);
      await nextTick(); // 预览集合变了，DOM 要等一次渲染

      expect(host.scrollTop).toBeGreaterThan(0);
      expect(pickedIds(w)).toEqual([1, 2, 3]);

      await fire(c[1].element, "pointerup", { clientX: 60, clientY: 296 });
      expect(w.emitted("select-many")).toEqual([[[1, 2, 3]]]);
    } finally {
      window.requestAnimationFrame = realRaf;
      (globalThis as { requestAnimationFrame?: unknown }).requestAnimationFrame = realRaf;
      host.remove();
    }
  });
});

/* ==================== 复核补测：他没覆盖的真实边界 ==================== */
/* 这一组不是重跑已有用例，而是补「真机上会出事、但用例里没人钉」的口子：
   非左键按下、松手在容器之外、拖动中列表被改、自动滚动的 rAF 到底停没停、
   以及从空库起家时触摸滚动拦截还挂不挂得上。 */

describe("PosterGrid 滑动多选 · 边界补测", () => {
  /**
   * 规则：**只有左键**才起手拖动。
   * 右键（button !== 0）是上下文菜单 / 长按菜单，绝不能被抢成框选。
   */
  it("右键（非左键）按下不起手拖动", async () => {
    const w = mountGrid([item(1), item(2), item(3)]);
    const c = cards(w);

    await fire(c[0].element, "pointerdown", { button: 2, clientX: 0, clientY: 0 });
    hit.el = c[2].element;
    await fire(c[2].element, "pointermove", { button: 2, clientX: 120, clientY: 0 });
    await fire(c[2].element, "pointerup", { button: 2, clientX: 120, clientY: 0 });

    expect(w.emitted("select-many")).toBeUndefined();
    expect(pickedIds(w)).toEqual([]);
  });

  /**
   * 规则：松手**常常发生在列表之外**，所以 pointerup 必须挂在 document 上。
   * 这条钉的是"抬手在容器外也要正常提交一次"。
   */
  it("松手发生在网格之外（document 级 pointerup）仍正常提交", async () => {
    const w = mountGrid([item(1), item(2), item(3)]);
    const c = cards(w);

    await fire(c[0].element, "pointerdown", { clientX: 0, clientY: 0 });
    hit.el = c[1].element;
    await fire(c[1].element, "pointermove", { clientX: 60, clientY: 0 });
    // 抬手时指针已经跑到网格外面了
    await fire(document, "pointerup", { clientX: 900, clientY: 900 });

    expect(w.emitted("select-many")).toEqual([[[1, 2]]]);
  });

  /**
   * 规则：拖动**中途列表被改**（扫描完成 / 筛选变化都会重渲染）不能写出
   * 已经不在界面上的 id，也不能越界报错。
   */
  it("拖动途中列表被改成更短，结果自动收敛到新列表", async () => {
    const w = mountGrid([item(1), item(2), item(3), item(4)]);
    const c = cards(w);

    await fire(c[0].element, "pointerdown", { clientX: 0, clientY: 0 });
    hit.el = c[2].element;
    await fire(c[2].element, "pointermove", { clientX: 120, clientY: 0 });

    await w.setProps({ items: [item(1), item(2)] });
    hit.el = null; // 旧节点已卸载，几何命中取不到任何东西
    await fire(w.find(".grid").element, "pointermove", { clientX: 0, clientY: 0 });
    await fire(w.find(".grid").element, "pointerup", { clientX: 0, clientY: 0 });

    // 3 已经不在界面上了，不能被带进结果（"不能删到看不见的条目"）
    expect(w.emitted("select-many")).toEqual([[[1, 2]]]);
  });

  /**
   * 规则：触摸下必须拦掉原生滚动，否则长按进入多选后一拖页面跟着一起滚。
   * 兜底顺序：先断言「首屏就有数据」这条正常路径拦得住，
   * 再断言「首屏是空库、之后才填充」这条路径同样拦得住。
   */
  it("触摸拖动期间拦掉原生滚动（含「先空库后填充」的首屏）", async () => {
    // ① 首屏就有数据：正常路径
    const w = mountGrid([item(1), item(2), item(3)]);
    const c = cards(w);
    await fire(c[0].element, "pointerdown", { clientX: 0, clientY: 0 });
    hit.el = c[1].element;
    await fire(c[1].element, "pointermove", { clientX: 60, clientY: 0 });

    const tm = new Event("touchmove", { bubbles: true, cancelable: true });
    w.find(".grid").element.dispatchEvent(tm);
    expect(tm.defaultPrevented).toBe(true);
    await fire(c[1].element, "pointerup", { clientX: 60, clientY: 0 });

    // ② 首屏是空库（.grid 根本没渲染），扫描完才有数据
    const late = mountGrid([]);
    expect(late.find(".grid").exists()).toBe(false);
    await late.setProps({ items: [item(1), item(2), item(3)] });
    const lc = cards(late);

    await fire(lc[0].element, "pointerdown", { clientX: 0, clientY: 0 });
    hit.el = lc[1].element;
    await fire(lc[1].element, "pointermove", { clientX: 60, clientY: 0 });

    const tm2 = new Event("touchmove", { bubbles: true, cancelable: true });
    late.find(".grid").element.dispatchEvent(tm2);
    expect(tm2.defaultPrevented).toBe(true);
    await fire(lc[1].element, "pointerup", { clientX: 60, clientY: 0 });
  });

  /**
   * 规则：贴边自动滚动的 rAF **必须真的被取消** ——
   * 松手后还在滚（更别说卸载后还在滚）就是典型的内存/行为泄漏。
   */
  it("松手后边缘滚动的 rAF 被取消（不会一直滚下去）", async () => {
    const host = makeScrollHost();

    let seq = 0;
    const raf = vi.fn(() => ++seq);
    const caf = vi.fn();
    const realRaf = window.requestAnimationFrame;
    const realCaf = window.cancelAnimationFrame;
    window.requestAnimationFrame = raf as unknown as typeof window.requestAnimationFrame;
    window.cancelAnimationFrame = caf as unknown as typeof window.cancelAnimationFrame;
    (globalThis as { requestAnimationFrame?: unknown }).requestAnimationFrame = raf;
    (globalThis as { cancelAnimationFrame?: unknown }).cancelAnimationFrame = caf;

    try {
      const w = mount(
        PosterGrid,
        // 滑动多选只在「选择」模式里生效（见 mountGrid 的说明）
        { props: { items: [item(1), item(2), item(3)], selectable: true }, attachTo: host },
      );
      wrappers.push(w);
      const c = cards(w);

      await fire(c[0].element, "pointerdown", { clientX: 0, clientY: 100 });
      hit.el = c[1].element;
      await fire(c[1].element, "pointermove", { clientX: 60, clientY: 296 }); // 贴下边缘
      expect(raf).toHaveBeenCalledTimes(1);
      const frameId = seq;

      await fire(c[1].element, "pointerup", { clientX: 60, clientY: 296 });
      expect(caf).toHaveBeenCalledWith(frameId);
    } finally {
      window.requestAnimationFrame = realRaf;
      window.cancelAnimationFrame = realCaf;
      (globalThis as { requestAnimationFrame?: unknown }).requestAnimationFrame = realRaf;
      (globalThis as { cancelAnimationFrame?: unknown }).cancelAnimationFrame = realCaf;
      host.remove();
    }
  });

  /** 规则同上，但走的是**卸载**这条退出路径：卸载后绝不能再排帧。 */
  it("拖动中直接卸载也会停掉边缘滚动", async () => {
    const host = makeScrollHost();

    let seq = 0;
    const raf = vi.fn(() => ++seq);
    const caf = vi.fn();
    const realRaf = window.requestAnimationFrame;
    const realCaf = window.cancelAnimationFrame;
    window.requestAnimationFrame = raf as unknown as typeof window.requestAnimationFrame;
    window.cancelAnimationFrame = caf as unknown as typeof window.cancelAnimationFrame;
    (globalThis as { requestAnimationFrame?: unknown }).requestAnimationFrame = raf;
    (globalThis as { cancelAnimationFrame?: unknown }).cancelAnimationFrame = caf;

    try {
      const w = mount(
        PosterGrid,
        { props: { items: [item(1), item(2), item(3)], selectable: true }, attachTo: host },
      );
      const c = cards(w);

      await fire(c[0].element, "pointerdown", { clientX: 0, clientY: 100 });
      hit.el = c[1].element;
      await fire(c[1].element, "pointermove", { clientX: 60, clientY: 296 });
      expect(raf).toHaveBeenCalledTimes(1);
      const frameId = seq;

      w.unmount(); // 不等松手，直接卸载
      expect(caf).toHaveBeenCalledWith(frameId);
    } finally {
      window.requestAnimationFrame = realRaf;
      window.cancelAnimationFrame = realCaf;
      (globalThis as { requestAnimationFrame?: unknown }).requestAnimationFrame = realRaf;
      (globalThis as { cancelAnimationFrame?: unknown }).cancelAnimationFrame = realCaf;
      host.remove();
    }
  });

  /**
   * 规则：卸载时**长按定时器必须被清掉** ——
   * 否则组件已经不在了，定时器还会把一次 `beginDragSelection` 打出来。
   */
  it("卸载时清掉触摸长按定时器（卸载后不会再起手）", async () => {
    const setT = vi.spyOn(window, "setTimeout");
    const clearT = vi.spyOn(window, "clearTimeout");
    try {
      const w = mountGrid([item(1), item(2), item(3)]);
      const c = cards(w);
      await fire(c[0].element, "pointerdown", { pointerType: "touch", clientX: 0, clientY: 0 });

      const idx = setT.mock.calls.findIndex((call) => call[1] === LONG_PRESS_MS);
      expect(idx).toBeGreaterThanOrEqual(0); // 确实排了长按定时器
      const timerId = setT.mock.results[idx].value as unknown;

      w.unmount();
      wrappers = wrappers.filter((x) => x !== w);
      expect(clearT).toHaveBeenCalledWith(timerId);
      void c;
    } finally {
      setT.mockRestore();
      clearT.mockRestore();
    }
  });

  /**
   * 规则：`pointercancel` 是"当作没发生过"，**不能装 click 吞除**。
   * 系统打断之后浏览器不会再派发那次尾随 click，装了反而会吞掉用户
   * **随后**的真实点击（表现为"打断一次之后卡片点不动了"）。
   * 兜底顺序：先断言 cancel 路径不吞，再断言 pointerup 路径照旧吞 ——
   * 两条放在一起，防止有人把 armSwallow 又挪回公共收尾里。
   */
  it("pointercancel 不装 click 吞除（对照：pointerup 会吞）", async () => {
    // ① cancel 路径：紧随的 click 必须原样放行
    const w = mountGrid([item(1), item(2), item(3)]);
    const c = cards(w);
    await fire(c[0].element, "pointerdown", { clientX: 0, clientY: 0 });
    hit.el = c[1].element;
    await fire(c[1].element, "pointermove", { clientX: 60, clientY: 0 });
    await fire(c[1].element, "pointercancel", { clientX: 60, clientY: 0 });

    const afterCancel = new Event("click", { bubbles: true, cancelable: true });
    c[1].element.dispatchEvent(afterCancel);
    expect(afterCancel.defaultPrevented).toBe(false);
    // cancel 也不提交任何结果
    expect(w.emitted("select-many")).toBeUndefined();

    // ② 对照：同样一串动作走 pointerup，那次尾随 click 是要被吞掉的
    const w2 = mountGrid([item(1), item(2), item(3)]);
    const c2 = cards(w2);
    await fire(c2[0].element, "pointerdown", { clientX: 0, clientY: 0 });
    hit.el = c2[1].element;
    await fire(c2[1].element, "pointermove", { clientX: 60, clientY: 0 });
    await fire(c2[1].element, "pointerup", { clientX: 60, clientY: 0 });

    const afterUp = new Event("click", { bubbles: true, cancelable: true });
    c2[1].element.dispatchEvent(afterUp);
    expect(afterUp.defaultPrevented).toBe(true);
  });

  /**
   * 规则：根节点是 `v-if` 出来的，会在「空库 ↔ 有数据」之间反复出现/消失。
   * `watch(o.root)` 每次都要跟着换绑：**旧节点的监听必须摘掉**（否则泄漏），
   * **新节点必须挂得上且不重复挂**（否则拖拽滚动时灵时不灵）。
   */
  it("网格在空↔非空之间反复切换，touchmove 监听跟着根节点换绑", async () => {
    const w = mountGrid([item(1), item(2), item(3)]);
    const grid1 = w.find(".grid").element;

    // ① 有数据：正常拦截
    await fire(cards(w)[0].element, "pointerdown", { clientX: 0, clientY: 0 });
    hit.el = cards(w)[1].element;
    await fire(cards(w)[1].element, "pointermove", { clientX: 60, clientY: 0 });
    const tm1 = new Event("touchmove", { bubbles: true, cancelable: true });
    const pd1 = vi.spyOn(tm1, "preventDefault");
    grid1.dispatchEvent(tm1);
    expect(tm1.defaultPrevented).toBe(true);
    const baseline = pd1.mock.calls.length; // 首次挂载时被处理几次
    await fire(cards(w)[1].element, "pointerup", { clientX: 60, clientY: 0 });

    // ② 清空：网格消失，旧节点上的监听必须被摘掉
    const remSpy = vi.spyOn(grid1, "removeEventListener");
    await w.setProps({ items: [] });
    expect(w.find(".grid").exists()).toBe(false);
    expect(remSpy.mock.calls.some((call) => call[0] === "touchmove")).toBe(true);
    remSpy.mockRestore();

    // ③ 再填充：这是个**新节点**，监听必须重新挂上，且只挂一次
    const addSpy = vi.spyOn(HTMLElement.prototype, "addEventListener");
    try {
      await w.setProps({ items: [item(1), item(2), item(3)] });
      const grid2 = w.find(".grid").element;
      expect(grid2).not.toBe(grid1); // 确认确实是新节点，不是复用
      expect(addSpy.mock.calls.some((call) => call[0] === "touchmove")).toBe(true); // 挂上了

      await fire(cards(w)[0].element, "pointerdown", { clientX: 0, clientY: 0 });
      hit.el = cards(w)[1].element;
      await fire(cards(w)[1].element, "pointermove", { clientX: 60, clientY: 0 });
      const tm2 = new Event("touchmove", { bubbles: true, cancelable: true });
      const pd = vi.spyOn(tm2, "preventDefault");
      grid2.dispatchEvent(tm2);
      expect(tm2.defaultPrevented).toBe(true);

      // 换绑之后的处理次数必须和首次挂载**一模一样** —— 多出来就说明旧监听没摘干净（真泄漏）。
      expect(pd.mock.calls.length).toBe(baseline);
      // 并且**绝对值必须是 1**：曾经同时走"模板 @touchmove"和"模块内 bindTouch"两条路，
      // Vue 注册的是它自己的 invoker 包装函数，与 `onTouchMove` 不是同一个引用，
      // DOM 按 (type, callback, capture) 去重时**不会合并**，于是每个 touchmove 跑两次。
      // 现在注册由 `bindTouch` 独占（不再导出 `onTouchMove`），这里就该是 1。
      // 盯住绝对值的意义：哪天 `onTouchMove` 里加了非幂等逻辑，重复注册就会静默跑两遍。
      expect(pd.mock.calls.length).toBe(1);
      await fire(cards(w)[1].element, "pointerup", { clientX: 60, clientY: 0 });
    } finally {
      addSpy.mockRestore();
    }
  });

  /**
   * 规则：`onTouchMove` **不该出现在对外 API 上**。
   *
   * 重复注册的根因是"模块内 bindTouch 挂一份 + 调用方在模板上再挂一份"。
   * 光删掉封面墙那句 `@touchmove` 只是消掉症状 —— 只要这个导出还在，
   * 下一个人照样能凑出两份。把它从返回值里撤掉，才算**结构上不可能**。
   * 这条用例盯的就是"以后有人为了图方便又把它导出回去"。
   */
  it("useSwipeSelect 不再导出 onTouchMove（重复注册在结构上不可能发生）", () => {
    let api: Record<string, unknown> = {};
    const Host = defineComponent({
      setup() {
        api = useSwipeSelect({
          root: ref(null),
          selected: () => [],
          order: () => [],
          commit: () => {},
        }) as unknown as Record<string, unknown>;
        return () => h("div");
      },
    });

    const w = mount(Host);
    try {
      expect(Object.keys(api)).not.toContain("onTouchMove");
      // 对外承诺仍然是这几项（多一个少一个都要有人** consciously **改这条用例）
      expect(Object.keys(api).sort()).toEqual(
        ["cancel", "dragging", "end", "onPointerDown", "onPointerMove", "preview"],
      );
    } finally {
      w.unmount();
    }
  });
});

/** 造一个"真的能竖向滚"的祖先容器（jsdom 没有布局，只能桩）。 */
function makeScrollHost(): HTMLElement {
  const host = document.createElement("div");
  host.setAttribute("style", "overflow-y: auto");
  Object.defineProperty(host, "scrollHeight", { configurable: true, value: 1000 });
  Object.defineProperty(host, "clientHeight", { configurable: true, value: 300 });
  Object.defineProperty(host, "scrollTop", { configurable: true, writable: true, value: 0 });
  stubRect(host, 0, 300);
  document.body.appendChild(host);
  return host;
}

/**
 * 规则：`blockSelector` **不能把选项元素自身拦掉** —— 卡片根元素就是 `button`，
 * 一旦把 `button` 写进去，拖动**永远起不来**（症状是"滑动完全没反应"，极难排查）。
 * 这组用例把这条约定钉在代码上，而不只是写在注释里。
 */
describe("pressBlocked 起手拦截", () => {
  it("选项自身的标签写进 blockSelector 也不拦起手，选项内的控件照旧拦", () => {
    document.body.innerHTML = '<button class="card" data-id="1"><input /></button>';
    const card = document.querySelector(".card") as Element;
    const input = document.querySelector("input") as Element;

    // 旧规则（直接 closest(blockSelector)）会把卡片自己判成"要拦的控件"
    expect(card.closest("button")).toBe(card);
    expect(pressBlocked(card, "[data-id]", "button")).toBe(false);
    // 卡片内部的控件仍然要拦（否则按钮会被点两次）
    expect(pressBlocked(input, "[data-id]", "input, a")).toBe(true);
    // 没命中任何被拦控件时当然不拦
    expect(pressBlocked(card, "[data-id]", "input, a")).toBe(false);
    // 不在任何选项元素里 → 根本起不来拖动，按"拦掉"处理
    expect(pressBlocked(input, "li[data-ep-id]", "input")).toBe(true);
  });
});

/**
 * 规则：手势被系统打断（`pointercancel`）**不能**把半截选择提交上去。
 *
 * 真机上手指划到一半会被来电 / 系统返回手势 / 侧滑 / 切后台打断，派发的是
 * `pointercancel`。把它按"松手"处理，用户没做完的那半截就成了最终选择。
 * `cancel` 的语义是"当作没发生过"：不 `commit`、预览一起丢掉。
 */
describe("PosterGrid 手势被系统打断", () => {
  it("pointercancel 丢弃这一次拖动，不提交任何结果", async () => {
    const w = mountGrid([item(1), item(2), item(3), item(4)]);
    const c = cards(w);

    await fire(c[0].element, "pointerdown", { clientX: 0, clientY: 0 });
    hit.el = c[2].element;
    await fire(c[2].element, "pointermove", { clientX: 120, clientY: 0 });
    expect(pickedIds(w)).toEqual([1, 2, 3]); // 拖动中只是预览

    await fire(c[2].element, "pointercancel", { clientX: 120, clientY: 0 });

    expect(w.emitted("select-many")).toBeUndefined(); // 半截选择不能提交
    expect(pickedIds(w)).toEqual([]); // 预览也一起丢掉
    expect(w.find(".grid").classes()).not.toContain("dragging");

    // 打断之后再划过不该有任何作用
    await fire(c[3].element, "pointermove", { clientX: 180, clientY: 0 });
    expect(pickedIds(w)).toEqual([]);
  });

  /** 对照：同一串动作走 `pointerup` 是要提交的 —— 防止两条路径被合并回一个处理函数。 */
  it("同一串动作走 pointerup 则正常提交（与 cancel 对照）", async () => {
    const w = mountGrid([item(1), item(2), item(3), item(4)]);
    const c = cards(w);

    await fire(c[0].element, "pointerdown", { clientX: 0, clientY: 0 });
    hit.el = c[2].element;
    await fire(c[2].element, "pointermove", { clientX: 120, clientY: 0 });
    await fire(c[2].element, "pointerup", { clientX: 120, clientY: 0 });

    expect(w.emitted("select-many")).toEqual([[[1, 2, 3]]]);
  });
});