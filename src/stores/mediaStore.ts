import { defineStore } from "pinia";
import { ref } from "vue";
import {
  clearLibrary, deleteMedia, getMediaDetail, getMediaList, getScanHistory,
  playMedia, revealInExplorer, scanLibrary, scrapeAll as apiScrapeAll, scrapeMedia,
  type MediaDetail, type MediaItem, type ScanHistoryItem, type ScanStats,
} from "../api/commands";

export type ListFilter = "all" | "movie" | "tv";

export const useMediaStore = defineStore("media", () => {
  const items = ref<MediaItem[]>([]);
  const filter = ref<ListFilter>("all");
  const keyword = ref("");

  const loading = ref(false);
  const scanning = ref(false);
  const scraping = ref(false);
  const error = ref<string | null>(null);
  const toast = ref<string>("");

  const lastScan = ref<ScanStats | null>(null);
  const scanHistory = ref<ScanHistoryItem[]>([]);

  const selectedId = ref<number | null>(null);
  const detail = ref<MediaDetail | null>(null);

  function note(msg: string): void {
    toast.value = msg;
  }

  /** 按当前过滤条件加载列表。 */
  async function load(): Promise<void> {
    loading.value = true;
    error.value = null;
    try {
      items.value = await getMediaList(filter.value);
    } catch (e) {
      error.value = String(e);
      items.value = [];
    } finally {
      loading.value = false;
    }
  }

  /** 切换过滤条件并重新加载。 */
  async function setFilter(f: ListFilter): Promise<void> {
    filter.value = f;
    selectedId.value = null;
    detail.value = null;
    await load();
  }

  /** 扫描媒体库并刷新。 */
  async function scan(): Promise<void> {
    scanning.value = true;
    error.value = null;
    note("");
    try {
      const stats = await scanLibrary();
      lastScan.value = stats;
      note(
        `扫描完成：发现 ${stats.scanned} 个视频（新增电影 ${stats.addedMovies}、剧集 ${stats.addedSeries}、单集 ${stats.addedEpisodes}）`,
      );
      await load();
      await loadHistory();
    } catch (e) {
      error.value = String(e);
      note("扫描失败：" + error.value);
    } finally {
      scanning.value = false;
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
      note(r.ok ? `刮削成功：${r.title ?? ""}` : r.message);
      await openDetail(id);
      await load();
    } catch (e) {
      error.value = String(e);
      note("刮削失败：" + error.value);
    }
  }

  /** 批量刮削缺海报的条目。 */
  async function scrapeMissing(): Promise<void> {
    scraping.value = true;
    error.value = null;
    note("");
    try {
      const s = await apiScrapeAll();
      if (s.noKey) {
        note("请先在「设置」填写 TMDB API Key");
      } else {
        note(`批量刮削：共 ${s.total} 条，成功 ${s.ok}，失败 ${s.failed}`);
      }
      await load();
      if (selectedId.value) await openDetail(selectedId.value);
    } catch (e) {
      error.value = String(e);
      note("批量刮削失败：" + error.value);
    } finally {
      scraping.value = false;
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
      const hay = `${it.title} ${it.genre ?? ""} ${it.year ?? ""}`.toLowerCase();
      return hay.includes(kw);
    });
  }

  return {
    items, filter, keyword, loading, scanning, scraping, error, toast,
    lastScan, scanHistory, selectedId, detail,
    load, setFilter, scan, loadHistory, openDetail, closeDetail,
    scrape, scrapeMissing, play, reveal, remove, clearAll, visibleItems, note,
  };
});
