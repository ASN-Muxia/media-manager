import { describe, expect, it, vi } from "vitest";
import { mount } from "@vue/test-utils";
import { createPinia, setActivePinia } from "pinia";
import { defineComponent, h } from "vue";
import App from "./App.vue";
import { useMediaStore } from "./stores/mediaStore";
import { useSettingsStore } from "./stores/settingsStore";

/**
 * 顶栏「选择」/「聚合」两个模式开关的可见性契约。
 *
 * 需求原话：「上方选择功能开启时，按钮不从快捷栏消失。」
 *
 * 旧实现用 v-if="!media.selecting" / v-if="!media.grouping" 把按钮整块摘掉，
 * 于是点进去之后那个位置就空了 —— 用户既看不到"我正处于这个模式"，
 * 也找不到原路退出的按钮（只能去工具条最右边找「退出选择」）。
 * 这里钉住的是：**模式开着时按钮仍然在，且文案与 aria-pressed 同步翻转**。
 *
 * 三个子组件全部桩掉：这条用例只关心顶栏，不该被封面墙 / 详情页 / 设置页的
 * 内部实现（以及它们各自的 mock 依赖）拖住。
 */

/*
 * ⚠️ 桩必须**内联**写在 vi.mock 的工厂里：vi.mock 会被提升到文件顶部，
 * 工厂函数在模块初始化之前就执行了，引用外部 const（哪怕只是同一个文件里的）
 * 会直接 "Cannot access ... before initialization"。所以这里重复写六遍
 * defineComponent，而不是抽一个 Stub 帮助函数。
 */
vi.mock("./components/Sidebar.vue", () => ({
  default: defineComponent({ name: "sidebar-stub", setup: () => () => h("div") }),
}));
vi.mock("./components/PosterGrid.vue", () => ({
  // 声明 props（而不是让它落成凡尔赛 attrs）：下面那条用例要断言 App 真的把
  // `manualOrderGroups` 传进来了 —— 没有 props 声明时 `props()` 查不到它。
  default: defineComponent({
    name: "grid-stub",
    props: ["items", "manualOrderGroups", "collapsedGroups"],
    setup: () => () => h("div"),
  }),
}));
vi.mock("./components/MediaDetail.vue", () => ({
  default: defineComponent({ name: "detail-stub", setup: () => () => h("div") }),
}));
vi.mock("./components/SettingsPanel.vue", () => ({
  default: defineComponent({ name: "settings-stub", setup: () => () => h("div") }),
}));
vi.mock("./components/MediaEditDialog.vue", () => ({
  default: defineComponent({ name: "edit-stub", setup: () => () => h("div") }),
}));
vi.mock("./components/ImportFolderDialog.vue", () => ({
  default: defineComponent({ name: "import-stub", setup: () => () => h("div") }),
}));
vi.mock("./components/LibraryCatalog.vue", () => ({
  default: defineComponent({ name: "catalog-stub", setup: () => () => h("div") }),
}));

/** 后端命令一律桩掉：挂载时 App 会 load 设置 / 分类 / 列表 / 历史。 */
vi.mock("./api/commands", async () => {
  const actual = await vi.importActual<typeof import("./api/commands")>("./api/commands");
  return {
    ...actual,
    getSettings: vi.fn(async () => ({})),
    setSettings: vi.fn(async () => undefined),
    getMediaList: vi.fn(async () => []),
    getScanHistory: vi.fn(async () => []),
    listCategories: vi.fn(async () => []),
    listTags: vi.fn(async () => []),
    onScanProgress: vi.fn(async () => () => {}),
    onScrapeProgress: vi.fn(async () => () => {}),
    recentPlayback: vi.fn(async () => []),
    getPlaybackMany: vi.fn(async () => ({})),
  };
});

function buttonByText(w: ReturnType<typeof mount>, text: string) {
  return w.findAll("button").find((b) => b.text().includes(text));
}

async function mountApp() {
  const w = mount(App, { attachTo: document.body });
  await new Promise((r) => setTimeout(r, 0));
  await w.vm.$nextTick();
  return w;
}

