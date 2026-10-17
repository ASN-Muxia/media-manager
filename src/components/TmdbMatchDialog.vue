<script setup lang="ts">
/**
 * TMDB 候选选择面板。
 *
 * 替代了原来详情页里那个「输入框 + 一条条列表」的内嵌区块：
 * - 候选**不再受第一页 20 条的限制**（后端一次拉到设置里的上限）；
 * - 关键字**实时过滤**（本地过滤，不改关键词就不重新联网）；
 * - 每条可勾选，底部固定条**清楚显示当前已选**；
 * - 年份容差 / 模糊匹配 / 排除词三个筛选条件可现场调（会重新检索）；
 * - 候选多时**滚动加载**（一次渲染 30 条），不会一次性塞几百个 DOM。
 *
 * 只做展示与选择的改动：调用的命令、返回的数据结构、应用后的刮削流程
 * 都与原来完全一致。
 */
import { computed, nextTick, ref, watch } from "vue";
import { imageUrl, searchTmdb, tmdbTitle, tmdbYear } from "../api/commands";
import type { MediaItem, TmdbSearchItem } from "../api/commands";
import { useSettingsStore } from "../stores/settingsStore";
import { useHotkeys } from "../composables/useHotkeys";
import { formatCombo } from "../utils/shortcuts";

const props = defineProps<{
  open: boolean;
  /** 本地条目：拿标题 / 原标题 / 年份 / 类型去检索。 */
  media: MediaItem;
}>();

const emit = defineEmits<{
  (e: "update:open", v: boolean): void;
  /** 用户确认了某个候选：把 TMDB id 交给上层去刮削。 */
  (e: "apply", tmdbId: number): void;
}>();

const settings = useSettingsStore();

const loading = ref(false);
const errMsg = ref("");
/** 后端返回的全部候选（已按匹配度排好序）。 */
const all = ref<TmdbSearchItem[]>([]);
/** 本地关键字过滤词。 */
const keyword = ref("");
/** 检索用的关键词（改了它才会重新联网）。 */
const query = ref("");
const picked = ref<number | null>(null);
/** 本次检索的筛选条件快照（显示用）。 */
const appliedFuzzy = ref(true);
const appliedTolerance = ref(1);

/** 一次渲染多少条；滚到底再加一批。 */
const PAGE = 30;
const shown = ref(PAGE);
const listEl = ref<HTMLElement | null>(null);

/* ---------------- 筛选条件（改动后需重新检索） ---------------- */
const fuzzy = ref(settings.matchFuzzy);
const yearTolerance = ref(settings.matchYearTolerance);
const blacklist = ref(settings.matchBlacklist);
const limit = ref(settings.matchLimit);

/* ---------------- 本地即时过滤 ---------------- */
const onlyWithPoster = ref(false);
const onlySameYear = ref(false);

function titleOf(it: TmdbSearchItem): string {
  return tmdbTitle(it);
}
function yearOf(it: TmdbSearchItem): number | null {
  return tmdbYear(it);
}
function originalOf(it: TmdbSearchItem): string | null {
  return (it.original_title || it.original_name || "").trim() || null;
}

/** 关键字命中：标题 / 原标题 / 年份 / TMDB id 都算。 */
function hitKeyword(it: TmdbSearchItem): boolean {
  const k = keyword.value.trim().toLowerCase();
  if (!k) return true;
  const y = yearOf(it);
  const hay = [
    titleOf(it),
    originalOf(it) ?? "",
    y ? String(y) : "",
    String(it.id),
    it.overview ?? "",
  ]
    .join(" ")
    .toLowerCase();
  return hay.includes(k);
}

const filtered = computed<TmdbSearchItem[]>(() => {
  return all.value.filter((it) => {
    if (onlyWithPoster.value && !imageUrl(it.poster_path, "w92")) return false;
    if (onlySameYear.value) {
      const y = yearOf(it);
      const want = props.media.year ?? null;
      if (y == null || want == null) return false;
      if (Math.abs(y - want) > Math.max(0, yearTolerance.value)) return false;
    }
    return hitKeyword(it);
  });
});

