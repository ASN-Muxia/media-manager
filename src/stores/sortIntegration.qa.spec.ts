/**
 * QA 独立审计（严过关 / Edward）——**store 接线层**。
 *
 * `sort.ts` 是纯函数，纯函数对了不代表接对了：这里验证
 * 「设置 → store → 界面」这条链真的通：切换排序方式后列表**立即**重排，
 * 过滤作用在排序之后，全选 / 反选作用于排序后的可见列表，
 * 脏的持久化值不会漏到 store 里。
 */
import { beforeEach, describe, expect, it, vi } from "vitest";
import { createPinia, setActivePinia } from "pinia";
import { nextTick } from "vue";
import { useMediaStore } from "./mediaStore";
import { useSettingsStore } from "./settingsStore";
import type { MediaItem } from "../api/commands";

const mocks = vi.hoisted(() => ({
  getMediaList: vi.fn(),
  getSettings: vi.fn(),
  setSettings: vi.fn(),
}));

vi.mock("../api/commands", () => mocks);

function item(over: Partial<MediaItem> = {}): MediaItem {
  return {
    id: 1,
    type: "movie",
    title: "T",
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
    added_at: 1,
    display_title: "T",
    original_title: null,
    group_key: null,
    ...over,
  };
}

/** 一张刻意难排的表：中英数混排 + 一条 unknown + 一条缺年份 + 一条无标题。 */
function dataset(): MediaItem[] {
  return [
    item({ id: 1, display_title: "宙斯之子", year: 2014, added_at: 400 }),
    item({ id: 2, display_title: "Avatar", year: 2009, added_at: 200 }),
    item({ id: 3, display_title: "阿甘正传", year: 1994, added_at: 300 }),
    item({ id: 4, display_title: "2012", year: 2009, added_at: 100 }),
    item({ id: 5, type: "unknown", display_title: "待确认资源", year: 2011, added_at: 50 }),
    item({ id: 6, type: "movie", display_title: "", title: "", year: 2011, added_at: 60 }),
    item({ id: 7, type: "movie", display_title: "没年份的片子", year: null, added_at: 70 }),
  ];
}

function seedSettings(over: Record<string, unknown> = {}): void {
  mocks.getSettings.mockResolvedValue({
    tmdbKey: "",
    mediaDirs: [],
    potPlayerPath: "",
    hosts: [],
    tmdbLanguage: "zh-CN",
    sortBy: "name",
    sortDir: "asc",
    ...over,
  });
}

describe("QA store 接线：切换排序方式立即重排", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    mocks.getMediaList.mockResolvedValue(dataset());
    mocks.setSettings.mockResolvedValue(undefined);
    seedSettings();
  });

  it("默认 name/asc：2012 → Avatar → 阿甘正传 → 宙斯之子，未识别沉底", async () => {
    const settings = useSettingsStore();
    const media = useMediaStore();
    await settings.load();
    await media.load();

    // 未识别三条内部按名称排：空标题的名称键是空流（短者在前）→ 排最靠前
    expect(media.visibleItems().map((i) => i.display_title)).toEqual([
      "2012", "Avatar", "阿甘正传", "宙斯之子", "", "待确认资源", "没年份的片子",
    ]);
  });

  it("切到 added/asc 立刻按添加时间重排（无需 reload）", async () => {
    const settings = useSettingsStore();
    const media = useMediaStore();
    await settings.load();
    await media.load();

    settings.sortBy = "added";
    await nextTick();
    // 分区优先于主键：即使三条未识别的 added_at（50/60/70）比任何已识别都早，
    // 它们也只能在自己那个分区里排——已识别恒在前。
    expect(media.visibleItems().map((i) => i.id)).toEqual([4, 2, 3, 1, 5, 6, 7]);
  });

  it("切到 year/desc 立刻按年份倒序重排，缺年份仍在末尾", async () => {
    const settings = useSettingsStore();
    const media = useMediaStore();
    await settings.load();
    await media.load();

    settings.sortBy = "year";
    settings.sortDir = "desc";
    await nextTick();
    const titles = media.visibleItems().map((i) => i.display_title);
    // 已识别的四条：2014 → 2009(同年 Avatar / 2012 由名称兜底) → 1994；
    // 未识别三条：同年的空标题在前（`""` 名称键最短），缺年份的恒在分区末尾
    expect(titles).toEqual([
      "宙斯之子", "2012", "Avatar", "阿甘正传", "", "待确认资源", "没年份的片子",
    ]);
  });

  it("任何排序组合下，三条未识别恒在四条已识别之后", async () => {
    const settings = useSettingsStore();
    const media = useMediaStore();
    await settings.load();
    await media.load();

    for (const by of ["name", "year", "added"] as const) {
      for (const dir of ["asc", "desc"] as const) {
        settings.sortBy = by;
        settings.sortDir = dir;
        await nextTick();
        const out = media.visibleItems().map((i) => i.id);
        const head = out.slice(0, 4).sort((a, b) => a - b);
        const tail = out.slice(4).sort((a, b) => a - b);
        expect(head, `${by}/${dir} 已识别`).toEqual([1, 2, 3, 4]);
        expect(tail, `${by}/${dir} 未识别`).toEqual([5, 6, 7]);
      }
    }
  });

  it("关键字过滤作用在排序之后，且保持排序后的相对次序", async () => {
    const media = useMediaStore();
    await media.load();

    media.keyword = "2012";
    expect(media.visibleItems().map((i) => i.id)).toEqual([4]);
    media.keyword = "";
    const sorted = media.visibleItems().map((i) => i.id);
    media.keyword = "a";
    const filtered = media.visibleItems().map((i) => i.id);
    expect(sorted.filter((id) => filtered.includes(id))).toEqual(filtered);
  });
});

