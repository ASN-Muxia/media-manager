import { beforeEach, describe, expect, it, vi } from "vitest";
import { mount } from "@vue/test-utils";
import { createPinia, setActivePinia } from "pinia";
import { defineComponent, h } from "vue";
import App from "./App.vue";
import { useMediaStore } from "./stores/mediaStore";
import { usePlaybackStore } from "./stores/playbackStore";
import { useCategoryStore } from "./stores/categoryStore";
import { useSettingsStore } from "./stores/settingsStore";
import type { MediaItem } from "./api/commands";

/**
 * 首屏启动契约（App.vue 的 onMounted）。
 *
 * 钉住三件事：
 * 1. 五个启动请求**并行发出** —— 不能有任何一个在等前一个返回。
 *    串行症状很明确：第一条不 resolve 时，后面几条的命令根本不会被调用。
 * 2. 某一条失败**不连累其余** —— 裸 Promise.all 会整体 reject，
 *    这里要求「设置挂了，媒体列表照样加载」「历史挂了，设置照样加载」。
 * 3. 启动全程**不写设置** —— persistSort() 的 loaded 守卫靠的就是
 *    「设置加载完成前没有写库路径」，这条不能破。
 *
 * ⚠️ 桩必须**内联**写在 vi.mock 的工厂里：vi.mock 会被提升到文件顶部，
 * 工厂在模块初始化之前执行，引用外部 const（哪怕同一个文件里的）会直接
 * "Cannot access ... before initialization"。
 */

/** 后端命令桩；各用例自己设返回值，beforeEach 里先铺一层成功默认值。 */
const mocks = vi.hoisted(() => ({
  getSettings: vi.fn(),
  setSettings: vi.fn(),
  getMediaList: vi.fn(),
  getMediaDetail: vi.fn(),
  getScanHistory: vi.fn(),
  listCategories: vi.fn(),
  listTags: vi.fn(),
  onScanProgress: vi.fn(),
  onScrapeProgress: vi.fn(),
  recentPlayback: vi.fn(),
  getPlaybackMany: vi.fn(),
}));

vi.mock("./api/commands", () => mocks);

/*
 * 七个子组件全部桩掉：这条用例只关心「启动阶段发了哪些 IPC 请求」，
 * 不该被封面墙 / 详情页 / 设置页的内部实现（及各自的 mock 依赖）拖住。
 */