describe("App 顶栏模式开关", () => {
  it("普通状态：两个按钮都在，文案是「选择」/「聚合」", async () => {
    setActivePinia(createPinia());
    const w = await mountApp();
    expect(buttonByText(w, "选择")).toBeTruthy();
    expect(buttonByText(w, "聚合")).toBeTruthy();
    w.unmount();
  });

  it("进入选择模式后：按钮不消失，文案变「退出选择」且 aria-pressed=true", async () => {
    setActivePinia(createPinia());
    const w = await mountApp();
    const media = useMediaStore();
    media.selecting = true;
    await w.vm.$nextTick();

    const btn = buttonByText(w, "退出选择");
    expect(btn, "开启选择后按钮必须仍在快捷栏里").toBeTruthy();
    expect(btn!.attributes("aria-pressed")).toBe("true");
    w.unmount();
  });

  it("进入聚合模式后：按钮不消失，文案变「退出聚合」且 aria-pressed=true", async () => {
    setActivePinia(createPinia());
    const w = await mountApp();
    const media = useMediaStore();
    media.grouping = true;
    await w.vm.$nextTick();

    const btn = buttonByText(w, "退出聚合");
    expect(btn, "开启聚合后按钮必须仍在快捷栏里").toBeTruthy();
    expect(btn!.attributes("aria-pressed")).toBe("true");
    w.unmount();
  });

  /** 两个模式互斥：开着选择时点「聚合」，选择必须被关掉（不能两条工具条叠着）。 */
  it("两个模式互斥：从选择切到聚合，selecting 被关掉", async () => {
    setActivePinia(createPinia());
    const w = await mountApp();
    const media = useMediaStore();
    media.selecting = true;
    await w.vm.$nextTick();

    await buttonByText(w, "聚合")!.trigger("click");
    expect(media.grouping).toBe(true);
    expect(media.selecting).toBe(false);
    w.unmount();
  });

  /** 点同一个按钮两次回到普通状态（开关语义）。 */
  it("再点一次「退出聚合」回到普通状态", async () => {
    setActivePinia(createPinia());
    const w = await mountApp();
    const media = useMediaStore();
    media.grouping = true;
    await w.vm.$nextTick();

    await buttonByText(w, "退出聚合")!.trigger("click");
    expect(media.grouping).toBe(false);
    expect(buttonByText(w, "聚合")).toBeTruthy();
    w.unmount();
  });
});
/**
 * 资源目录库入口。
 *
 * 它和别的内置入口有一条关键差别：**数据源不是媒体列表**（resource_assets 表），
 * 所以切过去时不能去查媒体列表，也不该把封面墙的筛选 / 排序 / 模式开关留在顶栏上
 * —— 那些控件筛的是「作品」，和这份「磁盘文件清单」没有关系。
 */
/**
 * 封面墙的 `manualOrderGroups` 必须由 App 传进去。
 *
 * 它是组内次序的判据（手工排过的组按库内 sort_order，其余跟随顶栏排序）。
 * 组件本身**不** `useSettingsStore()`（纯展示 + 手势组件，保持无 store 依赖才好
 * 单独挂载测试），所以这条线一旦忘了接，标记就永远传不到墙上：用户手工排过的
 * 分组会在封面墙上退回顶栏序 —— 静默失效，没有任何报错。这里把它钉住。
 */
describe("App → PosterGrid 的 manualOrderGroups", () => {
  it("把 settings.manualOrderGroups 原样传给封面墙", async () => {
    setActivePinia(createPinia());
    const w = await mountApp();
    const settings = useSettingsStore();
    settings.manualOrderGroups = [14135, 14150];
    await w.vm.$nextTick();

    const grid = w.findComponent({ name: "grid-stub" });
    expect(grid.props("manualOrderGroups")).toEqual([14135, 14150]);
    w.unmount();
  });
});

/**
 * 展开态记忆的**接线**用例。
 *
 * 需求原话：「对这个聚合的展开与收缩进行一个记忆，回到主页的时候留存我最后一次的操作，
 * 而不是直接全展开或者全收缩起来」。
 *
 * 真正的根因在 `App.vue`：封面墙挂在 `v-if/v-else` 链上（目录库 / 详情页 /
 * 设置页都会把它换下去），组件一旦卸载，它内部的 `expanded` / `collapsedByUser`
 * 就没了；回来重建时「新分组自动展开」的 watch 又把一切摊开。
 *
 * 所以修复必须**跨组件生命周期**：折叠名单由 settingsStore 持有并落库，
 * 经 `collapsedGroups` prop 传下去、`collapsed-change` 事件收回来。
 * 这一组钉的就是那条线有没有接上（少任何一半都会静默退化成"每次回来全展开"）。
 */
