/**
 * MediaDetail 的「聚合成员」用例。
 *
 * 剧集层级栏目（`SeriesTree.vue`）撤掉之后，成员与顺序的调整整体迁进了
 * 详情弹窗，**电影与剧集走同一套代码路径**：判据只有 `child_count > 0`
 * 或「确实有 `parent_id` 指向本条目的条目」，**不看 `type`**。这里钉的就是这一块。
 *
 * 为什么单独一个文件：`MediaDetail.spec.ts` 里那 30 条全是滑动多选，
 * 关注点不同、前置桩也不同（这里要造层级数据 + 打桩 store 的层级操作），
 * 混在一起只会互相干扰。mock / mount 套路与 `MediaDetail.spec.ts` 保持一致：
 * 后端命令整体 mock，store 用真 Pinia，三个层级入口用 spy 顶掉
 * （不真去改数据，否则 `reloadDetail` 会把 `detail` 换成 mock 的那一条）。
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { mount, type VueWrapper } from "@vue/test-utils";
import { nextTick } from "vue";
import { createPinia, setActivePinia } from "pinia";
import MediaDetail from "./MediaDetail.vue";
import { useMediaStore } from "../stores/mediaStore";
import { useSettingsStore } from "../stores/settingsStore";
import { askConfirm } from "../utils/confirm";
import type { SortBy, SortDir } from "../utils/sort";
import { getMediaList, setSettings, type MediaItem } from "../api/commands";

/** 原生目录选择器的桩（本组用不到，但组件导入了它）。 */
const dialog = vi.hoisted(() => ({ open: vi.fn(async () => null as string | null) }));
vi.mock("@tauri-apps/plugin-dialog", () => dialog);

/** 「取消聚合」这类破坏性操作走这里，用例里自己决定用户点了「确定」还是「取消」。 */
vi.mock("../utils/confirm", () => ({ askConfirm: vi.fn(async () => true) }));

// 后端命令整体 mock：单测不碰真实 Tauri。工厂里不引用模块级常量（它在
// import 阶段就会执行，那时下面的 const 还没初始化）。
vi.mock("../api/commands", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../api/commands")>();
  return {
    ...actual,
    listCategories: vi.fn(async () => []),
    listTags: vi.fn(async () => []),
    getMediaCategories: vi.fn(async () => []),
    getMediaTags: vi.fn(async () => []),
    // 返回类型显式写成 `MediaItem[]`：否则 `vi.mocked(getMediaList)` 是 `never[]`，
    // 下面 mountDetail 里按用例改写返回值时会类型不匹配。
    getMediaList: vi.fn(async (): Promise<MediaItem[]> => []),
    getScanHistory: vi.fn(async () => []),
    getMediaDetail: vi.fn(async () => ({ media: null, episodes: [] })),
    moveMediaNodes: vi.fn(async () => ({ undo_depth: 0, message: "ok" })),
    undoTreeOp: vi.fn(async () => ({ undo_depth: 0, message: "ok" })),
    // 「手工排序」标记会顺手把设置写回库（`markManualOrder` → `save()`）：
    // 这两条必须打桩，否则用例会去调真实 Tauri IPC。
    getSettings: vi.fn(async () => ({})),
    setSettings: vi.fn(async () => undefined),
  };
});

/**
 * 造一条库里的条目。
 *
 * `added_at` 默认取 id：多数用例的成员都**没写 `sort_order`**（缺省 0），
 * 成员次序退回 `id` 序、正好等于 `added_at` 序，断言才唯一。
 * ⚠️ 「成员次序」那一组会显式写 `sort_order`，并把全局排序偏好设成**相反**的名字序。
 */
function item(id: number, over: Partial<MediaItem> = {}): MediaItem {
  return {
    id,
    type: "tv",
    title: `标题${id}`,
    year: 2000,
    tmdb_id: null,
    poster_path: null,
    backdrop_path: null,
    overview: null,
    vote_average: null,
    genre: null,
    file_path: null,
    file_name: null,
    episode_count: 0,
    added_at: id,
    display_title: `标题${id}`,
    ...over,
  };
}

/** store 上被顶掉的三个入口：层级操作 + 两处刷新。断言都打在它们身上。 */
function spyTree(store: ReturnType<typeof useMediaStore>) {
  return {
    moveNodes: vi.spyOn(store, "moveNodes").mockImplementation(async () => true),
    load: vi.spyOn(store, "load").mockImplementation(async () => undefined),
    openDetail: vi.spyOn(store, "openDetail").mockImplementation(async () => undefined),
  };
}
type Spies = ReturnType<typeof spyTree>;

interface Mounted {
  w: VueWrapper;
  sp: Spies;
}

let wrappers: VueWrapper[] = [];

/**
 * 挂一个详情弹窗。
 *
 * `items` 是**当前筛选后**的列表（成员与「加入聚合」的候选都从它里头筛），
 * `media` 是当前打开的那一条。
 *
 * `opts.by` / `opts.dir` 是全局展示排序偏好（默认「加入时间正序」）。成员次序与它
 * **无关**（成员按同级 `sort_order` 排），所以「成员次序」那一组会故意把它设成
 * 与 `sort_order` **相反**的名字序，用来证明两条序已经解耦。
 *
 * `opts.full` / `opts.fullError` 只影响「取消聚合」那条全量取数链路
 * （组件直接调 `getMediaList`，不带任何筛选参数）：
 * - 默认让「全库 = `items`」，此时「看得见的成员」与「全量成员」重合；
 * - 传 `full` 可以造出**确实挂在父下、但不在当前筛选里**的成员（漏搬的判据）；
 * - 传 `fullError` 可以让全量取数失败（不许静默降级的判据）。
 */
interface MountOpts {
  by?: SortBy;
  dir?: SortDir;
  /**
   * `settings.manualOrderGroups` 的初始值：哪些聚会被用户**亲手**排过。
   *
   * 空（默认）= 这一组跟随顶栏排序；填了父 id = 按库内 `sort_order` 显示。
   * 这个标记是本组件成员次序的**唯一**判据（不再看「组里有没有 sort_order > 0」）。
   */
  manual?: number[];
  /** 「取消聚合」取全量成员时 `getMediaList` 返回什么（默认 = `items`）。 */
  full?: MediaItem[];
  /** 让全量取数失败（错误文案原样进 `groupMsg`）。 */
  fullError?: string;
}

function mountDetail(
  media: MediaItem,
  items: MediaItem[],
  opts: MountOpts = {},
): Mounted {
  const store = useMediaStore();
  const settings = useSettingsStore();
  settings.sortBy = opts.by ?? "added";
  settings.sortDir = opts.dir ?? "asc";
  // 每个用例都重设一次：不重置的话上一条用例标记过的分组会漏进下一条
  settings.manualOrderGroups = [...(opts.manual ?? [])];
  store.items = items;
  store.undoDepth = 0;
  store.selectedId = media.id;
  store.detail = { media, episodes: [] };
  // 每个用例都重设一次实现（`clearMocks` 只清调用记录、不清实现），不留残留
  const list = vi.mocked(getMediaList);
  if (opts.fullError) list.mockRejectedValue(new Error(opts.fullError));
  else list.mockResolvedValue(opts.full ?? items);
  const sp = spyTree(store);
  const w = mount(MediaDetail, { attachTo: document.body });
  wrappers.push(w);
  return { w, sp };
}

