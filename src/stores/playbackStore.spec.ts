import { beforeEach, describe, expect, it, vi } from "vitest";
import { createPinia, setActivePinia } from "pinia";
import { usePlaybackStore } from "./playbackStore";
import type { MediaPlayback, PlayResult } from "../api/commands";

/**
 * 观看状态 store 的验收点：
 * 1. 后端返回的**字符串键**要转成数字键（JSON 对象的键必然是字符串）；
 * 2. 标记 / 清除都要写回本地 map，界面不用再拉一次；
 * 3. 口径是**集级**不是秒级 —— 进度文本走 `3/25`，绝不出现「几分几秒」。
 * 后端命令全部 mock，不依赖真实 Tauri 运行时。
 */
const mocks = vi.hoisted(() => ({
  clearPlayback: vi.fn(),
  getPlayback: vi.fn(),
  getPlaybackMany: vi.fn(),
  markWatched: vi.fn(),
  markWatchedBatch: vi.fn(),
  playEpisode: vi.fn(),
  recentPlayback: vi.fn(),
  setPlaybackPosition: vi.fn(),
}));

vi.mock("../api/commands", () => mocks);

function pb(over: Partial<MediaPlayback> = {}): MediaPlayback {
  return {
    mediaId: 1,
    watched: false,
    watchedEpisodes: 0,
    totalEpisodes: 0,
    playCount: 0,
    lastPlayedAt: null,
    resumeEpisodeId: null,
    lastEpisodeId: null,
    lastSeason: null,
    lastEpisode: null,
    lastPlayedEpisodeId: null,
    ...over,
  };
}

describe("playbackStore: 加载", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    mocks.getPlaybackMany.mockResolvedValue({});
    mocks.getPlayback.mockResolvedValue(pb());
    mocks.recentPlayback.mockResolvedValue([]);
  });

  it("批量拉会转成数字键（后端给的是字符串键）", async () => {
    mocks.getPlaybackMany.mockResolvedValue({
      "7": pb({ mediaId: 7, watched: true }),
      "8": pb({ mediaId: 8 }),
    });

    const s = usePlaybackStore();
    await s.loadMany([7, 8]);

    expect(s.of(7)?.watched).toBe(true);
    expect(s.of(8)?.watched).toBe(false);
    expect(s.isWatched(7)).toBe(true);
  });

  it("没有拉过的条目不算看过（不画角标，而不是画成未看）", () => {
    const s = usePlaybackStore();
    expect(s.of(999)).toBeNull();
    expect(s.isWatched(999)).toBe(false);
    expect(s.progressText(999)).toBe("");
  });

  it("空列表不发请求", async () => {
    const s = usePlaybackStore();
    await s.loadMany([]);
    expect(mocks.getPlaybackMany).not.toHaveBeenCalled();
  });

  it("批量拉取失败不抛错（观看状态拿不到不该让列表打不开）", async () => {
    mocks.getPlaybackMany.mockRejectedValue(new Error("boom"));
    const s = usePlaybackStore();

    await expect(s.loadMany([1])).resolves.toBeUndefined();
    expect(s.error).toContain("boom");
  });
});

describe("playbackStore: 进度文案（集级，不是秒级）", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    mocks.getPlaybackMany.mockResolvedValue({});
    mocks.getPlayback.mockResolvedValue(pb());
  });

  it("剧集显示 已看/总数", async () => {
    mocks.getPlaybackMany.mockResolvedValue({
      "3": pb({ mediaId: 3, watchedEpisodes: 3, totalEpisodes: 25 }),
    });
    const s = usePlaybackStore();
    await s.loadMany([3]);

    expect(s.progressText(3)).toBe("3/25");
  });

  it("电影没有总集数，返回空串（角标退回「看过」）", async () => {
    mocks.getPlaybackMany.mockResolvedValue({
      "4": pb({ mediaId: 4, watched: true, watchedEpisodes: 1, totalEpisodes: 0 }),
    });
    const s = usePlaybackStore();
    await s.loadMany([4]);

    expect(s.progressText(4)).toBe("");
    expect(s.isWatched(4)).toBe(true);
  });
});

