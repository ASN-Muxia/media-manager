import { defineStore } from "pinia";
import { computed, ref, watch } from "vue";
import {
  cancelScan as apiCancelScan,
  cancelScrape as apiCancelScrape,
  clearLibrary, createMedia as apiCreateMedia,
  deleteEpisodes as apiDeleteEpisodes, deleteMedia, deleteMediaBatch as apiDeleteMediaBatch,
  getMediaDetail, getMediaList,
  getScanHistory, onScanProgress, onScrapeProgress,
  playMedia, reclassifyMedia as apiReclassify, revealInExplorer,
  scanLibrary, scrapeAll as apiScrapeAll, scrapeMedia,
  updateMedia as apiUpdateMedia,
  importFolder as apiImportFolder, importFolderInto as apiImportFolderInto,
  doubanApply as apiDoubanApply,
  moveMediaNodes as apiMoveMediaNodes, undoTreeOp as apiUndoTreeOp,
  type CreateMediaInput, type ImportFolderResult, type MediaDetail, type MediaItem,
  type ReclassifyResult,
  type ScanHistoryItem, type ScanProgress, type ScanStats, type ScrapeProgress,
  type UnlistenFn, type UpdateMediaInput,
} from "../api/commands";
import { parseView, type ViewKey } from "../types";
import { buildHanOrder, rawTitleOf, sortMediaItems, type HanOrder } from "../utils/sort";
import { resolveClick } from "../utils/selection";
import { useSettingsStore } from "./settingsStore";

/** 媒体类型筛选；`unknown` = 判定不出来的「待确认」条目。 */
export type ListFilter = "all" | "movie" | "tv" | "unknown";

