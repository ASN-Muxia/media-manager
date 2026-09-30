<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from "vue";
import {
  getMediaCategories, getMediaTags, imageUrl, searchTmdb, setMediaCategories,
  setMediaTags, tmdbTitle, tmdbYear, tvdbApply,
  type Category, type Episode, type MediaItem, type Tag, type TmdbSearchItem,
} from "../api/commands";
import { open } from "@tauri-apps/plugin-dialog";
import { useMediaStore } from "../stores/mediaStore";
import { useCategoryStore } from "../stores/categoryStore";
import { formatScore, typeIcon, typeLabel } from "../constants/mediaType";
import {
  applyDragTo, beginDragSelection, finishDragSelection, toggleOne,
  type DragSelection,
} from "../utils/dragSelect";

const store = useMediaStore();
const catStore = useCategoryStore();

/** 「编辑信息」上抛给 App，由它打开编辑对话框（新建 / 编辑共用一个弹窗）。 */
const emit = defineEmits<{ (e: "edit", item: MediaItem): void }>();

/* ---------------- 单集（聚合剧目下的资源）管理 ---------------- */
/* 只针对当前这一条剧目：删掉的单集不会影响其他剧目，剧目主记录本身也保留。 */

const epSelected = ref<number[]>([]);

function isEpSelected(id: number): boolean {
  return epSelected.value.includes(id);
}

function toggleEp(id: number): void {
  epSelected.value = toggleOne(epSelected.value, id);
}

function toggleEpAll(): void {
  const all = (detail.value?.episodes ?? []).map((e) => e.id);
  epSelected.value =
    epSelected.value.length >= all.length && all.every(isEpSelected) ? [] : all;
}

/* ---------------- 拖动划过连续多选 ---------------- */
/* 在某一行上按下并拖动：起点那一行决定本次是「选中」还是「取消选中」，
   划过哪几行就作用到哪几行（框选一片 / 抹掉一片）。
   状态机在 `utils/dragSelect.ts`，这里只负责接 DOM 事件。

   事件用**列表容器上的委托**而不是每个 `<li>` 各挂一份：行是 `v-for` 出来的，
   挂在容器上少一堆监听，也不会因为重渲染丢状态；`pointerup` 仍然挂在 document 上，
   因为松手常常发生在列表之外。 */

let dragState: DragSelection | null = null;
const dragging = ref(false);

/** 当前界面上可见的单集 id，按显示顺序：用于过滤掉已消失行的残留勾选。 */
function visibleEpIds(): number[] {
  return seasons.value.flatMap((s) => s.list.map((e) => e.id));
}

function syncFromDrag(): void {
  if (!dragState) return;
  epSelected.value = finishDragSelection(dragState, visibleEpIds());
}

/** 行内的可交互控件不该被拖动接管，否则复选框会被点两次。 */
const EP_BLOCKED_SELECTOR = "button, input, label, a, select, textarea";

/** 从 id 字符串反解出行号；拿不到（划过按钮、行间空隙）就返回 null。 */
function rowIdOf(el: Element | null | undefined): number | null {
  const raw = (el?.closest("li[data-ep-id]") as HTMLElement | null)?.dataset.epId;
  if (!raw) return null;
  const id = Number(raw);
  return Number.isFinite(id) ? id : null;
}

/** 几何命中测试，仅触摸路径用；jsdom 没有布局引擎，取不到就算了。 */
function elementAt(x: number, y: number): Element | null {
  if (typeof document.elementFromPoint !== "function") return null;
  try {
    return document.elementFromPoint(x, y);
  } catch {
    return null;
  }
}

/**
 * 取当前指针下的行。
 *
 * 鼠标走 `event.target`（事件委托，最快，也不依赖布局信息）；
 * 触摸必须走几何命中 —— 浏览器对触摸有**隐式指针捕获**，
 * 拖动期间 `event.target` 会一直停在起点那一行上，用它会把整段都算成第一行。
 */
function rowIdAt(ev: PointerEvent): number | null {
  const el: Element | null =
    ev.pointerType === "touch"
      ? elementAt(ev.clientX, ev.clientY)
      : (ev.target as Element | null);
  return rowIdOf(el);
}

