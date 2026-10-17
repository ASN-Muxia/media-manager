import { beforeEach, describe, expect, it, vi } from "vitest";
import { createPinia, setActivePinia } from "pinia";
import { nextTick } from "vue";
import { useMediaStore } from "./mediaStore";
import { useSettingsStore } from "./settingsStore";
import type { MediaItem, ScanProgress } from "../api/commands";

/**
 * mediaStore 是界面的中枢，这里覆盖三块最容易出错的逻辑：
 * 1. 关键字过滤（拼了 display_title / title / genre / year 四个字段）；
 * 2. 扫描的「起任务 → 事件驱动收尾」流程（命令立即返回，结果靠事件）；
 * 3. 资源的新建 / 编辑（成功后重载列表、同步详情，失败时给出可读提示）。
 *
 * 所有后端命令都 mock 掉：单测不该依赖真实 Tauri 运行时。
 */
const mocks = vi.hoisted(() => ({
  getMediaList: vi.fn(),
  getMediaDetail: vi.fn(),
  getScanHistory: vi.fn(),
  scanLibrary: vi.fn(),
  cancelScan: vi.fn(),
  onScanProgress: vi.fn(),
  scrapeAll: vi.fn(),
  cancelScrape: vi.fn(),
  onScrapeProgress: vi.fn(),
  scrapeMedia: vi.fn(),
  playMedia: vi.fn(),
  revealInExplorer: vi.fn(),
  deleteMedia: vi.fn(),
  clearLibrary: vi.fn(),
  createMedia: vi.fn(),
  updateMedia: vi.fn(),
  deleteMediaBatch: vi.fn(),
  deleteEpisodes: vi.fn(),
  importFolder: vi.fn(),
  reclassifyMedia: vi.fn(),
}));

vi.mock("../api/commands", () => mocks);

/** 保存订阅到的进度回调，便于用例里手动推事件。 */
let scanCb: ((p: ScanProgress) => void) | null = null;

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

function progress(over: Partial<ScanProgress> = {}): ScanProgress {
  return {
    running: true,
    finished: false,
    cancelled: false,
    phase: "遍历目录",
    found: 0,
    imported: 0,
    total: 0,
    current: "",
    message: "",
    stats: null,
    ...over,
  };
}

describe("mediaStore: 关键字过滤", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    scanCb = null;
    mocks.getMediaList.mockResolvedValue([
      item({ id: 1, title: "Inception", year: 2010, genre: "科幻" }),
      item({
        id: 2,
        type: "tv",
        title: "Show",
        display_title: "Show · Show A",
        year: 2019,
        genre: "剧情",
      }),
    ]);
  });

  it("空关键字返回全部", async () => {
    const s = useMediaStore();
    await s.load();
    expect(s.visibleItems()).toHaveLength(2);
  });

  it("按标题过滤，大小写不敏感", async () => {
    const s = useMediaStore();
    await s.load();
    s.keyword = "incep";
    expect(s.visibleItems().map((i) => i.id)).toEqual([1]);
  });

  it("同名剧集也能按目录后缀搜到（用的是 display_title）", async () => {
    const s = useMediaStore();
    await s.load();
    s.keyword = "show a";
    expect(s.visibleItems().map((i) => i.id)).toEqual([2]);
  });

  it("可按类型与年份搜索", async () => {
    const s = useMediaStore();
    await s.load();
    s.keyword = "2019";
    expect(s.visibleItems().map((i) => i.id)).toEqual([2]);
    s.keyword = "科幻";
    expect(s.visibleItems().map((i) => i.id)).toEqual([1]);
  });

  it("首尾空格不影响匹配", async () => {
    const s = useMediaStore();
    await s.load();
    s.keyword = "  inception  ";
    expect(s.visibleItems()).toHaveLength(1);
  });

  it("可按原始标题（英文原名）与目录名检索", async () => {
    const s = useMediaStore();
    mocks.getMediaList.mockResolvedValue([
      item({
        id: 3,
        title: "流浪地球",
        original_title: "The Wandering Earth",
        group_key: "流浪地球 (2019)",
      }),
      item({ id: 4, title: "独行月球", original_title: "Moon Man" }),
    ]);
    await s.load();
    // 修复前：只搜 display_title/title/genre/year，搜英文原名一条都中不了
    s.keyword = "wandering";
    expect(s.visibleItems().map((i) => i.id)).toEqual([3]);
    // 目录名（group_key）也要能命中
    s.keyword = "2019)";
    expect(s.visibleItems().map((i) => i.id)).toEqual([3]);
    // 普通关键词多结果匹配不受影响
    s.keyword = "moon";
    expect(s.visibleItems().map((i) => i.id)).toEqual([4]);
  });
});

