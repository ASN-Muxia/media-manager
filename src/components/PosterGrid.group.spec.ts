/**
 * 封面墙的**分组聚合**与**无级缩放**用例。
 *
 * 为什么单独一个文件：`PosterGrid.spec.ts` 里那 30 条全是滑动多选，
 * 两组关注点不同、前置桩也不同（这里要造层级数据 + HTML5 拖拽事件），
 * 混在一起只会互相干扰。这里只测新增的两块能力，
 * 并额外钉一条「分组没有把滑动多选弄坏」的回归。
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { mount, type VueWrapper } from "@vue/test-utils";
import { nextTick } from "vue";
import PosterGrid from "./PosterGrid.vue";
// Vite 的 `?raw`：把组件源码当字符串拿进来（tsc 里由 `vite/client` 声明）
import posterGridSource from "./PosterGrid.vue?raw";
import posterCardSource from "./PosterCard.vue?raw";
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

let wrappers: VueWrapper[] = [];

/**
 * 挂一块网格。
 *
 * `selected` 单独放在第二个参数（本文件里用得最多），其余 props 走第三个。
 * 默认**不带** `selectable`：默认模式（不勾选、不聚合）就是"按住即拖"，
 * 正是用户要的那个默认行为。
 */
function mountGrid(
  items: MediaItem[],
  second: number[] | Record<string, unknown> = [],
  third: Record<string, unknown> = {},
): VueWrapper {
  // 第二个参数既能直接给 props（老写法），也能先给勾选集合再给 props。
  const selected = Array.isArray(second) ? second : [];
  const props = Array.isArray(second) ? third : second;
  const w = mount(PosterGrid, { props: { items, selected, ...props }, attachTo: document.body });
  wrappers.push(w);
  return w;
}

/** 旧名字（老用例里 `mountGrid(items, { props })` 的写法）保持可用。 */
const mountGridP = mountGrid;

function cells(w: VueWrapper) {
  return w.findAll(".cell");
}

/** 一行里的卡片 id（`data-id` 写在 `.cell` 上）。 */
function cellIds(w: VueWrapper): number[] {
  return cells(w).map((n) => Number(n.attributes("data-id")));
}

/**
 * 造一个 HTML5 拖拽事件。
 *
 * 组件已经**不再使用**原生 HTML5 拖拽（见 PosterGrid.vue 里
 * 「剧集聚合：指针驱动的拖放」一节的四条理由），这里只留一条反向用例：
 * 外部乱入的原生 drop 不该凭空搬数据。
 */
function dragEv(type: string, extra: Record<string, unknown> = {}): Event {
  const ev = new Event(type, { bubbles: true, cancelable: true });
  Object.assign(ev, {
    dataTransfer: { effectAllowed: "", dropEffect: "", setData: vi.fn(), getData: () => "" },
    clientX: 0,
    clientY: 0,
    ...extra,
  });
  return ev;
}

/** 派发一次拖拽事件并等一次渲染。 */
async function fireDrag(target: EventTarget, type: string): Promise<void> {
  target.dispatchEvent(dragEv(type));
  await nextTick();
}

/**
 * 落点命中桩。
 *
 * 组件用 `document.elementFromPoint` 定落点（没有它就没法脱离"某个可拖元素"
 * 这套语义）。jsdom 没有布局引擎，所以几何命中在用例里只能靠这个桩：
 * 设了 `geom.el` 就返回它，`null` 表示"指针落在空白处"。
 */
const geom: { el: Element | null } = { el: null };

/**
 * 一次完整的拖放：按住 `from` 那张卡 → 越过阈值 → 落到 `to`（或空白）。
 *
 * `to` 传 `.rootzone` 的包装表示"放回顶层"。
 */
async function dragTo(
  w: VueWrapper,
  from: number,
  to: number | "root" | null,
): Promise<void> {
  const fromEl = cells(w)[from].element;
  // 先按住并越过阈值 → 「放回顶层」区这时才渲染出来，之后才取得到它
  pointer(fromEl.querySelector(".card") ?? fromEl, "pointerdown", { clientX: 0, clientY: 0 });
  await nextTick();
  pointer(fromEl, "pointermove", { clientX: 40, clientY: 0 });
  await nextTick();

  // 落点由 elementFromPoint 决定；「放回顶层」区是 fixed 的，用矩形命中
  if (to === "root") {
    const rz = w.find(".rootzone").element as HTMLElement;
    Object.defineProperty(rz, "getBoundingClientRect", {
      configurable: true,
      value: () => ({ x: 0, y: 0, top: 0, bottom: 40, left: 0, right: 1280, width: 1280, height: 40, toJSON: () => ({}) }),
    });
    geom.el = rz;
  } else {
    geom.el = to === null ? null : cells(w)[to].element;
  }
  pointer(window, "pointermove", { clientX: 100, clientY: 20 });
  await nextTick();
  pointer(window, "pointerup", { clientX: 100, clientY: 20 });
  await nextTick();
}