function onEpPointerDown(ev: PointerEvent, id: number): void {
  if (ev.button !== 0) return;
  const target = ev.target as Element | null;
  if (target?.closest(EP_BLOCKED_SELECTOR)) return;
  ev.preventDefault(); // 不然一路划过去会把标题选蓝 / 触发原生拖拽
  dragState = beginDragSelection(epSelected.value, id);
  dragging.value = true;
  syncFromDrag();
}

function onEpPointerMove(ev: PointerEvent): void {
  if (!dragState) return;
  const id = rowIdAt(ev);
  if (id === null) return;
  // 幂等：同一行反复划过只写一次，只在真的变了时才刷新界面
  if (applyDragTo(dragState, id)) syncFromDrag();
}

function endDrag(): void {
  if (!dragState) return;
  syncFromDrag();
  dragState = null;
  dragging.value = false;
}

/** 丢弃进行中的拖动（不提交）：换条目 / 卸载时用。 */
function abortDrag(): void {
  dragState = null;
  dragging.value = false;
}

onMounted(() => {
  document.addEventListener("pointerup", endDrag);
  document.addEventListener("pointercancel", endDrag);
});

onBeforeUnmount(() => {
  document.removeEventListener("pointerup", endDrag);
  document.removeEventListener("pointercancel", endDrag);
  abortDrag();
});

/**
 * 删除单集（单条或批量都走这里）。
 *
 * 二次确认里必须说清楚：删的是库里的单集记录，本地文件不动；
 * 剧目主记录保留（删光了也还是一条「空剧目」，方便以后重新导入）。
 */
async function removeEpisodes(ids: number[]): Promise<void> {
  const d = detail.value;
  if (!d || !ids.length) return;
  const n = ids.length;
  const ok = window.confirm(
    n === 1
      ? "确定删除这条资源？\n\n删除后不可恢复；本地视频文件不会被删除。"
      : `确定删除选中的 ${n} 条资源？\n\n删除后不可恢复；本地视频文件不会被删除。`,
  );
  if (!ok) return;
  await store.removeEpisodes(d.media.id, [...ids]);
  epSelected.value = [];
}

/** 刮削过的用 TMDB 集名，未刮削的退回文件名（更可读）。 */
function epTitle(e: Episode): string {
  const scraped = Boolean(e.still_path || e.overview);
  return scraped
    ? e.title || e.file_name || "(未命名)"
    : e.file_name || e.title || "(未命名)";
}

const detail = computed(() => store.detail);
const poster = computed(() => imageUrl(detail.value?.media.poster_path, "w342"));
const backdrop = computed(() => imageUrl(detail.value?.media.backdrop_path, "w1280"));
const score = computed(() => formatScore(detail.value?.media.vote_average));
/** 类型标签 / 图标：`unknown` 是「判定不出来」，不能显示成电影。 */
const mediaTypeLabel = computed(() => typeLabel(detail.value?.media.type));
const mediaTypeIcon = computed(() => typeIcon(detail.value?.media.type));
/** 单文件条目（电影，以及还没定型的「待确认」）都能直接播放。 */
const isSingleFile = computed(() => detail.value?.media.type !== "tv");

/** 单集按季分组。 */
const seasons = computed(() => {
  const eps = detail.value?.episodes ?? [];
  const map = new Map<number, typeof eps>();
  for (const e of eps) {
    const arr = map.get(e.season) ?? [];
    arr.push(e);
    map.set(e.season, arr);
  }
  return [...map.entries()]
    .sort((a, b) => a[0] - b[0])
    .map(([season, list]) => ({ season, list }));
});

/* ---------------- TMDB 手动匹配 ---------------- */
const showMatch = ref(false);
const q = ref("");
const searching = ref(false);
const results = ref<TmdbSearchItem[]>([]);
const matchMsg = ref("");

/* ---------------- 分类与标签 ---------------- */
/* 一条影视可以属于多个分类（多对多），也可以有多个标签。
   分类改动会立刻反映到左侧导航，标签则用于筛选。 */

const allCategories = computed(() => catStore.categories);
const myCategories = ref<Category[]>([]);
const myTags = ref<Tag[]>([]);
const tagInput = ref("");
const catMsg = ref("");
const tvdbBusy = ref(false);

