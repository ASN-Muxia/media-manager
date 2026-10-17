<script setup lang="ts">
/**
 * 资源目录库：一份**文件系统视角**的持久清单，加上「这次检测相对上次变了什么」。
 *
 * ## 为什么要有这个面板
 *
 * 媒体库里存的是「我们认定这是一部片子」，目录库里存的是「这个文件当时在那儿」。
 * 用户把文件删了 / 挪了 / 改名了，媒体库不会自己知道；这里靠**每次检测写一份快照**，
 * 下一次检测逐条比对，把差异摆出来：多了什么、变了什么、哪些不见了。
 * 「定位旧资源」就是靠最下面那张清单——`已丢失` 的行就是需要人去确认的那批。
 *
 * ## 组件自己不存「是否已同步」
 *
 * 真实来源是后端的 `resource_snapshots` 表。这里**不写 localStorage、不写 store**，
 * 只把三个命令（检测 / 清单 / 对比）的返回结果画出来。一旦前端也存一份，
 * 两边迟早对不上（比如用户换了媒体目录、或另一个窗口跑过检测）。
 *
 * ## 分组展示而不是一张大表
 *
 * 「新增 / 变化 / 丢失」三组用的是对比结果，条数一眼可见；下面那张表才是可筛选的
 * 全量清单。两者数据源不同（`diff_resource_catalog` vs `get_resource_catalog`），
 * 所以分开请求、分开渲染，任何一边失败都不影响另一边照常显示。
 */
import { computed, onMounted, ref, watch } from "vue";
import {
  diffResourceCatalog,
  getResourceCatalog,
  syncResourceCatalog,
  type CatalogDiff,
  type CatalogEntry,
  type CatalogSyncResult,
} from "../api/commands";

/** 筛选用的状态：`all` 只是界面上的「全部」，发给后端时转成 `null`。 */
type StatusFilter = "all" | "present" | "missing";

/** 清单（表格区）。 */
const entries = ref<CatalogEntry[]>([]);
/** 与上一次检测的差异（对比区）。 */
const diff = ref<CatalogDiff | null>(null);
/** 最近一次「检测资源库」的统计（摘要区）。 */
const summary = ref<CatalogSyncResult | null>(null);
/** 首次挂载就要拉数据，初值给 true：否则会先闪一下「空列表」再变成内容。 */
const loading = ref(true);
const syncing = ref(false);
const error = ref("");

const statusFilter = ref<StatusFilter>("all");
const keyword = ref("");

/** 丢了几个（当前清单范围内）——大于 0 才提示用户去「重新检测」。 */
const missingCount = computed(
  () => entries.value.filter((e) => e.status === "missing").length,
);

/** 对比区的三组，顺序固定：新增 → 变化 → 丢失。 */
const diffGroups = computed(() => {
  const d = diff.value;
  if (!d) return [];
  return [
    { key: "added", label: "新增", items: d.added },
    { key: "changed", label: "变化", items: d.changed },
    { key: "missing", label: "丢失", items: d.missing },
  ];
});

/**
 * 最近一次检测时间。
 *
 * 用的是**对比结果**里的 `lastSyncAt`（后端的快照时间），本地时区格式化。
 * 为 `null` 时如实说「从未检测过」，不能画成 1970 或当前时间——那是假信息。
 */
const lastSyncText = computed(() => {
  const ts = diff.value?.lastSyncAt ?? null;
  return ts ? formatTime(ts) : "从未检测过";
});

function statusLabel(status: CatalogEntry["status"]): string {
  return status === "missing" ? "已丢失" : "在库";
}

function statusClass(status: CatalogEntry["status"]): string {
  return status === "missing" ? "tag-missing" : "tag-present";
}

/** Unix 秒 → 本地时间；取不到时间显示 `-`（与设置页同一写法）。 */
function formatTime(ts: number | null): string {
  if (!ts) return "-";
  return new Date(ts * 1000).toLocaleString();
}

