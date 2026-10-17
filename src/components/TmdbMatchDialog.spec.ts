import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { mount, type VueWrapper } from "@vue/test-utils";
import { createPinia, setActivePinia } from "pinia";
import { nextTick } from "vue";
import TmdbMatchDialog from "./TmdbMatchDialog.vue";
import { searchTmdb } from "../api/commands";
import type { MediaItem, TmdbSearchItem } from "../api/commands";
import { useSettingsStore } from "../stores/settingsStore";

/**
 * TMDB 候选面板的用例：盯「筛得准」和「选得上」两件事。
 *
 * 后端命令整体 mock（单测不碰真实 Tauri / 网络），
 * 所以这里验证的是**面板自己的过滤、勾选、滚动加载与确认**，
 * 后端打分排序由 `core/matching.rs` 的单测覆盖。
 */
vi.mock("../api/commands", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../api/commands")>();
  return {
    ...actual,
    searchTmdb: vi.fn(async () => [] as TmdbSearchItem[]),
  };
});

const MEDIA: MediaItem = {
  id: 7,
  type: "movie",
  title: "言叶之庭",
  year: 2013,
  tmdb_id: null,
  poster_path: null,
  backdrop_path: null,
  overview: null,
  vote_average: null,
  genre: null,
  file_path: "D:\\m\\a.mkv",
  file_name: "a.mkv",
  episode_count: 0,
  added_at: 1,
  display_title: "言叶之庭",
  original_title: "The Garden of Words",
};

function cand(
  id: number,
  title: string,
  opts: { year?: number; original?: string; poster?: boolean; score?: number } = {},
): TmdbSearchItem {
  return {
    id,
    title,
    name: null,
    original_title: opts.original ?? null,
    original_name: null,
    poster_path: opts.poster ? "/p.jpg" : null,
    vote_average: 7.5,
    release_date: opts.year ? `${opts.year}-05-31` : null,
    first_air_date: null,
    match_score: opts.score ?? 100,
    match_reasons: ["片名完全一致"],
  };
}

/** 造 N 条候选，标题形如 `候选 1` … `候选 N`。 */
function many(n: number): TmdbSearchItem[] {
  return Array.from({ length: n }, (_, i) => cand(100 + i, `候选 ${i + 1}`, { year: 2013 }));
}

const wrappers: VueWrapper[] = [];

function mountDialog(items: TmdbSearchItem[]): VueWrapper {
  vi.mocked(searchTmdb).mockResolvedValue(items);
  const w = mount(TmdbMatchDialog, {
    props: { open: true, media: MEDIA },
    attachTo: document.body,
  });
  wrappers.push(w);
  return w;
}

beforeEach(() => {
  setActivePinia(createPinia());
  vi.mocked(searchTmdb).mockClear();
});

afterEach(() => {
  while (wrappers.length) wrappers.pop()?.unmount();
});

/** 等「打开即检索」那次请求落地并渲染完。 */
async function settled(): Promise<void> {
  await nextTick();
  await nextTick();
  await nextTick();
  await new Promise((r) => setTimeout(r, 0));
  await nextTick();
}

