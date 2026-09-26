<script setup lang="ts">
/**
 * 左侧导航。
 *
 * 分三段：
 * 1. **内置入口**（全部 / 电影 / 剧集）——按媒体类型过滤，不是分类；
 * 2. **分类栏目**——全部来自 `categories` 表，**前端没有任何硬编码枚举**：
 *    手动建的与自动识别（TMDB 类型）补出来的都在这里，可就地新建 / 改名 / 删除；
 * 3. **标签**——用于筛选，不占栏目位。
 */
import { computed, ref } from "vue";
import { useCategoryStore } from "../stores/categoryStore";
import {
  categoryView, tagView, VIEW_ALL, VIEW_COLLECTIONS, VIEW_MOVIE, VIEW_SETTINGS,
  VIEW_TV, VIEW_UNKNOWN, VIEW_UNCLASSIFIED, type ViewKey,
} from "../types";
import { MEDIA_TYPE_ICON } from "../constants/mediaType";

const props = defineProps<{ modelValue: ViewKey }>();
const emit = defineEmits<{ (e: "update:modelValue", v: ViewKey): void }>();

const cat = useCategoryStore();

const entries: { key: ViewKey; label: string; icon: string }[] = [
  { key: VIEW_ALL, label: "全部", icon: "📁" },
  { key: VIEW_MOVIE, label: "电影", icon: MEDIA_TYPE_ICON.movie },
  { key: VIEW_TV, label: "剧集", icon: MEDIA_TYPE_ICON.tv },
  // 判定不出类型的条目单独放一栏：不再让它们无声无息地混进「电影」。
  { key: VIEW_UNKNOWN, label: "待确认", icon: MEDIA_TYPE_ICON.unknown },
  // 同一系列 / IP 的电影归到一起（派生视图，按上映时间排序）。
  { key: VIEW_COLLECTIONS, label: "系列合集", icon: "🎞" },
];

/* --------------------------- 分类的就地管理 --------------------------- */

const adding = ref(false);
const newName = ref("");
/** 正在改名的分类 id（null 表示没有）。 */
const editingId = ref<number | null>(null);
const editingName = ref("");
/** 展开标签区（标签多的时候不占满侧栏）。 */
const tagsOpen = ref(false);

const sortedCategories = computed(() => [...cat.categories]);

function go(v: ViewKey): void {
  emit("update:modelValue", v);
}

async function submitNew(): Promise<void> {
  const n = newName.value.trim();
  if (!n) {
    adding.value = false;
    return;
  }
  const created = await cat.addCategory(n);
  newName.value = "";
  adding.value = false;
  if (created) go(categoryView(created.id));
}

function startEdit(id: number, name: string): void {
  editingId.value = id;
  editingName.value = name;
}

async function submitEdit(): Promise<void> {
  const id = editingId.value;
  if (id === null) return;
  const ok = await cat.editCategory(id, editingName.value);
  editingId.value = null;
  if (!ok) return;
  editingName.value = "";
}

async function remove(id: number, name: string): Promise<void> {
  if (!window.confirm(`删除分类「${name}」？\n影视本身会保留，只是解除归类。`)) return;
  const ok = await cat.removeCategory(id);
  // 正在看这个分类时退回「全部」，否则会停在一个已不存在的栏目上
  if (ok && props.modelValue === categoryView(id)) go(VIEW_ALL);
}
</script>

<template>
  <aside class="sidebar">
    <div class="brand">影视库</div>

    <div class="scroll">
      <nav class="nav">
        <button
          v-for="e in entries"
          :key="e.key"
          class="item"
          :class="{ active: e.key === modelValue }"
          @click="go(e.key)"
        ><span class="ico">{{ e.icon }}</span>{{ e.label }}</button>
      </nav>

      <div class="group">
        <div class="group-head">
          <span>分类</span>
          <button class="plus" title="新建分类" @click="adding = !adding">＋</button>
        </div>

        <div v-if="adding" class="row">
          <input
            v-model="newName"
            class="mini-input"
            placeholder="分类名"
            @keyup.enter="submitNew"
            @keyup.esc="adding = false"
          />
          <button class="mini" @click="submitNew">建</button>
        </div>

        <template v-if="editingId === null">
          <button
            v-for="c in sortedCategories"
            :key="c.id"
            class="item"
            :class="{ active: categoryView(c.id) === modelValue }"
            :title="c.kind === 'manual' ? '手动创建' : '自动识别（' + c.origin + '）'"
            @click="go(categoryView(c.id))"
          >
            <span class="ico">{{ c.kind === "manual" ? "🗂" : "✨" }}</span>
            <span class="name">{{ c.name }}</span>
            <span class="count">{{ c.seriesCount }}</span>
            <span class="ops">
              <button class="op" title="重命名" @click.stop="startEdit(c.id, c.name)">✎</button>
              <button class="op danger" title="删除" @click.stop="remove(c.id, c.name)">✕</button>
            </span>
          </button>
          <p v-if="!sortedCategories.length" class="empty">还没有分类，点 ＋ 建一个</p>
        </template>

        <div v-else class="row">
          <input
            v-model="editingName"
            class="mini-input"
            @keyup.enter="submitEdit"
            @keyup.esc="editingId = null"
          />
          <button class="mini" @click="submitEdit">改</button>
        </div>

        <button
          class="item subtle"
          :class="{ active: modelValue === 'unclassified' }"
          @click="go(VIEW_UNCLASSIFIED)"
        ><span class="ico">❔</span>未分类</button>
      </div>

      <div class="group">
        <div class="group-head">
          <span>标签</span>
          <button class="plus" @click="tagsOpen = !tagsOpen">{{ tagsOpen ? "−" : "＋" }}</button>
        </div>
        <template v-if="tagsOpen">
          <button
            v-for="t in cat.tags"
            :key="t.id"
            class="item"
            :class="{ active: tagView(t.id) === modelValue }"
            @click="go(tagView(t.id))"
          >
            <span class="ico">🏷</span>
            <span class="name">{{ t.name }}</span>
            <span class="count">{{ t.usageCount }}</span>
          </button>
          <p v-if="!cat.tags.length" class="empty">还没有标签（可在详情页添加）</p>
        </template>
      </div>
    </div>

    <button
      class="item settings"
      :class="{ active: modelValue === VIEW_SETTINGS }"
      @click="go(VIEW_SETTINGS)"
    ><span class="ico">⚙️</span>设置</button>
  </aside>
