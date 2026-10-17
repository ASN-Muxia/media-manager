/**
 * LibraryCatalog 的用例：盯「看得见」和「送得对」两件事。
 *
 * 这个面板的价值全在**信息呈现**上——用户要知道这次检测相对上次多了什么、
 * 少了什么，以及哪几条文件已经不在原路径了。所以这里的断言集中在
 * 「渲染出了什么」与「筛选条件有没有真发给后端」，而不是样式细节。
 *
 * 后端命令整体 mock（单测不碰真实 Tauri / 文件系统），与
 * `TmdbMatchDialog.spec.ts` / `MediaDetail.group.spec.ts` 同一套路子：
 * 命令层返回类型显式写出来，否则 `vi.mocked(getResourceCatalog)` 是 `never[]`，
 * 后面按用例改写返回值时会类型不匹配。
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { mount, type VueWrapper } from "@vue/test-utils";
import { nextTick } from "vue";
import LibraryCatalog from "./LibraryCatalog.vue";
import {
  diffResourceCatalog,
  getResourceCatalog,
  syncResourceCatalog,
  type CatalogDiff,
  type CatalogEntry,
  type CatalogSyncResult,
} from "../api/commands";

// 工厂里只用字面量：它在 import 阶段就会执行，那时下面的 helper 还没初始化。
vi.mock("../api/commands", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../api/commands")>();
  return {
    ...actual,
    syncResourceCatalog: vi.fn(
      async (): Promise<CatalogSyncResult> => ({
        startedAt: 0,
        finishedAt: 0,
        roots: 0,
        scanned: 0,
        added: 0,
        updated: 0,
        unchanged: 0,
        missing: 0,
        restored: 0,
        message: "",
      }),
    ),
    getResourceCatalog: vi.fn(async (): Promise<CatalogEntry[]> => []),
    diffResourceCatalog: vi.fn(
      async (): Promise<CatalogDiff> => ({
        added: [],
        changed: [],
        missing: [],
        present: 0,
        lastSyncAt: null,
      }),
    ),
  };
});

/** 造一条目录库记录：默认「在库、未关联」，用例按需覆写。 */
function entry(id: number, over: Partial<CatalogEntry> = {}): CatalogEntry {
  return {
    id,
    path: `D:/影片/${id}.mkv`,
    fileName: `影片${id}.mkv`,
    relDir: "电影",
    rootPath: "D:/影片",
    size: 1024,
    mtime: 1_700_000_000,
    status: "present",
    mediaId: null,
    displayTitle: null,
    mediaType: null,
    ...over,
  };
}

function syncResult(over: Partial<CatalogSyncResult> = {}): CatalogSyncResult {
  return {
    startedAt: 1_700_000_000,
    finishedAt: 1_700_000_010,
    roots: 2,
    scanned: 0,
    added: 0,
    updated: 0,
    unchanged: 0,
    missing: 0,
    restored: 0,
    message: "检测完成",
    ...over,
  };
}

function diffOf(over: Partial<CatalogDiff> = {}): CatalogDiff {
  return { added: [], changed: [], missing: [], present: 0, lastSyncAt: null, ...over };
}

/**
 * 三个命令各自的返回值，由用例在挂载前改写。
 *
 * `fail` 非空时三个命令一起失败：真实情况下后端出错也不会只错一个，
 * 这样能验证「出错时整块都不静默」。
 */
const backends = {
  entries: [] as CatalogEntry[],
  diff: null as CatalogDiff | null,
  sync: null as CatalogSyncResult | null,
  fail: "",
};

const ZERO_SYNC: CatalogSyncResult = syncResult({ message: "" });

function applyBackends(): void {
  vi.mocked(getResourceCatalog).mockImplementation(async () => {
    if (backends.fail) throw new Error(backends.fail);
    return backends.entries;
  });
  vi.mocked(diffResourceCatalog).mockImplementation(async () => {
    if (backends.fail) throw new Error(backends.fail);
    return backends.diff ?? diffOf();
  });
  vi.mocked(syncResourceCatalog).mockImplementation(async () => {
    if (backends.fail) throw new Error(backends.fail);
    return backends.sync ?? ZERO_SYNC;
  });
}

const wrappers: VueWrapper[] = [];

function mountPanel(over: Partial<typeof backends> = {}): VueWrapper {
  Object.assign(backends, over);
  applyBackends();
  const w = mount(LibraryCatalog, { attachTo: document.body });
  wrappers.push(w);
  return w;
}

beforeEach(() => {
  backends.entries = [];
  backends.diff = null;
  backends.sync = null;
  backends.fail = "";
});

afterEach(() => {
  while (wrappers.length) wrappers.pop()?.unmount();
});

/** 等挂载 / 点击后那几次异步请求落地并渲染完（Vue 的 DOM 更新是异步的）。 */
async function settled(): Promise<void> {
  await nextTick();
  await nextTick();
  await new Promise((r) => setTimeout(r, 0));
  await nextTick();
}