describe("TmdbMatchDialog", () => {
  it("打开即用条目标题检索，并带上原标题去比对多语言", async () => {
    mountDialog([cand(1, "言叶之庭", { original: "The Garden of Words", year: 2013 })]);
    await settled();

    expect(searchTmdb).toHaveBeenCalledTimes(1);
    const [type, query, year, filter] = vi.mocked(searchTmdb).mock.calls[0];
    expect(type).toBe("movie");
    expect(query).toBe("言叶之庭");
    expect(year).toBe(2013);
    expect(filter?.originalTitle).toBe("The Garden of Words");
  });

  it("候选**全部**渲染出来，不受 20 条限制", async () => {
    const w = mountDialog(many(45));
    await settled();
    // 首屏只渲染 30 条（滚动加载），但计数必须是全量 45
    expect(w.text()).toContain("共 45 个候选");
    expect(w.findAll(".row").length).toBe(30);
  });

  it("滚到底再加载一批", async () => {
    const w = mountDialog(many(75));
    await settled();
    expect(w.findAll(".row").length).toBe(30);

    const list = w.find(".p-list").element;
    list.scrollTop = 9999;
    await w.find(".p-list").trigger("scroll");
    await nextTick();
    expect(w.findAll(".row").length).toBe(60);
  });

  it("关键字实时过滤：标题 / 原名 / 年份 / ID 都能命中", async () => {
    const w = mountDialog([
      cand(101, "言叶之庭", { year: 2013 }),
      cand(102, "你的名字", { year: 2016 }),
      cand(103, "Kotonoha no Niwa", { original: "言叶之庭", year: 2013 }),
    ]);
    await settled();
    expect(w.findAll(".row").length).toBe(3);

    const kw = w.find(".kw");
    await kw.setValue("你的名字");
    expect(w.findAll(".row").length).toBe(1);
    expect(w.text()).toContain("过滤后 1 个");

    // 关键字只做本地过滤，**不会**再发一次网络请求
    expect(searchTmdb).toHaveBeenCalledTimes(1);

    await kw.setValue("2016");
    expect(w.findAll(".row").length).toBe(1);
    // 按 TMDB id 过滤
    await kw.setValue("102");
    expect(w.findAll(".row").length).toBe(1);
    // 按原标题（跨语言）过滤：中文关键字要能命中英文片名的那条
    await kw.setValue("言叶之庭");
    expect(w.findAll(".row").length).toBe(2);
    await kw.setValue("");
    expect(w.findAll(".row").length).toBe(3);
  });

  it("只看有海报 / 只看年份吻合 是即时过滤", async () => {
    const w = mountDialog([
      cand(1, "有图同年份", { year: 2013, poster: true }),
      cand(2, "无图同年份", { year: 2013, poster: false }),
      cand(3, "有图差年份", { year: 2001, poster: true }),
    ]);
    await settled();
    expect(w.findAll(".row").length).toBe(3);

    const boxes = w.findAll(".p-filters input[type=checkbox]");
    // 顺序：模糊匹配 / 只看有海报 / 只看年份吻合
    await boxes[1].setValue(true);
    expect(w.findAll(".row").length).toBe(2);
    await boxes[2].setValue(true);
    expect(w.findAll(".row").length).toBe(1);
    expect(w.text()).toContain("有图同年份");
  });

  it("点一行即选中，再点一次取消；底部清楚显示当前已选", async () => {
    const w = mountDialog([cand(1, "言叶之庭", { year: 2013 }), cand(2, "别的片")]);
    await settled();
    expect(w.text()).toContain("还没选");

    await w.findAll(".row")[0].trigger("click");
    expect(w.text()).toContain("已选：");
    expect(w.text()).toContain("TMDB #1");
    expect(w.findAll(".row")[0].classes()).toContain("on");

    await w.findAll(".row")[0].trigger("click");
    expect(w.text()).toContain("还没选");
  });

  it("单选：选第二条会顶掉第一条", async () => {
    const w = mountDialog([cand(1, "第一条"), cand(2, "第二条")]);
    await settled();
    await w.findAll(".row")[0].trigger("click");
    await w.findAll(".row")[1].trigger("click");
    expect(w.text()).toContain("TMDB #2");
    expect(w.findAll(".row")[0].classes()).not.toContain("on");
    expect(w.findAll(".row")[1].classes()).toContain("on");
  });

  it("没选时「应用所选」是禁用的；选了以后点它把 id 交出去并关闭", async () => {
    const w = mountDialog([cand(42, "言叶之庭", { year: 2013 })]);
    await settled();
    const apply = w.find(".p-foot .btn-primary");
    expect(apply.attributes("disabled")).toBeDefined();

    await w.findAll(".row")[0].trigger("click");
    expect(w.find(".p-foot .btn-primary").attributes("disabled")).toBeUndefined();
    await w.find(".p-foot .btn-primary").trigger("click");

    expect(w.emitted("apply")).toEqual([[42]]);
    expect(w.emitted("update:open")).toEqual([[false]]);
  });

  it("改筛选条件（年份容差）会重新检索，且把新条件带给后端", async () => {
    const w = mountDialog([cand(1, "言叶之庭")]);
    await settled();
    expect(searchTmdb).toHaveBeenCalledTimes(1);

    await w.find(".p-filters select").setValue("0");
    // 防抖 350ms
    await new Promise((r) => setTimeout(r, 420));
    await nextTick();

    expect(searchTmdb).toHaveBeenCalledTimes(2);
    expect(vi.mocked(searchTmdb).mock.calls[1][3]?.yearTolerance).toBe(0);
  });

  it("关闭按钮发出 update:open=false", async () => {
    const w = mountDialog([cand(1, "言叶之庭")]);
    await settled();
    await w.find(".p-head .btn").trigger("click");
    expect(w.emitted("update:open")).toEqual([[false]]);
  });

  it("底部标出「确认」当前绑的快捷键", async () => {
    const settings = useSettingsStore();
    settings.shortcutConfirm = "Ctrl+Enter";
    const w = mountDialog([cand(1, "言叶之庭")]);
    await settled();
    expect(w.text()).toContain("Ctrl+Enter");
  });

  it("检索失败时把错误显示出来，而不是空列表骗人", async () => {
    vi.mocked(searchTmdb).mockRejectedValueOnce(new Error("尚未配置 TMDB API Key"));
    const w = mount(TmdbMatchDialog, {
      props: { open: true, media: MEDIA },
      attachTo: document.body,
    });
    wrappers.push(w);
    await settled();
    expect(w.text()).toContain("尚未配置 TMDB API Key");
  });
});