/** 造一个指针事件（jsdom 没有 PointerEvent 时退回 Event + 贴字段）。 */
function pointer(target: EventTarget, type: string, extra: Record<string, unknown> = {}): void {
  const init: PointerEventInit = {
    bubbles: true, cancelable: true, button: 0, pointerType: "mouse", clientX: 0, clientY: 0,
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
  // ⚠️ 跟手 / 落点由组件挂在 window 上（指针要能划出网格、划到窗口顶部）。
  // 在卡片元素上派发之后事件冒泡到 document 就停了，**不会**再走到 window，
  // 所以这三个类型必须再往 window 上补一次。
  // `bubbles` / `cancelable` 在 Event 上是只读访问器，只能交给构造函数。
  if (target !== window && (type === "pointermove" || type === "pointerup" || type === "pointercancel")) {
    const { bubbles: _b, cancelable: _c, ...rest } = init;
    const clone = typeof PointerEvent === "function"
      ? new PointerEvent(type, { ...init, bubbles: false })
      : Object.assign(new Event(type, { bubbles: false, cancelable: true }), rest);
    window.dispatchEvent(clone);
  }
}

beforeEach(() => {
  geom.el = null;
  Object.defineProperty(document, "elementFromPoint", {
    configurable: true,
    writable: true,
    value: () => geom.el,
  });
});

afterEach(() => {
  for (const w of wrappers) w.unmount();
  wrappers = [];
  document.body.innerHTML = "";
});

/* ==================== 分组的渲染 ==================== */

describe("PosterGrid 分组渲染", () => {
  it("没有分组时每张卡片一格，与改动前完全一致", () => {
    const w = mountGridP([item(1), item(2), item(3)]);
    expect(cells(w)).toHaveLength(3);
    expect(cellIds(w)).toEqual([1, 2, 3]);
    // 没有子级的卡片不给展开控件、不给解除分组按钮
    expect(w.findAll(".ops")).toHaveLength(0);
  });

  /** 刚拖出来的分组必须**默认展开**：否则子项凭空消失，用户会以为片子丢了。 */
  it("有子级的卡片默认展开，子项紧跟在父项之后", () => {
    const w = mountGrid([
      item(1, { child_count: 1 }),
      item(2, { parent_id: 1 }),
    ]);
    expect(cellIds(w)).toEqual([1, 2]);
    expect(cells(w)[1].attributes("data-depth")).toBe("1");
    // 整行的分组条：说明后面这批卡片属于上面那张
    expect(w.find(".groupbar").exists()).toBe(true);
    expect(w.find(".groupbar").text()).toContain("Movie 1");
  });

  /* ==================== 操作条的排布（不许遮挡信息） ==================== */
  /*
   * 用户截图报的问题：展开 / 解除分组两个按钮压在海报下方的**年份 / 评分行**上，
   * 把 `7.5` 盖成 `7.`。根因是 `.ops` 绝对定位在 `.cell` 的底边，而 `.cell`
   * 不只有海报、还包含标题与年份。
   *
   * jsdom 没有布局引擎，量不到真实坐标，所以这里钉的是**源码契约 + DOM 结构**：
   * 契约那条确保"别再浮起来"，结构那条确保"它排在卡片内容之后"。
   */

  it("操作条走常规文档流，不再绝对定位（否则会压住年份 / 评分）", () => {
    const src = posterGridSource;
    // 把 .ops 的规则块单独切出来，只看它自己的声明
    const block = /\.ops\s*\{([^}]*)\}/.exec(src)?.[1];
    // 样式块本身必须存在（否则下面两条断言会对着空串空转，永远绿）
    expect(block).toBeTypeOf("string");
    expect(block).not.toContain("position: absolute");
    expect(block).not.toContain("bottom:");
    // 老实现那两条"浮起来"的定位声明一个都不能回来
    expect(src).not.toContain(".ops {\n  position: absolute");
  });

  /** 收起态必须一眼看见（还有内容被藏着）：主色描边；展开后不再喊。 */
  it("收起态的主色描边与展开态的中性色（源码契约）", () => {
    const src = posterGridSource;
    expect(src).toMatch(/\.op-expand\s*\{[^}]*var\(--accent\)/);
    expect(src).toMatch(/\.op-expand\.open\s*\{[^}]*var\(--border-strong\)/);
  });

  /**
   * 既是聚合、本身又挂在别人下面：**同一张卡上两个按钮并排**。
   * 这是最容易挤坏的一种格子，必须两个都在、且都能点。
   *
   * 注意：展开按钮的显示条件是 `row.hasChildren`（**建树算出来的**，
   * 来自当前列表里的真实父子关系），不是 `MediaItem.child_count`——
   * 后者是库里的记录数，可能因为筛选而大于当前可见子级数。所以这里
   * 必须给出真正的三层结构，而不是只把 `child_count` 填成 3。
   */
  it("既展开又移出：同一格里两个按钮并排，各发各的事件", async () => {
    const w = mountGrid([
      item(1, { child_count: 1 }), // 顶层
      item(2, { parent_id: 1, child_count: 1 }), // 子项，自己也有子级
      item(3, { parent_id: 2 }), // 孙级
    ]);
    const cell = cells(w)[1];
    expect(cell.findAll(".op")).toHaveLength(2);
    expect(cell.find(".op-expand").exists()).toBe(true);
    expect(cell.find(".op-ungroup").exists()).toBe(true);

    // 两个按钮互不串：点移出只发 move，点展开只改展开态
    await cell.find(".op-ungroup").trigger("click");
    expect(w.emitted("move")).toEqual([[{ ids: [2], parentId: null, index: null }]]);

    await cell.find(".op-expand").trigger("click");
    expect(w.emitted("move")).toHaveLength(1); // 没有多发一次 move
    expect(cellIds(w)).toContain(2); // 卡片自己还在
  });

  /** 操作条不能把卡片本身变成"不可点"：按钮的点击必须被 .stop 拦住。 */
  it("点操作条不会连带触发卡片的打开 / 勾选", async () => {
    const w = mountGrid([
      item(1, { child_count: 1 }),
      item(2, { parent_id: 1, child_count: 1 }),
      item(3, { parent_id: 2 }),
    ]);
    // ⚠️ 先点子项那格的「移出」，**再**点父项的展开按钮：
    // 反过来会把子项收起来，`cells(w)[1]` 当场就取不到了（折叠不渲染）。
    await cells(w)[1].find(".op-ungroup").trigger("click");
    await cells(w)[0].find(".op-expand").trigger("click");
    expect(w.emitted("open")).toBeUndefined();
    expect(w.emitted("select")).toBeUndefined();
  });

  it("操作条排在卡片（海报 + 标题 + 评分）之后，且在 .cell 内部", () => {
    const w = mountGrid([item(1, { child_count: 1 }), item(2, { parent_id: 1 })]);
    const cell = cells(w)[0];
    const card = cell.find(".card");
    const ops = cell.find(".ops");

    expect(card.exists()).toBe(true);
    expect(ops.exists()).toBe(true);
    // DOM 顺序：海报卡片在前、操作条在后 —— 常规流里后者必然在下方，
    // 不会像绝对定位那样回叠到前者身上。
    const kids = [...cell.element.children];
    expect(kids.indexOf(card.element)).toBeLessThan(kids.indexOf(ops.element));
  });

  /** 按钮要自解释：只有「▸ 2」看不出是展开还是两个版本，所以必须带文字。 */
  it("展开按钮带「展开 / 折叠」文字与子项数，解除分组带「移出」文字", async () => {
    const w = mountGrid([item(1, { child_count: 2 }), item(2, { parent_id: 1 })]);
    // 新出现的分组默认是**展开**的（否则子项凭空消失），所以初始文案是「折叠」
    const expand = cells(w)[0].find(".op-expand");
    expect(expand.text()).toContain("折叠");
    expect(expand.find(".op-n").text()).toBe("2");
    // 展开态是可见状态：`aria-expanded` 与主色描边一起走
    expect(expand.attributes("aria-expanded")).toBe("true");
    expect(expand.classes()).toContain("open");

    // 收起之后文案与状态一起翻过来
    await expand.trigger("click");
    const collapsed = cells(w)[0].find(".op-expand");
    expect(collapsed.text()).toContain("展开");
    expect(collapsed.attributes("aria-expanded")).toBe("false");
    expect(collapsed.classes()).not.toContain("open");

    // 再展开回来，两个方向都要能读
    await collapsed.trigger("click");
    expect(cells(w)[0].find(".op-expand").text()).toContain("折叠");

    // 子项那格只有「移出」，没有展开按钮
    const ungroup = cells(w)[1].find(".op-ungroup");
    expect(ungroup.exists()).toBe(true);
    expect(ungroup.text()).toContain("移出");
    expect(cells(w)[1].find(".op-expand").exists()).toBe(false);
  });

  it("折叠后子项不再渲染，再点一次又回来", async () => {
    const w = mountGrid([
      item(1, { child_count: 1 }),
      item(2, { parent_id: 1 }),
    ]);
    expect(cellIds(w)).toEqual([1, 2]);

    // 父卡片上的第一个按钮就是展开 / 折叠
    await cells(w)[0].findAll(".op")[0].trigger("click");
    expect(cellIds(w)).toEqual([1]);
    expect(w.find(".groupbar").exists()).toBe(false);

    await cells(w)[0].findAll(".op")[0].trigger("click");
    expect(cellIds(w)).toEqual([1, 2]);
  });

  /** 手动折叠过的分组不该被"新分组自动展开"的逻辑重新拉开。 */
  it("手动折叠之后，列表刷新也不会把它自动展开", async () => {
    const w = mountGrid([
      item(1, { child_count: 1 }),
      item(2, { parent_id: 1 }),
    ]);
    await cells(w)[0].findAll(".op")[0].trigger("click");
    expect(cellIds(w)).toEqual([1]);

    // 模拟「移动完成后列表重拉」：数据还是同一批
    await w.setProps({ items: [item(1, { child_count: 1 }), item(2, { parent_id: 1 })] });
    expect(cellIds(w)).toEqual([1]);
  });

  /** 父节点不在当前列表里（被筛选掉了）时按顶层处理，不能整片消失。 */
  it("父节点不在列表里的条目按顶层渲染，不会整片消失", () => {
    const w = mountGridP([item(7, { parent_id: 999 }), item(8)]);
    expect(cellIds(w)).toEqual([7, 8]);
  });
});