describe("playbackStore: 标记与清除", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    mocks.getPlaybackMany.mockResolvedValue({});
    mocks.markWatched.mockResolvedValue(pb({ mediaId: 1, watched: true }));
    mocks.clearPlayback.mockResolvedValue(pb({ mediaId: 1 }));
    mocks.getPlayback.mockResolvedValue(pb({ mediaId: 1, watched: true }));
  });

  it("标记已看会写回本地 map（界面不用再拉一次）", async () => {
    const s = usePlaybackStore();
    const r = await s.mark(1, true);

    expect(mocks.markWatched).toHaveBeenCalledWith({
      mediaId: 1, episodeId: null, watched: true,
    });
    expect(r?.watched).toBe(true);
    expect(s.isWatched(1)).toBe(true);
  });

  it("可以按单集标记（剧集的集级记录）", async () => {
    mocks.markWatched.mockResolvedValue(pb({ mediaId: 2, watchedEpisodes: 1, totalEpisodes: 10 }));
    const s = usePlaybackStore();
    await s.mark(2, true, 55);

    expect(mocks.markWatched).toHaveBeenCalledWith({
      mediaId: 2, episodeId: 55, watched: true,
    });
    expect(s.progressText(2)).toBe("1/10");
  });

  it("批量标记会给一句可读反馈并重拉这批", async () => {
    mocks.markWatchedBatch.mockResolvedValue(3);
    mocks.getPlaybackMany.mockResolvedValue({
      "1": pb({ mediaId: 1, watched: true }),
      "2": pb({ mediaId: 2, watched: true }),
      "3": pb({ mediaId: 3, watched: true }),
    });
    const s = usePlaybackStore();
    const n = await s.markBatch([1, 2, 3], true);

    expect(n).toBe(3);
    expect(mocks.markWatchedBatch).toHaveBeenCalledWith([1, 2, 3], true);
    expect(s.toast).toContain("3");
    expect(s.isWatched(2)).toBe(true);
  });

  it("空集合不发请求", async () => {
    const s = usePlaybackStore();
    expect(await s.markBatch([], true)).toBe(0);
    expect(mocks.markWatchedBatch).not.toHaveBeenCalled();
  });

  it("清除记录后不再是看过", async () => {
    const s = usePlaybackStore();
    await s.loadOne(1);
    expect(s.isWatched(1)).toBe(true);

    await s.clear(1);
    expect(s.isWatched(1)).toBe(false);
  });
});

describe("playbackStore: 播放", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    mocks.getPlaybackMany.mockResolvedValue({});
    mocks.getPlayback.mockResolvedValue(pb({ mediaId: 1, watched: true }));
  });

  it("连播把多个文件交给播放器，提示里给出文件数", async () => {
    const r: PlayResult = {
      player: "potplayer", files: 8, mediaId: 1, episodeId: 5, season: 1, episode: 2,
    };
    mocks.playEpisode.mockResolvedValue(r);

    const s = usePlaybackStore();
    const got = await s.play(1, 5, true);

    expect(mocks.playEpisode).toHaveBeenCalledWith({
      mediaId: 1, episodeId: 5, continuous: true,
    });
    expect(got?.files).toBe(8);
    expect(s.toast).toContain("8");
    // 播的是哪一集必须说出来：连播从某一集起，用户要知道起点
    expect(s.toast).toContain("S01E02");
  });

  /* ---------------- 双击直接播放 ---------------- */

  /**
   * 双击 = 不指定目标，交给后端决定。
   * 前端**不能**自己挑一集：判据（上次播放 > 第一个未看集）在后端，两处各写一套迟早对不上。
   */
  it("双击播放不传 episodeId，把选哪一集交给后端", async () => {
    const r: PlayResult = {
      player: "potplayer", files: 1, mediaId: 1, episodeId: 7, season: 1, episode: 3,
    };
    mocks.playEpisode.mockResolvedValue(r);
    const s = usePlaybackStore();

    const got = await s.playDefault(1);

    expect(mocks.playEpisode).toHaveBeenCalledWith({
      mediaId: 1, episodeId: null, continuous: false,
    });
    expect(got?.episodeId).toBe(7);
  });

  /** 具体集号要播报出来，否则用户会怀疑"是不是又从头放了"。 */
  it("播放提示写明播的是哪一集（S01E03）", async () => {
    mocks.playEpisode.mockResolvedValue({
      player: "potplayer", files: 1, mediaId: 1, episodeId: 7, season: 1, episode: 3,
    } as PlayResult);
    const s = usePlaybackStore();

    await s.playDefault(1);

    expect(s.toast).toContain("S01E03");
  });

  /** 电影没有季集号：提示退回「已用 X 播放」，不能显示成 S00E00。 */
  it("电影（没有季集号）的提示不带 S00E00", async () => {
    mocks.playEpisode.mockResolvedValue({
      player: "system", files: 1, mediaId: 1, episodeId: null, season: null, episode: null,
    } as PlayResult);
    const s = usePlaybackStore();

    await s.playDefault(1);

    expect(s.toast).toContain("已用 system 播放");
    expect(s.toast).not.toContain("S00E00");
    expect(s.toast).not.toContain("E00");
  });

  it("播放失败不静默", async () => {
    mocks.playEpisode.mockRejectedValue(new Error("找不到播放器"));
    const s = usePlaybackStore();

    expect(await s.play(1, null, false)).toBeNull();
    expect(s.error).toContain("找不到播放器");
  });

  it("手动填播放位置（字段预留，本期不自动采集）", async () => {
    mocks.setPlaybackPosition.mockResolvedValue(pb({ mediaId: 1, playCount: 2 }));
    const s = usePlaybackStore();
    await s.setPosition(1, 192, 3600, null);

    expect(mocks.setPlaybackPosition).toHaveBeenCalledWith({
      mediaId: 1, episodeId: null, positionSecs: 192, durationSecs: 3600,
    });
  });
});