vi.mock("./components/Sidebar.vue", () => ({
  default: defineComponent({ name: "sidebar-stub", setup: () => () => h("div") }),
}));
vi.mock("./components/PosterGrid.vue", () => ({
  default: defineComponent({ name: "grid-stub", setup: () => () => h("div") }),
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

function item(over: Partial<MediaItem> = {}): MediaItem {
  return {
    id: 1,
    type: "movie",
    title: "Inception",
    year: 2010,
    tmdb_id: null,
    poster_path: null,
    backdrop_path: null,
    overview: null,
    vote_average: null,
    genre: "动作 / 科幻",
    file_path: null,
    file_name: null,
    episode_count: 0,
    added_at: 1,
    display_title: "Inception",
    ...over,
  };
}

/** 把所有已排队的微任务跑干净（挂载后启动链要推进若干轮）。 */
async function flush(): Promise<void> {
  for (let i = 0; i < 8; i += 1) await Promise.resolve();
}

/** 永不 resolve 的 Promise：用来证明「没等前面返回就已经往下走了」。 */
function pending<T>(): Promise<T> {
  return new Promise<T>(() => {});
}

/** 铺一层成功默认值；各用例再按需覆盖其中某一条。 */
function seedHappyPath(): void {
  mocks.getSettings.mockResolvedValue({});
  mocks.setSettings.mockResolvedValue(undefined);
  mocks.getMediaList.mockResolvedValue([item()]);
  mocks.getScanHistory.mockResolvedValue([]);
  mocks.listCategories.mockResolvedValue([]);
  mocks.listTags.mockResolvedValue([]);
  mocks.onScanProgress.mockResolvedValue(() => {});
  mocks.onScrapeProgress.mockResolvedValue(() => {});
  mocks.recentPlayback.mockResolvedValue([]);
  mocks.getPlaybackMany.mockResolvedValue({});
}

beforeEach(() => {
  setActivePinia(createPinia());
  seedHappyPath();
});

describe("App 首屏启动：五个请求并行发出", () => {
  /**
   * 核心用例：让六次命令**全部挂起**（永不返回），再数它们有没有都被调用。
   * 串行实现下这时只会有第一条（设置）被调用；并行实现下同一轮里全部发出。
   */
  it("任何一条都还没返回时，命令就已全部发出，且非首屏关键的两条排在最后", async () => {
    const called: string[] = [];
    mocks.getSettings.mockImplementation(() => {
      called.push("settings");
      return pending();
    });
    mocks.listCategories.mockImplementation(() => {
      called.push("categories");
      return pending();
    });
    mocks.listTags.mockImplementation(() => {
      called.push("tags");
      return pending();
    });
    mocks.getMediaList.mockImplementation(() => {
      called.push("media");
      return pending();
    });
    mocks.getScanHistory.mockImplementation(() => {
      called.push("history");
      return pending();
    });
    mocks.recentPlayback.mockImplementation(() => {
      called.push("recent");
      return pending();
    });

    const w = mount(App, { attachTo: document.body });
    await flush();

    // 一条都没返回，但六次命令调用都已发生 —— 这就是并行。
    // 设置 / 分类+标签 / 列表 属于首屏关键，先发；
    // 扫描历史与最近播放非首屏关键，放在最后。
    expect(called).toEqual(["settings", "categories", "tags", "media", "history", "recent"]);

    w.unmount();
  });

  /** 首屏关键的三条互不等待：设置还没回来，列表请求已经发出去了。 */
  it("设置未加载完成时，分类与媒体列表的请求也已发出", async () => {
    mocks.getSettings.mockImplementation(() => pending());

    const w = mount(App, { attachTo: document.body });
    await flush();

    expect(useSettingsStore().loaded).toBe(false);
    expect(mocks.listCategories).toHaveBeenCalledTimes(1);
    expect(mocks.getMediaList).toHaveBeenCalledTimes(1);

    w.unmount();
  });
});

describe("App 首屏启动：单条失败不连累其余", () => {
  /**
   * 非首屏关键的扫描历史 reject：设置 / 分类 / 列表必须照常加载完成。
   * 这是「各自 catch」相对裸 Promise.all 的差别所在。
   */
  it("扫描历史命令失败：settings.loaded 仍为 true，媒体列表仍有值", async () => {
    mocks.getScanHistory.mockRejectedValue(new Error("历史命令挂了"));
    mocks.getMediaList.mockResolvedValue([item({ id: 7, title: "七" })]);

    const w = mount(App, { attachTo: document.body });
    await flush();

    expect(useSettingsStore().loaded).toBe(true);
    expect(useSettingsStore().error).toBeNull();
    const media = useMediaStore();
    expect(media.items.map((i) => i.id)).toEqual([7]);
    // 历史自己的失败只写回自己那份状态（空数组），不外溢到别的 store
    expect(media.scanHistory).toEqual([]);
    expect(usePlaybackStore().error).toBeNull();

    w.unmount();
  });

  /**
   * 反向用例：首屏关键的设置命令 reject，同样不能挡住后面几条。
   * 串行实现下「设置挂了」会让分类 / 列表 / 历史 / 最近播放一条都不跑。
   */
  it("设置命令失败：分类、媒体列表、历史、最近播放照常发出", async () => {
    mocks.getSettings.mockRejectedValue(new Error("设置命令挂了"));
    mocks.getMediaList.mockResolvedValue([item({ id: 3 })]);

    const w = mount(App, { attachTo: document.body });
    await flush();

    const settings = useSettingsStore();
    expect(settings.loaded).toBe(false);
    expect(settings.error).toBeTruthy();
    // 失败被吞在自己那一条里：后面的照跑
    expect(mocks.listCategories).toHaveBeenCalledTimes(1);
    expect(mocks.getMediaList).toHaveBeenCalledTimes(1);
    expect(useMediaStore().items.map((i) => i.id)).toEqual([3]);
    expect(mocks.getScanHistory).toHaveBeenCalledTimes(1);
    expect(mocks.recentPlayback).toHaveBeenCalledTimes(1);

    w.unmount();
  });

  /** 启动全程不得写设置：persistSort() 的 loaded 守卫前提就是「没人抢先写」。 */
  it("启动期间不调用 setSettings", async () => {
    const w = mount(App, { attachTo: document.body });
    await flush();

    expect(mocks.setSettings).not.toHaveBeenCalled();
    w.unmount();
  });

  /**
   * 「各自兜底」相对裸 Promise.all 的真正差别：一个没有内建 try/catch 的 store
   * 方法 reject 时，裸 Promise.all 会把整条链变成**未处理的 rejection**，
   * 而各自兜底的写法把它吞在本条内。
   *
   * 这里直接把分类 store 的 load 换成会抛的实现（模拟"某个 store 没兜住"），
   * 断言：① 没有 unhandledrejection；② 另一条独立的请求（媒体列表）照常完成。
   */
  it("某条请求裸抛异常时：不产生 unhandledrejection，且其余请求照常完成", async () => {
    const unhandled: unknown[] = [];
    const onUnhandled = (e: unknown): void => {
      unhandled.push(e);
    };
    window.addEventListener("unhandledrejection", onUnhandled);

    const category = useCategoryStore();
    const boom = new Error("分类 store 没兜住的异常");
    vi.spyOn(category, "load").mockRejectedValue(boom);
    mocks.getMediaList.mockResolvedValue([item({ id: 5 })]);

    const w = mount(App, { attachTo: document.body });
    await flush();
    // 给 unhandledrejection 的派发留一轮宏任务
    await new Promise((r) => setTimeout(r, 0));

    expect(unhandled).toEqual([]);
    expect(useMediaStore().items.map((i) => i.id)).toEqual([5]);
    expect(mocks.getScanHistory).toHaveBeenCalledTimes(1);
    expect(mocks.recentPlayback).toHaveBeenCalledTimes(1);

    window.removeEventListener("unhandledrejection", onUnhandled);
    w.unmount();
  });
});

describe("App 首屏启动：列表变化仍然驱动观看状态", () => {
  /**
   * 挂在 watch(media.items) 上的 loadMany 不能被动掉：
   * 有值时要拉进 map，后来变成空列表时旧角标也必须被清掉。
   *
   * 注意：空列表这一轮 store 内部会短路（`loadMany` 直接置空 map、不发 IPC），
   * 所以这里断言的是**可见结果**（角标消失），而不是命令有没有再被调用一次。
   */
  it("列表有值后观看状态进 map；列表变空时旧角标被清掉", async () => {
    mocks.getMediaList.mockResolvedValue([item({ id: 11 })]);
    mocks.getPlaybackMany.mockResolvedValue({
      "11": {
        mediaId: 11, watched: true, watchedEpisodes: 1, totalEpisodes: 1,
        playCount: 1, lastPlayedAt: null, resumeEpisodeId: null,
        lastEpisodeId: null, lastSeason: null, lastEpisode: null,
      },
    });
    const w = mount(App, { attachTo: document.body });
    await flush();
    await w.vm.$nextTick();
    await flush();

    const playback = usePlaybackStore();
    expect(mocks.getPlaybackMany).toHaveBeenCalledWith([11]);
    expect(playback.isWatched(11)).toBe(true);

    // 再加载一次，后端这次返回空列表（例如库被清空）：
    // watch(media.items) 照常跑 loadMany([])，旧角标必须被清掉
    mocks.getMediaList.mockResolvedValue([]);
    await useMediaStore().load();
    await w.vm.$nextTick();

    expect(playback.map).toEqual({});
    expect(playback.isWatched(11)).toBe(false);

    w.unmount();
  });
});