/* ============ 组内次序：手工排过的组 vs 跟随顶栏排序 ============ */
/* `nodes` 喂给 `buildTree`，后者按 `(sort_order, title, id)` 排同级。谁会拿到
   「库里真实的 `sort_order`」由一个显式标记决定（`manualOrderGroups`）：

   - 组在名单里（用户亲手用详情页的 ↑↓ 排过）→ 用 `MediaItem.sort_order`，
     那是后端 `move_nodes_sync` 的 `(sort_order, id)` 同一把尺子；
   - 组不在名单里（默认）→ 用**列表下标**，也就是顶栏排序设置算出来的先后。

   旧实现用「这一桶里存在 `sort_order > 0`」当「手工排过」的启发式 —— 那是错的：
   任何一次 `move_nodes_sync`（含在墙上拖动聚合）都会把目标同级整组重编号成 0..n-1，
   实测真实库里连从没排过的组都是 0,1,2,… 于是墙上组内次序永远跟着历史重编号走，
   顶栏排序根本不生效。改成显式标记之后，用下标当 sort_order 也不再是「漂移」。 */

describe("PosterGrid 组内次序（手工标记）", () => {
  /**
   * 已标记的组：库内 `sort_order` 与列表先后**相反**时，必须以后者为准。
   * 用下标当 sort_order 会渲染成 `[1,2,3]`（墙跟着列表先后走）—— 那就丢了手工次序。
   */
  it("已标记的组：按库内 sort_order 排（手工次序不丢）", () => {
    const w = mountGrid(
      [
        item(1, { child_count: 2 }),
        item(2, { parent_id: 1, sort_order: 2 }),
        item(3, { parent_id: 1, sort_order: 1 }),
      ],
      { manualOrderGroups: [1] },
    );
    expect(cellIds(w)).toEqual([1, 3, 2]);
  });

  /**
   * 未标记的组（默认）：组内次序 = **列表下标**，也就是顶栏排序算出来的先后。
   * 库内 `sort_order` 故意与列表先后相反 —— 它一个字都不该影响渲染。
   */
  it("未标记的组：组内次序跟列表下标（顶栏序），不受库内 sort_order 影响", () => {
    const w = mountGrid([
      item(1, { child_count: 2 }),
      item(2, { parent_id: 1, sort_order: 2 }),
      item(3, { parent_id: 1, sort_order: 1 }),
    ]);
    // 没有 manualOrderGroups → 就是传入顺序；按库内 sort_order 会变成 [1, 3, 2]
    expect(cellIds(w)).toEqual([1, 2, 3]);
  });

  /**
   * 判据必须**按父级**看，不能全局一刀切：一个库里常常既有手工排过的组，
   * 也有没动过的组。全局判断会让没动过的那组也跟着库内 `sort_order` 走
   * （掉进标题兜底排序）→ 次序凭空变一遍。
   */
  it("混合库：标记过的那组按库内次序，没标记的那组仍按列表下标", () => {
    const w = mountGrid(
      [
        item(1, { child_count: 1 }),
        item(2, { parent_id: 1, sort_order: 5 }),
        item(10, { child_count: 2 }),
        item(12, { parent_id: 10, sort_order: 0 }),
        item(11, { parent_id: 10, sort_order: 0 }),
      ],
      { manualOrderGroups: [1] },
    );
    // 组 1 在名单里 → 用库内值（5 > 下标）；组 10 不在 → 用下标（12 在前）
    expect(cellIds(w)).toEqual([1, 2, 10, 12, 11]);
  });

  /**
   * 保底：标记了手工次序、但库内 `sort_order` 缺失（老后端 / 测试桩不给）时
   * 退回列表下标，稳定且不抛错。
   */
  it("已标记但 sort_order 缺失：退回列表下标，不抛错", () => {
    const w = mountGrid(
      [
        item(1, { child_count: 2 }),
        item(3, { parent_id: 1 }),
        item(2, { parent_id: 1 }),
      ],
      { manualOrderGroups: [1] },
    );
    expect(cellIds(w)).toEqual([1, 3, 2]); // 按标题会变成 [1,2,3]
  });
});

/* ==================== 拖拽聚合（指针驱动） ==================== */
/*
 * 整个拖放已经不用原生 HTML5 拖拽，改为指针驱动（见 PosterGrid.vue）。
 * 这一组钉住的行为契约与修复前**完全一致**：能不能放由 canDrop 判、
 * 非法落点给文字原因、松手才发一次 move；变的只是驱动它的手势。
 */