describe("mediaStore: 扫描（后台任务 + 进度事件）", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    scanCb = null;
    mocks.getMediaList.mockResolvedValue([item()]);
    mocks.getScanHistory.mockResolvedValue([]);
    mocks.onScanProgress.mockImplementation(async (cb) => {
      scanCb = cb;
      return () => undefined;
    });
    mocks.scanLibrary.mockResolvedValue({ started: true, message: "已开始扫描 1 个目录" });
    mocks.cancelScan.mockResolvedValue(true);
  });

  it("起任务后立刻返回，界面进入扫描中", async () => {
    const s = useMediaStore();
    await s.scan();

    expect(s.scanning).toBe(true);
    expect(s.toast).toBe("已开始扫描 1 个目录");
  });

  it("进度事件只更新计数，不结束任务", async () => {
    const s = useMediaStore();
    await s.scan();
    await scanCb?.(progress({ phase: "遍历目录", found: 120 }));

    expect(s.scanning).toBe(true);
    expect(s.scanProgress?.found).toBe(120);
    expect(s.toast).toContain("120");
  });

  it("结束事件收尾：退出扫描态、写 toast、刷新列表与历史", async () => {
    const s = useMediaStore();
    await s.scan();
    await scanCb?.(
      progress({
        finished: true,
        running: false,
        phase: "完成",
        message: "扫描完成：发现 10 个视频",
        stats: {
          roots: 1, scanned: 10, movies: 3, series: 1, episodes: 6, unknown: 1,
          addedMovies: 3, addedSeries: 1, addedEpisodes: 6, addedUnknown: 1,
          skipped: 0, failed: 0,
        },
      }),
    );

    expect(s.scanning).toBe(false);
    expect(s.toast).toBe("扫描完成：发现 10 个视频");
    expect(s.lastScan?.scanned).toBe(10);
    expect(mocks.getMediaList).toHaveBeenCalled();
    expect(mocks.getScanHistory).toHaveBeenCalled();
  });

  /** 后端拒绝启动（没配目录 / 已有任务在跑）时不能空等事件。 */
  it("未启动时不起扫描态，直接把原因告诉用户", async () => {
    mocks.scanLibrary.mockResolvedValue({ started: false, message: "尚未配置媒体目录" });
    const s = useMediaStore();
    await s.scan();

    expect(s.scanning).toBe(false);
    expect(s.toast).toBe("尚未配置媒体目录");
  });

  it("命令抛错时收尾并记录错误", async () => {
    mocks.scanLibrary.mockRejectedValue(new Error("boom"));
    const s = useMediaStore();
    await s.scan();

    expect(s.scanning).toBe(false);
    expect(s.error).toContain("boom");
  });

  it("取消扫描会调用后端并给出提示", async () => {
    const s = useMediaStore();
    await s.scan();
    await s.cancelScan();

    expect(mocks.cancelScan).toHaveBeenCalled();
    expect(s.toast).toContain("已请求取消");
  });
});