/** 某个成员行里的按钮（按**文字**找，别按下标——行内按钮的顺序会随改版变）。 */
function rowBtn(w: VueWrapper, i: number, text: string) {
  const rows = w.findAll(".g-list li");
  const b = rows[i].findAll("button").find((n) => n.text() === text);
  if (!b) throw new Error(`第 ${i + 1} 个成员行里没有「${text}」按钮`);
  return b;
}

/** 聚合区里的按钮（所属聚合 / 取消聚合 / 脱离聚合 / 刷新…）。 */
function groupBtn(w: VueWrapper, text: string) {
  const b = w.findAll(".group button").find((n) => n.text().includes(text));
  if (!b) throw new Error(`聚合区里没有「${text}」按钮`);
  return b;
}

/** 有没有这个按钮（不给「取消聚合」这种**不该出现**的按钮用，找不到不该抛）。 */
function hasGroupBtn(w: VueWrapper, text: string): boolean {
  return w.findAll(".group button").some((n) => n.text().includes(text));
}

/**
 * 把已排队的微任务全部跑完。
 *
 * 层级操作是 `await` 链（`op → load → openDetail`），一次 `nextTick()` 不够，
 * 不等就可能在 `groupBusy` 还为真时点下一个按钮。
 */
function flush(): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, 0));
}

beforeEach(() => {
  setActivePinia(createPinia());
  // jsdom 没有 elementFromPoint（也没有布局），统一换成可编程的桩
  Object.defineProperty(document, "elementFromPoint", {
    configurable: true,
    writable: true,
    value: () => null,
  });
});

afterEach(() => {
  for (const w of wrappers) w.unmount();
  wrappers = [];
  document.body.innerHTML = "";
});

/* ==================== 成员区 ==================== */

describe("MediaDetail 聚合成员", () => {
  /** 父子都是 `movie`：这是「电影与剧集行为一致」的核心判据，不看 `type`。 */
  const parent = () => item(101, { display_title: "西游记合集", child_count: 2 });
  const memA = () => item(201, { display_title: "第一集", parent_id: 101 });
  const memB = () => item(202, { display_title: "第二集", parent_id: 101 });
  const other = () => item(901, { display_title: "别的剧" });

  it("有子成员的条目渲染成员行（序号 / 标题 / ↑ / ↓ / ✎ / 移出）", () => {
    const { w } = mountDetail(parent(), [parent(), memA(), memB(), other()]);

    expect(w.find("section.group").exists()).toBe(true);
    const rows = w.findAll(".g-list li");
    expect(rows).toHaveLength(2);

    // 序号从 1 开始，标题走 display_title，五个操作一个都不能少
    expect(rows[0].find(".g-no").text()).toBe("1");
    expect(rows[1].find(".g-no").text()).toBe("2");
    expect(rows[0].find(".g-title").text()).toBe("第一集");
    expect(rows[0].findAll("button").map((n) => n.text())).toEqual([
      "第一集", "↑", "↓", "✎", "移出",
    ]);

    // 顶层 / 别的父节点下的条目不能混进来
    expect(w.find(".g-list").text()).not.toContain("别的剧");
  });

  /** `index` 必须是**同级内**下标，父 id 是自己：传全局下标会挤乱别的聚合。 */
  it("上移发 moveNodes([id], 自己, i-1)；下移发 moveNodes([id], 自己, i+1)", async () => {
    const { w, sp } = mountDetail(parent(), [parent(), memA(), memB(), other()]);

    await rowBtn(w, 1, "↑").trigger("click");
    await flush();
    expect(sp.moveNodes).toHaveBeenCalledWith([202], 101, 0);

    await rowBtn(w, 0, "↓").trigger("click");
    await flush();
    expect(sp.moveNodes).toHaveBeenCalledWith([201], 101, 1);
    // 父 id 恒为自己，不是 null、也不是其它聚合
    for (const call of sp.moveNodes.mock.calls) expect(call[1]).toBe(101);
  });

  /** 移出 = 挂回顶层，条目本身还在库里（不是删除）。 */
  it("移出成员发 moveNodes([id], null, null)", async () => {
    const { w, sp } = mountDetail(parent(), [parent(), memA(), memB(), other()]);

    await rowBtn(w, 0, "移出").trigger("click");
    await flush();

    expect(sp.moveNodes).toHaveBeenCalledWith([201], null, null);
  });

  /** 信息编辑复用详情页自己的 `emit("edit", item)`，由 App 打开编辑对话框。 */
  it("成员的信息编辑走 emit('edit', 成员)", async () => {
    const { w } = mountDetail(parent(), [parent(), memA(), memB(), other()]);

    await rowBtn(w, 1, "✎").trigger("click");

    const ev = w.emitted("edit");
    expect(ev).toHaveLength(1);
    expect((ev?.[0]?.[0] as MediaItem).id).toBe(202);
    // 没有另起一个弹窗：编辑期间聚合区还在原地
    expect(w.find("section.group").exists()).toBe(true);
  });

  /** 电影端同构：父与成员都是 `movie` 时成员区照旧渲染（判据不看 `type`）。 */
  it("电影同构：父节点与成员都是 movie，成员区照样渲染", () => {
    const p = item(101, { type: "movie", display_title: "电影合集", child_count: 1 });
    const m = item(201, { type: "movie", display_title: "电影A", parent_id: 101 });
    const { w } = mountDetail(p, [p, m]);

    expect(w.find("section.group").exists()).toBe(true);
    const rows = w.findAll(".g-list li");
    expect(rows).toHaveLength(1);
    expect(rows[0].text()).toContain("电影A");
    expect(rows[0].findAll("button").map((n) => n.text())).toEqual([
      "电影A", "↑", "↓", "✎", "移出",
    ]);
  });

  /**
   * 空聚合：`child_count > 0` 但一条成员都取不到（被删了 / 不在当前筛选里）。
   * 必须给出说明与「刷新」，不能渲染成一片空白，更不能抛错。
   */
  it("空聚合渲染空态文案与「刷新」按钮，刷新会重拉列表与详情", async () => {
    const p = item(101, { display_title: "空聚合", child_count: 3 });
    const { w, sp } = mountDetail(p, [p, other()]);

    expect(w.find("section.group").exists()).toBe(true);
    expect(w.find(".g-list").exists()).toBe(false);
    const empty = w.find(".g-empty");
    expect(empty.exists()).toBe(true);
    expect(empty.text()).toContain("3"); // 说清楚记录到几个
    // 空态文案把「换个栏目再看」从**唯一出路**降成可选，并给出「取消聚合」这条去路
    expect(empty.text()).toContain("取消聚合");
    expect(empty.find("button").text()).toBe("刷新");

    await empty.find("button").trigger("click");
    await flush();
    expect(sp.load).toHaveBeenCalled();
    expect(sp.openDetail).toHaveBeenCalledWith(101);
  });

  it("显示所属聚合（点标题打开父详情），「脱离聚合」= moveNodes([自己], null, null)", async () => {
    const g = item(101, { display_title: "西游记合集", child_count: 1 });
    const me = item(202, { display_title: "第二集", parent_id: 101 });
    const { w, sp } = mountDetail(me, [g, me]);

    const bar = w.find(".g-parent");
    expect(bar.exists()).toBe(true);
    expect(bar.find(".g-link").text()).toBe("西游记合集");

    await bar.find(".g-link").trigger("click");
    expect(sp.openDetail).toHaveBeenCalledWith(101);

    await groupBtn(w, "脱离聚合（挂回顶层）").trigger("click");
    await flush();
    expect(sp.moveNodes).toHaveBeenCalledWith([202], null, null);
  });
});