/** 字节数 → 人性化（B / KB / MB / GB / TB）。非有限值或负数按 0 处理。 */
function formatSize(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return "0 B";
  const units = ["B", "KB", "MB", "GB", "TB"];
  let v = bytes;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i += 1;
  }
  return (i === 0 ? String(v) : v.toFixed(1)) + " " + units[i];
}

/**
 * 拉清单。
 *
 * 出错时**清空列表并给出错误文案**：留着上一次的数据再配一行红字，用户分不清
 * 屏幕上到底是新结果还是旧结果。
 */
async function load(): Promise<void> {
  loading.value = true;
  error.value = "";
  try {
    entries.value = await getResourceCatalog({
      status: statusFilter.value === "all" ? null : statusFilter.value,
      keyword: keyword.value,
    });
  } catch (e) {
    entries.value = [];
    error.value = "读取资源目录库失败：" + String(e);
  } finally {
    loading.value = false;
  }
}

/** 拉对比。失败只让对比区空着，不干扰清单（两边是独立的请求）。 */
async function loadDiff(): Promise<void> {
  try {
    diff.value = await diffResourceCatalog();
  } catch {
    diff.value = null;
  }
}

/** 重新拉清单与对比（不改后端数据）。 */
async function refresh(): Promise<void> {
  await Promise.all([load(), loadDiff()]);
}

/**
 * 检测资源库。
 *
 * 检测会遍历各扫描根，可能要几十秒：进行中把按钮禁掉并改成「检测中…」，
 * 避免用户连点出好几次全盘遍历。跑完顺手刷新清单与对比，
 * 让「摘要 / 对比 / 清单」三块讲的是同一轮检测的事。
 */
async function onSync(): Promise<void> {
  if (syncing.value) return;
  syncing.value = true;
  error.value = "";
  try {
    summary.value = await syncResourceCatalog();
    await refresh();
  } catch (e) {
    error.value = "检测资源库失败：" + String(e);
  } finally {
    syncing.value = false;
  }
}

/**
 * 筛选条件一变就重新问后端。
 *
 * 筛选在 SQL 层做（与媒体列表一致），前端不二次过滤：一次只传输用户要看的那部分。
 * 这里不加防抖——本地 SQLite 的 LIKE 查询本来就快，加了反而让输入看起来有延迟。
 */
watch([statusFilter, keyword], () => {
  void load();
});

onMounted(() => {
  void refresh();
});

/** 供外部（如扫描 / 导入完成后）重新拉取，不触发检测。 */
defineExpose({ refresh });
</script>

