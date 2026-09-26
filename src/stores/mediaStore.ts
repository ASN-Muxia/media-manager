import { defineStore } from "pinia";
import { ref } from "vue";
import {
  cancelScan as apiCancelScan,
  cancelScrape as apiCancelScrape,
  clearLibrary, createMedia as apiCreateMedia,
  deleteEpisodes as apiDeleteEpisodes, deleteMedia, deleteMediaBatch as apiDeleteMediaBatch,
  getMediaDetail, getMediaList, getMovieCollections as apiGetMovieCollections,
  getScanHistory, onScanProgress, onScrapeProgress,
  playMedia, reclassifyMedia as apiReclassify, revealInExplorer,
  scanLibrary, scrapeAll as apiScrapeAll, scrapeMedia,
  updateMedia as apiUpdateMedia,
  importFolder as apiImportFolder,
  type CreateMediaInput, type ImportFolderResult, type MediaDetail, type MediaItem,
  type MovieCollection, type ReclassifyResult,
  type ScanHistoryItem, type ScanProgress, type ScanStats, type ScrapeProgress,
  type UnlistenFn, type UpdateMediaInput,
} from "../api/commands";
import { parseView, type ViewKey } from "../types";

/** 媒体类型筛选；`unknown` = 判定不出来的「待确认」条目。 */
export type ListFilter = "all" | "movie" | "tv" | "unknown";