describe("mediaStore: 资源的新建与编辑", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    vi.clearAllMocks();
    mocks.getMediaList.mockResolvedValue([item()]);
    mocks.getMediaDetail.mockResolvedValue({ media: item(), episodes: [] });
  });

  it("新建电影成功后重载列表并给出提示", async () => {
    mocks.createMedia.mockResolvedValue(item({ id: 9, title: "沙丘", display_title: "沙丘" }));
    const s = useMediaStore();
    const created = await s.createItem({ title: "沙丘", mediaType: "movie" });

    expect(created?.id).toBe(9);
    expect(mocks.createMedia).toHaveBeenCalledWith(
      expect.objectContaining({ title: "沙丘", mediaType: "movie" }),
    );
    expect(mocks.getMediaList).toHaveBeenCalled();
    expect(s.toast).toContain("已新建电影「沙丘」");
    expect(s.error).toBeNull();
  });

  it("新建失败时记录错误并返回 null（列表不强行重载也能保持原样）", async () => {
    mocks.createMedia.mockRejectedValue(new Error("名称不能为空"));
    const s = useMediaStore();
    const created = await s.createItem({ title: " ", mediaType: "tv" });

    expect(created).toBeNull();
    expect(s.error).toContain("名称不能为空");
    expect(s.toast).toContain("新建失败");
  });

  it("更新成功后重载列表；若详情页正开着这条则同步刷新详情", async () => {
    mocks.updateMedia.mockResolvedValue(item({ id: 1, title: "新名", display_title: "新名" }));
    const s = useMediaStore();
    await s.openDetail(1);
    vi.clearAllMocks();
    mocks.getMediaList.mockResolvedValue([item({ id: 1, title: "新名", display_title: "新名" })]);
    mocks.getMediaDetail.mockResolvedValue({
      media: item({ id: 1, title: "新名", display_title: "新名" }),
      episodes: [],
    });

    const updated = await s.updateItem({ id: 1, title: "新名", mediaType: "movie" });

    expect(updated?.title).toBe("新名");
    expect(mocks.getMediaList).toHaveBeenCalled();
    expect(mocks.getMediaDetail).toHaveBeenCalledWith(1);
    expect(s.detail?.media.title).toBe("新名");
    expect(s.toast).toContain("已更新「新名」");
  });

  it("更新别的条目时不碰当前详情页", async () => {
    mocks.updateMedia.mockResolvedValue(item({ id: 2, title: "别的" }));
    const s = useMediaStore();
    await s.openDetail(1);
    vi.clearAllMocks();

    await s.updateItem({ id: 2, title: "别的", mediaType: "movie" });

    expect(mocks.getMediaDetail).not.toHaveBeenCalled();
  });

  it("更新失败时记录错误并返回 null", async () => {
    mocks.updateMedia.mockRejectedValue(new Error("不支持的类型"));
    const s = useMediaStore();
    const updated = await s.updateItem({ id: 1, title: "x", mediaType: "movie" });

    expect(updated).toBeNull();
    expect(s.error).toContain("不支持的类型");
    expect(s.toast).toContain("更新失败");
  });
});

describe("mediaStore: 多选与批量删除", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    vi.clearAllMocks();
    mocks.getMediaList.mockResolvedValue([
      item({ id: 1, title: "Inception" }),
      item({ id: 2, type: "tv", title: "Show" }),
      item({ id: 3, title: "Dune" }),
    ]);
    mocks.getMediaDetail.mockResolvedValue({ media: item(), episodes: [] });
  });

  it("勾选与取消勾选是幂等的", async () => {
    const s = useMediaStore();
    await s.load();
    s.toggleSelect(2);
    expect(s.isSelected(2)).toBe(true);
    s.toggleSelect(2);
    expect(s.isSelected(2)).toBe(false);
  });

  it("全选只作用于当前可见条目；再点一次清空", async () => {
    const s = useMediaStore();
    await s.load();
    s.toggleSelectAll();
    expect(s.selected.sort()).toEqual([1, 2, 3]);

    // 关键字过滤后「可见」只剩一条，全选也应只选这一条
    s.keyword = "dune";
    s.clearSelection();
    s.toggleSelectAll();
    expect(s.selected).toEqual([3]);

    // 已全选时再点一次应取消全选
    s.toggleSelectAll();
    expect(s.selected).toEqual([]);
  });

  it("批量删除后清空选中、刷新列表并提示真实条数", async () => {
    mocks.deleteMediaBatch.mockResolvedValue(2);
    const s = useMediaStore();
    await s.load();
    s.toggleSelect(1);
    s.toggleSelect(3);
    const n = await s.removeSelected();

    expect(n).toBe(2);
    expect(mocks.deleteMediaBatch).toHaveBeenCalledWith([1, 3]);
    expect(s.selected).toEqual([]);
    // 删除后应重新加载列表（load 一次 + 初始 load 一次 = 2）
    expect(mocks.getMediaList).toHaveBeenCalledTimes(2);
    expect(s.toast).toBe("已删除 2 条资源");
  });

  it("被删掉的条目若正开着详情，要退回列表", async () => {
    mocks.deleteMediaBatch.mockResolvedValue(1);
    const s = useMediaStore();
    await s.load();
    await s.openDetail(1);
    s.toggleSelect(1);
    await s.removeSelected();

    expect(s.selectedId).toBeNull();
    expect(s.detail).toBeNull();
  });

  it("空选中时批量删除是空操作，不打扰后端", async () => {
    const s = useMediaStore();
    await s.load();
    await s.removeSelected();
    expect(mocks.deleteMediaBatch).not.toHaveBeenCalled();
  });

  it("切换栏目会清空选中，避免误删上一栏的条目", async () => {
    const s = useMediaStore();
    await s.load();
    s.toggleSelect(1);
    await s.setView("movie");
    expect(s.selected).toEqual([]);
  });

  it("批量删除失败时给出明确提示", async () => {
    mocks.deleteMediaBatch.mockRejectedValue(new Error("数据库被占用"));
    const s = useMediaStore();
    await s.load();
    s.toggleSelect(1);
    const n = await s.removeSelected();

    expect(n).toBe(0);
    expect(s.error).toContain("数据库被占用");
    expect(s.toast).toContain("批量删除失败");
  });
});