<template>
  <section class="catalog">
    <h2>资源目录库</h2>
    <p class="panel-hint">
      这里存的是<strong>文件系统视角</strong>的清单：每次检测都会遍历扫描根，
      与上一次的快照逐条比对后<strong>增量</strong>写入。
      文件被删除 / 移动 / 改名后，靠下面的「变更对比」与清单里的「已丢失」定位它们；
      <strong>媒体库条目不会被自动删除</strong>。
    </p>

    <div class="panel">
      <h3 class="panel-title">
        检测
        <span class="badge">只读文件系统 · 不动媒体库</span>
      </h3>
      <div class="toolbar">
        <button
          type="button"
          class="btn btn-primary catalog-sync"
          :disabled="syncing"
          @click="onSync"
        >
          {{ syncing ? "检测中…" : "检测资源库" }}
        </button>
        <select v-model="statusFilter" class="select catalog-status" aria-label="按状态筛选">
          <option value="all">全部状态</option>
          <option value="present">在库</option>
          <option value="missing">已丢失</option>
        </select>
        <input
          v-model="keyword"
          class="catalog-keyword"
          type="search"
          placeholder="按文件名筛选…"
          aria-label="按文件名筛选"
        />
      </div>

      <!-- 同步摘要：每次检测完先看这几个数字，判断这一轮到底动了什么 -->
      <div v-if="summary" class="summary catalog-summary">
        <span class="sum"><span class="sum-num">{{ summary.scanned }}</span><span class="sum-label">扫描</span></span>
        <span class="sum"><span class="sum-num">{{ summary.added }}</span><span class="sum-label">新增</span></span>
        <span class="sum"><span class="sum-num">{{ summary.updated }}</span><span class="sum-label">更新</span></span>
        <span class="sum"><span class="sum-num">{{ summary.unchanged }}</span><span class="sum-label">未变</span></span>
        <span class="sum"><span class="sum-num">{{ summary.missing }}</span><span class="sum-label">丢失</span></span>
        <span class="sum"><span class="sum-num">{{ summary.restored }}</span><span class="sum-label">恢复</span></span>
      </div>
      <p v-else class="panel-hint">
        还没有本轮的检测结果：点「检测资源库」跑一次，这里会给出扫描 / 新增 / 更新 / 未变 / 丢失 / 恢复 各多少。
      </p>
      <p class="panel-hint">最近同步时间：{{ lastSyncText }}</p>
      <p v-if="summary && summary.message" class="panel-hint">{{ summary.message }}</p>
    </div>

    <div class="panel">
      <h3 class="panel-title">
        变更对比
        <span class="badge">相对上一次检测</span>
      </h3>
      <template v-if="diff">
        <p class="panel-hint">
          当前在库 <strong>{{ diff.present }}</strong> 条 ·
          上次检测：{{ lastSyncText }}
        </p>
        <div v-for="g in diffGroups" :key="g.key" class="diff-group">
          <h4 class="diff-title">
            {{ g.label }}
            <span class="diff-count">（{{ g.items.length }}）</span>
          </h4>
          <ul v-if="g.items.length" class="diff-list">
            <li v-for="it in g.items" :key="it.id" class="diff-item">
              <span class="tag" :class="statusClass(it.status)">{{ statusLabel(it.status) }}</span>
              <span class="diff-name">{{ it.fileName }}</span>
              <span class="diff-dir">{{ it.relDir || "（根目录）" }}</span>
            </li>
          </ul>
          <p v-else class="panel-hint">无</p>
        </div>
      </template>
      <p v-else class="panel-hint">
        暂时读不到变更对比。跑一次「检测资源库」，或确认后端已有历史快照。
      </p>
    </div>

    <div class="panel">
      <h3 class="panel-title">
        清单
        <span v-if="entries.length" class="badge">{{ entries.length }} 条</span>
      </h3>

      <p v-if="loading" class="panel-hint catalog-loading">读取中…</p>
      <p v-else-if="error" class="catalog-error">{{ error }}</p>
      <!-- 空列表要给可操作的下一步，不能是一片空白 -->
      <p v-else-if="!entries.length" class="catalog-empty">
        没有符合条件的记录。点上方「检测资源库」跑一次检测；
        如果刚筛过，把状态换成「全部状态」、清空文件名再试。
      </p>

      <table v-else class="tt">
        <thead>
          <tr>
            <th>状态</th>
            <th>文件名</th>
            <th>相对目录</th>
            <th>大小</th>
            <th>修改时间</th>
            <th>关联条目</th>
          </tr>
        </thead>
        <tbody>
          <tr
            v-for="e in entries"
            :key="e.id"
            :class="{ 'is-missing': e.status === 'missing' }"
          >
            <td>
              <span class="tag" :class="statusClass(e.status)">{{ statusLabel(e.status) }}</span>
            </td>
            <td class="name">{{ e.fileName }}</td>
            <td class="dir">{{ e.relDir || "（根目录）" }}</td>
            <td class="num">{{ formatSize(e.size) }}</td>
            <td class="num">{{ formatTime(e.mtime) }}</td>
            <td>{{ e.displayTitle || "未关联" }}</td>
          </tr>
        </tbody>
      </table>

      <p v-if="!loading && !error && missingCount" class="panel-hint catalog-missing-tip">
        清单里有 {{ missingCount }} 条标记为「已丢失」：文件已不在原路径（被删 / 移动 / 改名）。
        把它们找回或整理好之后，再点上方「检测资源库」重新检测，状态会自动恢复。
      </p>
    </div>
  </section>
</template>

