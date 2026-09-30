<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import Sidebar from "./components/Sidebar.vue";
import PosterGrid from "./components/PosterGrid.vue";
import MediaDetail from "./components/MediaDetail.vue";
import SettingsPanel from "./components/SettingsPanel.vue";
import MediaEditDialog from "./components/MediaEditDialog.vue";
import ImportFolderDialog from "./components/ImportFolderDialog.vue";
import CollectionPanel from "./components/CollectionPanel.vue";
import { useSettingsStore } from "./stores/settingsStore";
import { useMediaStore } from "./stores/mediaStore";
import { useCategoryStore } from "./stores/categoryStore";
import {
  parseView, tagView, VIEW_ALL, VIEW_COLLECTIONS, VIEW_SETTINGS, type ViewKey,
} from "./types";
import type { MediaItem } from "./api/commands";

const settings = useSettingsStore();
const media = useMediaStore();
const category = useCategoryStore();
const activeView = ref<ViewKey>(VIEW_ALL);

/**
 * 资源编辑对话框的目标：
 * `undefined` = 关闭；`null` = 新建；`MediaItem` = 编辑该条目。
 */
const editTarget = ref<MediaItem | null | undefined>(undefined);
const showImport = ref(false);

/** 当前可见条目是否已全部勾选（「全选 / 取消全选」按钮的文案切换）。 */
const allPicked = computed(() => {
  const visible = media.visibleItems();
  return visible.length > 0 && visible.every((i) => media.isSelected(i.id));
});

/** 进入 / 退出多选模式。退出时必须清空选中，避免残留 id 被误删。 */
function exitSelecting(): void {
  media.clearSelection();
  media.selecting = false;
}

/**
 * 滑动多选松手时**一次性**提交的结果。
 *
 * 滑动可以在非多选模式下发起（那是它最方便的用法），但选完若还停在普通模式，
 * 勾选态就看不见 —— 所以有选中结果时顺手进入多选模式。
 * 一条都没选到（比如只是长按后原地松手）就保持原样，不打扰。
 */
function onSelectMany(ids: number[]): void {
  media.setSelected(ids);
  if (ids.length) media.selecting = true;
}

/**
 * 重新识别存量数据的类型。
 *
 * 这是**修正历史误判**的入口：老版本只有一条判定规则，把大量剧集判成了电影。
 * 多选模式下只重判所选，否则跑全库；结果由后端一句话回传（改了几条、跳过几条）。
 */
async function onReclassify(): Promise<void> {
  const ids = media.selecting ? [...media.selected] : null;
  if (ids && !ids.length) {
    media.note("请先勾选要重新识别的条目");
    return;
  }
  if (
    ids &&
    !window.confirm(
      `确定重新识别选中的 ${ids.length} 条？\n\n类型判定变化时会移动记录：` +
        `电影 → 剧集会挂到同名剧集下，已锁定的条目不受影响。`,
    )
  ) {
    return;
  }
  await media.reclassify(ids);
}

/**
 * 批量删除的二次确认。
 *
 * 删的是**库里的记录**，本地文件不动——这句必须写清楚，否则用户会以为文件被删了。
 */
async function confirmDeleteSelected(): Promise<void> {
  const n = media.selected.length;
  if (!n) return;
  const ok = window.confirm(
    `确定删除选中的 ${n} 条资源？\n\n删除后不可恢复；本地视频文件不会被删除。`,
  );
  if (!ok) return;
  await media.removeSelected();
}

/** 内置视图的标题；分类 / 标签的标题要从数据里取（前端不硬编码栏目名）。 */
const TITLES: Record<string, string> = {
  all: "全部媒体",
  movie: "电影",
  tv: "剧集",
  unknown: "待确认",
  collections: "系列合集",
  settings: "设置",
  unclassified: "未分类",
};

const inSettings = computed(() => activeView.value === VIEW_SETTINGS);
/** 「系列合集」是另一份派生数据，走单独的面板而不是海报墙。 */
const inCollections = computed(() => activeView.value === VIEW_COLLECTIONS);

const heading = computed(() => {
  if (media.selectedId && !inSettings.value) return "媒体详情";
  const p = parseView(activeView.value);
  if (p.kind === "category") {
    return category.categories.find((c) => c.id === p.id)?.name ?? "分类";
  }
  if (p.kind === "tag") {
    return `标签：${category.tags.find((t) => t.id === p.id)?.name ?? ""}`;
  }
  return TITLES[activeView.value] ?? "全部媒体";
});

/** 刮削进度百分比（后台任务的实时反馈）。 */
const scrapePercent = computed(() => {
  const p = media.scrapeProgress;
  if (!p || p.total <= 0) return 0;
  return Math.min(100, Math.round((p.done / p.total) * 100));
});