describe("PosterGrid 拖拽聚合", () => {
  it("把一张卡拖到另一张卡上 → 交出 {ids, parentId}", async () => {
    const w = mountGridP([item(1), item(2)]);
    // 按住之后、还没跨出去：还没起拖
    pointer(cells(w)[0].element, "pointerdown", { clientX: 0, clientY: 0 });
    await nextTick();
    expect(w.find(".rootzone").exists()).toBe(false);

    // 越过阈值 → 起拖，「放回顶层」区出现
    pointer(cells(w)[0].element, "pointermove", { clientX: 40, clientY: 0 });
    await nextTick();
    expect(w.find(".rootzone").exists()).toBe(true);

    geom.el = cells(w)[1].element;
    pointer(window, "pointermove", { clientX: 268, clientY: 150 });
    await nextTick();
    expect(cells(w)[1].classes()).toContain("over");

    pointer(window, "pointerup", { clientX: 268, clientY: 150 });
    await nextTick();
    expect(w.emitted("move")).toEqual([[{ ids: [1], parentId: 2, index: null }]]);
    // 放完就收起顶层放置区
    expect(w.find(".rootzone").exists()).toBe(false);
  });

  /** 非法放置：拖到自己身上。必须**不发请求**并把原因说给用户。 */
  it("拖到自己身上：不 emit，并给出原因", async () => {
    const w = mountGridP([item(1), item(2)]);
    await dragTo(w, 0, 0);

    expect(w.emitted("move")).toBeUndefined();
    expect(w.find(".reject").text()).toContain("自己");
    expect(cells(w)[0].classes()).toContain("rejected");
  });

  /** 非法放置：拖进自己的子级里（会成环）。 */
  it("拖进自己的子级里：不 emit，并说明会形成循环", async () => {
    const w = mountGrid([
      item(1, { child_count: 1 }),
      item(2, { parent_id: 1 }),
    ]);
    await dragTo(w, 0, 1);

    expect(w.emitted("move")).toBeUndefined();
    expect(w.find(".reject").text()).toContain("循环");
  });

  /** 非法放置：超过 4 层。 */
  it("超过层数上限：不 emit，并报出具体层数", async () => {
    const chain = [
      item(1, { child_count: 1 }),
      item(2, { parent_id: 1, child_count: 1 }),
      item(3, { parent_id: 2, child_count: 1 }),
      item(4, { parent_id: 3 }),
      item(5),
    ];
    const w = mountGrid(chain);
    // 5 挂到第 4 层（id=4）下面会变成第 5 层，超限
    await dragTo(w, 4, 3);

    expect(w.emitted("move")).toBeUndefined();
    expect(w.find(".reject").text()).toContain("层级最多");
  });

  /** 放到第 4 层（depth 3）是允许的，别把上限算错成"最多 3 层"。 */
  it("放到第 4 层仍然放行（上限是 4 层，不是 3 层）", async () => {
    const w = mountGrid([
      item(1, { child_count: 1 }),
      item(2, { parent_id: 1, child_count: 1 }),
      item(3, { parent_id: 2 }),
      item(9),
    ]);
    await dragTo(w, 3, 2);

    expect(w.emitted("move")).toEqual([[{ ids: [9], parentId: 3, index: null }]]);
  });

  /** 解除分组：拖到「放回顶层」。 */
  it("拖到「放回顶层」= 解除分组（parentId 为 null）", async () => {
    const w = mountGrid([
      item(1, { child_count: 1 }),
      item(2, { parent_id: 1 }),
    ]);
    await dragTo(w, 1, "root");

    expect(w.emitted("move")).toEqual([[{ ids: [2], parentId: null, index: null }]]);
  });

  /**
   * 松在空白处 = 取消这一次拖拽，什么都不发。
   *
   * 拖放界面的通行语义：落点必须明确。这条同时兜住"手一抖就搬家"。
   */
  it("松在空白处：发出 0 次 move，也不报错", async () => {
    const w = mountGridP([item(1), item(2)]);
    await dragTo(w, 0, null);

    expect(w.emitted("move")).toBeUndefined();
    expect(w.find(".reject").exists()).toBe(false);
  });

  /**
   * 用户报的场景（禁止光标的真正来源）：**从海报图片上**按下也要能起拖。
   *
   * `<img>` 天生 `draggable`，浏览器会把这一次手势认成"拖一张图片"——
   * 载荷是图片地址、光标是禁止图标、放下什么都不发生。所以图片必须先被
   * 关掉可拖（模板 + CSS 两道），这一次手势才归组件。
   */
  it("从海报 <img> 上按下：仍然起拖并发出 move（图片不再抢走手势）", async () => {
    // 必须真的有图：没海报时卡片画的是占位块，根本不存在 <img> 这个拖拽源
    const w = mountGrid([item(1, { poster_path: "/a.jpg" }), item(2, { poster_path: "/b.jpg" })]);
    const img = cells(w)[0].find("img").element;
    expect(img).toBeTruthy();
    expect((img as HTMLElement).draggable).toBe(false);

    pointer(img, "pointerdown", { clientX: 0, clientY: 0 });
    pointer(cells(w)[0].element, "pointermove", { clientX: 40, clientY: 0 });
    await nextTick();
    expect(w.find(".rootzone").exists()).toBe(true);

    geom.el = cells(w)[1].element;
    pointer(window, "pointermove", { clientX: 268, clientY: 150 });
    pointer(window, "pointerup", { clientX: 268, clientY: 150 });
    await nextTick();
    expect(w.emitted("move")).toEqual([[{ ids: [1], parentId: 2, index: null }]]);
  });

  /**
   * 同上，但按的是**标题文字**：文字可选中的话，浏览器起的是文本拖拽，
   * 光标同样是禁止图标。所以卡片整体 `user-select: none`。
   */
  it("从标题文字上按下：仍然起拖并发出 move（文本拖拽不再抢走手势）", async () => {
    const w = mountGrid([item(1), item(2)]);
    const title = cells(w)[0].find(".title").element;
    pointer(title, "pointerdown", { clientX: 0, clientY: 0 });
    pointer(cells(w)[0].element, "pointermove", { clientX: 40, clientY: 0 });
    await nextTick();
    expect(w.find(".rootzone").exists()).toBe(true);

    geom.el = cells(w)[1].element;
    pointer(window, "pointermove", { clientX: 268, clientY: 150 });
    pointer(window, "pointerup", { clientX: 268, clientY: 150 });
    await nextTick();
    expect(w.emitted("move")).toEqual([[{ ids: [1], parentId: 2, index: null }]]);
  });

  /** 跟手预览：拖拽期间必须有"正在搬什么"的可见反馈（自绘拖拽没有系统拖影）。 */
  it("拖拽期间显示跟手预览，松手后消失", async () => {
    const w = mountGrid([item(1), item(2)]);
    pointer(cells(w)[0].element, "pointerdown", { clientX: 0, clientY: 0 });
    pointer(cells(w)[0].element, "pointermove", { clientX: 40, clientY: 0, pointerType: "mouse" });
    await nextTick();
    const ghost = w.find(".ghost");
    expect(ghost.exists()).toBe(true);
    expect(ghost.text()).toContain("Movie 1");

    pointer(window, "pointercancel", {});
    await nextTick();
    expect(w.find(".ghost").exists()).toBe(false);
  });

  /** 解除分组：子项卡片上的 ⤒ 按钮（触摸 / 不想拖的时候用）。 */
  it("子项卡片上的「解除分组」按钮同样挂回顶层", async () => {
    const w = mountGrid([
      item(1, { child_count: 1 }),
      item(2, { parent_id: 1 }),
    ]);
    const ops = cells(w)[1].findAll(".op");
    await ops[ops.length - 1].trigger("click");
    expect(w.emitted("move")).toEqual([[{ ids: [2], parentId: null, index: null }]]);
  });

  /** 勾选即批量：拖已勾选的卡片 = 整批一起搬。 */
  it("多选模式下拖已勾选的卡片 → 整批一起搬", async () => {
    const w = mountGridP([item(1), item(2), item(3)], { selected: [1, 2] });
    await dragTo(w, 0, 2);
    expect(w.emitted("move")).toEqual([[{ ids: [1, 2], parentId: 3, index: null }]]);
  });

  /**
   * 勾选即批量，**判据只有 `selected`，不看 `selectable`**：聚合不必先进
   * 「选择」模式。不在「选择」模式时遗留勾选的惊吓由 `App.vue` 的
   * `enterGrouping` / `exitGrouping` 清空勾选来防。
   */
  it("聚合模式下勾选 2 张以上 → 拖其中一张即整批搬（不必先进选择模式）", async () => {
    const w = mountGridP([item(1), item(2), item(3)], { grouping: true, selected: [1, 2] });
    await dragTo(w, 0, 2);
    expect(w.emitted("move")).toEqual([[{ ids: [1, 2], parentId: 3, index: null }]]);
  });

  /** 反向判据：拖的是**没被勾上**的那张，就只搬它自己。 */
  it("只拖未勾选的单张 → 只搬这一张", async () => {
    const w = mountGridP([item(1), item(2), item(3)], { grouping: true, selected: [2, 3] });
    await dragTo(w, 0, 2);
    const payload = w.emitted("move")?.[0]?.[0] as { ids: number[] } | undefined;
    expect(payload?.ids).toEqual([1]);
  });

  /** 整批解除分组：拖到「放回顶层」，`parentId` 为 `null`。 */
  it("聚合模式下整批拖到「放回顶层」→ parentId 为 null", async () => {
    const w = mountGridP([item(1, { child_count: 2 }), item(2, { parent_id: 1 }), item(3, { parent_id: 1 })],
      { grouping: true, selected: [2, 3] },
    );
    await dragTo(w, 1, "root");
    expect(w.emitted("move")).toEqual([[{ ids: [2, 3], parentId: null, index: null }]]);
  });

  /**
   * 外部乱入的原生拖拽（没有对应的按下手势）不该凭空搬数据。
   * 反向用例：组件已经不听原生拖拽事件了。
   */
  it("外部乱入的原生 drop：什么也不做", async () => {
    const w = mountGridP([item(1), item(2)]);
    await fireDrag(cells(w)[1].element, "dragstart");
    await fireDrag(cells(w)[1].element, "dragover");
    await fireDrag(cells(w)[1].element, "drop");
    expect(w.emitted("move")).toBeUndefined();
  });
});
/* ============ 层数上限判定用**后端的真实 depth** ============ */
/* 放行判定（`utils/tree.ts::canDrop`）算的是"目标 depth + 1 + 子树高度"。
   前端自己数 depth 时，`byId` 只有当前筛选到的条目 —— 父节点被筛掉就被当成
   走到了根，深度**偏小**；后端 `check_depth` 用的是**全表**边表，于是出现
   "界面画可放置、emit 了 moveNodes、后端回层级超限"的白操作。
   方向是单向的：前端只会多放行，不会反过来拦合法的。 */