export const useMediaStore = defineStore("media", () => {
  const items = ref<MediaItem[]>([]);
  /** 媒体类型筛选（内置入口）。 */
  const filter = ref<ListFilter>("all");
  const keyword = ref("");
  /** 分类筛选：来自左侧导航的分类栏目（id 由数据决定，前端不硬编码）。 */
  const categoryId = ref<number | null>(null);
  /** 标签筛选。 */
  const tagId = ref<number | null>(null);
  /** 只看还没归类的条目。 */
  const unclassified = ref(false);

  /** 是否处于多选模式（界面上的「选择」开关）。 */
  const selecting = ref(false);
  /** 当前选中的条目 id（批量删除用）。 */
  const selected = ref<number[]>([]);

  const loading = ref(false);
  const scanning = ref(false);
  const scraping = ref(false);
  const error = ref<string | null>(null);
  const toast = ref<string>("");

  const lastScan = ref<ScanStats | null>(null);
  const scanHistory = ref<ScanHistoryItem[]>([]);

  const selectedId = ref<number | null>(null);
  const detail = ref<MediaDetail | null>(null);

  /** 系列电影合集（派生视图，进「系列合集」栏目时才加载）。 */
  const collections = ref<MovieCollection[]>([]);
  const loadingCollections = ref(false);
  /** 正在重新识别存量数据。 */
  const reclassifying = ref(false);

  /** 批量刮削的实时进度（后台任务推来的最后一个事件）。 */
  const scrapeProgress = ref<ScrapeProgress | null>(null);
  let unlistenProgress: UnlistenFn | null = null;

  /** 扫描的实时进度（后台任务推来的最后一个事件）。 */
  const scanProgress = ref<ScanProgress | null>(null);
  let unlistenScan: UnlistenFn | null = null;

  function note(msg: string): void {
    toast.value = msg;
  }

  /**
   * 订阅刮削进度事件（只订阅一次）。
   *
   * 刮削全程在后台线程，界面靠这个事件更新进度条；任务结束时再刷新列表与详情。
   */
  async function ensureProgressListener(): Promise<void> {
    if (unlistenProgress) return;
    unlistenProgress = await onScrapeProgress(async (p) => {
      scrapeProgress.value = p;
      if (p.current) note(`刮削中 ${p.done}/${p.total}：${p.current}`);
      if (!p.finished) return;

      scraping.value = false;
      if (p.message) note(p.message);
      await load();
      if (selectedId.value) await openDetail(selectedId.value);
    });
  }

  /**
   * 订阅扫描进度事件（只订阅一次）。
   *
   * 扫描在后台线程跑，界面靠这个事件更新进度；任务结束时再刷新列表与历史。
   */
  async function ensureScanListener(): Promise<void> {
    if (unlistenScan) return;
    unlistenScan = await onScanProgress(async (p) => {
      scanProgress.value = p;
      if (!p.finished) {
        if (p.phase === "遍历目录") note(`扫描中：已发现 ${p.found} 个视频`);
        return;
      }

      scanning.value = false;
      lastScan.value = p.stats;
      if (p.message) note(p.message);
      await load();
      await loadHistory();
    });
  }

  /** 按当前过滤条件加载列表（分类 / 标签 / 未分类都在 SQL 层过滤）。 */
  async function load(): Promise<void> {
    loading.value = true;
    error.value = null;
    try {
      items.value = await getMediaList({
        mediaType: filter.value,
        categoryId: categoryId.value,
        tagId: tagId.value,
        unclassified: unclassified.value,
      });
    } catch (e) {
      error.value = String(e);
      items.value = [];
    } finally {
      loading.value = false;
    }
  }

  /** 切换媒体类型并重新加载。 */
  async function setFilter(f: ListFilter): Promise<void> {
    filter.value = f;
    selectedId.value = null;
    detail.value = null;
    await load();
  }

  /**
   * 按视图键切换列表。
   *
   * `cat:<id>` / `tag:<id>` 都是从数据库来的，所以这里只做「解析 → 赋值」，
   * 不认识的分类不会报错，回落到全部列表。
   */
  async function setView(v: ViewKey): Promise<void> {
    const parsed = parseView(v);
    categoryId.value = null;
    tagId.value = null;
    unclassified.value = false;
    if (parsed.kind === "builtin") {
      // `collections` 不是「另一种类型」，而是从电影标题算出来的派生视图，
      // 所以它落在「全部」上，另外单独拉一份合集数据。
      filter.value =
        parsed.key === "movie" || parsed.key === "tv" || parsed.key === "unknown"
          ? parsed.key
          : "all";
    } else if (parsed.kind === "category") {
      filter.value = "all";
      categoryId.value = parsed.id;
    } else if (parsed.kind === "tag") {
      filter.value = "all";
      tagId.value = parsed.id;
    } else {
      filter.value = "all";
      unclassified.value = true;
    }
    selectedId.value = null;
    detail.value = null;
    // 换栏目必须清空选中：上一栏的 id 在新栏目里可能根本不存在，
    // 留着会让「删除所选」删到看不见的条目。
    selected.value = [];
    await load();
    // 系列合集是另一份数据（由电影标题算出来），只在切到那个栏目时才算。
    if (parsed.kind === "builtin" && parsed.key === "collections") {
      await loadCollections();
    }
  }

  /* ------------------------------ 系列电影合集 ------------------------------ */

  /**
   * 加载系列电影合集。
   *
   * 后端按「系列基名」把同一 IP 的电影归到一起（按上映时间排序），
   * 这是**纯派生数据**：改了片名重算一次即可，不动库里任何一行。
   */
  async function loadCollections(): Promise<void> {
    loadingCollections.value = true;
    error.value = null;
    try {
      collections.value = await apiGetMovieCollections();
    } catch (e) {
      error.value = String(e);
      collections.value = [];
    } finally {
      loadingCollections.value = false;
    }
  }

  /**
   * 重新识别存量数据的类型（修正历史误判）。
   *
   * 传 `ids` 只重判所选；不传就是全库。结束后刷新列表与合集——
   * 类型变了，所在栏目和合集归属都会跟着变。
   */
  async function reclassify(ids?: number[] | null): Promise<ReclassifyResult | null> {
    reclassifying.value = true;
    error.value = null;
    try {
      const r = await apiReclassify(ids ?? null);
      note(r.message);
      await load();
      if (collections.value.length) await loadCollections();
      return r;
    } catch (e) {
      error.value = String(e);
      note("重新识别失败：" + String(e));
      return null;
    } finally {
      reclassifying.value = false;
    }
  }

  /* ------------------------------ 多选与批量删除 ------------------------------ */

  function isSelected(id: number): boolean {
    return selected.value.includes(id);
  }

  function toggleSelect(id: number): void {
    const i = selected.value.indexOf(id);
    if (i >= 0) selected.value.splice(i, 1);
    else selected.value.push(id);
  }

  /** 全选 / 取消全选：作用范围是**当前可见**的条目（关键字过滤后看到的）。 */
  function toggleSelectAll(): void {
    const visible = visibleItems();
    selected.value =
      selected.value.length >= visible.length && visible.every((i) => isSelected(i.id))
        ? []
        : visible.map((i) => i.id);
  }

  function clearSelection(): void {
    selected.value = [];
  }

  /**
   * 批量删除选中的条目。
   *
   * 后端一个事务删完并返回真实条数；这里据此提示，并刷新列表持久化状态。
   * 若详情页正开着被删掉的条目，一并返回列表页。
   */
  async function removeSelected(): Promise<number> {
    const ids = [...selected.value];
    if (!ids.length) return 0;
    error.value = null;
    try {
      const n = await apiDeleteMediaBatch(ids);
      note(`已删除 ${n} 条资源`);
      if (selectedId.value !== null && ids.includes(selectedId.value)) closeDetail();
      clearSelection();
      await load();
      return n;
    } catch (e) {
      error.value = String(e);
      note("批量删除失败：" + String(e));
      return 0;
    }
  }

  /**
   * 从本地文件夹导入（解析 → 分组 → 入库，重复自动跳过）。
   *
   * 后端在后台线程执行，这里等待结果并给出「新增 / 跳过 / 失败」的明确反馈。
   */
  async function importFolder(
    path: string,
    categoryId: number | null,
  ): Promise<ImportFolderResult | null> {
    error.value = null;
    try {
      const r = await apiImportFolder(path, categoryId);
      note(r.message);
      await load();
      await loadHistory();
      return r;
    } catch (e) {
      error.value = String(e);
      note("导入失败：" + String(e));
      return null;
    }
  }

  /**
   * 删减某个聚合剧目下的单集（单条 / 批量都走这里）。
   *
   * 只删传入的单集 id，其他剧目不受影响；删完同步刷新详情与列表（集数变了）。
   */
  async function removeEpisodes(mediaId: number, ids: number[]): Promise<number> {
    if (!ids.length) return 0;
    error.value = null;
    try {
      const n = await apiDeleteEpisodes(ids);
      note(`已删除 ${n} 条资源`);
      await openDetail(mediaId);
      await load();
      return n;
    } catch (e) {
      error.value = String(e);
      note("删除单集失败：" + String(e));
      return 0;
    }
  }

  /**
   * 手动新建一条资源（电影或剧集）。
   *
   * 成功后立刻重载列表，新条目会出现在「全部」与它的类型栏目里
   * （若当前正按分类 / 标签过滤且新条目不满足条件，则不会出现在当前视图——
   * 这是正确的：列表永远只反映当前过滤条件）。
   */
  async function createItem(input: CreateMediaInput): Promise<MediaItem | null> {
    error.value = null;
    try {
      const created = await apiCreateMedia(input);
      note(`已新建${created.type === "tv" ? "剧集" : "电影"}「${created.display_title || created.title}」`);
      await load();
      return created;
    } catch (e) {
      error.value = String(e);
      note("新建失败：" + String(e));
      return null;
    }
  }

  /**
   * 更新一条资源的基本信息。
   *
   * 成功后重载列表（名称 / 类型可能变了，所在栏目可能随之变化）；
   * 若详情页正开着这条，同步刷新详情，保证两处看到的一致。
   */
  async function updateItem(input: UpdateMediaInput): Promise<MediaItem | null> {
    error.value = null;
    try {
      const updated = await apiUpdateMedia(input);
      note(`已更新「${updated.display_title || updated.title}」`);
      await load();
      if (selectedId.value === updated.id) await openDetail(updated.id);
      return updated;
    } catch (e) {
      error.value = String(e);
      note("更新失败：" + String(e));
      return null;
    }
  }

  /**
   * 扫描媒体库。
   *
   * 命令只是「起任务」并立即返回，遍历目录与入库都在后台线程；
   * 期间界面照常可操作，进度由 `scan-progress` 事件驱动。
   */
  async function scan(): Promise<void> {
    error.value = null;
    try {
      await ensureScanListener();
      const s = await scanLibrary();
      if (!s.started) {
        note(s.message || "未能启动扫描");
        return;
      }
      scanning.value = true;
      scanProgress.value = null;
      note(s.message || "扫描已开始…");
    } catch (e) {
      error.value = String(e);
      note("扫描失败：" + error.value);
      scanning.value = false;
    }
  }

  /** 取消正在进行的扫描（遍历阶段生效，入库阶段不中断）。 */
  async function cancelScan(): Promise<void> {
    try {
      await apiCancelScan();
      note("已请求取消扫描，正在等待当前目录遍历完…");
    } catch (e) {
      note("取消失败：" + String(e));
    }
  }

  async function loadHistory(): Promise<void> {
    try {
      scanHistory.value = await getScanHistory(10);
    } catch {
      scanHistory.value = [];
    }
  }

  async function openDetail(id: number): Promise<void> {
    error.value = null;
    try {
      detail.value = await getMediaDetail(id);
      selectedId.value = id;
    } catch (e) {
      error.value = String(e);
    }
  }

  function closeDetail(): void {
    selectedId.value = null;
    detail.value = null;
  }

  /** 刮削单条（可选指定 TMDB id）。 */
  async function scrape(id: number, tmdbId?: number | null): Promise<void> {
    error.value = null;
    try {
      const r = await scrapeMedia(id, tmdbId);
      note(
        r.ok
          ? `刮削成功：${r.title ?? ""}${r.episodes > 0 ? `（同步 ${r.episodes} 集）` : ""}`
          : r.message,
      );
      await openDetail(id);
      await load();
    } catch (e) {
      error.value = String(e);
      note("刮削失败：" + error.value);
    }
  }

  /**
   * 批量刮削缺海报的条目。
   *
   * 命令只是「起任务」并立即返回，真正的抓取在后台线程，
   * 期间界面照常可操作，进度由 `scrape-progress` 事件驱动。
   */
  async function scrapeMissing(): Promise<void> {
    error.value = null;
    try {
      await ensureProgressListener();
      const s = await apiScrapeAll();
      if (!s.started) {
        note(s.message || "未能启动刮削");
        return;
      }
      scraping.value = true;
      scrapeProgress.value = null;
      note(s.message || `已开始刮削 ${s.total} 条`);
    } catch (e) {
      error.value = String(e);
      note("批量刮削失败：" + error.value);
      scraping.value = false;
    }
  }

  /** 取消正在进行的批量刮削（已发出的单个请求不中断，下一轮循环停止）。 */
  async function cancelScrape(): Promise<void> {
    try {
      await apiCancelScrape();
      note("已请求取消，正在等待当前条目处理完…");
    } catch (e) {
      note("取消失败：" + String(e));
    }
  }

  /** 用外部播放器打开文件。 */
  async function play(path: string | null): Promise<void> {
    if (!path) {
      note("该条目没有关联的本地文件");
      return;
    }
    try {
      const used = await playMedia(path);
      note(used === "system" ? "已用系统默认播放器打开" : `已用 ${used} 播放`);
    } catch (e) {
      error.value = String(e);
      note("播放失败：" + error.value);
    }
  }

  async function reveal(path: string | null): Promise<void> {
    if (!path) return;
    try {
      await revealInExplorer(path);
    } catch (e) {
      note("打开目录失败：" + String(e));
    }
  }

  async function remove(id: number): Promise<void> {
    try {
      await deleteMedia(id);
      closeDetail();
      await load();
      note("已从媒体库移除");
    } catch (e) {
      note("移除失败：" + String(e));
    }
  }

  async function clearAll(): Promise<void> {
    try {
      await clearLibrary();
      closeDetail();
      await load();
      await loadHistory();
      note("媒体库已清空");
    } catch (e) {
      note("清空失败：" + String(e));
    }
  }

  /** 关键字过滤后的列表。 */
  function visibleItems(): MediaItem[] {
    const kw = keyword.value.trim().toLowerCase();
    if (!kw) return items.value;
    return items.value.filter((it) => {
      const hay =
        `${it.display_title} ${it.title} ${it.genre ?? ""} ${it.year ?? ""}`.toLowerCase();
      return hay.includes(kw);
    });
  }

  return {
    items, filter, keyword, categoryId, tagId, unclassified,
    loading, scanning, scraping, error, toast,
    lastScan, scanHistory, selectedId, detail, scrapeProgress, scanProgress,
    selecting, selected,
    collections, loadingCollections, reclassifying,
    load, setFilter, setView, createItem, updateItem, scan, cancelScan, loadHistory,
    openDetail, closeDetail,
    scrape, scrapeMissing, cancelScrape, play, reveal, remove, clearAll,
    visibleItems, note,
    isSelected, toggleSelect, toggleSelectAll, clearSelection, removeSelected,
    importFolder, removeEpisodes, loadCollections, reclassify,
  };
});