describe("App 展开态记忆接线", () => {
  it("把 settings.collapsedGroups 传给封面墙", async () => {
    setActivePinia(createPinia());
    const w = await mountApp();
    const settings = useSettingsStore();
    settings.collapsedGroups = [14135];
    await w.vm.$nextTick();

    const grid = w.findComponent({ name: "grid-stub" });
    expect(grid.props("collapsedGroups")).toEqual([14135]);
    w.unmount();
  });

  /**
   * 🔑 核心：**切走再切回来**（封面墙被卸载又重建），折叠名单必须还在。
   * 这正是用户描述的「回到主页」那一步 —— 也是原先丢状态的地方。
   */
  it("切到目录库再切回「全部」：封面墙重建后仍带着折叠名单", async () => {
    setActivePinia(createPinia());
    const w = await mountApp();
    const settings = useSettingsStore();
    const vm = w.vm as unknown as { onNav: (v: string) => Promise<void> };

    // 用户在封面墙上收起了 14135（封面墙通过 collapsed-change 报上来）
    settings.setCollapsedGroups([14135]);
    await w.vm.$nextTick();
    settings.collapsedGroups = [14135];

    // 切走 → 封面墙被卸载
    await vm.onNav("catalog");
    await w.vm.$nextTick();
    expect(w.findComponent({ name: "grid-stub" }).exists()).toBe(false);

    // 切回来 → 封面墙重建，必须还原成"收着"的状态
    await vm.onNav("all");
    await w.vm.$nextTick();
    const grid = w.findComponent({ name: "grid-stub" });
    expect(grid.exists()).toBe(true);
    expect(grid.props("collapsedGroups")).toEqual([14135]);
    w.unmount();
  });

  it("封面墙报上来的折叠变化会落进 settings", async () => {
    setActivePinia(createPinia());
    const w = await mountApp();
    const settings = useSettingsStore();
    const grid = w.findComponent({ name: "grid-stub" });

    grid.vm.$emit("collapsed-change", [14135, 14150]);
    await w.vm.$nextTick();

    expect(settings.collapsedGroups).toEqual([14135, 14150]);
    w.unmount();
  });
});
describe("App 资源目录库入口", () => {
  it("切到目录库视图：挂出目录库面板，封面墙让位", async () => {
    setActivePinia(createPinia());
    const w = await mountApp();
    // 侧边栏被桩掉了，所以直接驱动 App 的视图状态：等价于点侧栏那一项
    await (w.vm as unknown as { onNav: (v: string) => Promise<void> }).onNav("catalog");
    await w.vm.$nextTick();

    expect(w.findComponent({ name: "catalog-stub" }).exists()).toBe(true);
    expect(w.findComponent({ name: "grid-stub" }).exists()).toBe(false);
    w.unmount();
  });

  it("目录库视图下：媒体列表的筛选与模式开关都不显示", async () => {
    setActivePinia(createPinia());
    const w = await mountApp();
    await (w.vm as unknown as { onNav: (v: string) => Promise<void> }).onNav("catalog");
    await w.vm.$nextTick();

    // 搜索框 / 选择 / 聚合 在目录库视图下都该让位给面板自己的控件
    expect(w.find(".search").exists()).toBe(false);
    expect(buttonByText(w, "选择")).toBeFalsy();
    expect(buttonByText(w, "聚合")).toBeFalsy();
    w.unmount();
  });

  it("从目录库切回「全部」：封面墙与按钮都回来", async () => {
    setActivePinia(createPinia());
    const w = await mountApp();
    const vm = w.vm as unknown as { onNav: (v: string) => Promise<void> };
    await vm.onNav("catalog");
    await w.vm.$nextTick();
    await vm.onNav("all");
    await w.vm.$nextTick();

    expect(w.findComponent({ name: "catalog-stub" }).exists()).toBe(false);
    expect(w.findComponent({ name: "grid-stub" }).exists()).toBe(true);
    expect(buttonByText(w, "选择")).toBeTruthy();
    w.unmount();
  });
});