/* ==================== 取消聚合 ==================== */

describe("MediaDetail 取消聚合", () => {
  const parent = () => item(101, { display_title: "合集", child_count: 2 });
  const memA = () => item(201, { display_title: "甲", parent_id: 101 });
  const memB = () => item(202, { display_title: "乙", parent_id: 101 });

  /** 取消聚合是破坏性的：必须走确认，确认后**一次**把全部成员挂回顶层。 */
  it("先走确认，确认后一次性把全部成员挂回顶层；取消确认则什么都不做", async () => {
    const { w, sp } = mountDetail(parent(), [parent(), memA(), memB()]);

    await groupBtn(w, "取消聚合").trigger("click");
    await flush();

    expect(vi.mocked(askConfirm)).toHaveBeenCalledTimes(1);
    expect(vi.mocked(askConfirm).mock.calls[0][1]).toBe("取消聚合");
    expect(sp.moveNodes).toHaveBeenCalledWith([201, 202], null, null);

    // 用户在确认框里点了「取消」→ 一次请求都不发
    vi.mocked(askConfirm).mockResolvedValueOnce(false);
    await groupBtn(w, "取消聚合").trigger("click");
    await flush();
    expect(sp.moveNodes).toHaveBeenCalledTimes(1);
  });

  /**
   * ⚠️ 核心判据：搬的是**全量子级**，不是「当前筛选里看得见的那些」。
   * 造一条挂在父下、但不在 `store.items` 里的成员（模拟被栏目 / 分类 / 关键字
   * 挡住的条目），它也必须出现在 `moveNodes` 的 ids 里 —— 否则 `child_count`
   * 归不了零，聚合区永远收不起来，而文案却说「全部成员」。
   */
  it("搬的是全量子级：被当前筛选挡住的成员也一起挂回顶层", async () => {
    const p = item(101, { display_title: "合集", child_count: 3 });
    const a = item(201, { display_title: "甲", parent_id: 101 });
    const b = item(202, { display_title: "乙", parent_id: 101 });
    // 库里确实挂在 p 下，但当前筛选看不到它（不在 store.items 里）
    const hidden = item(203, { display_title: "丙（被筛选挡住）", parent_id: 101 });
    const { w, sp } = mountDetail(p, [p, a, b], { full: [p, a, b, hidden] });

    // 渲染口径**不变**：还是只显示看得见的 2 个（「记录到 3 / 能看到 2」）
    expect(w.findAll(".g-list li")).toHaveLength(2);
    expect(w.find(".g-count").text()).toContain("3");

    // 旧路径的取数来源（`members` / `orderedItems`）只有 2 条 —— 这就是漏搬的证据：
    // 谁把 `ids` 换回 `members.value.map(...)`，下面的 [201,202,203] 立刻变红
    expect(
      useMediaStore().orderedItems.filter((it) => it.parent_id === 101).map((it) => it.id),
    ).toEqual([201, 202]);

    await groupBtn(w, "取消聚合").trigger("click");
    await flush();

    // N 用**全量子级数**（3），并明说有几个不在筛选内
    const msg = String(vi.mocked(askConfirm).mock.calls[0][0]);
    expect(msg).toContain("全部 3 个");
    expect(msg).toContain("1 个不在当前筛选内");
    expect(sp.moveNodes).toHaveBeenCalledWith([201, 202, 203], null, null);
  });

  /**
   * 取全量失败时**绝不能**静默退回「只搬看得见的」：那会留下永远收不起来的
   * 半截聚合，比什么都不做更糟。判据：一个请求都不发、连确认框都不弹，
   * 把原因写进聚合区提示，用户还能重试。
   */
  it("取全量子级失败时不发 moveNodes，并把原因写进聚合区提示", async () => {
    const p = item(101, { display_title: "合集", child_count: 2 });
    const a = item(201, { display_title: "甲", parent_id: 101 });
    const b = item(202, { display_title: "乙", parent_id: 101 });
    const { w, sp } = mountDetail(p, [p, a, b], { fullError: "IPC 断了" });

    await groupBtn(w, "取消聚合").trigger("click");
    await flush();

    expect(sp.moveNodes).not.toHaveBeenCalled();
    expect(vi.mocked(askConfirm)).not.toHaveBeenCalled();
    const tip = w.find(".group .match-msg");
    expect(tip.exists()).toBe(true);
    expect(tip.text()).toContain("读取全部聚合成员失败");
    expect(tip.text()).toContain("IPC 断了");
    // 界面照旧：成员还在眼前、按钮还能再点（groupBusy 已复位）
    expect(w.findAll(".g-list li")).toHaveLength(2);
    expect(groupBtn(w, "取消聚合").attributes("disabled")).toBeUndefined();
  });

  /**
   * ⚠️ 这条用例原来的结论（"成员为空时不显示『取消聚合』按钮"）**已作废**，
   * 旧理由是「`ids = []` 会被后端拒，等于给用户一条假出路」。
   *
   * 那条理由现在**只对「真的没有子级」成立**：`disbandGroup` 已改成走
   * `allChildren()` 全量取数（task-6），哪怕当前筛选下一个成员都看不到，
   * 也能把筛选外的真实子级一次搬完 —— `ids` 不再是空。
   * 所以空态（`child_count > 0`、`members` 为空）里按钮**必须显示**：
   * 否则用户被迫先去别的栏目把成员找出来，才能做「解散这一组」这一件事。
   * 真正的反向判据见下面「真的没有子级」那条。
   */
  it("空态（child_count > 0 但当前看不到成员）仍显示「取消聚合」，并搬走筛选外的真实子级", async () => {
    const p = item(101, { display_title: "空聚合", child_count: 2 });
    // 两条都挂在 p 下、都不在当前筛选里（不在 store.items，所以渲染成空态）
    const h1 = item(211, { display_title: "筛选外甲", parent_id: 101 });
    const h2 = item(212, { display_title: "筛选外乙", parent_id: 101 });
    const { w, sp } = mountDetail(p, [p], { full: [p, h1, h2] });

    expect(w.find(".g-empty").exists()).toBe(true);
    expect(w.find(".g-list").exists()).toBe(false);

    const btn = groupBtn(w, "取消聚合");
    expect(btn.attributes("disabled")).toBeUndefined();

    await btn.trigger("click");
    await flush();

    expect(vi.mocked(askConfirm)).toHaveBeenCalledTimes(1);
    // N 仍是**全量子级数**（2），不是「看得见的 0 个」
    expect(String(vi.mocked(askConfirm).mock.calls[0][0])).toContain("全部 2 个");
    expect(sp.moveNodes).toHaveBeenCalledWith([211, 212], null, null);
  });

  /**
   * 反向判据：真的不是聚合（`child_count === 0` 且名下一个子成员都没有）时
   * 按钮不该出现 —— 别把「取消聚合」变成常态按钮。
   */
  it("真的没有子级（child_count === 0）时不显示「取消聚合」按钮", () => {
    // 它是别人的成员（所以聚合区那一块会渲染），但自己名下一个子级都没有
    const g = item(101, { display_title: "合集", child_count: 2 });
    const me = item(202, { display_title: "同组成员", parent_id: 101 });
    const solo = item(303, { display_title: "无子级条目", parent_id: 101 });
    const { w } = mountDetail(solo, [g, me, solo]);

    expect(w.find(".g-parent").exists()).toBe(true); // 聚合区在（「所属聚合」那一行）
    expect(w.find(".g-bar").exists()).toBe(false); // 但不是聚合 → 没有工具条
    expect(hasGroupBtn(w, "取消聚合")).toBe(false);
  });

  /**
   * 按钮现在按「记录到的子级数」显示，而 `child_count` 可能**已经过期**
   * （成员被删 / 被移走）。这种「点了没动静」比报错更难查，所以必须说话。
   */
  it("child_count 过期（记录到有子级、实际一个都没有）：不发请求，给出说明", async () => {
    const p = item(101, { display_title: "幽灵聚合", child_count: 3 });
    const { w, sp } = mountDetail(p, [p], { full: [p] }); // 全量里也没有

    await groupBtn(w, "取消聚合").trigger("click");
    await flush();

    expect(sp.moveNodes).not.toHaveBeenCalled();
    expect(vi.mocked(askConfirm)).not.toHaveBeenCalled();
    expect(w.find(".group .match-msg").text())
      .toContain("没有找到实际挂在这个聚合下的成员");
  });
});