describe("QA store 接线：选择行为作用于排序后的可见列表", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    mocks.getMediaList.mockResolvedValue(dataset());
    mocks.setSettings.mockResolvedValue(undefined);
    seedSettings();
  });

  it("全选的顺序与 visibleItems 完全一致", async () => {
    const media = useMediaStore();
    await media.load();

    media.toggleSelectAll();
    expect(media.selected).toEqual(media.visibleItems().map((i) => i.id));

    // 倒序后全选的顺序必须跟着变
    const settings = useSettingsStore();
    await settings.load();
    settings.sortDir = "desc";
    await nextTick();
    media.toggleSelectAll(); // 取消
    media.toggleSelectAll(); // 再全选
    expect(media.selected).toEqual(media.visibleItems().map((i) => i.id));
    expect(media.selected[0]).toBe(media.visibleItems()[0].id);
  });

  it("反选后剩余集合仍与可见列表顺序一致", async () => {
    const media = useMediaStore();
    await media.load();

    const visible = media.visibleItems().map((i) => i.id);
    media.setSelected([visible[0]]);
    media.invertSelection();
    expect(media.selected).toEqual(visible.slice(1));
  });
});

describe("QA 持久化：脏值不进状态、脏值不落库", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    mocks.setSettings.mockResolvedValue(undefined);
  });

  it("老库缺 sortBy / sortDir → name / asc", async () => {
    seedSettings({ sortBy: undefined, sortDir: undefined });
    const s = useSettingsStore();
    await s.load();
    expect(s.sortBy).toBe("name");
    expect(s.sortDir).toBe("asc");
  });

  it("脏值（NAME / bogus / 空串 / null）一律回落默认", async () => {
    for (const dirty of ["NAME", "bogus", "", "   ", null, undefined, 42, []]) {
      seedSettings({ sortBy: dirty, sortDir: dirty });
      setActivePinia(createPinia());
      const s = useSettingsStore();
      await s.load();
      expect(s.sortBy, `sortBy=${String(dirty)}`).toBe("name");
      expect(s.sortDir, `sortDir=${String(dirty)}`).toBe("asc");
    }
  });

  it("store 里被塞进脏值时，落库前必须洗掉（脏值不进表）", async () => {
    seedSettings();
    const s = useSettingsStore();
    await s.load();

    s.sortBy = "DESC-value" as unknown as typeof s.sortBy;
    s.sortDir = "sideways" as unknown as typeof s.sortDir;
    await s.save();
    expect(mocks.setSettings.mock.calls[0][0].sortBy).toBe("name");
    expect(mocks.setSettings.mock.calls[0][0].sortDir).toBe("asc");
  });

  it("合法值原样读出、原样落库", async () => {
    seedSettings({ sortBy: "added", sortDir: "desc" });
    const s = useSettingsStore();
    await s.load();
    expect(s.sortBy).toBe("added");
    expect(s.sortDir).toBe("desc");

    await s.save();
    expect(mocks.setSettings.mock.calls[0][0].sortBy).toBe("added");
    expect(mocks.setSettings.mock.calls[0][0].sortDir).toBe("desc");
  });
});