/** 扫描入库阶段的百分比（遍历阶段没有总数，界面改用不确定态动画条）。 */
const scanPercent = computed(() => {
  const p = media.scanProgress;
  if (!p || p.total <= 0) return 0;
  return Math.min(100, Math.round((p.imported / p.total) * 100));
});

async function onNav(v: ViewKey): Promise<void> {
  activeView.value = v;
  if (v === VIEW_SETTINGS) {
    media.closeDetail();
    await media.loadHistory();
    return;
  }
  // 分类 / 标签视图由 store 解析成查询条件（id 来自数据库）
  await media.setView(v);
}

/** 顶栏的标签下拉：选中即切到该标签视图。 */
async function onTagPick(value: string): Promise<void> {
  if (!value) {
    activeView.value = VIEW_ALL;
    await media.setView(VIEW_ALL);
    return;
  }
  const v = tagView(Number(value));
  activeView.value = v;
  await media.setView(v);
}

const tagFilter = computed(() => {
  const p = parseView(activeView.value);
  return p.kind === "tag" ? String(p.id) : "";
});

onMounted(async () => {
  await settings.load();
  await category.load();
  await media.load();
  await media.loadHistory();
});
</script>

<template>
  <div class="layout">
    <Sidebar :model-value="activeView" @update:model-value="onNav" />

    <main class="content">
      <header class="topbar">
        <h1>{{ heading }}</h1>
        <input
          v-if="!inSettings"
          v-model="media.keyword"
          class="search"
          placeholder="搜索标题 / 类型 / 年份"
        />
        <select
          v-if="!inSettings"
          class="tagselect"
          :value="tagFilter"
          @change="onTagPick(($event.target as HTMLSelectElement).value)"
        >
          <option value="">全部标签</option>
          <option v-for="t in category.tags" :key="t.id" :value="t.id">
            🏷 {{ t.name }}（{{ t.usageCount }}）
          </option>
        </select>
        <button
          v-if="!inSettings"
          class="btn btn-ghost"
          title="手动新建一条电影或剧集（与扫描结果同库管理）"
          @click="editTarget = null"
        >
          ＋ 新建资源
        </button>
        <button
          v-if="!inSettings"
          class="btn btn-ghost"
          title="从本地文件夹导入视频（重复文件自动跳过）"
          @click="showImport = true"
        >
          📂 导入文件夹
        </button>
        <button
          v-if="!inSettings && !media.selecting"
          class="btn btn-ghost"
          title="勾选多条资源，批量删除"
          @click="media.selecting = true"
        >
          ☑ 选择
        </button>
        <button
          v-if="!inSettings"
          class="btn btn-ghost"
          :disabled="media.reclassifying"
          title="按新规则重新判定类型，修正历史误判（多选时只判所选）"
          @click="onReclassify"
        >
          {{ media.reclassifying ? "重新识别中…" : "🔍 重新识别" }}
        </button>
        <button class="btn btn-primary" :disabled="media.scanning" @click="media.scan()">
          {{ media.scanning ? "扫描中…" : "扫描媒体库" }}
        </button>
        <button class="btn btn-ghost" :disabled="media.scraping" @click="media.scrapeMissing()">
          {{ media.scraping ? "刮削中…" : "批量刮削" }}
        </button>
        <button
          v-if="media.scanning"
          class="btn btn-danger"
          title="停止扫描（遍历目录阶段生效）"
          @click="media.cancelScan()"
        >
          停止扫描
        </button>
        <button
          v-if="media.scraping"
          class="btn btn-danger"
          title="停止刮削（当前这一条处理完即停）"
          @click="media.cancelScrape()"
        >
          停止刮削
        </button>
      </header>

      <!-- 后台扫描的进度反馈：遍历/入库都在后台线程跑，这里只负责显示 -->
      <div v-if="media.scanning" class="progress">
        <div class="bar">
          <!-- 入库阶段有明确总数，用实进度；遍历阶段总数未知，走来回滚动的动画条 -->
          <div
            v-if="media.scanProgress?.total"
            class="fill"
            :style="{ width: scanPercent + '%' }"
          ></div>
          <div v-else class="fill indeterminate"></div>
        </div>
        <span class="pct">
          {{ media.scanProgress?.phase ?? "准备中" }}
          <template v-if="media.scanProgress?.found">
            · 已发现 {{ media.scanProgress.found }}
          </template>
          <template v-if="media.scanProgress?.imported">
            · 已入库 {{ media.scanProgress.imported }}/{{ media.scanProgress.total }}
          </template>
        </span>
      </div>

      <!-- 后台刮削的进度反馈：抓取在后台线程跑，这里只负责显示 -->
      <div v-if="media.scraping" class="progress">
        <div class="bar">
          <div class="fill" :style="{ width: scrapePercent + '%' }"></div>
        </div>
        <span class="pct">
          {{ media.scrapeProgress ? media.scrapeProgress.done : 0 }} /
          {{ media.scrapeProgress ? media.scrapeProgress.total : 0 }}
          <template v-if="media.scrapeProgress?.current">
            · {{ media.scrapeProgress.current }}
          </template>
        </span>
      </div>

      <!-- 多选工具条：只在「选择」模式下出现，避免平时占地方 -->
      <div v-if="media.selecting && !inSettings" class="selbar">
        <span class="sel-count">
          已选 {{ media.selected.length }} / {{ media.visibleItems().length }}
        </span>
        <button class="btn btn-ghost" @click="media.toggleSelectAll()">
          {{ allPicked ? "取消全选" : "全选" }}
        </button>
        <button
          class="btn btn-danger"
          :disabled="!media.selected.length"
          @click="confirmDeleteSelected"
        >
          删除所选
        </button>
        <button class="btn btn-ghost" @click="exitSelecting">退出选择</button>
      </div>

      <p v-if="media.toast" class="toast">{{ media.toast }}</p>
      <p v-if="media.error" class="err">{{ media.error }}</p>

      <MediaDetail
        v-if="media.selectedId && !inSettings"
        @edit="editTarget = $event"
      />
      <SettingsPanel v-else-if="inSettings" />
      <CollectionPanel
        v-else-if="inCollections"
        :collections="media.collections"
        :loading="media.loadingCollections"
        @open="media.openDetail"
      />
      <PosterGrid
        v-else
        :items="media.visibleItems()"
        :selectable="media.selecting"
        :selected="media.selected"
        @open="media.openDetail"
        @select="media.toggleSelect"
        @select-many="onSelectMany"
      />
    </main>

    <MediaEditDialog
      v-if="editTarget !== undefined"
      :initial="editTarget"
      @close="editTarget = undefined"
    />
    <ImportFolderDialog v-if="showImport" @close="showImport = false" />
  </div>