/** 实际渲染的那一段（滚动加载）。 */
const visible = computed<TmdbSearchItem[]>(() => filtered.value.slice(0, shown.value));
const hasMore = computed(() => filtered.value.length > visible.value.length);

function onScroll(): void {
  const el = listEl.value;
  if (!el || !hasMore.value) return;
  // 离底 120px 就续一批，用户不会看到空白
  if (el.scrollTop + el.clientHeight >= el.scrollHeight - 120) {
    shown.value += PAGE;
  }
}

/* ---------------- 检索 ---------------- */

async function runSearch(): Promise<void> {
  const m = props.media;
  const q = query.value.trim() || m.title;
  if (!q) return;
  loading.value = true;
  errMsg.value = "";
  all.value = [];
  picked.value = null;
  shown.value = PAGE;
  try {
    const items = await searchTmdb(m.type, q, m.year ?? null, {
      originalTitle: m.original_title ?? null,
      season: null,
      limit: limit.value,
      fuzzy: fuzzy.value,
      yearTolerance: yearTolerance.value,
      blacklist: blacklist.value,
    });
    all.value = items;
    appliedFuzzy.value = fuzzy.value;
    appliedTolerance.value = yearTolerance.value;
    if (!items.length) errMsg.value = "没有找到结果：换个关键词，或放宽年份容差 / 关掉排除词";
  } catch (e) {
    errMsg.value = String(e);
  } finally {
    loading.value = false;
    void nextTick(() => {
      if (listEl.value) listEl.value.scrollTop = 0;
    });
  }
}

/** 筛选条件变了 → 重新检索（防抖，避免拖滑块时连发请求）。 */
let timer: number | undefined;
function scheduleSearch(): void {
  if (timer) window.clearTimeout(timer);
  timer = window.setTimeout(() => void runSearch(), 350);
}

function close(): void {
  emit("update:open", false);
}

function confirmPick(): void {
  if (picked.value == null) return;
  emit("apply", picked.value);
  close();
}

function pick(id: number): void {
  // 单选：再点一次同一条 = 取消
  picked.value = picked.value === id ? null : id;
}

const pickedItem = computed<TmdbSearchItem | null>(
  () => all.value.find((it) => it.id === picked.value) ?? null,
);

const shortcutHint = computed(() => formatCombo(settings.shortcutConfirm));

/**
 * 面板打开时接管「确认 / 清空」两个快捷键。
 *
 * `active` 只在面板打开时为真，所以列表页那组快捷键不会被抢——
 * 两个作用域互斥，同一个 `Ctrl+Enter` 在不同场景下做不同的事。
 */
useHotkeys({
  active: computed(() => props.open),
  bindings: () => ({
    confirm: settings.shortcutConfirm,
    clear: settings.shortcutClear,
  }),
  handlers: {
    confirm: () => confirmPick(),
    clear: () => {
      picked.value = null;
    },
  },
});

watch(
  () => props.open,
  (v) => {
    if (!v) return;
    // 每次打开都从条目的当前信息出发，并同步设置里的筛选偏好
    query.value = props.media.title ?? "";
    keyword.value = "";
    fuzzy.value = settings.matchFuzzy;
    yearTolerance.value = settings.matchYearTolerance;
    blacklist.value = settings.matchBlacklist;
      limit.value = settings.matchLimit;
    void runSearch();
  },
  // `immediate` 是必须的：面板常常是「已经 open 着」被挂载的
  //（父组件 `v-model:open` 先置真再渲染），不带它的话首次检索根本不会跑，
  // 用户看到的就是一个空列表。
  { immediate: true },
);

defineExpose({ scheduleSearch, runSearch });
</script>