async function loadClassification(): Promise<void> {
  const id = store.selectedId;
  if (!id) return;
  if (!catStore.categories.length) await catStore.load();
  try {
    const [c, t] = await Promise.all([getMediaCategories(id), getMediaTags(id)]);
    myCategories.value = c;
    myTags.value = t;
  } catch {
    myCategories.value = [];
    myTags.value = [];
  }
}

/**
 * 换条目时的重置钩子。
 *
 * ⚠️ 这段**必须放在所有被它引用的状态之后**：`immediate: true` 会在 setup
 * 期间同步执行一次回调，如果 `tagInput` / `catMsg` 还处在 TDZ 里，回调会在
 * 第一行引用它们时直接抛错 —— 而 Vue 只把错误吞进告警，后面几行（清空勾选、
 * 丢弃拖动、加载分类）就**静默不执行**了。这不是小事：单集勾选会跨剧目残留，
 * 删一条剧目的选集时可能删到另一部剧的集。
 *
 * 清空单集勾选是硬约定：上一个剧目的 id 不能带到下一个剧目，
 * 与列表中「切换视图必须清空多选」是同一条安全规则。
 */
watch(
  () => store.selectedId,
  () => {
    showMatch.value = false;
    results.value = [];
    q.value = "";
    matchMsg.value = "";
    tagInput.value = "";
    catMsg.value = "";
    abortDrag();
    epSelected.value = [];
    void loadClassification();
  },
  { immediate: true },
);

function hasCategory(id: number): boolean {
  return myCategories.value.some((c) => c.id === id);
}

/** 勾选 / 取消一个分类（全量覆盖式写回，后端保证幂等）。 */
async function toggleCategory(id: number): Promise<void> {
  const id2 = store.selectedId;
  if (!id2) return;
  const next = hasCategory(id)
    ? myCategories.value.filter((c) => c.id !== id).map((c) => c.id)
    : [...myCategories.value.map((c) => c.id), id];
  try {
    myCategories.value = await setMediaCategories(id2, next);
    await catStore.loadCategories();
  } catch (e) {
    catMsg.value = "更新分类失败：" + String(e);
  }
}

function parseTagInput(): string[] {
  return tagInput.value
    .split(/[,，、\s]+/)
    .map((s) => s.trim())
    .filter(Boolean);
}

async function addTags(): Promise<void> {
  const id = store.selectedId;
  const names = parseTagInput();
  if (!id || !names.length) return;
  const merged = [...new Set([...myTags.value.map((t) => t.name), ...names])];
  try {
    myTags.value = await setMediaTags(id, merged);
    tagInput.value = "";
    await catStore.loadTags();
  } catch (e) {
    catMsg.value = "更新标签失败：" + String(e);
  }
}

async function dropTag(name: string): Promise<void> {
  const id = store.selectedId;
  if (!id) return;
  try {
    myTags.value = await setMediaTags(
      id,
      myTags.value.map((t) => t.name).filter((n) => n !== name),
    );
    await catStore.loadTags();
  } catch (e) {
    catMsg.value = "更新标签失败：" + String(e);
  }
}

/**
 * 手动套用 TheTVDB 的季 / 集结构。
 *
 * 只在 TMDb 缺集信息（或集顺序跟 TMDb 不一致）时才需要——它是**补结构的**，
 * 不是刮削的替代品，片名 / 简介 / 评分仍然以 TMDb 为准。
 */
async function applyTvdb(): Promise<void> {
  const id = store.selectedId;
  if (!id) return;
  tvdbBusy.value = true;
  catMsg.value = "";
  try {
    const r = await tvdbApply(id);
    catMsg.value = `已应用 TheTVDB #${r.tvdbId}，回填 ${r.episodes} 集`;
    await store.openDetail(id);
  } catch (e) {
    catMsg.value = "TheTVDB 应用失败：" + String(e);
  } finally {
    tvdbBusy.value = false;
  }
}

/* ---------------- 指定归属 / 豆瓣刮削 ---------------- */
/* 「添加文件到这部剧」是推断失败时的兜底入口：`86版西游记.01.mkv`、`XYJ.01.mkv`
   这类名字里既没有剧名、目录名也不是剧名时，系统无从推断归属，只会另建一条剧集。
   推断做不到的就让用户显式指定 —— 他正站在这部剧的页面上，这个信息是确定的。 */

