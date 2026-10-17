import { defineStore } from "pinia";
import { ref } from "vue";
import {
  clearPlayback as apiClearPlayback,
  getPlayback as apiGetPlayback,
  getPlaybackMany as apiGetPlaybackMany,
  markWatched as apiMarkWatched,
  markWatchedBatch as apiMarkWatchedBatch,
  playEpisode as apiPlayEpisode,
  recentPlayback as apiRecentPlayback,
  setPlaybackPosition as apiSetPosition,
  type MediaPlayback, type PlayResult,
} from "../api/commands";

/**
 * 观看状态 store（v6）。
 *
 * 口径必须写清楚：这是**集级**进度，不是秒级。
 * 本软件调用的是外部播放器（PotPlayer），播放器进程不会把播放位置回传给我们，
 * 所以「看到第 3 分 12 秒」这种精度本期拿不到 —— 「继续播放」= 播下一集未看的。
 * 想要秒级精度只能内嵌播放器，那会改动主流程（B1），本期不做。
 */
export const usePlaybackStore = defineStore("playback", () => {
  /** media_id → 摘要。列表页一次拉全，避免每条一个请求。 */
  const map = ref<Record<number, MediaPlayback>>({});
  /** 最近播放过的条目（「继续观看」用）。 */
  const recent = ref<MediaPlayback[]>([]);
  const loading = ref(false);
  const error = ref<string | null>(null);
  const toast = ref("");

  function note(msg: string): void {
    toast.value = msg;
  }

  /** 取一条摘要；没拉过就返回 null（界面按「未看」渲染）。 */
  function of(mediaId: number): MediaPlayback | null {
    return map.value[mediaId] ?? null;
  }

  /** 只看「有没有看过」这个布尔量（海报角标用）。 */
  function isWatched(mediaId: number): boolean {
    return map.value[mediaId]?.watched ?? false;
  }

  /** 剧集进度文本，如「3/25」；非剧集返回空串。 */
  function progressText(mediaId: number): string {
    const p = map.value[mediaId];
    if (!p || p.totalEpisodes <= 0) return "";
    return `${p.watchedEpisodes}/${p.totalEpisodes}`;
  }

  /** 写回一份摘要，并同步维护 `recent`（若它也在最近列表里）。 */
  function put(p: MediaPlayback): void {
    map.value = { ...map.value, [p.mediaId]: p };
    const i = recent.value.findIndex((x) => x.mediaId === p.mediaId);
    if (i >= 0) recent.value.splice(i, 1, p);
  }

  /** 按一批 id 批量拉（列表渲染前调用；空数组不发请求）。 */
  async function loadMany(ids: number[]): Promise<void> {
    if (!ids.length) {
      map.value = {};
      return;
    }
    loading.value = true;
    error.value = null;
    try {
      // 后端返回的是 `{ "12": {...} }`（JSON 对象的键必然是字符串），这里转回数字键。
      const raw = await apiGetPlaybackMany(ids);
      const next: Record<number, MediaPlayback> = {};
      for (const [k, v] of Object.entries(raw ?? {})) {
        const n = Number(k);
        if (Number.isFinite(n)) next[n] = v;
      }
      map.value = next;
    } catch (e) {
      // 观看状态拿不到不该让整个列表打不开：留空即可，界面按「未看」渲染
      error.value = String(e);
    } finally {
      loading.value = false;
    }
  }

  /** 单独刷新一条（详情页打开、或刚改完状态时用）。 */
  async function loadOne(mediaId: number): Promise<MediaPlayback | null> {
    error.value = null;
    try {
      const p = await apiGetPlayback(mediaId);
      put(p);
      return p;
    } catch (e) {
      error.value = String(e);
      return null;
    }
  }

  /** 最近播放（「继续观看」）。 */
  async function loadRecent(limit = 12): Promise<void> {
    try {
      recent.value = await apiRecentPlayback(limit);
    } catch {
      recent.value = [];
    }
  }

  /** 标记看过 / 没看过（集级：`episodeId` 为空表示整部）。 */
  async function mark(mediaId: number, watched: boolean, episodeId?: number | null): Promise<MediaPlayback | null> {
    error.value = null;
    try {
      const p = await apiMarkWatched({ mediaId, episodeId: episodeId ?? null, watched });
      put(p);
      return p;
    } catch (e) {
      error.value = String(e);
      note("更新观看状态失败：" + String(e));
      return null;
    }
  }

  /** 批量标记（列表页多选后用）。返回受影响条数。 */
  async function markBatch(mediaIds: number[], watched: boolean): Promise<number> {
    if (!mediaIds.length) return 0;
    error.value = null;
    try {
      const n = await apiMarkWatchedBatch(mediaIds, watched);
      note(watched ? `已标记 ${n} 条为看过` : `已取消 ${n} 条的看过标记`);
      await loadMany(mediaIds);
      return n;
    } catch (e) {
      error.value = String(e);
      note("批量标记失败：" + String(e));
      return 0;
    }
  }

  /**
   * 手动填播放位置（秒）。
   *
   * 本期不自动采集（外部播放器不回传进度），但字段留着，
   * 用户愿意自己填就填；填了以后「继续播放」能更准一点。
   */
  async function setPosition(
    mediaId: number,
    positionSecs: number | null,
    durationSecs?: number | null,
    episodeId?: number | null,
  ): Promise<MediaPlayback | null> {
    error.value = null;
    try {
      const p = await apiSetPosition({
        mediaId,
        episodeId: episodeId ?? null,
        positionSecs,
        durationSecs: durationSecs ?? null,
      });
      put(p);
      return p;
    } catch (e) {
      error.value = String(e);
      return null;
    }
  }

  /** 清掉一条的观看记录（播放次数、续播点一起清）。 */
  async function clear(mediaId: number): Promise<MediaPlayback | null> {
    error.value = null;
    try {
      const p = await apiClearPlayback(mediaId);
      put(p);
      note("已清除观看记录");
      return p;
    } catch (e) {
      error.value = String(e);
      return null;
    }
  }

  /**
   * 播放某一集；`continuous` 为真时从这一集开始连播。
   *
   * 连播**不生成临时播放列表文件** —— 多路径直接作为参数交给播放器，
   * 媒体目录里不会多出任何东西（视频文件只读红线）。
   */
  /**
   * 播放某一集；`continuous` 为真时从这一集开始连播。
   *
   * 连播**不生成临时播放列表文件** —— 多路径直接作为参数交给播放器，
   * 媒体目录里不会多出任何东西（视频文件只读红线）。
   */
  async function play(mediaId: number, episodeId?: number | null, continuous = false): Promise<PlayResult | null> {
    error.value = null;
    try {
      const r = await apiPlayEpisode({ mediaId, episodeId: episodeId ?? null, continuous });
      note(describePlay(r));
      // 播完顺手把续播点刷新一下：后端已经把这一集记成「最近播放」
      await loadOne(mediaId);
      return r;
    } catch (e) {
      error.value = String(e);
      note("播放失败：" + String(e));
      return null;
    }
  }

  /**
   * 双击封面直接播放（不给 episodeId，让后端决定播哪一集）。
   *
   * 这个入口**只负责转发**「不指定目标」这件事，目标选择规则在后端
   * `play_episode` 里（上次播放过的那一集 > 第一个未看集 > 电影的 file_path）。
   * 规则放后端的原因：它要读库，而且要和服务端其它调用方（连播、详情页）
   * 用同一套判据 —— 前端再抄一份迟早对不上。
   */
  async function playDefault(mediaId: number): Promise<PlayResult | null> {
    return play(mediaId, null, false);
  }

  /**
   * 一句人话，说清**到底播了哪一集**。
   *
   * 双击播「上次那一集」是隐式行为，不告诉用户播了哪集，他会怀疑是不是又从头放了。
   */
  function describePlay(r: PlayResult): string {
    if (r.season != null && r.episode != null) {
      const tag = `S${String(r.season).padStart(2, "0")}E${String(r.episode).padStart(2, "0")}`;
      return r.files > 1
        ? `已用 ${r.player} 从 ${tag} 起连播 ${r.files} 个文件`
        : `已用 ${r.player} 播放 ${tag}`;
    }
    return r.files > 1 ? `已用 ${r.player} 连播 ${r.files} 个文件` : `已用 ${r.player} 播放`;
  }

  return {
    map, recent, loading, error, toast,
    of, isWatched, progressText, note,
    loadMany, loadOne, loadRecent,
    mark, markBatch, setPosition, clear, play, playDefault,
  };
});