/* ==================== 刷新与并发守卫 ==================== */

describe("MediaDetail 聚合后的刷新与并发守卫", () => {
  it("层级操作成功后重拉列表与详情；groupBusy 期间重复点击被守卫挡掉", async () => {
    const p = item(101, { display_title: "合集", child_count: 1 });
    const a = item(201, { display_title: "甲", parent_id: 101 });
    const { w, sp } = mountDetail(p, [p, a]);

    // 卡住这一次操作，制造「正在跑」的窗口
    let release!: (v: boolean) => void;
    const gate = new Promise<boolean>((resolve) => { release = resolve; });
    sp.moveNodes.mockReturnValue(gate);

    const btn = rowBtn(w, 0, "移出");
    await btn.trigger("click");
    await nextTick();

    // 跑着的期间按钮置灰，再点也不会再发一次（否则顺序会被连点点乱）
    expect(btn.attributes("disabled")).toBeDefined();
    await btn.trigger("click");
    await nextTick();
    expect(sp.moveNodes).toHaveBeenCalledTimes(1);

    release(true);
    await vi.waitFor(() => expect(sp.load).toHaveBeenCalled());
    expect(sp.openDetail).toHaveBeenCalledWith(101);
  });
});

/* ==================== 候选与不该渲染的情形 ==================== */

describe("MediaDetail 聚合的边界", () => {
  /** 自己与自己的后代都不能出现在候选里（会形成环）。 */
  it("加入聚合的候选排除自己与自己的后代（不成环）", () => {
    const p = item(101, { display_title: "合集", child_count: 1 });
    const c1 = item(202, { display_title: "子级", parent_id: 101, child_count: 1 });
    const c2 = item(303, { display_title: "孙级", parent_id: 202 });
    const free = item(404, { display_title: "无关条目" });
    const { w } = mountDetail(p, [p, c1, c2, free]);

    const opts = w.findAll(".g-add option").map((n) => n.text());
    expect(opts).toContain("无关条目");
    expect(opts).not.toContain("合集"); // 自己
    expect(opts).not.toContain("子级"); // 已在组里的成员
    expect(opts).not.toContain("孙级"); // 自己的后代
  });

  /** 既不是聚合、也不属于任何聚合 → 整块都不该出现。 */
  it("非聚合条目（无子成员、也不属于任何聚合）不渲染这一整块", () => {
    const solo = item(101, { display_title: "独立条目" });
    const { w } = mountDetail(solo, [solo, item(901)]);

    expect(w.find("section.group").exists()).toBe(false);
  });

  /** 老层级数据（老的 `parent_id`）要照原样读出来显示，不清空也不归一化。 */
  it("老层级数据（老 parent_id）原样读取显示，不清空也不归一化", () => {
    const p = item(101, { display_title: "老合集", child_count: 2 });
    const a = item(201, { display_title: "老成员甲", parent_id: 101 });
    const b = item(202, { display_title: "老成员乙", parent_id: 101 });
    const { w, sp } = mountDetail(p, [p, a, b]);

    const rows = w.findAll(".g-list li");
    expect(rows).toHaveLength(2);
    expect(rows[0].text()).toContain("老成员甲");
    expect(rows[1].text()).toContain("老成员乙");
    expect(w.find(".g-count").text()).toContain("2");
    // 只是读，不写：没有任何一次移动 / 归一化
    expect(sp.moveNodes).not.toHaveBeenCalled();
  });
});

/* ==================== 成员次序（跟随顶栏排序 vs 手工次序） ==================== */
/*
 * 成员次序有两档，判据是**显式标记** `settings.manualOrderGroups`：
 *
 * - 组**不在**名单里（默认）→ 跟随顶栏排序设置（`store.orderedItems` 的既定子序列）；
 * - 组**在**名单里 → 按库内同级 `sort_order`。
 *
 * 为什么不能再用「组里存在 `sort_order > 0`」当判据：任何一次 `move_nodes_sync`
 * （含在封面墙上拖动聚合）都会把目标同级整组重编号成 0..n-1，真实库里连从没排过的
 * 组都是 0,1,2,… —— 那条启发式恒为真，顶栏排序永远不生效（用户「看不到变化」）。
 *
 * 手工组仍然必须用 `sort_order`：`moveMember` 发出的 `index` 是喂给后端
 * `move_nodes_sync`（`siblings.sort_by_key(|id| (sort_order, id))`）的落点，两条序一旦
 * 错位，点「↑ / ↓」就会「看不出变化」或插到与预期不同的位置。
 */