/**
 * 删除链路最容易出事的地方不是「删不掉」，而是**删多了 / 以为删了其实没删**：
 * 选中集合里混进看不见的 id、删完之后脏 id 还留在集合里、条数对不上时闷声不响。
 */
describe("mediaStore: 删除链路（防误删 / 幂等 / 反馈）", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    mocks.getMediaList.mockResolvedValue([
      item({ id: 1, title: "Inception", display_title: "Inception" }),
      item({ id: 2, title: "Dune", display_title: "Dune" }),
      item({ id: 3, title: "Arrival", display_title: "Arrival" }),
    ]);
    mocks.deleteMediaBatch.mockResolvedValue(0);
    mocks.deleteMedia.mockResolvedValue(undefined);
    mocks.getMediaDetail.mockResolvedValue(null);
  });

  it("改了关键字就剔掉被过滤掉的选中项，「删除所选」不会误删看不见的条目", async () => {
    const s = useMediaStore();
    await s.load();
    s.setSelected([1, 2, 3]);
    s.keyword = "dune";
    await nextTick(); // watch 是异步的，断言前先让裁剪跑完

    // 只有还看得见的那条继续选着
    expect(s.selected).toEqual([2]);
    mocks.deleteMediaBatch.mockResolvedValue(1);
    await s.removeSelected();
    expect(mocks.deleteMediaBatch).toHaveBeenCalledTimes(1);
    // 绝不能把另外两条一起删掉
    expect(mocks.deleteMediaBatch.mock.calls[0][0]).toEqual([2]);
  });

  it("单条删除后要把该 id 从选中集合里剔除", async () => {
    const s = useMediaStore();
    await s.load();
    s.setSelected([1, 2]);
    await s.remove(1);

    // 留着已删的 id 会让下一次批量删除带着它再跑一趟
    expect(s.selected).toEqual([2]);
  });

  it("实际删掉的条数少于请求数时要说清楚，不让用户以为漏删", async () => {
    mocks.deleteMediaBatch.mockResolvedValue(2);
    const s = useMediaStore();
    await s.load();
    s.setSelected([1, 2, 3]);
    await s.removeSelected();

    expect(s.toast).toBe("已删除 2 条资源，另有 1 条已不在库中");
    // 删过的和本来就不存在的都要剔掉
    expect(s.selected).toEqual([]);
  });

  it("按 id 删除只剔这一批，用户在别处的勾选不受影响", async () => {
    mocks.deleteMediaBatch.mockResolvedValue(1);
    const s = useMediaStore();
    await s.load();
    s.setSelected([1, 2, 3]);
    await s.removeByIds([2]);

    // 树视图删节点不该顺手清空海报墙上的勾选
    expect(s.selected).toEqual([1, 3]);
  });

  it("重复 id 与非正 id 先在本地清掉，不麻烦后端", async () => {
    mocks.deleteMediaBatch.mockResolvedValue(2);
    const s = useMediaStore();
    await s.load();
    await s.removeByIds([1, 1, 0, -3, 2]);

    expect(mocks.deleteMediaBatch).toHaveBeenCalledWith([1, 2]);
  });

  it("单条删除失败时如实报错，不谎报成功", async () => {
    mocks.deleteMedia.mockRejectedValue(new Error("找不到 id 为 9 的资源，可能已经被删除了"));
    const s = useMediaStore();
    await s.load();
    await s.remove(9);

    expect(s.toast).toContain("移除失败");
    expect(s.toast).toContain("找不到 id 为 9 的资源");
  });
});

