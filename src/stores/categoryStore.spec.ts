import { beforeEach, describe, expect, it, vi } from "vitest";
import { createPinia, setActivePinia } from "pinia";
import { useCategoryStore } from "./categoryStore";
import type { Category, SyncResult, Tag } from "../api/commands";

/**
 * 分类 / 标签是「用户能直接改、重启后必须还在」的数据，
 * 所以这里锁住的是：CRUD 都真的走了后端、改完会重新拉列表、空名不落库。
 * 后端命令全部 mock，不依赖真实 Tauri 运行时。
 */
const mocks = vi.hoisted(() => ({
  listCategories: vi.fn(),
  listTags: vi.fn(),
  createCategory: vi.fn(),
  renameCategory: vi.fn(),
  deleteCategory: vi.fn(),
  createTag: vi.fn(),
  renameTag: vi.fn(),
  deleteTag: vi.fn(),
  syncCategoriesFromTmdb: vi.fn(),
}));

vi.mock("../api/commands", () => mocks);

function cat(id: number, name: string, kind = "manual"): Category {
  return { id, name, kind, origin: kind === "manual" ? "manual" : "tmdb", sortOrder: 0, seriesCount: 0 };
}

function tag(id: number, name: string): Tag {
  return { id, name, usageCount: 1 };
}

describe("categoryStore: 加载", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    mocks.listCategories.mockResolvedValue([
      cat(1, "我的栏目"),
      cat(2, "剧情", "auto"),
    ]);
    mocks.listTags.mockResolvedValue([tag(1, "看过")]);
  });

  it("同时拉分类与标签", async () => {
    const s = useCategoryStore();
    await s.load();

    expect(s.categories).toHaveLength(2);
    expect(s.tags).toHaveLength(1);
    expect(s.loading).toBe(false);
  });

  it("手动建的分类与自动识别的分开算", async () => {
    const s = useCategoryStore();
    await s.load();

    expect(s.manualCategories.map((c) => c.name)).toEqual(["我的栏目"]);
    expect(s.autoCategories.map((c) => c.name)).toEqual(["剧情"]);
  });
});

describe("categoryStore: 分类 CRUD", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    mocks.listCategories.mockResolvedValue([cat(1, "剧情", "auto")]);
    mocks.listTags.mockResolvedValue([]);
    mocks.createCategory.mockResolvedValue(cat(9, "科幻"));
    mocks.renameCategory.mockResolvedValue(undefined);
    mocks.deleteCategory.mockResolvedValue(undefined);
  });

  it("新建分类会落库并刷新列表", async () => {
    const s = useCategoryStore();
    const created = await s.addCategory("科幻");

    expect(mocks.createCategory).toHaveBeenCalledWith("科幻");
    expect(created?.id).toBe(9);
    expect(s.categories).toHaveLength(1); // 用刷新后的远端结果
    expect(s.notice).toContain("科幻");
  });

  it("空名字不发请求（界面侧先挡一道）", async () => {
    const s = useCategoryStore();
    expect(await s.addCategory("   ")).toBeNull();
    expect(mocks.createCategory).not.toHaveBeenCalled();
    expect(s.notice).toBe("分类名不能为空");
  });

  it("重命名失败会记录下来，不静默", async () => {
    mocks.renameCategory.mockRejectedValue(new Error("与其他分类重名"));
    const s = useCategoryStore();
    const ok = await s.editCategory(1, "剧情");

    expect(ok).toBe(false);
    expect(s.error).toContain("重名");
  });

  it("删除分类只解关联，影视本身不动（提示里写清楚）", async () => {
    const s = useCategoryStore();
    const ok = await s.removeCategory(1);

    expect(ok).toBe(true);
    expect(mocks.deleteCategory).toHaveBeenCalledWith(1);
    expect(s.notice).toContain("影视本身保留");
  });
});

describe("categoryStore: 从 TMDB 同步（历史数据迁移）", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    mocks.listCategories.mockResolvedValue([cat(1, "剧情", "auto")]);
    mocks.listTags.mockResolvedValue([]);
  });

  it("同步结果给用户一句可读反馈", async () => {
    const r: SyncResult = { created: 3, linked: 12, scanned: 20 };
    mocks.syncCategoriesFromTmdb.mockResolvedValue(r);

    const s = useCategoryStore();
    expect(await s.syncFromTmdb()).toEqual(r);
    expect(s.notice).toContain("20");
    expect(s.notice).toContain("3");
  });

  it("同步失败不吞错", async () => {
    mocks.syncCategoriesFromTmdb.mockRejectedValue(new Error("boom"));
    const s = useCategoryStore();

    expect(await s.syncFromTmdb()).toBeNull();
    expect(s.error).toContain("boom");
  });
});