describe("MediaDetail 成员次序：默认跟随顶栏排序", () => {
  const parent = () => item(101, { display_title: "合集", child_count: 3 });
  /*
   * ⚠️ 这一组是本任务的核心判据：**名字序与 sort_order 故意相反**。
   * 顶栏选「名称正序」时展示序是 A→B→C，而库内 sort_order 是 B(0)→C(1)→A(2)。
   * 只要成员列表还在按 sort_order 排（旧的默认行为），断言必红。
   */
  const memA = () => item(1011, { title: "A", display_title: "A", parent_id: 101, sort_order: 2 });
  const memB = () => item(1012, { title: "B", display_title: "B", parent_id: 101, sort_order: 0 });
  const memC = () => item(1013, { title: "C", display_title: "C", parent_id: 101, sort_order: 1 });

  it("组没有被手工排过 → 成员顺序 = 顶栏排序（与库内 sort_order 相反也照顶栏走）", () => {
    const p = parent();
    const { w } = mountDetail(p, [p, memA(), memB(), memC()], { by: "name", dir: "asc" });

    // 先证明「顶栏排序算出来的就是名字序」——它就是这一组要跟随的那条序
    const store = useMediaStore();
    expect(
      store.orderedItems.filter((it) => it.parent_id === 101).map((it) => it.title),
    ).toEqual(["A", "B", "C"]);

    // 渲染顺序 = 顶栏序（A→B→C），**不是** sort_order 序（B→C→A）
    expect(w.findAll(".g-list li .g-title").map((n) => n.text())).toEqual(["A", "B", "C"]);
    expect(w.findAll(".g-list li .g-no").map((n) => n.text())).toEqual(["1", "2", "3"]);
  });

  /** 倒序也照样跟随：证明跟的是顶栏那条**算出来的**序，不是碰巧。 */
  it("顶栏切倒序 → 成员顺序跟着倒过来", () => {
    const p = parent();
    const { w } = mountDetail(p, [p, memA(), memB(), memC()], { by: "name", dir: "desc" });
    expect(w.findAll(".g-list li .g-title").map((n) => n.text())).toEqual(["C", "B", "A"]);
  });

  /**
   * 老数据 `sort_order` 全 0 / 缺字段：不在名单里就跟随顶栏序，不抛错。
   * （旧用例的意图是「退回稳定 id 序」——那是默认就是库内序时的表现，已作废。）
   */
  it("老数据 sort_order 全 0 / undefined：跟随顶栏序且不抛错", () => {
    const p = item(101, { display_title: "老合集", child_count: 3 });
    const m1 = item(511, { title: "老甲", display_title: "老甲", parent_id: 101 });
    const m2 = item(133, { title: "老乙", display_title: "老乙", parent_id: 101, sort_order: 0 });
    const m3 = item(877, { title: "老丙", display_title: "老丙", parent_id: 101, sort_order: 0 });
    const { w } = mountDetail(p, [p, m1, m2, m3], { by: "name", dir: "asc" });

    // 名字序：老丙 → 老甲 → 老乙（id 序是 133 → 511 → 877，两者不同才有判据）
    expect(w.findAll(".g-list li .g-title").map((n) => n.text()))
      .toEqual(["老丙", "老甲", "老乙"]);
  });
});

describe("MediaDetail 成员次序：手工排过的组", () => {
  const parent = () => item(101, { display_title: "合集", child_count: 3 });
  /** 名字序 A→B→C，与下面的 sort_order 序（B→C→A）相反。 */
  const memA = () => item(1011, { title: "A", display_title: "A", parent_id: 101, sort_order: 2 });
  const memB = () => item(1012, { title: "B", display_title: "B", parent_id: 101, sort_order: 0 });
  const memC = () => item(1013, { title: "C", display_title: "C", parent_id: 101, sort_order: 1 });

  it("成员按同级 sort_order 渲染，不再受全局排序偏好影响", () => {
    const p = parent();
    // manual: [101] = 用户亲手排过这一组 → 走库内次序
    const { w } = mountDetail(p, [p, memA(), memB(), memC()], {
      by: "name", dir: "asc", manual: [101],
    });

    // 先证明「全局展示顺序确实是名字序」——非手工情形就是拿它当成员次序的
    const store = useMediaStore();
    expect(
      store.orderedItems.filter((it) => it.parent_id === 101).map((it) => it.title),
    ).toEqual(["A", "B", "C"]);

    // 成员次序只认 sort_order：B(0) → C(1) → A(2)；序号跟着渲染次序走
    expect(w.findAll(".g-list li .g-title").map((n) => n.text())).toEqual(["B", "C", "A"]);
    expect(w.findAll(".g-list li .g-no").map((n) => n.text())).toEqual(["1", "2", "3"]);
  });

  it("↑ / ↓ 发出的 index 与 sort_order 序（后端同级序）一致", async () => {
    const p = parent();
    // `full` 显式给全量序：每次移动后组件都会重取同级序，桩要一直答得出同一份
    const all = [p, memA(), memB(), memC()];
    const { w, sp } = mountDetail(p, [p, memA(), memB(), memC()], {
      by: "name", dir: "asc", manual: [101], full: all,
    });
    await flush(); // 等挂载时那次全量取数落地

    // 第 1 行是 B(1012)：下移一位 → 同级序里落到下标 1
    await rowBtn(w, 0, "↓").trigger("click");
    await flush();
    expect(sp.moveNodes).toHaveBeenCalledWith([1012], 101, 1);

    // 第 2 行是 C(1013)：上移 → 0，下移 → 2
    await rowBtn(w, 1, "↑").trigger("click");
    await flush();
    expect(sp.moveNodes).toHaveBeenCalledWith([1013], 101, 0);
    await rowBtn(w, 1, "↓").trigger("click");
    await flush();
    expect(sp.moveNodes).toHaveBeenCalledWith([1013], 101, 2);

    // 第 3 行是 A(1011)：上移 → 1。
    // 按名字排时这一行是 C，发出的 id 就会是 1013 —— 这条断言正是判据。
    await rowBtn(w, 2, "↑").trigger("click");
    await flush();
    expect(sp.moveNodes).toHaveBeenCalledWith([1011], 101, 1);

    // 边界按钮跟着**同级序**置灰，不是跟着名字序
    expect(rowBtn(w, 0, "↑").attributes("disabled")).toBeDefined();
    expect(rowBtn(w, 2, "↓").attributes("disabled")).toBeDefined();
    expect(rowBtn(w, 1, "↑").attributes("disabled")).toBeUndefined();
    expect(rowBtn(w, 1, "↓").attributes("disabled")).toBeUndefined();

    // 已经手工排过的组：**不**再走「先把默认序落库」那一步，只有 4 次真正的移动
    expect(sp.moveNodes).toHaveBeenCalledTimes(4);
  });
});