export const useMediaStore = defineStore("media", () => {
  /** 排序偏好在设置里（单向依赖，无环：`stores/settingsStore` 不反向引用本 store）。 */
  const settings = useSettingsStore();
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
  /**
   * 是否处于聚合模式（界面上的「聚合」开关）：封面墙里卡片直接可拖，
   * 拖到另一张封面上即合并为分组。
   *
   * 与 `selecting` **相互独立**：聚合不要求先进「选择」模式，它自己就会把
   * `selectable` 打开（见 `App.vue` 里 `:selectable` 的绑定）；反过来开着
   * 聚合也不等于开着多选工具条。
   */
  const grouping = ref(false);
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

  /** 正在重新识别存量数据。 */
  const reclassifying = ref(false);

  /* ------------------------- 剧集层级（聚合分组） ------------------------- */

  /**
   * 后端撤销栈还剩几步（`moveNodes` / `undoTree` 回写，撤销入口的置灰依据）。
   *
   * 层级数据本身不再单独存一份：剧集层级视图已撤掉，`parent_id` 这层关系
   * 由封面墙直接从 `items` 里聚合成分组，避免两份数据互相打架。
   */
  const undoDepth = ref(0);

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
  }

  /* ------------------------------ 类型重新识别 ------------------------------ */

  /**
   * 重新识别存量数据的类型（修正历史误判）。
   *
   * 传 `ids` 只重判所选；不传就是全库。结束后刷新列表——类型变了，
   * 所在栏目会跟着变。
   */
  async function reclassify(ids?: number[] | null): Promise<ReclassifyResult | null> {
    reclassifying.value = true;
    error.value = null;
    try {
      const r = await apiReclassify(ids ?? null);
      note(r.message);
      await load();
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

  /**
   * Shift 区间的**锚点**：上一次点击的那张。
   *
   * 归 store 而不是组件：勾选集合本来就由 store 持有，锚点是它的伴生状态；
   * 放组件里会在「详情页返回 / 换栏目重挂」时丢掉，中间的 Shift 就没了起点。
   */
  const selectAnchor = ref<number | null>(null);

  /**
   * 多选模式下点击一张卡片（带修饰键）。
   *
   * 语义全在 `utils/selection.ts`（纯函数，已单测）：
   * - 普通点击 / Ctrl·Cmd+点击 → 切换这一张；
   * - **Shift+点击** → 把「上次点的那张 → 这一张」在当前可见顺序里的整段**并入**选中。
   *
   * 区间用的是 `visibleItems()`（**当前看得见的顺序**），不是全库顺序 ——
   * 用户划的区间永远基于屏幕上那一段，换栏目 / 敲了关键字之后的顺序才对得上。
   */
  function selectWithMods(id: number, mods: { shift?: boolean; ctrl?: boolean; meta?: boolean }): void {
    const order = visibleItems().map((i) => i.id);
    const r = resolveClick({
      order,
      selected: selected.value,
      anchor: selectAnchor.value,
      id,
      mods,
    });
    selected.value = r.selected;
    selectAnchor.value = r.anchor;
  }

  /** 全选 / 取消全选：作用范围是**当前可见**的条目（关键字过滤后看到的）。 */
  function toggleSelectAll(): void {
    const visible = visibleItems();
    selected.value =
      selected.value.length >= visible.length && visible.every((i) => isSelected(i.id))
        ? []
        : visible.map((i) => i.id);
  }

  /**
   * 反选：把**当前可见**条目里的选中状态整个翻过来。
   *
   * 作用范围与 `toggleSelectAll` 一致——只看得见的那些。否则用户按一下反选，
   * 会把关键字过滤掉的一大批看不见的条目也选进来，点「删除所选」就误删了。
   */
  function invertSelection(): void {
    const visible = visibleItems().map((i) => i.id);
    const picked = new Set(selected.value);
    selected.value = visible.filter((id) => !picked.has(id));
  }

  function clearSelection(): void {
    selected.value = [];
    // 锚点必须跟着清：留着它，下一次 Shift 会从一个用户早就取消掉的起点算区间。
    selectAnchor.value = null;
  }

  /**
   * 整批覆盖选中集合（滑动多选**松手时统一提交**走这里）。
   *
   * 与 `toggleSelect` 的区别：一次赋值、一次渲染、一次持久化，
   * 不会在拖动过程中把外部状态改来改去 —— 拖动全程只改组件本地的预览集合。
   */
  function setSelected(ids: number[]): void {
    selected.value = [...ids];
  }

  /**
   * 从选中集合里剔除一批 id。
   *
   * 删完之后必须调用：留在 `selected` 里的已删 id 会让下一次「删除所选」
   * 带着一批已经不存在的 id 再跑一趟，界面上的计数也就对不上了。
   */
  function dropFromSelection(ids: number[]): void {
    if (!ids.length) return;
    const gone = new Set(ids);
    selected.value = selected.value.filter((id) => !gone.has(id));
  }

  /**
   * 把选中集合里**当前看不见**的条目剔除。
   *
   * 换栏目时是整体清空（见 `setView`），改关键字则只剔掉被过滤掉的那部分——
   * 否则「删除所选」按钮数的是 `selected.length`，而列表里只显示过滤后的几条，
   * 用户点下去会删掉一批自己根本看不见的条目，这是最典型的误删。
   */
  function pruneSelection(): void {
    if (!selected.value.length) return;
    const alive = new Set(visibleItems().map((i) => i.id));
    selected.value = selected.value.filter((id) => alive.has(id));
  }

  /** 丢掉重复项与非法的 id（后端会再做一遍，这里先做是为了让提示的条数准）。 */
  function cleanIds(ids: number[]): number[] {
    return [...new Set(ids.filter((id) => Number.isInteger(id) && id > 0))];
  }

  /**
   * 批量删除指定 id —— **删除的唯一真正入口**（批量删除走这里）。
   *
   * 后端一个事务删完并返回**真实条数**；这里据此提示并刷新列表。
   * 不吞异常：失败交给调用方转成界面提示。
   */
  async function removeByIds(ids: number[]): Promise<number> {
    const want = cleanIds(ids);
    if (!want.length) return 0;
    const n = await apiDeleteMediaBatch(want);
    if (n < want.length) {
      // 后端是单事务，要么全成要么全败；条数变少只可能是「有些 id 已经不在库里」
      // （别处刚删过 / 手快点了两次）。这不是失败，但必须说清楚，
      // 否则用户会以为有一批没删掉、于是再点一次。
      note(`已删除 ${n} 条资源，另有 ${want.length - n} 条已不在库中`);
    } else {
      note(`已删除 ${n} 条资源`);
    }
    if (selectedId.value !== null && want.includes(selectedId.value)) closeDetail();
    // 删掉的和本来就不存在的都要剔除，不能只剔 n 条
    dropFromSelection(want);
    await load();
    return n;
  }

  /**
   * 批量删除**当前选中的**条目。
   *
   * 真正的删除逻辑在 `removeByIds`，这里只负责取选中集合 + 把失败转成提示。
   */
  async function removeSelected(): Promise<number> {
    const ids = [...selected.value];
    if (!ids.length) return 0;
    error.value = null;
    try {
      return await removeByIds(ids);
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
   * 把一个文件夹的视频**指定**归到某条剧集下（详情页的「添加文件到这部剧」）。
   *
   * 与 `importFolder` 的区别：归属由用户指定，不靠文件名推断。用在
   * 「文件名和目录名都不含剧名」、推断必然失败的场景。
   */
  async function importFolderInto(
    mediaId: number,
    path: string,
    categoryId: number | null,
  ): Promise<ImportFolderResult | null> {
    error.value = null;
    try {
      const r = await apiImportFolderInto(mediaId, path, categoryId);
      note(r.message);
      await load();
      await loadHistory();
      // 详情里的单集列表变了，要同步；当前正打开这条就刷新它
      if (selectedId.value !== null) await openDetail(selectedId.value);
      return r;
    } catch (e) {
      error.value = String(e);
      note("添加失败：" + String(e));
      return null;
    }
  }

  /**
   * 用豆瓣刮削指定条目（TMDb 有结果时只补缺失，否则整条由豆瓣接管）。
   *
   * 与 `scrape` 走同一套结果契约，所以提示语与刷新逻辑一致。
   */
  async function scrapeDouban(
    id: number,
    query?: string | null,
    year?: number | null,
  ): Promise<void> {
    error.value = null;
    try {
      const r = await apiDoubanApply(id, query ?? null, year ?? null);
      note(
        r.ok
          ? `豆瓣刮削成功：${r.title ?? ""}${r.episodes > 0 ? `（同步 ${r.episodes} 集）` : ""}`
          : r.message,
      );
      await openDetail(id);
      await load();
    } catch (e) {
      error.value = String(e);
      note("豆瓣刮削失败：" + String(e));
    }
  }

  /**
   * 删减某个聚合剧目下的单集（单条 / 批量都走这里）。
   *
   * 只删传入的单集 id，其他剧目不受影响；删完同步刷新详情与列表（集数变了）。
   */
  async function removeEpisodes(mediaId: number, ids: number[]): Promise<number> {
    const want = cleanIds(ids);
    if (!want.length) return 0;
    error.value = null;
    try {
      const n = await apiDeleteEpisodes(want);
      note(
        n < want.length
          ? `已删除 ${n} 条资源，另有 ${want.length - n} 条已不在库中`
          : `已删除 ${n} 条资源`,
      );
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
   * 刮削**指定**的一批条目（多选工具条里的「刮削所选」）。
   *
   * 与 `scrapeMissing` 的区别：那个是「缺海报的都来一遍」的后台任务，
   * 这里刮的是用户亲手勾选的几条，逐条走 `scrape`，最后给一句汇总。
   */
  async function scrapeIds(ids: number[]): Promise<void> {
    if (!ids.length) return;
    error.value = null;
    let ok = 0;
    let failed = 0;
    for (const id of ids) {
      try {
        const r = await scrapeMedia(id, null);
        if (r.ok) ok += 1;
        else failed += 1;
      } catch {
        failed += 1;
      }
    }
    await load();
    note(`刮削所选完成：成功 ${ok} 条${failed ? `，失败 ${failed} 条` : ""}`);
    if (failed > 0 && ok === 0) {
      error.value = "所选条目都刮削失败了，请检查 TMDB API Key 与网络";
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
      // 这条可能同时在多选集合里，不剔掉的话下一次「删除所选」会带着它再跑一趟
      dropFromSelection([id]);
      closeDetail();
      await load();
      note("已从媒体库移除");
    } catch (e) {
      // 后端删不到会明确报错（不存在 / 已被删过），原样说给用户，不要谎报成功
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

  /**
   * 当前列表里出现过的汉字 → 拼音秩（**只依赖 `items`**）。
   *
   * 拼音秩是按「这批数据里实际出现的汉字」现算的，所以 `items` 一变就得重算；
   * 好在不依赖关键字与排序方向——敲搜索框不会触发它重算。
   */
  const hanOrder = computed<HanOrder>(() => buildHanOrder(items.value.map(rawTitleOf)));

  /**
   * 排序后的全量列表：`sort(已识别) ++ sort(未识别)`。
   *
   * 必须是 `computed`：`visibleItems()` 一次渲染会被调用七八次
   * （`:items` / 全选计数 / 按钮 disabled / 全选判等 / store 内三个裁剪入口），
   * 把排序塞进函数体就等于一次渲染排七遍。
   */
  const orderedItems = computed<MediaItem[]>(() =>
    sortMediaItems(items.value, settings.sortBy, settings.sortDir, hanOrder.value));

  /**
   * 移动一批剧集（封面墙的拖拽聚合 / 解除分组落点）。
   *
   * 成功后**保留选中**：用户刚拖完的那几项通常还要接着操作（再拖、打开详情），
   * 清掉勾选等于逼他重新选一遍。
   *
   * 这里**不重拉列表**：`parent_id` 变了但 `items` 里的行没变，重拉的时机由
   * 调用方决定（封面墙要立刻重画分组，所以在 `App.vue` 里跟着 `load()`）。
   */
  async function moveNodes(
    ids: number[],
    targetParentId: number | null,
    index?: number | null,
  ): Promise<boolean> {
    if (!ids.length) return false;
    error.value = null;
    try {
      const res = await apiMoveMediaNodes({ ids, targetParentId, index });
      undoDepth.value = res.undo_depth;
      note(res.message);
      return true;
    } catch (e) {
      // 后端整批在一个事务里，失败即回滚；这里只是把原因说清楚
      error.value = String(e);
      note(String(e));
      return false;
    }
  }

  /**
   * 撤销最后一次层级操作。
   *
   * 撤销会改回 `parent_id`，所以必须重拉列表——否则封面墙上的分组
   * 还停在撤销前的样子，用户会以为没生效。
   */
  async function undoTree(): Promise<boolean> {
    error.value = null;
    try {
      const res = await apiUndoTreeOp();
      undoDepth.value = res.undo_depth;
      note(res.message);
      await load();
      return true;
    } catch (e) {
      error.value = String(e);
      note(String(e));
      return false;
    }
  }

  /**
   * 关键字过滤后的列表。
   *
   * 过滤发生在**排序之后**：排序是一次全量计算（挂在 `orderedItems` 上），
   * 敲关键字只做一遍 O(n) 的字符串匹配，不会每按一次键就重排一次。
   */
  function visibleItems(): MediaItem[] {
    const kw = keyword.value.trim().toLowerCase();
    if (!kw) return orderedItems.value;
    return orderedItems.value.filter((it) => {
      // 与后端 LIST_WHERE 的检索口径一致：标题 / 原名 / 类型 / 年份 / 目录名。
      // 少了 original_title 时，用英文原名（刮削前的文件名标题）会一条都搜不到。
      // 注意：toLowerCase 必须包住整个拼接结果——`.toLowerCase()` 只作用于
      // 紧邻的那个模板串，拼到前面去的 display_title 不会被降成小写。
      const hay = (
        `${it.display_title} ${it.title} ${it.original_title ?? ""} ` +
        `${it.group_key ?? ""} ${it.genre ?? ""} ${it.year ?? ""}`
      ).toLowerCase();
      return hay.includes(kw);
    });
  }

  /**
   * 关键字一变就剔掉被过滤掉的选中项。
   *
   * 放在这里（而不是 store 顶部）是因为回调要用到 `visibleItems`，
   * 而它的定义必须在前——`watch` 的声明位置是有语义的，别往上挪。
   */
  watch(keyword, () => pruneSelection());

  return {
    items, filter, keyword, categoryId, tagId, unclassified,
    loading, scanning, scraping, error, toast,
    lastScan, scanHistory, selectedId, detail, scrapeProgress, scanProgress,
    selecting, grouping, selected,
    hanOrder, orderedItems,
    reclassifying,
    load, setFilter, setView, createItem, updateItem, scan, cancelScan, loadHistory,
    openDetail, closeDetail,
    scrape, scrapeMissing, cancelScrape, scrapeIds, play, reveal, remove, clearAll,
    visibleItems, note,
    isSelected, toggleSelect, toggleSelectAll, clearSelection, setSelected, removeSelected,
    selectWithMods, selectAnchor,
    pruneSelection, dropFromSelection, removeByIds, invertSelection,
    importFolder, importFolderInto, removeEpisodes, reclassify,
    undoDepth, moveNodes, undoTree,
    scrapeDouban,
  };
});