const addingFiles = ref(false);
const doubanBusy = ref(false);

async function addFilesToThisSeries(): Promise<void> {
  const d = detail.value;
  if (!d || addingFiles.value) return;
  const picked = await open({ directory: true, multiple: false, title: "选择要添加的文件夹" });
  const path = Array.isArray(picked) ? picked[0] : picked;
  if (!path) return;
  addingFiles.value = true;
  try {
    await store.importFolderInto(d.media.id, String(path), null);
  } finally {
    addingFiles.value = false;
  }
}

async function scrapeDouban(): Promise<void> {
  const d = detail.value;
  if (!d || doubanBusy.value) return;
  doubanBusy.value = true;
  try {
    await store.scrapeDouban(d.media.id);
  } finally {
    doubanBusy.value = false;
  }
}

async function doSearch(): Promise<void> {
  const d = detail.value;
  if (!d) return;
  const query = q.value.trim() || d.media.title;
  if (!query) return;
  searching.value = true;
  matchMsg.value = "";
  results.value = [];
  try {
    results.value = await searchTmdb(d.media.type, query, d.media.year);
    if (!results.value.length) matchMsg.value = "没有找到结果，换个关键词试试";
  } catch (e) {
    matchMsg.value = String(e);
  } finally {
    searching.value = false;
  }
}

function pick(it: TmdbSearchItem): void {
  const d = detail.value;
  if (!d) return;
  showMatch.value = false;
  store.scrape(d.media.id, it.id);
}
</script>

