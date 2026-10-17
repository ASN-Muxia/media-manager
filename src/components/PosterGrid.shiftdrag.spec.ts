/**
 * 回归：**用 Shift 多选时，不能把聚合里的电影拖出来**。
 *
 * 用户原话：「现在在使用shift多选的时候会把已经在聚合里的电影拖拽出来，复现并解决这个问题」。
 *
 * 复现到的机制（本文件早期的探针已确认）：
 *   - 聚合模式下 selectable = selecting || grouping 为 **true**（点击要能选中），
 *     而 selectMode = selectable && !grouping 为 **false**（拖动 = 搬运）；
 *   - 于是 Shift 点选把卡片放进 selected，接着只要指针移动超过
 *     DRAG_THRESHOLD_PX（6px —— 鼠标下按一下手必然会抖过它）就被判成拖拽，
 *     松手落在哪张卡上就整批 move 过去；
 *   - 用户看到的就是「Shift 多选的时候，聚合里的电影被拖出来了」。
 *
 * 修法：**带修饰键的按下一律不起拖**（Shift / Ctrl / Cmd = 选择；不按修饰键的拖动 = 搬动），
 * 与 Windows 资源管理器一致。触摸没有修饰键，不受影响。
 */
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { mount, type VueWrapper } from "@vue/test-utils";
import { nextTick } from "vue";
import PosterGrid from "./PosterGrid.vue";
import type { MediaItem } from "../api/commands";

function item(id: number, over: Partial<MediaItem> = {}): MediaItem {
  return {
    id, type: "movie", title: "M" + id, year: 2020, tmdb_id: null,
    poster_path: null, backdrop_path: null, overview: null, vote_average: null,
    genre: null, file_path: null, file_name: null, episode_count: 0,
    added_at: 1, display_title: "M" + id, parent_id: null, child_count: 0,
    sort_order: 0, depth: 0, ...over,
  };
}

/** 一个聚合（父 1 + 两个子 11/12）+ 一个独立条目 9 当落点。 */
const ITEMS: MediaItem[] = [
  item(1, { child_count: 2 }),
  item(11, { parent_id: 1, depth: 1 }),
  item(12, { parent_id: 1, depth: 1 }),
  item(9),
];

let wrappers: VueWrapper[] = [];
const geom: { el: Element | null } = { el: null };

function mountGrid(props: Record<string, unknown>): VueWrapper {
  const w = mount(PosterGrid, { props: { items: ITEMS, ...props }, attachTo: document.body });
  wrappers.push(w);
  return w;
}

function cells(w: VueWrapper) {
  return w.findAll(".cell");
}

function cellById(w: VueWrapper, id: number) {
  const c = cells(w).find((n) => Number(n.attributes("data-id")) === id);
  if (!c) throw new Error("找不到卡片 " + id);
  return c;
}

function pointer(target: EventTarget, type: string, extra: Record<string, unknown> = {}): void {
  const init = {
    bubbles: true, cancelable: true, button: 0, pointerType: "mouse",
    clientX: 0, clientY: 0, shiftKey: false, ctrlKey: false, metaKey: false, ...extra,
  };
  let ev: Event;
  if (typeof PointerEvent === "function") ev = new PointerEvent(type, init as PointerEventInit);
  else {
    const { bubbles, cancelable, ...rest } = init;
    ev = Object.assign(new Event(type, { bubbles, cancelable }), rest);
  }
  target.dispatchEvent(ev);
  // 跟手 / 落点挂在 window 上（组件挂的是 window），元素上派发不会冒泡到 window。
  if (target !== window && (type === "pointermove" || type === "pointerup" || type === "pointercancel")) {
    const { bubbles: _b, cancelable: _c, ...rest } = init;
    const clone = typeof PointerEvent === "function"
      ? new PointerEvent(type, { ...(init as PointerEventInit), bubbles: false })
      : Object.assign(new Event(type, { bubbles: false, cancelable: true }), rest);
    window.dispatchEvent(clone);
  }
}