/* ==================== 加入聚合的候选（成环的两个方向） ==================== */
/*
 * 「＋ 加入聚合」的候选必须挡掉**成环的两个方向**：
 * - 向下：自己 + 自己的后代（`subtreeOf`）；
 * - 向上：自己的父 / 祖父（`ancestorsOf`）—— 只挡后代时下拉里会留下一条
 *   点下去必然被后端 `validate_drop` 判 `Cycle` 的出路（祖先方向同样成环）。
 *
 * 反向判据同样重要：挡过头会把候选清空，那是另一种「没出路」。
 */

describe("MediaDetail 加入聚合的候选（成环两个方向）", () => {
  /** A→B→C 三层：打开中间的 B，它既是别人的成员（有祖先）又有子级（是聚合）。 */
  it("三层 A→B→C，打开 B：祖先 A / 自己 B / 后代 C 都不在候选里，无关条目仍在", () => {
    const a = item(101, { display_title: "祖父A", child_count: 1 });
    const b = item(202, { display_title: "父B", parent_id: 101, child_count: 1 });
    const c = item(303, { display_title: "子C", parent_id: 202 });
    const free = item(404, { display_title: "无关条目" });
    const { w } = mountDetail(b, [a, b, c, free]);

    const opts = w.findAll(".g-add option").map((n) => n.text());
    expect(opts).toContain("无关条目");
    // 祖先方向：旧实现漏挡，这条选项真的出现在下拉里（点了必被后端拒）
    expect(opts).not.toContain("祖父A");
    expect(opts).not.toContain("父B"); // 自己
    expect(opts).not.toContain("子C"); // 后代
  });

  /** 反向判据：没有祖先时不能把候选整个清空（挡过头 = 另一种没出路）。 */
  it("顶层聚合（没有祖先）：普通条目照样留在候选里，没被清空", () => {
    const p = item(101, { display_title: "合集", child_count: 1 });
    const m = item(201, { display_title: "成员", parent_id: 101 });
    const x = item(404, { display_title: "无关甲" });
    const y = item(505, { display_title: "无关乙" });
    const { w } = mountDetail(p, [p, m, x, y]);

    expect(w.findAll(".g-add option").map((n) => n.text()))
      .toEqual(["选择要加入的条目…", "无关甲", "无关乙"]);
  });

  /** 成环脏数据：上溯必须靠 seen 停住，不能死循环 / 抛错（挂死会整个界面白屏）。 */
  it("成环脏数据（a 的父是 b、b 的父是 a）：候选计算不挂死、不抛错", () => {
    const a = item(101, { display_title: "环A", parent_id: 202, child_count: 1 });
    const b = item(202, { display_title: "环B", parent_id: 101, child_count: 1 });
    const x = item(404, { display_title: "无关条目" });
    const { w } = mountDetail(b, [a, b, x]);

    // 环上的两条互为自己祖先 → 全部挡掉；只有环外的条目可选
    expect(w.findAll(".g-add option").map((n) => n.text()))
      .toEqual(["选择要加入的条目…", "无关条目"]);
  });
});

/* ==================== 成员 ↑↓ 的 index（全量同级空间） ==================== */
/*
 * 与「成员次序」那一组是**两条正交的轴**：
 * - 那一组管「展示排序偏好 vs `sort_order`」；
 * - 这一组管「可见子集 vs 全量同级」—— `index` 是喂给后端
 *   `tree.rs::move_nodes_sync` 的落点，它在**全部** `parent_id = 自己` 的行里解释，
 *   而 UI 的行号只是「当前筛选看得见的那些」的下标。两个空间不一致时，
 *   点「↓」可能发出一个等于当前位置的下标 → 后端回「没有改动」，用户点了没反应。
 */

/**
 * 按后端 `move_nodes_sync`（`tree.rs:267-291`）的口径重放一次「插到第 `at` 位」：
 * `siblings` = 全量同级**排除被搬者**，`final = siblings[..at] ++ [id] ++ siblings[at..]`。
 *
 * 用它断言「位置真的变了」，而不是只比一个数字 —— 旧实现发的**可见下标**喂进
 * 同一个算法是**原地不动**，这一点由用例里显式的对照断言钉住。
 */
function replayMove(full: number[], id: number, at: number): number[] {
  const siblings = full.filter((x) => x !== id);
  const cut = Math.max(0, Math.min(at, siblings.length));
  return [...siblings.slice(0, cut), id, ...siblings.slice(cut)];
}