<template>
  <article v-if="detail" class="detail">
    <button class="back" @click="store.closeDetail()">← 返回列表</button>

    <header class="head" :style="backdrop ? { backgroundImage: `url(${backdrop})` } : {}">
      <div class="shade"></div>
      <div class="head-inner">
        <img v-if="poster" class="poster" :src="poster" :alt="detail.media.display_title" />
        <div v-else class="poster ph">{{ mediaTypeIcon }}</div>
        <div class="info">
          <h2>{{ detail.media.display_title }}</h2>
          <p class="line">
            <span class="tag">{{ mediaTypeLabel }}</span>
            <span v-if="detail.media.year">{{ detail.media.year }}</span>
            <span v-if="detail.media.genre">{{ detail.media.genre }}</span>
            <span v-if="score" class="score">★ {{ score }}</span>
            <span v-if="detail.media.episode_count">共 {{ detail.media.episode_count }} 集</span>
          </p>
          <p class="overview">{{ detail.media.overview || "暂无简介（可用下方「手动匹配 TMDB」补全）" }}</p>
          <p v-if="detail.media.file_name" class="file" :title="detail.media.file_path ?? ''">
            {{ detail.media.file_name }}
          </p>
        </div>
      </div>
    </header>

    <div class="actions">
      <button
        v-if="isSingleFile"
        class="btn primary"
        @click="store.play(detail.media.file_path)"
      >▶ 播放</button>
      <button class="btn" @click="store.scrape(detail.media.id)">
        {{ detail.media.tmdb_id ? "重新刮削" : "刮削元数据" }}
      </button>
      <button
        class="btn"
        :disabled="doubanBusy"
        title="TMDb 已有时只补缺失字段，TMDb 没有时整条由豆瓣接管"
        @click="scrapeDouban"
      >{{ doubanBusy ? "豆瓣刮削中…" : "用豆瓣刮削" }}</button>
      <button
        v-if="detail.media.type === 'tv'"
        class="btn"
        :disabled="addingFiles"
        title="不靠文件名推断，直接把这个文件夹里的视频归到这部剧下"
        @click="addFilesToThisSeries"
      >{{ addingFiles ? "添加中…" : "＋ 添加文件到这部剧" }}</button>
      <button class="btn" title="修改名称 / 类型 / 年份 / 流派 / 简介" @click="emit('edit', detail.media)">
        ✎ 编辑信息
      </button>
      <button class="btn" @click="showMatch = !showMatch">手动匹配 TMDB</button>
      <button
        v-if="detail.media.type === 'tv'"
        class="btn ghost"
        :disabled="tvdbBusy"
        title="TMDb 缺集信息或集顺序不一致时，用 TheTVDB 的季/集结构补"
        @click="applyTvdb"
      >{{ tvdbBusy ? "应用中…" : "套用 TheTVDB 结构" }}</button>
      <button
        v-if="detail.media.file_path"
        class="btn ghost"
        @click="store.reveal(detail.media.file_path)"
      >在文件夹中显示</button>
      <button class="btn danger" @click="store.remove(detail.media.id)">从库中移除</button>
    </div>

    <!-- 归类与打标签：分类同时决定它在左侧导航里出现在哪个栏目下 -->
    <section class="classify">
      <div class="c-block">
        <h3>分类</h3>
        <div class="chips">
          <button
            v-for="c in allCategories"
            :key="c.id"
            class="chip"
            :class="{ on: hasCategory(c.id) }"
            :title="c.kind === 'manual' ? '手动创建' : '自动识别（' + c.origin + '）'"
            @click="toggleCategory(c.id)"
          >{{ c.name }}</button>
          <span v-if="!allCategories.length" class="hint">
            还没有分类，可到左侧导航「＋」新建
          </span>
        </div>
      </div>

      <div class="c-block">
        <h3>标签</h3>
        <div class="tag-row">
          <span v-for="t in myTags" :key="t.id" class="tag-chip">
            {{ t.name }}
            <button class="x" title="移除标签" @click="dropTag(t.name)">✕</button>
          </span>
          <input
            v-model="tagInput"
            class="tag-input"
            placeholder="添加标签，逗号或空格分隔"
            @keyup.enter="addTags"
          />
          <button class="btn small" :disabled="!parseTagInput().length" @click="addTags">
            添加
          </button>
        </div>
      </div>
      <p v-if="catMsg" class="match-msg">{{ catMsg }}</p>
    </section>

    <section v-if="showMatch" class="match">
      <div class="match-row">
        <input
          v-model="q"
          :placeholder="`搜索词（默认：${detail.media.title}）`"
          @keyup.enter="doSearch"
        />
        <button class="btn" :disabled="searching" @click="doSearch">
          {{ searching ? "搜索中…" : "搜索" }}
        </button>
      </div>
      <p v-if="matchMsg" class="match-msg">{{ matchMsg }}</p>
      <ul v-if="results.length" class="results">
        <li v-for="it in results" :key="it.id">
          <img
            v-if="imageUrl(it.poster_path, 'w92')"
            :src="imageUrl(it.poster_path, 'w92')!"
            alt=""
          />
          <div class="r-info">
            <span class="r-title">{{ tmdbTitle(it) }}</span>
            <span class="r-sub">
              <span v-if="tmdbYear(it)">{{ tmdbYear(it) }}</span>
              <span v-if="it.vote_average">★ {{ it.vote_average.toFixed(1) }}</span>
              <span class="r-id">TMDB #{{ it.id }}</span>
            </span>
          </div>
          <button class="btn small" @click="pick(it)">应用</button>
        </li>
      </ul>
    </section>

    <section
      v-if="seasons.length"
      class="episodes"
      :class="{ dragging }"
      @pointerover="onEpPointerMove"
      @pointermove="onEpPointerMove"
    >
      <div class="ep-bar">
        <span class="ep-count">
          共 {{ detail.media.episode_count }} 条资源<template v-if="epSelected.length">
            · 已选 {{ epSelected.length }}
          </template>
        </span>
        <button class="btn small ghost" @click="toggleEpAll">
          {{ epSelected.length >= detail.media.episode_count ? "取消全选" : "全选" }}
        </button>
        <button
          class="btn small danger"
          :disabled="!epSelected.length"
          @click="removeEpisodes(epSelected)"
        >
          删除所选
        </button>
      </div>
      <p class="ep-tip">提示：在某一行上按住拖动可连续勾选（起点已选中则连续取消）</p>

      <div v-for="s in seasons" :key="s.season" class="season">
        <h3>第 {{ s.season }} 季</h3>
        <ul>
          <li
            v-for="e in s.list"
            :key="e.id"
            :data-ep-id="e.id"
            :class="{ picked: isEpSelected(e.id) }"
            @pointerdown="onEpPointerDown($event, e.id)"
          >
            <label class="ep-check">
              <input
                type="checkbox"
                :checked="isEpSelected(e.id)"
                @change="toggleEp(e.id)"
              />
            </label>
            <img
              v-if="imageUrl(e.still_path, 'w92')"
              class="ep-thumb"
              :src="imageUrl(e.still_path, 'w92')!"
              alt=""
            />
            <span class="ep-no">E{{ String(e.episode).padStart(2, "0") }}</span>
            <span class="ep-main">
              <span class="ep-title" :title="e.file_name ?? ''">{{ epTitle(e) }}</span>
              <span v-if="e.air_date" class="ep-date">{{ e.air_date }}</span>
            </span>
            <button class="btn small" @click="store.play(e.file_path)">▶ 播放</button>
            <button class="btn small ghost" @click="store.reveal(e.file_path)">目录</button>
            <button
              class="btn small danger"
              title="删除这一条资源"
              @click="removeEpisodes([e.id])"
            >✕</button>
          </li>
        </ul>
      </div>
    </section>
    <section v-else-if="detail.media.type === 'tv'" class="episodes empty">
      暂无单集记录，请先扫描媒体库，或用顶部的「导入文件夹」添加。
    </section>
  </article>