<style scoped>
.catalog { max-width: 900px; display: flex; flex-direction: column; gap: calc(var(--space) * 5); }
h2 { margin: 0 0 calc(var(--space) * 1); font-size: var(--fs-20); font-weight: 650; }

.toolbar { display: flex; flex-wrap: wrap; align-items: center; gap: 10px; }

/* 关键字输入框：与设置页的 .hosts / .select 同一套取值 */
.catalog-keyword {
  flex: 1; min-width: 160px; max-width: 280px;
  padding: 9px 12px; border-radius: var(--radius-s); font-size: var(--fs-13);
  border: 1px solid var(--border); background: var(--surface-2); color: var(--text);
  outline: none;
}
.catalog-keyword:focus { border-color: var(--accent); }
.catalog-keyword::placeholder { color: var(--muted); }
.select {
  padding: 9px 12px; border-radius: var(--radius-s); font-size: var(--fs-13);
  border: 1px solid var(--border); background: var(--surface-2); color: var(--text);
  outline: none; max-width: 200px;
}
.select:focus { border-color: var(--accent); }

/* 同步摘要：一行数字，用底色与面板本身分开 */
.summary {
  display: flex; flex-wrap: wrap; gap: calc(var(--space) * 3);
  margin-top: calc(var(--space) * 3);
  padding: 10px 12px; border-radius: var(--radius-s);
  background: var(--surface-2); border: 1px solid var(--border);
}
.sum { display: flex; align-items: baseline; gap: 6px; }
.sum-num { font-size: var(--fs-18); font-weight: 650; color: var(--text); }
.sum-label { font-size: var(--fs-12); color: var(--muted); }

/* 变更对比：每组一个标题 + 条目列表 */
.diff-group { margin-top: calc(var(--space) * 3); }
.diff-title { margin: 0 0 calc(var(--space) * 1); font-size: var(--fs-14); font-weight: 600; }
.diff-count { color: var(--muted); font-weight: 400; font-size: var(--fs-13); }
.diff-list { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; gap: 4px; }
.diff-item {
  display: flex; align-items: baseline; gap: 8px;
  padding: 6px 10px; border-radius: var(--radius-s);
  background: var(--surface-2); border: 1px solid var(--border);
  font-size: var(--fs-13);
}
.diff-name { color: var(--text); word-break: break-all; }
.diff-dir { color: var(--muted); font-size: var(--fs-12); word-break: break-all; }

/* 状态标签：在库低调，丢失用强调色（唯一需要用户动手处理的状态） */
.tag {
  flex: none; padding: 1px 8px; border-radius: 999px;
  font-size: var(--fs-12); font-weight: 500;
  border: 1px solid var(--border);
}
.tag-present { color: var(--muted); background: var(--surface-2); }
.tag-missing { color: var(--danger); background: var(--accent-soft); border-color: var(--danger); }

/* 清单表格：与设置页「最近扫描记录」同一套排版 */
.tt { width: 100%; border-collapse: collapse; font-size: var(--fs-12); }
.tt th, .tt td { padding: 8px 10px; text-align: left; border-bottom: 1px solid var(--border); }
.tt th { color: var(--muted); font-weight: 500; }
.tt tbody tr:last-child td { border-bottom: 0; }
.tt .name { word-break: break-all; }
.tt .dir { color: var(--muted); word-break: break-all; }
.tt .num { white-space: nowrap; color: var(--muted); }

/* 已丢失的行：整行压暗 + 文件名划掉，扫一眼就能挑出来 */
.tt tr.is-missing td { color: var(--muted); }
.tt tr.is-missing .name { text-decoration: line-through; }

.catalog-error {
  margin: 0; padding: 10px 12px; border-radius: var(--radius-s);
  background: var(--accent-soft); border: 1px solid var(--border);
  color: var(--danger); font-size: var(--fs-13);
}
.catalog-empty { margin: 0; font-size: var(--fs-13); color: var(--muted); line-height: 1.7; }
.catalog-missing-tip { margin-top: calc(var(--space) * 3); line-height: 1.7; }
</style>