describe("MediaDetail 成员 ↑↓ 的 index（全量同级序）", () => {
  const parent = () => item(101, { display_title: "合集", child_count: 4 });
  /** 库里真实的同级序 A B C D。 */
  const siblings = () => [
    item(301, { display_title: "A", parent_id: 101, sort_order: 0 }),
    item(302, { display_title: "B", parent_id: 101, sort_order: 1 }),
    item(303, { display_title: "C", parent_id: 101, sort_order: 2 }),
    item(304, { display_title: "D", parent_id: 101, sort_order: 3 }),
  ];
  /** 当前筛选只看得见 B / D（A / C 被挡住）—— 两个空间的经典错位场面。 */
  const visible = () => [siblings()[1], siblings()[3]];
  const ORDER = [301, 302, 303, 304];

  it("可见子集只有 B / D：↓ / ↑ 的 index 在全量序里真的把成员挪了一位", async () => {
    const p = parent();
    const { w, sp } = mountDetail(p, [p, ...visible()], { full: [p, ...siblings()], manual: [101] });

    // 可见序只管渲染：只有两行
    expect(w.findAll(".g-list li .g-title").map((n) => n.text())).toEqual(["B", "D"]);

    // 第 1 行 B 在全量序里是第 2 位（前面还有 A）→ 下移一位
    await rowBtn(w, 0, "↓").trigger("click");
    await flush();
    const downIndex = sp.moveNodes.mock.calls[0][2] as number;
    expect(replayMove(ORDER, 302, downIndex)).toEqual([301, 303, 302, 304]);
    // 旧实现发的是**可见下标 1**：喂进同一算法原地不动 = 用户点了没反应
    expect(replayMove(ORDER, 302, 1)).toEqual(ORDER);

    // 第 2 行 D 在全量序里是最后一位 → 上移一位
    await rowBtn(w, 1, "↑").trigger("click");
    await flush();
    const upIndex = sp.moveNodes.mock.calls[1][2] as number;
    expect(replayMove(ORDER, 304, upIndex)).toEqual([301, 302, 304, 303]);
    // 旧实现发的可见下标 1 会把它往前挪**两位**（插到了 A 后面）—— 错位实证
    expect(replayMove(ORDER, 304, 1)).toEqual([301, 304, 302, 303]);

    // 落点具体值也钉一下（全量序位置 ± 1），父 id 恒为自己
    expect(sp.moveNodes).toHaveBeenNthCalledWith(1, [302], 101, 2);
    expect(sp.moveNodes).toHaveBeenNthCalledWith(2, [304], 101, 2);
  });

  it("置灰按全量序：可见第一行 B 前面还有 A，「↑」不灰；全量首 / 末才灰", async () => {
    const p = parent();
    const { w } = mountDetail(p, [p, ...visible()], { full: [p, ...siblings()], manual: [101] });
    await flush(); // 挂载时那次全量取数落地后才有置灰依据

    // B 是可见序第一行，但全量序里前面还有 A → 上移可用（旧实现按 i===0 会置灰）
    expect(rowBtn(w, 0, "↑").attributes("disabled")).toBeUndefined();
    expect(rowBtn(w, 0, "↓").attributes("disabled")).toBeUndefined();
    // D 是全量序最后一条 → 下移置灰；但它不是第一条 → 上移可用
    expect(rowBtn(w, 1, "↓").attributes("disabled")).toBeDefined();
    expect(rowBtn(w, 1, "↑").attributes("disabled")).toBeUndefined();
  });

  it("全量取数失败：↑ / ↓ 都不发 moveNodes，并把原因写进提示", async () => {
    const p = parent();
    const { w, sp } = mountDetail(p, [p, ...visible()], { fullError: "IPC 断了", manual: [101] });
    await flush();

    // 不静默置灰（否则用户连"为什么不能动"都看不到）
    expect(rowBtn(w, 0, "↓").attributes("disabled")).toBeUndefined();

    await rowBtn(w, 0, "↓").trigger("click");
    await flush();

    expect(sp.moveNodes).not.toHaveBeenCalled();
    const tip = w.find(".group .match-msg");
    expect(tip.text()).toContain("读取全部聚合成员失败");
    expect(tip.text()).toContain("IPC 断了");
  });

  /** 防回归：没有隐藏成员（全量 = 可见）时，行为与改造前一致。 */
  it("防回归：全量 = 可见时，↑↓ 仍是「挪一位」且首 / 末照旧置灰", async () => {
    const p = item(101, { display_title: "合集", child_count: 3 });
    const a = item(301, { display_title: "A", parent_id: 101, sort_order: 0 });
    const b = item(302, { display_title: "B", parent_id: 101, sort_order: 1 });
    const c = item(303, { display_title: "C", parent_id: 101, sort_order: 2 });
    const { w, sp } = mountDetail(p, [p, a, b, c], { manual: [101] }); // full 默认 = items
    await flush(); // 等挂载时那次全量取数落地（移动后还会再取一次）

    await rowBtn(w, 0, "↓").trigger("click");
    await flush();

    expect(sp.moveNodes).toHaveBeenLastCalledWith([301], 101, 1);
    expect(replayMove([301, 302, 303], 301, 1)).toEqual([302, 301, 303]);
    expect(rowBtn(w, 0, "↑").attributes("disabled")).toBeDefined();
    expect(rowBtn(w, 2, "↓").attributes("disabled")).toBeDefined();
  });
});

/* ============ 第一次 ↑↓：先把「跟随顶栏排序」的默认序落库，再转手工 ============ */
/*
 * 这一组钉的是本任务最容易做错的一处。组还没被标记时，界面显示的是**顶栏序**，
 * 而库里存的还是旧次序，两者通常不同。若用户一点 ↑ 就只记标记，显示会立刻改按库内
 * `sort_order` 渲染 —— 看到的就是「点了以后整个列表顺序全乱了」。
 *
 * 正确顺序（不可颠倒）：
 *   ① `moveNodes(当前渲染序, self, null)` —— 后端把这批按给定顺序重编号成 0..n-1，
 *      「用户看到的顺序」于是变成库内次序，视觉上完全不跳变；
 *   ② `markManualOrder(self)`；
 *   ③ 最后才是原本的 ↑/↓。
 */

describe("MediaDetail 首次 ↑↓：默认次序先落地", () => {
  const parent = () => item(101, { display_title: "合集", child_count: 3 });
  /*
   * 顶栏选「名称正序」→ 渲染序 A→B→C；库内 sort_order 故意是 A=2/B=0/C=1
   * （库内序 B→C→A）。两条序不同，才能证明「落库的是渲染序，不是库内序」。
   */
  const memA = () => item(1011, { title: "A", display_title: "A", parent_id: 101, sort_order: 2 });
  const memB = () => item(1012, { title: "B", display_title: "B", parent_id: 101, sort_order: 0 });
  const memC = () => item(1013, { title: "C", display_title: "C", parent_id: 101, sort_order: 1 });

  it("先发一次 moveNodes(当前渲染序, self, null) 落库，再发真正的移动，并标记手工", async () => {
    const p = parent();
    const { w, sp } = mountDetail(p, [p, memA(), memB(), memC()], { by: "name", dir: "asc" });
    const settings = useSettingsStore();
    settings.loaded = true; // markManualOrder 只在加载完成后才写库

    await flush(); // 等挂载时那次全量取数落地

    // 渲染序 = 顶栏序 A→B→C（不是库内序 B→C→A）
    expect(w.findAll(".g-list li .g-title").map((n) => n.text())).toEqual(["A", "B", "C"]);

    // 第 3 行 C 点「↑」：真正要移动的是 1013，与落库那一批（三个 id）区分得开
    await rowBtn(w, 2, "↑").trigger("click");
    await flush();

    // ① 第一次调用必须是「把当前渲染序整批落库」——id 顺序就是屏幕上的顺序
    expect(sp.moveNodes).toHaveBeenNthCalledWith(1, [1011, 1012, 1013], 101, null);
    // ② 之后才是真正的移动：落点按**全量同级序**（`(sort_order, id)`）算，
    //    这里库内序是 B(0)→C(1)→A(2)，C 在第 2 位 → ← 落到 0
    expect(sp.moveNodes).toHaveBeenNthCalledWith(2, [1013], 101, 0);
    expect(sp.moveNodes).toHaveBeenCalledTimes(2);

    // 顺序不可颠倒：落库那一次一定发生在真正的移动之前
    const first = sp.moveNodes.mock.calls[0];
    expect(first[2]).toBeNull();  // 落库：不带 index
    expect(first[0]).toEqual([1011, 1012, 1013]); // 三个成员，按渲染序

    // 标记为「手工排过」，且写回了设置（loaded=true）
    expect(settings.manualOrderGroups).toContain(101);
    expect(vi.mocked(setSettings)).toHaveBeenCalled();
  });

  /**
   * 落库失败（`moveNodes` 返回 false）：**不标记手工、也不发第二次移动**。
   * 否则用户会得到「顺序已经改成手工的、但库里还是旧序」这种半截状态。
   */
  it("落库失败：不标记手工、不发第二次移动，并把原因写进提示", async () => {
    const p = parent();
    const { w, sp } = mountDetail(p, [p, memA(), memB(), memC()], { by: "name", dir: "asc" });
    const settings = useSettingsStore();
    settings.loaded = true;
    sp.moveNodes.mockResolvedValueOnce(false); // 第一次（落库）失败

    await rowBtn(w, 0, "↓").trigger("click");
    await flush();

    expect(sp.moveNodes).toHaveBeenCalledTimes(1); // 只发了那一次落库
    expect(sp.moveNodes).toHaveBeenCalledWith([1011, 1012, 1013], 101, null);
    expect(settings.manualOrderGroups).not.toContain(101);
    expect(vi.mocked(setSettings)).not.toHaveBeenCalled();
    expect(w.find(".group .match-msg").text()).toContain("没有改动排序");
    // 界面照旧：成员还在，顺序还是顶栏序（没有被半截写入打乱）
    expect(w.findAll(".g-list li .g-title").map((n) => n.text())).toEqual(["A", "B", "C"]);
  });

  /** 已经手工排过的组：点 ↑↓ 只有一次移动，不再重播「落库」那一步。 */
  it("已手工排过的组：点 ↑↓ 只发一次真正的移动", async () => {
    const p = parent();
    const { w, sp } = mountDetail(p, [p, memA(), memB(), memC()], {
      by: "name", dir: "asc", manual: [101],
    });
    const settings = useSettingsStore();
    settings.loaded = true;

    await rowBtn(w, 2, "↑").trigger("click"); // 第 3 行 A（库内序里最后一位）
    await flush();

    expect(sp.moveNodes).toHaveBeenCalledTimes(1);
    expect(sp.moveNodes).toHaveBeenCalledWith([1011], 101, 1);
  });

  /** 连点保护必须覆盖「落库」那一段：加载还没回来时再点，不能发第二个请求。 */
  it("落库期间再点一次：被 groupBusy 挡掉，不会重复落库", async () => {
    const p = parent();
    const { w, sp } = mountDetail(p, [p, memA(), memB(), memC()], { by: "name", dir: "asc" });
    const settings = useSettingsStore();
    settings.loaded = true;

    let release!: (v: boolean) => void;
    sp.moveNodes.mockReturnValueOnce(new Promise<boolean>((r) => { release = r; }));

    // 第 1 行 A 点「↑」：库内序里 A 在第 3 位 → 上移落在 1，是合法落点
    const up = rowBtn(w, 0, "↑");
    await up.trigger("click");
    await nextTick();
    // 正在落库：这一行的按钮全部置灰
    expect(up.attributes("disabled")).toBeDefined();
    await up.trigger("click");
    await nextTick();
    expect(sp.moveNodes).toHaveBeenCalledTimes(1);

    release(true);
    await flush();
    // 放行之后才轮到真正的移动（整个 groupBusy 窗口里只发了这两次）
    expect(sp.moveNodes).toHaveBeenCalledTimes(2);
    expect(sp.moveNodes).toHaveBeenNthCalledWith(2, [1011], 101, 1);
  });
});