beforeEach(() => {
  geom.el = null;
  Object.defineProperty(document, "elementFromPoint", {
    configurable: true, writable: true, value: () => geom.el,
  });
});

afterEach(() => {
  for (const w of wrappers) w.unmount();
  wrappers = [];
  document.body.innerHTML = "";
});

/** 一次完整拖放：按住 fromId（可带修饰键）→ 越过阈值 → 落到 toId → 松手。 */
async function dragCard(
  w: VueWrapper,
  fromId: number,
  toId: number,
  mods: Record<string, unknown> = {},
): Promise<void> {
  const from = cellById(w, fromId);
  pointer(from.element, "pointerdown", { clientX: 0, clientY: 0, ...mods });
  await nextTick();
  pointer(from.element, "pointermove", { clientX: 40, clientY: 0 });
  await nextTick();
  geom.el = cellById(w, toId).element;
  pointer(window, "pointermove", { clientX: 268, clientY: 150 });
  await nextTick();
  pointer(window, "pointerup", { clientX: 268, clientY: 150 });
  await nextTick();
}

describe("Shift 多选不应把聚合成员拖出来", () => {
  it("聚合模式下 Shift 按住并拖动：不发出 move（Shift = 选择，不是搬动）", async () => {
    const w = mountGrid({ selectable: true, grouping: true, selected: [] });
    await dragCard(w, 11, 9, { shiftKey: true });
    expect(w.emitted("move"), "Shift 拖动不该搬动任何东西").toBeUndefined();
  });

  it("聚合模式下 Ctrl 按住并拖动：同样不发出 move", async () => {
    const w = mountGrid({ selectable: true, grouping: true, selected: [] });
    await dragCard(w, 11, 9, { ctrlKey: true });
    expect(w.emitted("move")).toBeUndefined();
  });

  it("聚合模式下 Cmd(meta) 按住并拖动：同样不发出 move", async () => {
    const w = mountGrid({ selectable: true, grouping: true, selected: [] });
    await dragCard(w, 11, 9, { metaKey: true });
    expect(w.emitted("move")).toBeUndefined();
  });

  /**
   * 完整场景：已经 Shift 选好一批（selected 里有多个），**再 Shift 按住**其中一张拖
   * —— 这正是「用 Shift 多选却把组里电影拖出来」那条。
   */
  it("已勾选一批后，再次 Shift 按住拖动：不发出 move", async () => {
    const w = mountGrid({ selectable: true, grouping: true, selected: [11, 12] });
    await dragCard(w, 11, 9, { shiftKey: true });
    expect(w.emitted("move"), "带 Shift 的按下一律不起拖").toBeUndefined();
  });

  /** 反向：**不按修饰键**的拖动仍然要能搬（别把正常聚合拖坏了）。 */
  it("反向回归：不按修饰键的拖动，照旧发出 move", async () => {
    const w = mountGrid({ selectable: true, grouping: true, selected: [] });
    await dragCard(w, 11, 9);
    expect(w.emitted("move")).toEqual([[{ ids: [11], parentId: 9, index: null }]]);
  });

  /** 反向：勾选了一批之后，不按修饰键拖其中一张 = 整批搬（原有批量行为保留）。 */
  it("反向回归：已勾选一批后，不按修饰键拖动 = 整批搬", async () => {
    const w = mountGrid({ selectable: true, grouping: true, selected: [11, 12] });
    await dragCard(w, 11, 9);
    expect(w.emitted("move")).toEqual([[{ ids: [11, 12], parentId: 9, index: null }]]);
  });

  /** 「选择」模式下拖动本来就不该搬（那条路径归滑动多选），补一条防回归。 */
  it("「选择」模式下拖动：不发出 move（归滑动多选）", async () => {
    const w = mountGrid({ selectable: true, grouping: false, selected: [11, 12] });
    await dragCard(w, 11, 9);
    expect(w.emitted("move")).toBeUndefined();
  });
});