describe("PosterGrid 层数上限判定（后端 depth）", () => {
  /**
   * 真实层级 A(0)→B(1)→C(2)→D(3)，自由条目 X。
   * 把 X 拖到 D 上会到第 5 层 → 两种列表下的结论必须一致：**都不放行**。
   *
   * 旧实现里截断那条会 `ok=true` 并把 moveNodes 发出去（审计复现）。
   */
  it("父节点被筛掉时仍按后端 depth 判定：第 5 层一样不放行", async () => {
    // ① 参考：完整列表（父链都在）
    const full = mountGrid(
      [
        item(1, { child_count: 1, depth: 0 }), // A
        item(2, { parent_id: 1, child_count: 1, depth: 1 }), // B
        item(3, { parent_id: 2, child_count: 1, depth: 2 }), // C
        item(4, { parent_id: 3, depth: 3 }), // D
        item(9, { depth: 0 }), // X（自由条目）
      ],
      { grouping: true },
    );
    await dragTo(full, 4, 3);
    expect(full.emitted("move")).toBeUndefined();
    expect(full.find(".reject").text()).toContain("层级最多");

    // ② 只显示 C/D：父链（A、B）被筛掉了，depth 只能由后端给
    const cut = mountGrid(
      [
        item(3, { parent_id: 2, child_count: 1, depth: 2 }),
        item(4, { parent_id: 3, depth: 3 }),
        item(9, { depth: 0 }),
      ],
      { grouping: true },
    );
    await dragTo(cut, 2, 1);
    expect(cut.emitted("move")).toBeUndefined();
    expect(cut.find(".reject").text()).toContain("层级最多");
  });

  /**
   * 反向判据：只给**第 3 层**（depth 2）的目标时，放行 —— 别把"用后端 depth"
   * 修成"一律拦"。depth 2 + 1 = 3 = 上限内的最后一层。
   */
  it("后端 depth 仍在限额内时照旧放行（不是一律拦）", async () => {
    const w = mountGrid(
      [
        item(3, { parent_id: 2, depth: 2 }), // 第 3 层
        item(9, { depth: 0 }),
      ],
      { grouping: true },
    );
    await dragTo(w, 1, 0);
    expect(w.emitted("move")).toEqual([[{ ids: [9], parentId: 3, index: null }]]);
  });

  /** 老后端 / 桩数据没有 depth 时仍退回前端自数（列表内的父链照样算得对）。 */
  it("depth 缺失时退回前端自数，层级判定仍然可用", async () => {
    // 列表内就有完整父链：A(0)→B(1)→C(2)，B/C 的 depth 不给
    const w = mountGrid(
      [
        item(1, { child_count: 1 }),
        item(2, { parent_id: 1, child_count: 1 }),
        item(3, { parent_id: 2, child_count: 1 }),
        item(4, { parent_id: 3 }), // 第 4 层
        item(9),
      ],
      { grouping: true },
    );
    await dragTo(w, 4, 3);
    // 自数也能算出 D 在第 4 层 → 不放行（与后端 depth 给的值同结论）
    expect(w.emitted("move")).toBeUndefined();
  });
});

/* ============ 手势分流：谁在什么时候拿到这一次按下 ============ */
/*
 * 「按住卡片横向划过 = 连续勾选」与「按住卡片拖到另一张上 = 聚合」是**同一个
 * 手势**（左键按下 → 移动 → 松手），没有几何判据能分开 —— 实测把"跨到别的
 * 卡片就改判"接上之后，滑动多选的 1→2→3 恰好就是"跨了好几次卡片"，会被当场
 * 改判成拖拽。所以分流只能靠一个明确的用户意图：顶栏的「选择」。
 *
 * 这一组钉的就是那条分流线。
 */