/* ==================== 「恢复默认排序」 ==================== */
/*
 * 入口只在**手工态**出现：跟随顶栏排序的组本来就没什么可恢复的，
 * 常显只会让用户以为「我改过什么」。它**只清标记**，不搬动任何成员。
 */

describe("MediaDetail 恢复默认排序", () => {
  const parent = () => item(101, { display_title: "合集", child_count: 2 });
  const memA = () => item(1011, { title: "A", display_title: "A", parent_id: 101, sort_order: 1 });
  const memB = () => item(1012, { title: "B", display_title: "B", parent_id: 101, sort_order: 0 });

  it("手工态才出现；点击只清标记（一个 media 行都不动）", async () => {
    const p = parent();
    const { w, sp } = mountDetail(p, [p, memA(), memB()], {
      by: "name", dir: "asc", manual: [101],
    });
    const settings = useSettingsStore();
    settings.loaded = true;

    // 手工态：按库内 sort_order 显示（B 0 → A 1）
    expect(w.findAll(".g-list li .g-title").map((n) => n.text())).toEqual(["B", "A"]);

    const btn = groupBtn(w, "恢复默认排序");
    expect(btn.attributes("title")).toContain("不会搬动任何成员");
    await btn.trigger("click");
    await flush();

    // 标记清掉 → 显示改回顶栏序（A→B）
    expect(settings.manualOrderGroups).not.toContain(101);
    expect(w.findAll(".g-list li .g-title").map((n) => n.text())).toEqual(["A", "B"]);
    // **只清标记**：没有任何一次层级写入
    expect(sp.moveNodes).not.toHaveBeenCalled();
    // 按钮也随之消失（已经不是手工态）
    expect(hasGroupBtn(w, "恢复默认排序")).toBe(false);
  });

  it("非手工态不显示这个按钮", () => {
    const p = parent();
    const { w } = mountDetail(p, [p, memA(), memB()], { by: "name", dir: "asc" });
    expect(hasGroupBtn(w, "恢复默认排序")).toBe(false);
  });
});

/* ==================== 边界：所属聚合取不到 ==================== */

describe("MediaDetail 所属聚合不在列表里", () => {
  /**
   * 父条目被删（或被当前筛选挡住）时，不能只留一个可点的 `#id`：
   * 点进去必然失败，用户拿到一句看不懂的报错。
   *
   * 判据复用 `parentTitle` 的查找结果（显示成 `#id` 就是取不到），
   * 所以这里断言的是"显示成 `#id` 且**不是**链接"。
   */
  it("父条目取不到：显示成灰字而非可点链接，并给出说明与出路", async () => {
    const me = item(2, { parent_id: 777 }); // 列表里没有 777
    const { w } = mountDetail(me, [me, item(3)]);
    await nextTick();

    expect(w.find(".g-dead").exists()).toBe(true);
    expect(w.find(".g-dead").text()).toContain("#777");
    expect(w.find(".g-link").exists()).toBe(false);
    expect(w.find(".g-hint").text()).toContain("脱离聚合");
  });

  it("父条目还在列表里：照旧是可点链接，没有那条说明", async () => {
    const parent = item(1, { child_count: 1 });
    const me = item(2, { parent_id: 1 });
    const { w } = mountDetail(me, [parent, me]);
    await nextTick();

    expect(w.find(".g-link").exists()).toBe(true);
    expect(w.find(".g-link").text()).toContain("标题1");
    expect(w.find(".g-dead").exists()).toBe(false);
    expect(w.find(".g-hint").exists()).toBe(false);
  });

  /** 取不到父条目时，「脱离聚合」仍然必须可用 —— 那是用户唯一的出路。 */
  it("父条目取不到时「脱离聚合」仍然可用", async () => {
    const me = item(2, { parent_id: 777 });
    const { w, sp } = mountDetail(me, [me, item(3)]);
    await nextTick();

    const btn = groupBtn(w, "脱离聚合");
    expect(btn.attributes("disabled")).toBeUndefined();
    await btn.trigger("click");
    await nextTick();
    expect(sp.moveNodes).toHaveBeenCalledWith([2], null, null);
  });
});