<template>
  <div v-if="open" class="mask" @click.self="close">
    <section class="panel" role="dialog" aria-modal="true" aria-label="TMDB 候选匹配">
      <header class="p-head">
        <h3>TMDB 候选匹配</h3>
        <span class="p-sub">
          {{ media.title }}<template v-if="media.year"> · {{ media.year }}</template>
        </span>
        <button class="btn btn-ghost small" aria-label="关闭" @click="close">✕</button>
      </header>

      <!-- 检索区：改关键词按「重新搜索」，下面的关键字框是即时过滤 -->
      <div class="p-search">
        <input
          v-model="query"
          placeholder="检索词（默认用条目标题）"
          @keyup.enter="runSearch"
        />
        <button class="btn" :disabled="loading" @click="runSearch">
          {{ loading ? "检索中…" : "重新搜索" }}
        </button>
      </div>

      <div class="p-filters">
        <input v-model="keyword" class="kw" placeholder="在结果里过滤：标题 / 原名 / 年份 / ID" />
        <label class="f">
          年份容差
          <select v-model.number="yearTolerance" @change="scheduleSearch">
            <option :value="0">同一年</option>
            <option :value="1">±1 年</option>
            <option :value="2">±2 年</option>
            <option :value="10">不限</option>
          </select>
        </label>
        <label class="f">
          候选上限
          <select v-model.number="limit" @change="scheduleSearch">
            <option :value="20">20</option>
            <option :value="60">60</option>
            <option :value="120">120</option>
            <option :value="200">200</option>
          </select>
        </label>
        <label class="f switch">
          <input v-model="fuzzy" type="checkbox" @change="scheduleSearch" />
          <span>模糊匹配</span>
        </label>
        <label class="f switch">
          <input v-model="onlyWithPoster" type="checkbox" />
          <span>只看有海报</span>
        </label>
        <label class="f switch">
          <input v-model="onlySameYear" type="checkbox" />
          <span>只看年份吻合</span>
        </label>
        <label class="f grow">
          排除词
          <input
            v-model="blacklist"
            placeholder="逗号分隔，如：剧场版, OVA"
            @change="scheduleSearch"
          />
        </label>
      </div>

      <p v-if="errMsg" class="p-msg">{{ errMsg }}</p>
      <p v-else class="p-msg dim">
        共 {{ all.length }} 个候选<template v-if="filtered.length !== all.length">
          · 过滤后 {{ filtered.length }} 个</template
        ><template v-if="appliedFuzzy"> · 已启用模糊匹配</template
        ><template v-else> · 未启用模糊匹配</template>
      </p>

      <!-- 候选列表：滚动加载，条目再多也不会一次性渲染完 -->
      <ul ref="listEl" class="p-list" @scroll="onScroll">
        <li
          v-for="it in visible"
          :key="it.id"
          class="row"
          :class="{ on: picked === it.id }"
          @click="pick(it.id)"
        >
          <label class="check" @click.stop>
            <input
              type="checkbox"
              :checked="picked === it.id"
              @change="pick(it.id)"
              :aria-label="`选择 ${titleOf(it)}`"
            />
          </label>
          <img v-if="imageUrl(it.poster_path, 'w92')" class="thumb" :src="imageUrl(it.poster_path, 'w92')!" alt="" />
          <span v-else class="thumb ph">无图</span>
          <div class="info">
            <span class="r-title">{{ titleOf(it) }}</span>
            <span v-if="originalOf(it)" class="r-orig">原名：{{ originalOf(it) }}</span>
            <span class="r-sub">
              <span v-if="yearOf(it)">{{ yearOf(it) }}</span>
              <span v-if="it.vote_average">★ {{ it.vote_average.toFixed(1) }}</span>
              <span class="r-id">TMDB #{{ it.id }}</span>
            </span>
            <span v-if="it.match_reasons?.length" class="r-why">
              {{ it.match_reasons.join(" · ") }}
            </span>
          </div>
          <span v-if="it.match_score != null" class="score" :title="`匹配度 ${it.match_score}`">
            {{ it.match_score }}
          </span>
        </li>
        <li v-if="hasMore" class="more">向下滚动加载更多…</li>
        <li v-if="!loading && !filtered.length" class="empty">没有符合条件的候选</li>
      </ul>

      <!-- 底部固定条：清楚显示当前已选 -->
      <footer class="p-foot">
        <span class="picked">
          <template v-if="pickedItem">
            已选：<b>{{ titleOf(pickedItem) }}</b>
            <span v-if="yearOf(pickedItem)">（{{ yearOf(pickedItem) }}）</span>
            <span class="dim">· TMDB #{{ pickedItem.id }}</span>
          </template>
          <template v-else>还没选（点一行即可选中）</template>
        </span>
        <span class="grow"></span>
        <span class="hint">确认：{{ shortcutHint }}</span>
        <button class="btn btn-ghost" @click="picked = null" :disabled="picked == null">
          取消选择
        </button>
        <button class="btn btn-primary" :disabled="picked == null" @click="confirmPick">
          应用所选
        </button>
      </footer>
    </section>
  </div>