describe("mediaStore: 从文件夹导入与单集删减", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    vi.clearAllMocks();
    mocks.getMediaList.mockResolvedValue([item()]);
    mocks.getScanHistory.mockResolvedValue([]);
    mocks.getMediaDetail.mockResolvedValue({ media: item(), episodes: [] });
  });

  it("导入成功后把后端的结果原样反馈并刷新列表", async () => {
    mocks.importFolder.mockResolvedValue({
      added: 3, skipped: 2, failed: 0, linked: 3, message: "新增 3 个文件，跳过 2 个已存在",
    });
    const s = useMediaStore();
    const r = await s.importFolder("D:\\影视\\X", 7);

    expect(r?.added).toBe(3);
    expect(mocks.importFolder).toHaveBeenCalledWith("D:\\影视\\X", 7);
    expect(s.toast).toContain("跳过 2 个已存在");
    expect(mocks.getMediaList).toHaveBeenCalled();
  });

  it("导入失败时把原因透传出来（目录不存在 / 无权限等）", async () => {
    mocks.importFolder.mockRejectedValue(new Error("目录不存在或不可访问：D:\\x"));
    const s = useMediaStore();
    const r = await s.importFolder("D:\\x", null);

    expect(r).toBeNull();
    expect(s.error).toContain("目录不存在或不可访问");
    expect(s.toast).toContain("导入失败");
    expect(mocks.getMediaList).not.toHaveBeenCalled();
  });

  it("删单集后刷新详情与列表，且只提交给定的 id", async () => {
    mocks.deleteEpisodes.mockResolvedValue(2);
    const s = useMediaStore();
    await s.openDetail(1);
    vi.clearAllMocks();
    mocks.getMediaList.mockResolvedValue([item()]);

    const n = await s.removeEpisodes(1, [11, 12]);

    expect(n).toBe(2);
    expect(mocks.deleteEpisodes).toHaveBeenCalledWith([11, 12]);
    // 应刷新当前剧目的详情
    expect(mocks.getMediaDetail).toHaveBeenCalledWith(1);
    expect(mocks.getMediaList).toHaveBeenCalled();
    expect(s.toast).toBe("已删除 2 条资源");
  });

  it("删单集失败时给出提示且不刷新", async () => {
    mocks.deleteEpisodes.mockRejectedValue(new Error("外键约束"));
    const s = useMediaStore();
    const n = await s.removeEpisodes(1, [11]);

    expect(n).toBe(0);
    expect(s.error).toContain("外键约束");
    expect(s.toast).toContain("删除单集失败");
    expect(mocks.getMediaList).not.toHaveBeenCalled();
  });
});

/**
 * 列表排序：排序是**展示层**行为，只改内存顺序——不写媒体行、不碰文件、
 * 也不改后端 `get_media_list` 的 ORDER BY。这里守三条：
 * ① 分区恒为「已识别 → 未识别」；② 倒序只在分区内部生效；③ 过滤不改顺序。
 */
