import { defineStore } from "pinia";
import { computed, ref } from "vue";
import {
  createCategory, createTag, deleteCategory, deleteTag, listCategories, listTags,
  renameCategory, renameTag, syncCategoriesFromTmdb,
  type Category, type SyncResult, type Tag,
} from "../api/commands";

/**
 * 分类 / 标签 store。
 *
 * 设计要点：
 * - **分类是数据，不是枚举**：界面上的栏目全部来自 `categories` 表，
 *   手动建的（`kind = manual`）与自动识别补出来的（`kind = auto`）混在一起展示；
 * - 改名 / 删除都直接落库，重启后不丢；
 * - 删除分类只解开关联，不会连带删掉影视（后端外键级联只清关联行）。
 */
export const useCategoryStore = defineStore("category", () => {
  const categories = ref<Category[]>([]);
  const tags = ref<Tag[]>([]);

  const loading = ref(false);
  const saving = ref(false);
  const error = ref<string | null>(null);
  const notice = ref("");

  /** 手动建的分类（左侧导航里排在前面）。 */
  const manualCategories = computed(() => categories.value.filter((c) => c.kind === "manual"));
  /** 自动识别补出来的分类（目前来自 TMDB 的类型字段）。 */
  const autoCategories = computed(() => categories.value.filter((c) => c.kind !== "manual"));

  function byId(id: number): Category | undefined {
    return categories.value.find((c) => c.id === id);
  }

  async function load(): Promise<void> {
    loading.value = true;
    error.value = null;
    try {
      const [c, t] = await Promise.all([listCategories(), listTags()]);
      categories.value = c;
      tags.value = t;
    } catch (e) {
      error.value = String(e);
      categories.value = [];
      tags.value = [];
    } finally {
      loading.value = false;
    }
  }

  async function loadCategories(): Promise<void> {
    try {
      categories.value = await listCategories();
    } catch (e) {
      error.value = String(e);
    }
  }

  async function loadTags(): Promise<void> {
    try {
      tags.value = await listTags();
    } catch (e) {
      error.value = String(e);
    }
  }

  async function addCategory(name: string): Promise<Category | null> {
    const n = name.trim();
    if (!n) {
      notice.value = "分类名不能为空";
      return null;
    }
    saving.value = true;
    error.value = null;
    try {
      const c = await createCategory(n);
      await loadCategories();
      notice.value = `已新建分类「${c.name}」`;
      return c;
    } catch (e) {
      error.value = String(e);
      notice.value = "新建分类失败：" + String(e);
      return null;
    } finally {
      saving.value = false;
    }
  }

  async function editCategory(id: number, name: string): Promise<boolean> {
    const n = name.trim();
    if (!n) {
      notice.value = "分类名不能为空";
      return false;
    }
    saving.value = true;
    error.value = null;
    try {
      await renameCategory(id, n);
      await loadCategories();
      notice.value = "分类已重命名";
      return true;
    } catch (e) {
      error.value = String(e);
      notice.value = "重命名失败：" + String(e);
      return false;
    } finally {
      saving.value = false;
    }
  }

  async function removeCategory(id: number): Promise<boolean> {
    saving.value = true;
    error.value = null;
    try {
      await deleteCategory(id);
      await loadCategories();
      notice.value = "分类已删除（影视本身保留）";
      return true;
    } catch (e) {
      error.value = String(e);
      notice.value = "删除失败：" + String(e);
      return false;
    } finally {
      saving.value = false;
    }
  }

  async function addTag(name: string): Promise<Tag | null> {
    const n = name.trim();
    if (!n) return null;
    try {
      const t = await createTag(n);
      tags.value = await listTags();
      return t;
    } catch (e) {
      error.value = String(e);
      return null;
    }
  }

  async function editTag(id: number, name: string): Promise<boolean> {
    const n = name.trim();
    if (!n) return false;
    try {
      await renameTag(id, n);
      tags.value = await listTags();
      return true;
    } catch (e) {
      error.value = String(e);
      return false;
    }
  }

  async function removeTag(id: number): Promise<boolean> {
    try {
      await deleteTag(id);
      tags.value = await listTags();
      notice.value = "标签已删除";
      return true;
    } catch (e) {
      error.value = String(e);
      return false;
    }
  }

  /**
   * 把库里已有条目的 TMDB 类型补成分类。
   *
   * 老数据（升级前刮过的）没有分类关联，靠它一次性补齐。
   */
  async function syncFromTmdb(): Promise<SyncResult | null> {
    saving.value = true;
    error.value = null;
    try {
      const r = await syncCategoriesFromTmdb();
      await load();
      notice.value = `已扫描 ${r.scanned} 条，新建 ${r.created} 个分类，新增 ${r.linked} 条关联`;
      return r;
    } catch (e) {
      error.value = String(e);
      notice.value = "同步失败：" + String(e);
      return null;
    } finally {
      saving.value = false;
    }
  }

  return {
    categories, tags, manualCategories, autoCategories,
    loading, saving, error, notice,
    byId, load, loadCategories, loadTags,
    addCategory, editCategory, removeCategory,
    addTag, editTag, removeTag,
    syncFromTmdb,
  };
});