</template>

<style scoped>
.detail { display: flex; flex-direction: column; gap: calc(var(--space) * 5); }
.back { align-self: flex-start; padding: 4px 0; border: 0; background: none; color: var(--muted); font-size: var(--fs-13); cursor: pointer; }
.back:hover { color: var(--text); }

.head {
  position: relative; border-radius: var(--radius-l); overflow: hidden;
  background-color: var(--surface); background-size: cover; background-position: center;
  border: 1px solid var(--border);
}
.shade { position: absolute; inset: 0; background: linear-gradient(90deg, rgba(19,19,21,.96) 0%, rgba(19,19,21,.85) 55%, rgba(19,19,21,.6) 100%); }
.head-inner { position: relative; display: flex; gap: calc(var(--space) * 6); padding: calc(var(--space) * 7); align-items: flex-start; }
.poster { width: 170px; border-radius: var(--radius); flex: none; display: block; border: 1px solid var(--border); }
.poster.ph { aspect-ratio: 2 / 3; display: flex; align-items: center; justify-content: center; font-size: 40px; background: rgba(255,255,255,.05); }
.info { display: flex; flex-direction: column; gap: 12px; min-width: 0; }
h2 { margin: 0; font-size: var(--fs-24); line-height: 1.25; }
.line { display: flex; flex-wrap: wrap; gap: 10px; margin: 0; font-size: var(--fs-13); color: var(--muted); align-items: center; }
.tag { padding: 2px 8px; border-radius: 4px; background: var(--accent); color: #fff; font-size: var(--fs-12); font-weight: 600; }
.score { color: var(--score); font-weight: 600; }
.overview { margin: 0; font-size: var(--fs-14); line-height: 1.75; color: var(--text-2); max-width: 72ch; }
.file { margin: 0; font-size: var(--fs-12); color: var(--muted); word-break: break-all; }

.actions { display: flex; flex-wrap: wrap; gap: 10px; }
.btn { padding: 9px 16px; border-radius: var(--radius-s); cursor: pointer; font-size: var(--fs-13); background: var(--surface-2); color: var(--text); border: 1px solid var(--border); transition: background .15s ease, opacity .15s ease; }
.btn:hover { background: var(--border-strong); }
.btn.primary { background: var(--accent); color: #fff; border-color: transparent; }
.btn.primary:hover { background: var(--accent-hover); }
.btn.danger { background: var(--accent-soft); color: var(--danger); border-color: transparent; }
.btn.small { padding: 6px 11px; font-size: var(--fs-12); }
.btn:disabled { opacity: .55; cursor: default; }

.match { display: flex; flex-direction: column; gap: 10px; }
.match-row { display: flex; gap: 8px; }
.match-row input { flex: 1; }
.match-msg { margin: 0; font-size: var(--fs-13); color: var(--muted); }
.results { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; gap: 8px; }
.results li { display: flex; align-items: center; gap: 12px; padding: 10px; border-radius: var(--radius-s); background: var(--surface); border: 1px solid var(--border); }
.results img { width: 46px; border-radius: 4px; flex: none; }
.r-info { flex: 1; display: flex; flex-direction: column; gap: 3px; min-width: 0; }
.r-title { font-size: var(--fs-14); font-weight: 550; }
.r-sub { display: flex; gap: 10px; font-size: var(--fs-12); color: var(--muted); }
.r-id { opacity: .65; }

.classify {
  display: flex; flex-direction: column; gap: calc(var(--space) * 4);
  padding: calc(var(--space) * 4); border-radius: var(--radius);
  background: var(--surface); border: 1px solid var(--border);
}
.c-block { display: flex; flex-direction: column; gap: 8px; }
.classify h3 { margin: 0; font-size: var(--fs-13); color: var(--muted); font-weight: 600; }
.chips { display: flex; flex-wrap: wrap; gap: 6px; }
.chip {
  padding: 5px 11px; border-radius: 999px; cursor: pointer; font-size: var(--fs-12);
  border: 1px solid var(--border); background: var(--surface-2); color: var(--muted);
  transition: background .15s ease, color .15s ease;
}
.chip:hover { color: var(--text); }
.chip.on { background: var(--accent); color: #fff; border-color: transparent; }
.hint { font-size: var(--fs-12); color: var(--muted); }
.tag-row { display: flex; flex-wrap: wrap; align-items: center; gap: 6px; }
.tag-chip {
  display: inline-flex; align-items: center; gap: 5px; padding: 4px 8px;
  border-radius: var(--radius-s); font-size: var(--fs-12);
  background: var(--surface-2); border: 1px solid var(--border); color: var(--text-2);
}
.tag-chip .x {
  border: 0; background: none; color: var(--muted); cursor: pointer; font-size: var(--fs-11);
  padding: 0 2px;
}
.tag-chip .x:hover { color: var(--danger); }
.tag-input {
  flex: 1; min-width: 180px; padding: 6px 10px; border-radius: var(--radius-s);
  font-size: var(--fs-12); border: 1px solid var(--border);
  background: var(--surface-2); color: var(--text); outline: none;
}
.tag-input:focus { border-color: var(--accent); }

/* 单集工具条：勾选 / 全选 / 批量删除 */
.ep-bar {
  display: flex; align-items: center; gap: 10px; padding: calc(var(--space) * 2) calc(var(--space) * 3);
  border-radius: var(--radius-s); background: var(--surface); border: 1px solid var(--border);
}
.ep-count { flex: 1; font-size: var(--fs-13); color: var(--text-2); }
.ep-tip { margin: 0; font-size: var(--fs-12); color: var(--muted); }
.ep-check { flex: none; display: flex; align-items: center; }
.ep-check input { width: 15px; height: 15px; accent-color: var(--accent); cursor: pointer; }
/* 勾上的行给个底纹，避免看不出选了哪些 */
.season li.picked { background: var(--accent-soft); }
/* 拖动划过期间禁掉文字选择：否则一路划过去会把标题选蓝。
   （按下时已 preventDefault，这里只是兜底与视觉提示。） */
.episodes.dragging { user-select: none; cursor: default; }
.episodes.dragging .season li { cursor: default; }

.episodes { display: flex; flex-direction: column; gap: calc(var(--space) * 5); }
.episodes.empty { color: var(--muted); font-size: var(--fs-13); }
h3 { margin: 0 0 10px; font-size: var(--fs-15); color: var(--muted); font-weight: 650; }
.season ul { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; gap: 6px; }
.season li { display: flex; align-items: center; gap: 12px; padding: 8px 12px; border-radius: var(--radius-s); background: var(--surface); border: 1px solid var(--border); }
.ep-no { flex: none; width: 34px; font-size: var(--fs-12); color: var(--muted); font-weight: 600; }
.ep-thumb { width: 60px; border-radius: 4px; flex: none; display: block; }
.ep-main { flex: 1; display: flex; flex-direction: column; gap: 2px; min-width: 0; }
.ep-title { font-size: var(--fs-13); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.ep-date { font-size: var(--fs-12); color: var(--muted); }
</style>