describe("PosterGrid 手势分流", () => {
  /** 「选择」模式：滑动多选照旧整段提交（上一轮交付的功能）。 */
  it("「选择」模式：按住划过卡片连续勾选、松手提交一次", async () => {
    const w = mountGridP([item(1), item(2), item(3)], [], { selectable: true });
    const cards = w.findAll(".card");

    pointer(cards[0].element, "pointerdown", { clientX: 0, clientY: 0 });
    pointer(cards[2].element, "pointermove", { clientX: 200, clientY: 0 });
    await nextTick();
    // 拖动期间只是预览，**没有提交**
    expect(w.findAll(".card.picked").map((n) => Number(n.attributes("data-id")))).toEqual([1, 2, 3]);
    expect(w.emitted("select-many")).toBeUndefined();
    // 也没有被改判成拖拽
    expect(w.find(".rootzone").exists()).toBe(false);

    pointer(cards[2].element, "pointerup", { clientX: 200, clientY: 0 });
    await nextTick();
    expect(w.emitted("select-many")).toEqual([[[1, 2, 3]]]);
  });

  /** 默认（选择 + 聚合都关）：同样是划过，但这一次归拖拽聚合。 */
  it("默认模式：同样的划过不再勾选，而是改判成拖拽", async () => {
    const w = mountGridP([item(1), item(2), item(3)], [], { selectable: false, grouping: false });
    const cards = w.findAll(".card");

    pointer(cards[0].element, "pointerdown", { clientX: 0, clientY: 0 });
    pointer(cards[0].element, "pointermove", { clientX: 40, clientY: 0 });
    await nextTick();
    expect(w.find(".rootzone").exists()).toBe(true);

    pointer(cards[2].element, "pointermove", { clientX: 200, clientY: 0 });
    pointer(cards[2].element, "pointerup", { clientX: 200, clientY: 0 });
    await nextTick();
    expect(w.emitted("select-many")).toBeUndefined();
  });

  /** 「选择」模式与「聚合」同时为真时聚合优先（App.vue 保证两者互斥，这里兜底）。 */
  it("selectable 与 grouping 同时为真：归聚合，不再勾选", async () => {
    const w = mountGridP([item(1), item(2)], [], { selectable: true, grouping: true });
    const cards = w.findAll(".card");
    pointer(cards[0].element, "pointerdown", { clientX: 0, clientY: 0 });
    pointer(cards[0].element, "pointermove", { clientX: 40, clientY: 0 });
    await nextTick();
    expect(w.find(".rootzone").exists()).toBe(true);
    pointer(window, "pointercancel", {});
  });

  /** 回归：普通点击仍然是「打开详情」，没有被拖拽 / 勾选抢走。 */
  it("回归：单击卡片仍然是打开详情", async () => {
    const w = mountGridP([item(11), item(12)], [], { selectable: false });
    await w.findAll(".card")[1].trigger("click");
    expect(w.emitted("open")).toEqual([[12]]);
  });

  /**
   * 触摸：长按之后跨到别的卡片上就变成拖拽聚合。
   * （原生 HTML5 拖拽在触摸下压根起不来，指针驱动没有这个问题。）
   */
  it("触摸：长按之后跨卡片拖动 → 起手拖拽", async () => {
    vi.useFakeTimers();
    try {
      const w = mountGridP([item(1), item(2)], [], { selectable: false });
      const cards = w.findAll(".card");
      pointer(cards[0].element, "pointerdown", { pointerType: "touch", clientX: 0, clientY: 0 });
      vi.advanceTimersByTime(400);
      await nextTick();
      expect(w.find(".rootzone").exists()).toBe(true);
      pointer(window, "pointercancel", {});
    } finally {
      vi.useRealTimers();
    }
  });

  /** 触摸：长按成立之前手指先动了 = 用户在滚页面，放弃这一次。 */
  it("触摸：长按成立前先移动 → 完全不介入", async () => {
    vi.useFakeTimers();
    try {
      const w = mountGridP([item(1), item(2)], [], { selectable: false });
      const cards = w.findAll(".card");
      pointer(cards[0].element, "pointerdown", { pointerType: "touch", clientX: 0, clientY: 0 });
      // 手指先动 = 用户在滚页面（触摸没有"跨卡片就算拖拽"这条旁路，
      // 必须先由长按把这一次手势判给拖拽）
      pointer(cards[1].element, "pointermove", { pointerType: "touch", clientX: 40, clientY: 0 });
      pointer(cards[1].element, "pointermove", { pointerType: "touch", clientX: 80, clientY: 0 });
      vi.advanceTimersByTime(400);
      await nextTick();
      expect(w.find(".rootzone").exists()).toBe(false);
      expect(w.emitted("move")).toBeUndefined();
    } finally {
      vi.useRealTimers();
    }
  });
});

/* ============ 「聚合」开关剩下的语义：外观 + 提示 ============ */
/*
 * 拖动**从来不需要**先打开任何模式（本次修复的核心诉求）。顶栏那个「聚合」
 * 开关现在只影响外观：分组提示条常显。这一组钉住它依然是"显式声明"，
 * 而不是"开启之后才拖得动"。
 */

describe("PosterGrid 聚合开关（只剩外观语义）", () => {
  it("聚合模式给 .grid 挂上 grouping 类；关掉时没有", () => {
    const on = mountGridP([item(1)], { grouping: true });
    expect(on.find(".grid").classes()).toContain("grouping");
    const off = mountGridP([item(1)]);
    expect(off.find(".grid").classes()).not.toContain("grouping");
  });

  /**
   * 核心契约：**默认状态下按住卡片拖动就能起拖**，不依赖 grouping。
   * 这正是用户报的"必须先打开某个模式才能拖"。
   */
  it("默认状态（未开任何模式）就能起拖，不依赖 grouping", async () => {
    const off = mountGridP([item(1), item(2)]);
    pointer(cells(off)[0].element, "pointerdown", { clientX: 0, clientY: 0 });
    pointer(cells(off)[0].element, "pointermove", { clientX: 40, clientY: 0 });
    await nextTick();
    expect(off.find(".rootzone").exists()).toBe(true);
    pointer(window, "pointercancel", {});

    const on = mountGridP([item(1), item(2)], { grouping: true });
    pointer(cells(on)[0].element, "pointerdown", { clientX: 0, clientY: 0 });
    pointer(cells(on)[0].element, "pointermove", { clientX: 40, clientY: 0 });
    await nextTick();
    expect(on.find(".rootzone").exists()).toBe(true);
    pointer(window, "pointercancel", {});
  });

  /**
   * 一个可拖元素都不留：拖拽源是"这一次手势"，不是某个 `draggable` 元素。
   * 留着它就是又给浏览器一次机会把划过去认成原生拖拽、顺手补一次
   * `pointercancel` 掐断 pointer 事件流 —— 滑动多选当场失灵。
   */
  it("任何模式下都不给 .cell / .card / img 留 draggable", () => {
    for (const props of [{}, { grouping: true }, { selectable: true }]) {
      const w = mountGridP([item(1), item(2)], props);
      for (const c of cells(w)) {
        expect((c.element as HTMLElement).draggable).toBe(false);
      }
      for (const card of w.findAll(".card")) {
        expect((card.element as HTMLElement).draggable).toBe(false);
      }
    }
  });

  it("CSS 契约：图片与卡片文字都不参与拖拽（禁止光标的真正来源）", () => {
    // 图片天生可拖 → 按住海报拖起来的是"图片拖拽"，光标就是禁止图标
    expect(posterGridSource).not.toContain("draggable=\"true\"");
    expect(posterCardSource).toContain(':draggable="false"');
    expect(posterCardSource).toContain("-webkit-user-drag: none");
    // 文字可选 → 按住标题拖起来的是"文本拖拽"，光标同样是禁止
    expect(posterCardSource).toContain("user-select: none");
  });
});