describe("LibraryCatalog", () => {
  it("挂载后自动拉取清单并渲染条目", async () => {
    const w = mountPanel({
      entries: [entry(1), entry(2, { fileName: "第二部.mkv", displayTitle: "第二部" })],
    });
    await settled();

    expect(vi.mocked(getResourceCatalog)).toHaveBeenCalledTimes(1);
    expect(w.findAll("tbody tr")).toHaveLength(2);
    expect(w.text()).toContain("影片1.mkv");
    expect(w.text()).toContain("第二部.mkv");
    // 关联条目：有 displayTitle 用它，没有就说「未关联」
    expect(w.text()).toContain("第二部");
    expect(w.text()).toContain("未关联");
  });

  it("大文件按 KB / MB 人性化显示大小", async () => {
    const w = mountPanel({
      entries: [
        entry(1, { size: 2048 }),
        entry(2, { size: 3 * 1024 * 1024 }),
      ],
    });
    await settled();

    const rows = w.findAll("tbody tr");
    expect(rows[0].text()).toContain("2.0 KB");
    expect(rows[1].text()).toContain("3.0 MB");
  });

  /* ---------------- 丢失条目：这是「定位旧资源」的落点 ---------------- */

  it("丢失条目有视觉区分与文案", async () => {
    const w = mountPanel({
      entries: [
        entry(1),
        entry(9, { fileName: "已删除.mkv", status: "missing", relDir: "剧集/第一季" }),
      ],
    });
    await settled();

    const rows = w.findAll("tbody tr");
    const missingRow = w.findAll("tbody tr").find((r) => r.text().includes("已删除.mkv"));
    expect(missingRow).toBeTruthy();
    expect(missingRow?.classes()).toContain("is-missing");
    expect(missingRow?.find(".tag-missing").exists()).toBe(true);
    expect(missingRow?.text()).toContain("已丢失");
    // 在库那条不能被一起划掉
    expect(rows[0].classes()).not.toContain("is-missing");
    expect(rows[0].text()).toContain("在库");
    // 给出「重新检测」的提示
    expect(w.text()).toContain("重新检测");
    expect(w.find(".catalog-missing-tip").exists()).toBe(true);
  });

  it("没有丢失条目时不提示重新检测", async () => {
    const w = mountPanel({ entries: [entry(1)] });
    await settled();
    expect(w.find(".catalog-missing-tip").exists()).toBe(false);
  });

  /* ---------------- 检测：调 sync 并显示摘要数字 ---------------- */

  it("点「检测资源库」调用 sync 并显示摘要数字", async () => {
    const w = mountPanel({
      entries: [entry(1)],
      sync: syncResult({
        scanned: 41,
        added: 3,
        updated: 2,
        unchanged: 35,
        missing: 1,
        restored: 4,
      }),
      diff: diffOf({ present: 41, lastSyncAt: 1_700_000_000 }),
    });
    await settled();

    // 检测前没有摘要
    expect(w.find(".catalog-summary").exists()).toBe(false);

    await w.find(".catalog-sync").trigger("click");
    await settled();

    expect(vi.mocked(syncResourceCatalog)).toHaveBeenCalledTimes(1);
    const text = w.find(".catalog-summary").text();
    for (const bit of ["41", "扫描", "3", "新增", "2", "更新", "35", "未变", "丢失", "恢复"]) {
      expect(text).toContain(bit);
    }
    // 检测完顺手刷新清单与对比：命令又被调用一次
    expect(vi.mocked(getResourceCatalog)).toHaveBeenCalledTimes(2);
    expect(vi.mocked(diffResourceCatalog)).toHaveBeenCalledTimes(2);
  });

  it("检测进行中按钮禁用并显示「检测中…」", async () => {
    // 手动控制这次检测什么时候返回，好在「进行中」那一刻做断言。
    let releaseSync: () => void = () => {};
    const pending = new Promise<CatalogSyncResult>((resolve) => {
      releaseSync = () => resolve(syncResult({ scanned: 7 }));
    });
    const w = mountPanel({ entries: [] });
    vi.mocked(syncResourceCatalog).mockImplementation(() => pending);
    await settled();

    await w.find(".catalog-sync").trigger("click");
    await nextTick();

    const btn = w.find(".catalog-sync");
    expect(btn.attributes("disabled")).toBeDefined();
    expect(btn.text()).toContain("检测中…");

    releaseSync();
    await settled();
    expect(w.find(".catalog-sync").text()).toContain("检测资源库");
    expect(w.find(".catalog-sync").attributes("disabled")).toBeUndefined();
    expect(w.find(".catalog-summary").text()).toContain("7");
  });

  /* ---------------- 变更对比：新增 / 变化 / 丢失 三组 ---------------- */

  it("分组展示新增、变化、丢失，并显示最近同步时间", async () => {
    const w = mountPanel({
      entries: [entry(1)],
      diff: diffOf({
        added: [entry(11, { fileName: "新片.mkv" })],
        changed: [entry(12, { fileName: "改过.mkv" })],
        missing: [entry(13, { fileName: "没了.mkv", status: "missing" })],
        present: 9,
        lastSyncAt: 1_700_000_000,
      }),
    });
    await settled();

    const groups = w.findAll(".diff-group");
    expect(groups).toHaveLength(3);
    expect(groups[0].text()).toContain("新增");
    expect(groups[0].text()).toContain("新片.mkv");
    expect(groups[1].text()).toContain("变化");
    expect(groups[1].text()).toContain("改过.mkv");
    expect(groups[2].text()).toContain("丢失");
    expect(groups[2].text()).toContain("没了.mkv");
    // 每组带条数
    expect(groups[0].text()).toContain("（1）");
    // 上次检测时间用本地时间格式化，不是裸时间戳
    expect(w.text()).toContain(new Date(1_700_000_000 * 1000).toLocaleString());
    expect(w.text()).not.toContain("1700000000");
  });

  it("空组显示「无」，不显示空列表", async () => {
    const w = mountPanel({ diff: diffOf({ lastSyncAt: 1_700_000_000 }) });
    await settled();
    const groups = w.findAll(".diff-group");
    expect(groups).toHaveLength(3);
    expect(groups[0].find(".diff-list").exists()).toBe(false);
    expect(groups[0].text()).toContain("无");
  });

  it("从未检测过时如实说明，不假装有时间", async () => {
    const w = mountPanel({ diff: diffOf({ lastSyncAt: null }) });
    await settled();
    expect(w.text()).toContain("从未检测过");
  });

  /* ---------------- 筛选条件要真的带进请求 ---------------- */

  it("切换状态筛选会把 status 带进请求；「全部」转成 null", async () => {
    const w = mountPanel({ entries: [entry(1)] });
    await settled();
    // 首次请求：默认全部
    expect(vi.mocked(getResourceCatalog).mock.calls[0][0]).toEqual({
      status: null,
      keyword: "",
    });

    await w.find(".catalog-status").setValue("missing");
    await settled();
    expect(vi.mocked(getResourceCatalog).mock.calls[1][0]).toEqual({
      status: "missing",
      keyword: "",
    });

    await w.find(".catalog-status").setValue("present");
    await settled();
    expect(vi.mocked(getResourceCatalog).mock.calls[2][0]).toEqual({
      status: "present",
      keyword: "",
    });

    await w.find(".catalog-status").setValue("all");
    await settled();
    expect(vi.mocked(getResourceCatalog).mock.calls[3][0]).toEqual({
      status: null,
      keyword: "",
    });
  });

  it("输入关键字会带进请求（按文件名模糊筛选）", async () => {
    const w = mountPanel({ entries: [entry(1)] });
    await settled();

    await w.find(".catalog-keyword").setValue("沙丘");
    await settled();

    expect(vi.mocked(getResourceCatalog).mock.calls[1][0]).toEqual({
      status: null,
      keyword: "沙丘",
    });
  });

  it("状态与关键字可以叠加", async () => {
    const w = mountPanel({ entries: [entry(1)] });
    await settled();

    await w.find(".catalog-status").setValue("missing");
    await settled();
    await w.find(".catalog-keyword").setValue("剧集");
    await settled();

    const last = vi.mocked(getResourceCatalog).mock.calls[2][0];
    expect(last).toEqual({ status: "missing", keyword: "剧集" });
  });

  /* ---------------- 空态与错误 ---------------- */

  it("空列表显示空态文案而不是一片空白", async () => {
    const w = mountPanel({ entries: [] });
    await settled();

    expect(w.find("table").exists()).toBe(false);
    expect(w.find(".catalog-empty").exists()).toBe(true);
    expect(w.text()).toContain("没有符合条件的记录");
    // 空态要给出下一步，而不是只报「空」
    expect(w.text()).toContain("检测资源库");
  });

  it("读取清单出错时显示错误文案，不留静默空白", async () => {
    const w = mountPanel({ fail: "数据库被锁" });
    await settled();

    const err = w.find(".catalog-error");
    expect(err.exists()).toBe(true);
    expect(err.text()).toContain("读取资源目录库失败");
    expect(err.text()).toContain("数据库被锁");
    expect(w.find(".catalog-empty").exists()).toBe(false);
    expect(w.find(".catalog-loading").exists()).toBe(false);
  });

  it("检测失败时显示错误文案，按钮恢复可用", async () => {
    const w = mountPanel({ entries: [entry(1)] });
    await settled();

    vi.mocked(syncResourceCatalog).mockRejectedValueOnce(new Error("没有配置媒体目录"));
    await w.find(".catalog-sync").trigger("click");
    await settled();

    expect(w.find(".catalog-error").text()).toContain("检测资源库失败");
    expect(w.find(".catalog-error").text()).toContain("没有配置媒体目录");
    expect(w.find(".catalog-sync").attributes("disabled")).toBeUndefined();
  });

  it("对比读不出来时只让对比区空着，清单照常显示", async () => {
    const w = mountPanel({ entries: [entry(1)] });
    await settled();
    vi.mocked(diffResourceCatalog).mockRejectedValueOnce(new Error("no snapshot"));
    await w.find(".catalog-sync").trigger("click");
    await settled();

    expect(w.findAll("tbody tr")).toHaveLength(1);
    expect(w.text()).toContain("影片1.mkv");
    expect(w.text()).toContain("暂时读不到变更对比");
  });
});