</template>

<style scoped>
.layout { display: flex; height: 100vh; }
.content { flex: 1; overflow-y: auto; padding: calc(var(--space) * 5) calc(var(--space) * 7) calc(var(--space) * 10); }

.topbar {
  display: flex; align-items: center; gap: 10px; margin-bottom: calc(var(--space) * 5);
  position: sticky; top: 0; z-index: 5; padding: calc(var(--space) * 3) 0;
  background: var(--bg);
}
h1 { flex: 1; margin: 0; font-size: var(--fs-18); font-weight: 650; }
.search {
  width: 240px; padding: 9px 12px; border-radius: var(--radius-s); font-size: var(--fs-13);
  border: 1px solid var(--border); background: var(--surface); color: var(--text);
  outline: none; transition: border-color 0.15s ease;
}
.search:focus { border-color: var(--accent); }
.search::placeholder { color: var(--muted); }
.tagselect {
  flex: none; max-width: 170px; padding: 9px 10px; border-radius: var(--radius-s);
  font-size: var(--fs-13); border: 1px solid var(--border);
  background: var(--surface); color: var(--text); outline: none;
}
.tagselect:focus { border-color: var(--accent); }

.progress {
  display: flex; align-items: center; gap: 10px; margin: 0 0 calc(var(--space) * 4);
}
.progress .bar {
  flex: 1; height: 6px; border-radius: 999px; overflow: hidden;
  background: var(--surface-2); border: 1px solid var(--border);
}
.progress .fill {
  height: 100%; background: var(--accent);
  transition: width .25s ease;
}
/* 总数未知时的不确定态：一条短块来回扫，表示「在动，但不知道还剩多少」 */
.progress .fill.indeterminate {
  width: 35%;
  animation: sweep 1.1s ease-in-out infinite;
}
@keyframes sweep {
  0% { margin-left: 0; }
  50% { margin-left: 65%; }
  100% { margin-left: 0; }
}
.progress .pct {
  flex: none; font-size: var(--fs-12); color: var(--muted);
  max-width: 46%; overflow: hidden; text-overflow: ellipsis; white-space: nowrap;
}

/* 多选工具条：和顶部工具条同高、同一套按钮样式，视觉上是一家人 */
.selbar {
  display: flex; align-items: center; gap: 10px; margin: 0 0 calc(var(--space) * 4);
  padding: calc(var(--space) * 2) calc(var(--space) * 3); border-radius: var(--radius-s);
  background: var(--surface); border: 1px solid var(--border);
}
.sel-count { flex: 1; font-size: var(--fs-13); color: var(--text-2); }

.toast {
  margin: 0 0 calc(var(--space) * 4); padding: 10px 14px; border-radius: var(--radius-s);
  font-size: var(--fs-13); background: var(--surface); border: 1px solid var(--border); color: var(--text-2);
}
.err {
  margin: 0 0 calc(var(--space) * 4); padding: 10px 14px; border-radius: var(--radius-s);
  font-size: var(--fs-13); background: var(--accent-soft); border: 1px solid rgba(229,9,20,.35);
  color: #ff8a90; word-break: break-all;
}
</style>