</template>

<style scoped>
.mask {
  position: fixed; inset: 0; z-index: 40;
  display: flex; align-items: center; justify-content: center;
  background: rgba(0, 0, 0, 0.55);
}
.panel {
  width: min(880px, 92vw);
  max-height: 88vh;
  display: flex; flex-direction: column; gap: calc(var(--space) * 3);
  padding: calc(var(--space) * 5);
  background: var(--bg-soft); border: 1px solid var(--border-strong);
  border-radius: var(--radius-l); box-shadow: var(--shadow);
}
.p-head { display: flex; align-items: center; gap: calc(var(--space) * 3); }
.p-head h3 { margin: 0; font-size: var(--fs-18); }
.p-sub { color: var(--muted); font-size: var(--fs-13); flex: 1; min-width: 0; }

.p-search { display: flex; gap: calc(var(--space) * 2); }
.p-search input { flex: 1; }

.p-filters {
  display: flex; flex-wrap: wrap; gap: calc(var(--space) * 3); align-items: center;
  padding: calc(var(--space) * 3); border: 1px solid var(--border);
  border-radius: var(--radius-s); background: var(--surface);
}
.p-filters .kw { flex: 2; min-width: 220px; }
.f { display: flex; align-items: center; gap: calc(var(--space) * 1.5); font-size: var(--fs-13); color: var(--text-2); }
.f.grow { flex: 1; min-width: 180px; }
.f.grow input { flex: 1; min-width: 0; }
.f.switch { gap: calc(var(--space) * 1.5); cursor: pointer; }

.p-msg { margin: 0; font-size: var(--fs-13); color: var(--danger); }
.p-msg.dim { color: var(--muted); }

.p-list {
  list-style: none; margin: 0; padding: 0;
  flex: 1; min-height: 240px; overflow-y: auto;
  display: flex; flex-direction: column; gap: calc(var(--space) * 2);
}
.row {
  display: flex; align-items: center; gap: calc(var(--space) * 3);
  padding: calc(var(--space) * 2.5);
  border: 1px solid var(--border); border-radius: var(--radius-s);
  background: var(--surface); cursor: pointer;
}
.row:hover { border-color: var(--border-strong); }
.row.on { border-color: var(--accent); background: var(--accent-soft); }
.check { display: flex; align-items: center; flex: none; }
.thumb { width: 46px; height: 69px; object-fit: cover; border-radius: 4px; flex: none; background: var(--surface-2); }
.thumb.ph {
  display: flex; align-items: center; justify-content: center;
  font-size: var(--fs-12); color: var(--muted);
}
.info { flex: 1; display: flex; flex-direction: column; gap: 2px; min-width: 0; }
.r-title { font-size: var(--fs-14); font-weight: 550; }
.r-orig { font-size: var(--fs-12); color: var(--text-2); }
.r-sub { display: flex; gap: calc(var(--space) * 2.5); font-size: var(--fs-12); color: var(--muted); }
.r-id { opacity: 0.65; }
.r-why { font-size: var(--fs-12); color: var(--muted); opacity: 0.85; }
.score {
  flex: none; min-width: 34px; text-align: center;
  font-size: var(--fs-13); font-weight: 600; color: var(--score);
}
.more, .empty { padding: calc(var(--space) * 3); text-align: center; color: var(--muted); font-size: var(--fs-13); }

.p-foot {
  display: flex; align-items: center; gap: calc(var(--space) * 2);
  padding-top: calc(var(--space) * 3);
  border-top: 1px solid var(--border);
}
.picked { font-size: var(--fs-13); color: var(--text-2); min-width: 0; }
.picked b { color: var(--text); }
.dim { color: var(--muted); }
.grow { flex: 1; }
.hint { font-size: var(--fs-12); color: var(--muted); }
</style>