/* ==================== 无级缩放（CSS 契约） ==================== */
/* jsdom 没有布局引擎，`repeat(auto-fill, …)` 到底排几列在这里量不出来，
   所以这两条钉的是**源码契约**：列宽必须由容器宽度与 `--card-min` 决定，
   不能再退回写死的 `minmax(150px, 1fr)`（那是"三档断点"而不是无级）。 */

describe("PosterGrid 无级缩放", () => {
  const src = posterGridSource;

  it("列宽由 auto-fill + --card-min 连续决定，不再写死 150px", () => {
    // 写死的那版长这样：minmax(150px, 1fr)
    expect(src).not.toMatch(/minmax\(\s*150px/);
    expect(src).toContain("repeat(auto-fill");
    expect(src).toContain("var(--card-min");
  });

  /** 极窄容器兜底：放不下一张最小卡片时退回单列，而不是顶出横向滚动条。 */
  it("极窄容器回落成 100% 单列（min() 兜底）", () => {
    expect(src).toMatch(/minmax\(\s*min\(\s*var\(--card-min/);
    expect(src).toContain("100%");
  });
});

/* ============ 非法落点：能放不能放只影响落点，不影响「目标可不可接收」 ============ */
/*
 * 上一轮踩过的坑：把"这一次不允许"表达成"这个目标不接收事件"（不
 * preventDefault），浏览器就会接管并画禁止光标 —— 两种失败混成一个症状。
 * 现在整套拖放由指针驱动，那套判罚**根本不存在了**，但契约要留在用例里：
 *
 *   能不能放 → 只决定这一次的落点（发不发 move）+ 文字原因；
 *   事件本身 → 永远照常送到组件（指针事件没有"取消"这个前置条件）。
 */

describe("PosterGrid 非法落点", () => {
  /** 合法目标：画主色描边、无红字、发 move。 */
  it("合法目标：可放置描边 + 无提示 + 发 move", async () => {
    const w = mountGridP([item(1), item(2)]);
    pointer(cells(w)[0].element, "pointerdown", { clientX: 0, clientY: 0 });
    pointer(cells(w)[0].element, "pointermove", { clientX: 40, clientY: 0 });
    geom.el = cells(w)[1].element;
    pointer(window, "pointermove", { clientX: 268, clientY: 150 });
    await nextTick();

    expect(cells(w)[1].classes()).toContain("over");
    expect(cells(w)[1].classes()).not.toContain("rejected");
    expect(w.find(".reject").exists()).toBe(false);
  });

  /**
   * 非法目标（拖到自己身上）：**事件照常送到**（指针事件本来就收得到），
   * 只是落点被拒 —— 红框 + 文字原因，且**不发 move**。
   */
  it("非法目标：仍然收到 pointermove（事件不因非法而消失），只是画禁止态", async () => {
    const w = mountGridP([item(1), item(2)]);
    pointer(cells(w)[0].element, "pointerdown", { clientX: 0, clientY: 0 });
    pointer(cells(w)[0].element, "pointermove", { clientX: 40, clientY: 0 });
    geom.el = cells(w)[0].element;
    pointer(window, "pointermove", { clientX: 100, clientY: 150 });
    await nextTick();

    expect(cells(w)[0].classes()).toContain("rejected");
    expect(w.find(".reject").text()).toContain("不能放到自己里面");

    // 松手：不发 move，原因留在界面上（短暂）
    pointer(window, "pointerup", { clientX: 100, clientY: 150 });
    await nextTick();
    expect(w.emitted("move")).toBeUndefined();
  });

  /**
   * 旧实现踩过的那条 HTML5 硬规则，**留在这里当反例**（真浏览器实测见
   * `_probe/dnd` 的 CDP 探针）：
   *
   * - 落点必须在 `dragenter` / `dragover` 里 `preventDefault()`，否则浏览器
   *   判它"不接收"，**根本不会投递 drop**，光标直接是禁止；
   * - "这一次不允许"只能靠 `dropEffect = "none"` 表达，**不能**靠"不取消事件"
   *   表达 —— 那会把"这次不放"和"这个目标不接收"混成同一个症状。
   *
   * 这两条现在都不是 PosterGrid 的实现基础了（整套拖放已改成指针驱动），
   * 所以第一半钉的是"组件不再依赖原生拖拽判罚"这条反向契约，
   * 第二半用真实的 DOM 事件把那条规则本身的对照关系固定下来。
   */
  it("组件不再依赖原生拖拽判罚（没有 dragover 监听、也没有 dropEffect 赋值）", () => {
    expect(posterGridSource).not.toContain("@dragover");
    expect(posterGridSource).not.toContain("@dragenter");
    expect(posterGridSource).not.toContain("dropEffect");
    expect(posterGridSource).not.toContain("effectAllowed");
  });

  it("HTML5 规则对照：取消 dragover 的落点被接受，只设 dropEffect 而不取消的被判不接收", () => {
    const canceler = document.createElement("div");
    const refuser = document.createElement("div");
    document.body.append(canceler, refuser);
    try {
      const seen: Array<{ who: string; prevented: boolean; effect: string }> = [];
      const dt = { dropEffect: "move", effectAllowed: "move", setData: vi.fn(), getData: () => "" };
      const listener = (who: string) => (ev: Event) => {
        const d = (ev as unknown as { dataTransfer: { dropEffect: string } }).dataTransfer;
        if (who === "canceler") ev.preventDefault(); // 可接收：恒取消
        else d.dropEffect = "none"; // 错法：只改 dropEffect、**不**取消
        seen.push({ who, prevented: ev.defaultPrevented, effect: d.dropEffect });
      };
      canceler.addEventListener("dragover", listener("canceler"));
      refuser.addEventListener("dragover", listener("refuser"));

      for (const el of [canceler, refuser]) {
        const ev = new Event("dragover", { bubbles: true, cancelable: true });
        Object.assign(ev, { dataTransfer: dt });
        el.dispatchEvent(ev);
      }

      expect(seen[0]).toEqual({ who: "canceler", prevented: true, effect: "move" });
      // 不取消 = 浏览器认为这个目标不接收（真机上就是禁止光标、且收不到 drop）；
      // dropEffect 设成 "none" 也救不回来，因为事件压根没被接受。
      expect(seen[1]).toEqual({ who: "refuser", prevented: false, effect: "none" });
    } finally {
      canceler.remove();
      refuser.remove();
    }
  });
  it("电影拖到剧集上可放置，剧集拖到电影上同样可放置", async () => {
    const w = mountGridP([item(1, { type: "movie" }), item(2, { type: "tv" })]);
    await dragTo(w, 0, 1);
    expect(w.emitted("move")).toEqual([[{ ids: [1], parentId: 2, index: null }]]);
    await dragTo(w, 1, 0);
    expect(w.emitted("move")).toEqual([
      [{ ids: [1], parentId: 2, index: null }],
      [{ ids: [2], parentId: 1, index: null }],
    ]);
  });
});

/* ==================== 顶层顺序 = 展示排序偏好 ==================== */
/*
 * `buildTree` 对同级按 `sort_order` 重排。真实库里 `sort_order` 常是全局递增的
 * 流水号（实测 0,3,4,5…63），于是"顶层桶里存在 sort_order > 0"恒成立 ——
 * 一旦把顶层也算进"有真实次序"的桶，整面墙的顺序就恒等于 DB 的 `sort_order`，
 * 用户选的「名称 / 年份 / 添加时间」被整个覆盖，表现就是"排序失效"。
 * 顶层必须恒按列表下标（也就是展示排序算出来的先后）走。
 */

describe("PosterGrid 顶层顺序", () => {
  /** 传入顺序与 DB `sort_order` 故意冲突：渲染必须跟传入顺序走。 */
  it("顶层恒按传入顺序渲染，不被 DB 的 sort_order 覆盖", () => {
    const w = mountGrid([
      item(103, { sort_order: 2 }),
      item(101, { sort_order: 0 }),
      item(102, { sort_order: 1 }),
    ]);
    expect(cellIds(w)).toEqual([103, 101, 102]);
  });

  /** 顶层永远不认库内 sort_order，但**组内**的手工次序仍然要保住。 */
  it("顶层按传入顺序，而标记过的子组仍按库内 sort_order 排（手工次序不丢）", () => {
    const w = mountGrid(
      [
        item(1, { child_count: 2 }),
        item(12, { parent_id: 1, sort_order: 1 }),
        item(11, { parent_id: 1, sort_order: 0 }),
      ],
      { manualOrderGroups: [1] },
    );
    expect(cellIds(w)).toEqual([1, 11, 12]);
  });
});

/* ==================== 禁止态只在「确实不可用」时渲染 ==================== */
/*
 * 界面上的禁止提示（红字 + 红框）**只在对应功能处于禁用 / 不可用状态时才出现**。
 * 常驻是缺陷：一次非法操作之后界面上永久挂着一条"禁止"，而那时既没有拖拽、
 * 也没有任何被禁用的东西，属于凭空渲染的「禁止」状态。
 */

describe("PosterGrid 禁止态的显示条件", () => {
  it("静止态（没在拖、也没被拒过）：不渲染任何禁止提示", async () => {
    const w = mountGridP([item(1), item(2)]);
    await nextTick();
    expect(w.find(".reject").exists()).toBe(false);
    expect(cells(w).some((c) => c.classes().includes("rejected"))).toBe(false);
  });

  /** 起拖之后、还没经过任何目标：同样不该有禁止态。 */
  it("刚起拖（还没碰到任何目标）：不画禁止态", async () => {
    const w = mountGridP([item(1), item(2)]);
    pointer(cells(w)[0].element, "pointerdown", { clientX: 0, clientY: 0 });
    pointer(cells(w)[0].element, "pointermove", { clientX: 40, clientY: 0 });
    await nextTick();
    expect(w.find(".reject").exists()).toBe(false);
    expect(cells(w).some((c) => c.classes().includes("rejected"))).toBe(false);
    pointer(window, "pointercancel", {});
  });

  /** 从非法目标挪回合法目标：禁止态必须立刻收掉，不能赖着不走。 */
  it("从非法目标挪到合法目标：禁止态立刻消失", async () => {
    const w = mountGridP([item(1), item(2)]);
    pointer(cells(w)[0].element, "pointerdown", { clientX: 0, clientY: 0 });
    pointer(cells(w)[0].element, "pointermove", { clientX: 40, clientY: 0 });
    geom.el = cells(w)[0].element;
    pointer(window, "pointermove", { clientX: 100, clientY: 150 });
    await nextTick();
    expect(w.find(".reject").exists()).toBe(true);

    geom.el = cells(w)[1].element;
    pointer(window, "pointermove", { clientX: 268, clientY: 150 });
    await nextTick();
    expect(w.find(".reject").exists()).toBe(false);
    expect(cells(w)[0].classes()).not.toContain("rejected");
    pointer(window, "pointercancel", {});
  });

  /** 成功放下：不残留禁止提示。 */
  it("成功放下：不残留禁止提示", async () => {
    const w = mountGridP([item(1), item(2)]);
    await dragTo(w, 0, 1);
    expect(w.emitted("move")).toEqual([[{ ids: [1], parentId: 2, index: null }]]);
    expect(w.find(".reject").exists()).toBe(false);
  });

  /** 新起一次拖拽：上一次留下的提示必须先收掉，不能带到这一轮。 */
  it("新起一次拖拽：上一次残留的禁止提示被清掉", async () => {
    const w = mountGridP([item(1), item(2)]);
    // 第一次：非法放置，留下提示
    await dragTo(w, 0, 0);
    expect(w.find(".reject").exists()).toBe(true);

    // 第二次：刚起拖、还没碰到任何目标 —— 不该还挂着上一次的原因
    geom.el = null; // 指针这次落在空白处
    pointer(cells(w)[0].element, "pointerdown", { clientX: 0, clientY: 0 });
    pointer(cells(w)[0].element, "pointermove", { clientX: 40, clientY: 0 });
    await nextTick();
    expect(w.find(".reject").exists()).toBe(false);
    pointer(window, "pointercancel", {});
  });

  /**
   * 非法放下之后要**短暂**说明原因，然后自己消失。
   * 常驻是缺陷，但**完全不说**也不行：用户会以为只是"没反应"。
   */
  it("非法放下：先说明原因，到点自动收掉（不常驻）", async () => {
    vi.useFakeTimers();
    try {
      const w = mountGridP([item(1), item(2)]);
      await dragTo(w, 0, 0);
      expect(w.find(".reject").text()).toContain("自己");

      vi.advanceTimersByTime(3000);
      await nextTick();
      expect(w.find(".reject").exists()).toBe(false);
      expect(cells(w).some((c) => c.classes().includes("rejected"))).toBe(false);
    } finally {
      vi.useRealTimers();
    }
  });

  /** 拖拽取消（pointercancel）：落点态一起收掉，不留任何禁止提示。 */
  it("pointercancel：落点态与禁止态一起收掉", async () => {
    const w = mountGridP([item(1), item(2)]);
    pointer(cells(w)[0].element, "pointerdown", { clientX: 0, clientY: 0 });
    pointer(cells(w)[0].element, "pointermove", { clientX: 40, clientY: 0 });
    geom.el = cells(w)[0].element;
    pointer(window, "pointermove", { clientX: 100, clientY: 150 });
    await nextTick();
    expect(w.find(".reject").exists()).toBe(true);

    pointer(window, "pointercancel", {});
    await nextTick();
    expect(w.find(".reject").exists()).toBe(false);
    expect(w.emitted("move")).toBeUndefined();
  });
});