</template>

<style scoped>
.sidebar {
  width: 200px; flex: none; display: flex; flex-direction: column;
  padding: calc(var(--space) * 3) calc(var(--space) * 2) calc(var(--space) * 3);
  background: var(--bg-soft); border-right: 1px solid var(--border);
}
.brand {
  padding: calc(var(--space) * 2) calc(var(--space) * 3);
  margin-bottom: calc(var(--space) * 2);
  font-size: var(--fs-15); font-weight: 650; letter-spacing: 0.5px;
  color: var(--text);
}
/* 分类可能很多，让中间区域自己滚，设置按钮始终留在底部 */
.scroll { flex: 1; overflow-y: auto; display: flex; flex-direction: column; gap: calc(var(--space) * 2); }
.nav { display: flex; flex-direction: column; gap: 2px; }
.group { display: flex; flex-direction: column; gap: 2px; margin-top: calc(var(--space) * 2); }
.group-head {
  display: flex; align-items: center; justify-content: space-between;
  padding: 6px 12px 4px; font-size: var(--fs-12); color: var(--muted); letter-spacing: .5px;
}
.plus {
  border: 0; background: none; color: var(--muted); cursor: pointer;
  font-size: var(--fs-14); line-height: 1; padding: 0 2px;
}
.plus:hover { color: var(--text); }

.item {
  position: relative; display: flex; align-items: center; gap: 8px; width: 100%;
  padding: 8px 12px; border: 0; border-radius: var(--radius-s);
  background: none; color: var(--muted); text-align: left; font-size: var(--fs-13);
  transition: background 0.15s ease, color 0.15s ease;
}
.item:hover { background: rgba(255, 255, 255, 0.05); color: var(--text); }
.item.active { color: var(--text); background: var(--surface-2); font-weight: 600; }
.item.active::before {
  content: ""; position: absolute; left: 0; top: 20%; bottom: 20%; width: 3px;
  border-radius: 3px; background: var(--accent);
}
.item.active .ico { filter: none; }
.item.subtle { color: var(--muted); font-size: var(--fs-12); }
.ico { font-size: 14px; filter: grayscale(0.4); flex: none; }
.name { flex: 1; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.count { font-size: var(--fs-11); opacity: .6; flex: none; }

/* 重命名 / 删除只在 hover 时出现，平时不占地方 */
.ops { display: none; gap: 2px; flex: none; }
.item:hover .ops { display: flex; }
.op {
  border: 0; background: none; color: var(--muted); cursor: pointer;
  font-size: var(--fs-12); padding: 1px 3px; border-radius: 3px;
}
.op:hover { color: var(--text); background: rgba(255,255,255,.08); }
.op.danger:hover { color: var(--danger); }

.row { display: flex; gap: 6px; padding: 2px 12px 6px; }
.mini-input {
  flex: 1; min-width: 0; padding: 5px 8px; border-radius: var(--radius-s);
  border: 1px solid var(--border); background: var(--surface); color: var(--text);
  font-size: var(--fs-12); outline: none;
}
.mini-input:focus { border-color: var(--accent); }
.mini {
  flex: none; padding: 5px 9px; border-radius: var(--radius-s); cursor: pointer;
  border: 1px solid var(--border); background: var(--surface-2); color: var(--text);
  font-size: var(--fs-12);
}
.mini:hover { background: var(--border-strong); }

.empty { margin: 2px 0 0; padding: 0 12px; font-size: var(--fs-11); color: var(--muted); }

.item.settings { margin-top: calc(var(--space) * 2); }
</style>