describe("mediaStore: 列表排序", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    vi.clearAllMocks();
    mocks.getMediaList.mockResolvedValue([
      item({ id: 1, title: "B", display_title: "B", year: 2000, added_at: 30 }),
      item({ id: 2, title: "A", display_title: "A", year: 2010, added_at: 10 }),
      // type 未知 → 未识别分区
      item({ id: 3, type: "unknown", title: "D", display_title: "D", year: 2015, added_at: 20 }),
      // 缺年份 → 未识别分区
      item({ id: 4, title: "无年份", display_title: "无年份", year: null, added_at: 5 }),
    ]);
  });

  /** 可见列表的 id 序列。 */
  function visibleIds(s: ReturnType<typeof useMediaStore>): number[] {
    return s.visibleItems().map((i) => i.id);
  }

  it("默认按名称正序：已识别在前，未识别沉底", async () => {
    const s = useMediaStore();
    await s.load();

    expect(visibleIds(s)).toEqual([2, 1, 3, 4]);
  });

  /**
   * `sort_order`（同级次序）是**数据**，不是展示顺序。
   *
   * 全局排序只认用户选的偏好；`sort_order` 要原样留在条目上，交给真正需要
   * 它的地方（详情页「聚合成员」）自己排。若哪天有人把它接进 `orderedItems`，
   * 用户选的排序就被悄悄改写了——那正是「成员 ↑↓ 无效果」这条 bug 的反面。
   */
  it("sort_order 不参与全局排序，但要原样留在条目上", async () => {
    mocks.getMediaList.mockResolvedValue([
      item({ id: 1, title: "A", display_title: "A", year: 2000, sort_order: 2 }),
      item({ id: 2, title: "B", display_title: "B", year: 2000, sort_order: 0 }),
    ]);
    const s = useMediaStore();
    await s.load();

    // 默认按名称正序：A 在前，尽管它的 sort_order 更大
    expect(visibleIds(s)).toEqual([1, 2]);
    // 字段没被 store 吞掉，上层要用时取得到
    expect(s.visibleItems().map((i) => i.sort_order)).toEqual([2, 0]);
  });

  it("按上映年份正序：旧→新，缺年份的在未识别分区末尾", async () => {
    const st = useSettingsStore();
    st.sortBy = "year";
    const s = useMediaStore();
    await s.load();

    expect(visibleIds(s)).toEqual([1, 2, 3, 4]);
  });

  it("按上映年份倒序：新→旧，未识别分区仍在最末", async () => {
    const st = useSettingsStore();
    st.sortBy = "year";
    st.sortDir = "desc";
    const s = useMediaStore();
    await s.load();

    const ids = visibleIds(s);
    expect(ids).toEqual([2, 1, 3, 4]);
    // 缺年份的那条（id 4）在任何方向下都是最后一条
    expect(ids[ids.length - 1]).toBe(4);
  });

  it("按添加时间排序：两个方向都只在分区内部生效", async () => {
    const st = useSettingsStore();
    st.sortBy = "added";
    const s = useMediaStore();
    await s.load();

    expect(visibleIds(s)).toEqual([2, 1, 4, 3]);

    st.sortDir = "desc";
    expect(visibleIds(s)).toEqual([1, 2, 3, 4]);
  });

  it("关键字过滤只做过滤，不改变排序结果", async () => {
    const st = useSettingsStore();
    st.sortBy = "year";
    st.sortDir = "desc";
    const s = useMediaStore();
    await s.load();
    const before = visibleIds(s);

    // 全部条目都有这个类型标签 → 一条都过滤不掉，顺序必须与过滤前完全一致
    s.keyword = "科幻";
    expect(visibleIds(s)).toEqual(before);
  });

  /** 排序后的列表就是用户看到的列表，多选必须作用在它上面（P0-7）。 */
  it("全选作用于**排序后**的可见列表，顺序即显示顺序", async () => {
    const st = useSettingsStore();
    st.sortBy = "added";
    st.sortDir = "asc";
    const s = useMediaStore();
    await s.load();

    s.toggleSelectAll();
    expect(s.selected).toEqual([2, 1, 4, 3]);
  });
});

/**
 * 「待确认 / 重新识别」两件事：
 * 类型判定改了规则，界面上必须能看到「哪些还没定下来」，
 * 并且能一键把存量的错误分类修回来。
 */
describe("mediaStore: 待确认 / 重新识别", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    vi.clearAllMocks();
    mocks.getMediaList.mockResolvedValue([item()]);
    mocks.getScanHistory.mockResolvedValue([]);
    mocks.getMediaDetail.mockResolvedValue({ media: item(), episodes: [] });
  });

  it("切到「待确认」栏目按 unknown 过滤", async () => {
    const s = useMediaStore();
    await s.setView("unknown");

    expect(s.filter).toBe("unknown");
    expect(mocks.getMediaList).toHaveBeenCalledWith(
      expect.objectContaining({ mediaType: "unknown" }),
    );
  });

  it("重新识别全库：不传 ids，结束后刷新列表", async () => {
    mocks.reclassifyMedia.mockResolvedValue({
      scanned: 30, changed: 4, toTv: 3, toMovie: 0, toUnknown: 1,
      skippedLocked: 2, message: "已重新识别 30 条，3 条修正为剧集",
    });
    const s = useMediaStore();
    const r = await s.reclassify();

    expect(r?.toTv).toBe(3);
    expect(mocks.reclassifyMedia).toHaveBeenCalledWith(null);
    expect(s.toast).toContain("3 条修正为剧集");
    expect(mocks.getMediaList).toHaveBeenCalled();
  });

  it("多选模式下只重新识别所选条目", async () => {
    mocks.reclassifyMedia.mockResolvedValue({
      scanned: 2, changed: 1, toTv: 1, toMovie: 0, toUnknown: 0,
      skippedLocked: 0, message: "已重新识别 2 条",
    });
    const s = useMediaStore();
    await s.reclassify([7, 8]);

    expect(mocks.reclassifyMedia).toHaveBeenCalledWith([7, 8]);
  });

  it("重新识别失败时给出可读提示", async () => {
    mocks.reclassifyMedia.mockRejectedValue(new Error("数据库被占用"));
    const s = useMediaStore();
    const r = await s.reclassify();

    expect(r).toBeNull();
    expect(s.toast).toContain("重新识别失败");
  });
});
