<script setup lang="ts">
import { computed, onMounted, ref, watch } from "vue";
import Sidebar from "./components/Sidebar.vue";
import PosterGrid from "./components/PosterGrid.vue";
import MediaDetail from "./components/MediaDetail.vue";
import SettingsPanel from "./components/SettingsPanel.vue";
import MediaEditDialog from "./components/MediaEditDialog.vue";
import ImportFolderDialog from "./components/ImportFolderDialog.vue";
import LibraryCatalog from "./components/LibraryCatalog.vue";
import { useSettingsStore } from "./stores/settingsStore";
import { useMediaStore } from "./stores/mediaStore";
import { useCategoryStore } from "./stores/categoryStore";
import { usePlaybackStore } from "./stores/playbackStore";
import {
  parseView, tagView, VIEW_ALL, VIEW_CATALOG, VIEW_SETTINGS, type ViewKey,
} from "./types";
import type { MediaItem } from "./api/commands";
import { askConfirm } from "./utils/confirm";
import { useHotkeys } from "./composables/useHotkeys";
import { formatCombo } from "./utils/shortcuts";
import { normalizeSortBy, type SortDir } from "./utils/sort";

const settings = useSettingsStore();
const media = useMediaStore();
const category = useCategoryStore();
const playback = usePlaybackStore();
const activeView = ref<ViewKey>(VIEW_ALL);

/**
 * 资源编辑对话框的目标：
 * `undefined` = 关闭；`null` = 新建；`MediaItem` = 编辑该条目。
 */
const editTarget = ref<MediaItem | null | undefined>(undefined);
/**
 * 「新建」时指定挂到哪个父节点下；`undefined` = 不指定（建在顶层）。
 *
 * 剧集层级视图撤掉后当前没有入口会设置它，但契约保留：以后从封面墙的
 * 聚合分组里「新建子级」时，直接给这个值就能挂上去，不用再改对话框。
 */
const editParentId = ref<number | null | undefined>(undefined);
const showImport = ref(false);

/**
 * 封面墙的句柄：只为「全部展开 / 全部收起」两个按钮服务。
 *
 * 展开态是**封面墙自己的**状态（哪些分组摊开了），所以能力由它 `defineExpose` 出来，
 * 顶栏只负责在合适的时候调一下 —— 不把 `expanded` 提升到 App，避免多一份真相来源。
 */
const wall = ref<{
  expandAll: () => void;
  collapseAll: () => void;
  expandedCount: number;
  groupCount: number;
} | null>(null);

/** 「全部展开」的可点前提：有分组，且不是所有分组都已经展开。 */
const canExpandAll = computed(
  () => (wall.value?.groupCount ?? 0) > 0 && (wall.value?.expandedCount ?? 0) < (wall.value?.groupCount ?? 0),
);

/** 「全部收起」的可点前提：至少有一个分组是展开的。 */
const canCollapseAll = computed(() => (wall.value?.expandedCount ?? 0) > 0);

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
 * 进入多选模式（顶栏「选择」）。
 *
 * 「选择」与「聚合」是**互斥的两个模式**：两条工具条都挂在同一个 `.stickytop`
 * 里，同时开会上下叠出两条 `.selbar`；而封面墙的拖拽语义也只认其中一种
 * （聚合模式才直接可拖）。所以切过去要顺手把另一个关掉。
 */
function enterSelecting(): void {
  media.grouping = false;
  media.selecting = true;
}

/**
 * 进入聚合模式（把封面拖到另一张封面上即合并为分组）。
 *
 * **必须先清空勾选**：勾选集合是「整批一起搬」的判据（见 `PosterGrid` 的
 * `onDragStart`），上一次「选择」模式残留的勾选会让用户拖一张卡却整批搬走
 * 十几张 —— 那是纯粹的惊吓。聚合模式下要勾选，用户会自己重新勾。
 *
 * 还要把 `selecting` 关掉（与 `enterSelecting` 对称）：否则「选择」工具条和
 * 「聚合」提示条同时挂着，而且退出聚合后会莫名其妙停在多选模式。
 */
function enterGrouping(): void {
  media.clearSelection();
  media.selecting = false;
  media.grouping = true;
}

/**
 * 退出聚合模式：同样清空勾选，理由与 `enterGrouping` 一致。
 *
 * 退出后一律回到**普通模式**（`selecting` 也一起关）：聚合期间的勾选属于
 * 聚合本身（提示条上已经显示「已选 N」），不是用户主动进了多选模式，
 * 不该把多选模式"继承"下来。
 */
function exitGrouping(): void {
  media.clearSelection();
  media.grouping = false;
  media.selecting = false;
}

/** 按钮上标出当前快捷键（`未绑定` 就不显示），省得用户去设置页查。 */
function fmt(combo: string): string {
  const s = formatCombo(combo);
  return s === "未绑定" ? "" : s;
}

/** 所选条目正在逐条刮削。 */
const scrapingSelected = ref(false);

/** 「确认所选刮削结果」：逐条刮削当前勾选的条目。 */
async function onScrapeSelected(): Promise<void> {
  if (!media.selected.length || scrapingSelected.value) return;
  scrapingSelected.value = true;
  try {
    await media.scrapeIds([...media.selected]);
  } finally {
    scrapingSelected.value = false;
  }
}

/**
 * 常用选择操作的**可自定义**快捷键。
 *
 * 只在多选模式下生效（`active`）——否则用户在别处按 `Ctrl+A` 会被莫名其妙地
 * 切进多选模式。绑定值每次按键都从设置里现取，改完立即生效，不用重开界面。
 */
useHotkeys({
  active: computed(() => media.selecting && !inSettings.value),
  bindings: () => ({
    selectAll: settings.shortcutSelectAll,
    invert: settings.shortcutInvert,
    confirm: settings.shortcutConfirm,
    clear: settings.shortcutClear,
  }),
  handlers: {
    selectAll: () => media.toggleSelectAll(),
    invert: () => media.invertSelection(),
    confirm: () => void onScrapeSelected(),
    clear: () => exitSelecting(),
  },
});

/**
 * 滑动多选松手时**一次性**提交的结果。
 *
 * 滑动可以在非多选模式下发起（那是它最方便的用法），但选完若还停在普通模式，
 * 勾选态就看不见 —— 所以有选中结果时顺手进入多选模式。
 * 一条都没选到（比如只是长按后原地松手）就保持原样，不打扰。
 */
function onSelectMany(ids: number[]): void {
  media.setSelected(ids);
  // 聚合模式下**不**顺带切进多选模式：那里的勾选是"这批要一起合并"，
  // 提示条上已经显示「已选 N」，再切一次就会两条工具条同时挂着。
  if (ids.length && !media.grouping) media.selecting = true;
}

/* --------------------------- 双击封面直接播放 --------------------------- */

/**
 * 双击封面 = 直接播放（不进详情页）。
 *
 * 播哪一集**不在前端判断**：后端 `play_episode` 不传 `episodeId` 时会依次取
 * 「上次真正播放过的那一集」→「第一个未看集」，电影则播 `media.file_path`。
 * 详情页的「继续播放」走的是同一条命令，两边因此不会给出不同答案。
 *
 * 播放结果由 `playbackStore.play` 统一播报（含「播放 S01E03」这种具体集号）：
 * 双击播上次那一集是**隐式**行为，不说清播了哪集，用户会怀疑又从头放了。
 */
async function onGridPlay(mediaId: number): Promise<void> {
  await playback.playDefault(mediaId);
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
    !(await askConfirm(
      `确定重新识别选中的 ${ids.length} 条？\n\n类型判定变化时会移动记录：` +
        `电影 → 剧集会挂到同名剧集下，已锁定的条目不受影响。`,
      "重新识别",
    ))
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
  const ok = await askConfirm(
    `确定删除选中的 ${n} 条资源？\n\n删除后不可恢复；本地视频文件不会被删除。`,
    "删除资源",
  );
  if (!ok) return;
  await media.removeSelected();
}

/* --------------------------- 封面墙的拖拽聚合 --------------------------- */

/**
 * 封面墙上的拖拽聚合 / 解除分组。
 *
 * 与已撤掉的剧集层级视图走**同一条** `moveNodes`（同一套 `parent_id`，
 * 没有第二套层级结构）。`moveNodes` 只改了层级关系、不重拉列表，
 * 所以这里跟着 `load()` 一次，否则拖完分组界面上看不出任何变化。
 * 失败时不用再提示一次：`moveNodes` 已经把后端的原因写进 toast 了。
 */
async function onGridMove(p: { ids: number[]; parentId: number | null; index: number | null }): Promise<void> {
  const ok = await media.moveNodes(p.ids, p.parentId, p.index);
  if (ok) await media.load();
}

/** 内置视图的标题；分类 / 标签的标题要从数据里取（前端不硬编码栏目名）。 */
const TITLES: Record<string, string> = {
  all: "全部媒体",
  movie: "电影",
  tv: "剧集",
  unknown: "待确认",
  settings: "设置",
  unclassified: "未分类",
  catalog: "资源目录库",
};

const inSettings = computed(() => activeView.value === VIEW_SETTINGS);

/**
 * 当前是否停在「资源目录库」。
 *
 * 它和别的内置入口不一样：**不走 `mediaStore` 的列表查询**（那是"作品"，这里是
 * "磁盘文件"）。所以 `onNav` 切到它时必须提前返回，既不该去拉媒体列表，
 * 也不该把 `keyword` 之类的筛选状态带到媒体列表上。
 */
const inCatalog = computed(() => activeView.value === VIEW_CATALOG);

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
  // 资源目录库有自己的数据源（`resource_assets` 表），不是媒体列表：
  // 切过去时**不查**媒体列表——查了也只是白跑一趟 IPC，还会把上一次的
  // 筛选状态（分类 / 标签 / 关键字）顺手盖掉，回来时列表就不是原来那一份了。
  if (v === VIEW_CATALOG) {
    media.closeDetail();
    return;
  }
  // 分类 / 标签视图由 store 解析成查询条件（id 来自数据库）
  await media.setView(v);
}

/**
 * 切换列表排序方式（下拉）。
 *
 * 排序只影响海报墙的展示顺序：改的是一个 `ref`，`mediaStore.orderedItems`
 * 这个 computed 同帧重算，没有 IPC、没有 loading，也不写任何媒体行。
 */
async function onSortBy(value: string): Promise<void> {
  settings.sortBy = normalizeSortBy(value);
  await persistSort();
}

/** 切换正序 / 倒序。倒序只在各分区内部生效，未识别条目始终排在末尾。 */
async function onToggleSortDir(): Promise<void> {
  const next: SortDir = settings.sortDir === "desc" ? "asc" : "desc";
  settings.sortDir = next;
  await persistSort();
}

/**
 * 排序选择落库：切换即写，不需要「保存」按钮。
 *
 * 两条保护：① 设置还没成功加载过（`loaded === false`）时**不写**，
 * 否则会用默认值覆盖用户已有的配置；② 落库失败**吞掉**——排序是纯前端
 * 展示行为，不该被一次写库失败打断，也不该弹错误框打扰用户。
 */
async function persistSort(): Promise<void> {
  if (!settings.loaded) return;
  try {
    await settings.save();
  } catch {
    // 排序照常生效，下次启动回到上次的持久化值而已
  }
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

/**
 * 列表一变就重拉观看状态（海报角标用）。
 *
 * 挂在 `items` 上而不是在每个「加载」入口里各写一遍：换栏目、扫描完、
 * 导入完、删完都会走 `load()`，挂一处就都覆盖了。空列表也要跑一次，
 * 否则旧角标会留在已经删掉的条目上。
 */
watch(
  () => media.items,
  (list) => {
    void playback.loadMany(list.map((i) => i.id));
  },
);

/**
 * 首屏启动：五个**互不依赖**的 IPC 请求并行发起，不再串行 await。
 *
 * ① 为什么可以并行：这五条之间没有任何数据依赖——分类不依赖设置、媒体列表
 *    不依赖扫描历史、最近播放不依赖列表。原来一条一条 await，等于让每一条都
 *    白等前面所有条目的 IPC 往返；启动耗时应是「最慢的那一条」而不是五条之和。
 *
 * ② 为什么**不能**用裸 Promise.all：串行时某一条 reject 会让它后面的几条
 *    再也不执行；换成裸 Promise.all 同样是一个 reject 整体 reject，
 *    只是失败点从「后面的不跑」变成「一起算失败」，并没有变好。所以这里每条
 *    请求**各自兜底**（allSettled 是同一语义的另一种写法），保证某条后端命令
 *    失败时其余照常加载。各 store 内部本来就把异常写进自己的 error / toast
 *    （见 mediaStore.load / categoryStore.load 的 try/catch），这里只需把失败
 *    吞在自己这一条里，不让它冒泡成 unhandledrejection。
 *
 * ③ settings 的约束：persistSort()（排序落库）在 settings.loaded === false 时
 *    **拒绝写库**，防止拿默认值覆盖用户已有配置。因此启动期间绝不能有任何路径
 *    去写设置。这里并行是安全的：写设置的入口只有用户点排序那两个控件，
 *    而首屏渲染完成到用户点得下去之间，settings.load() 早已完成。
 *    → 以后若在启动流程里加入「自动保存设置」这类动作，必须显式等
 *      settings.load() 之后再发。
 *
 * 顺序上：首屏关键的三条（设置 / 分类 / 媒体列表）先发；扫描历史与最近播放
 * 属于**非首屏关键**（切设置页、看「继续观看」才用得上），放在后面发，
 * 且**不 await 它俩**，不让它们挡住首屏可见。
 */
onMounted(() => {
  /** 每条请求各自兜底：拒绝只终止自己这一条，绝不连累其它请求。 */
  const quiet = (p: Promise<unknown>): Promise<void> => p.then(() => undefined, () => undefined);

  // 首屏关键：三条并行发出，谁都不等谁
  quiet(settings.load());
  quiet(category.load());
  quiet(media.load());

  // 非首屏关键：同样并行发出，但不参与首屏等待
  quiet(media.loadHistory());
  quiet(playback.loadRecent());
});
</script>

<template>
  <div class="layout">
    <Sidebar :model-value="activeView" @update:model-value="onNav" />

    <main class="content">
      <!-- 顶栏与多选工具条装在**同一个** sticky 容器里：
           滚动时两块一起钉在顶部，不会出现「工具条滚走了」或两块互相盖住。 -->
      <div class="stickytop">
        <header class="topbar">
          <h1>{{ heading }}</h1>
        <input
          v-if="!inSettings && !inCatalog"
          v-model="media.keyword"
          class="search"
          placeholder="搜索标题 / 类型 / 年份"
        />
        <select
          v-if="!inSettings && !inCatalog"
          class="tagselect"
          :value="tagFilter"
          @change="onTagPick(($event.target as HTMLSelectElement).value)"
        >
          <option value="">全部标签</option>
          <option v-for="t in category.tags" :key="t.id" :value="t.id">
            🏷 {{ t.name }}（{{ t.usageCount }}）
          </option>
        </select>
        <!-- 排序：展示类控件，紧挨「搜索 / 标签」这两个筛选控件，
             与右侧那排「新建 / 导入 / 扫描 / 刮削」操作按钮区分开。
             复用 .tagselect / .btn-ghost，零新增样式；设置页隐藏。 -->
        <select
          v-if="!inSettings && !inCatalog"
          class="tagselect"
          title="列表排序方式（无法识别为电影 / 剧集的条目始终排在末尾）"
          :value="settings.sortBy"
          @change="onSortBy(($event.target as HTMLSelectElement).value)"
        >
          <option value="name" title="按片名拼音排序；数字在最前，其次英文，再次中文">
            名称（拼音 A→Z）
          </option>
          <option
            value="year"
            title="以条目的上映 / 首播年份为准（库内只存到年份粒度）；没有年份的排在末尾"
          >
            上映 / 首播年份
          </option>
          <option value="added" title="以条目入库（添加）时间为准">添加时间</option>
        </select>
        <button
          v-if="!inSettings && !inCatalog"
          class="btn btn-ghost"
          title="切换正序 / 倒序（未识别的条目始终排在末尾）"
          @click="onToggleSortDir()"
        >
          {{ settings.sortDir === "desc" ? "↓ 倒序" : "↑ 正序" }}
        </button>
        <!-- 一键展开 / 收起所有分组。
             放在排序旁边（都是"改变列表怎么看"的展示操作），与右侧那排
             「新建 / 导入 / 扫描」的数据操作分开。没有分组时置灰，
             避免一个点了什么都不会发生的按钮。 -->
        <button
          v-if="!inSettings && !inCatalog"
          class="btn btn-ghost"
          :disabled="!canExpandAll"
          title="展开所有分组，显示每个聚合下的全部成员"
          @click="wall?.expandAll()"
        >
          ⊞ 全部展开
        </button>
        <button
          v-if="!inSettings && !inCatalog"
          class="btn btn-ghost"
          :disabled="!canCollapseAll"
          title="收起所有分组，只看每个聚合的封面"
          @click="wall?.collapseAll()"
        >
          ⊟ 全部收起
        </button>
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
        <!-- 「选择」/「聚合」：模式开关。开启时按钮**不从快捷栏消失**，
             而是转成「已开启」的高亮态 + 文案改成「退出…」，点一下即关闭。
             原实现用 v-if="!media.selecting" / v-if="!media.grouping" 把按钮整块
             摘掉，用户一旦点进去就再也找不到退出的地方（只能靠工具条最右边那个
             「退出选择」按钮），而且不知道自己是"在模式里"还是"按钮没了"。 -->
        <button
          v-if="!inSettings && !inCatalog"
          class="btn"
          :class="media.selecting ? 'btn-primary' : 'btn-ghost'"
          :aria-pressed="media.selecting"
          :title="media.selecting
            ? '退出选择模式（与聚合模式互斥）'
            : '勾选多条资源，批量删除'"
          @click="media.selecting ? exitSelecting() : enterSelecting()"
        >
          ☑ {{ media.selecting ? "退出选择" : "选择" }}
        </button>
        <button
          v-if="!inSettings && !inCatalog"
          class="btn"
          :class="media.grouping ? 'btn-primary' : 'btn-ghost'"
          :aria-pressed="media.grouping"
          :title="media.grouping
            ? '退出聚合模式'
            : '进入聚合模式：把封面拖到另一张封面上即合并为分组'"
          @click="media.grouping ? exitGrouping() : enterGrouping()"
        >
          🔗 {{ media.grouping ? "退出聚合" : "聚合" }}
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

        <!-- 多选工具条：只在「选择」模式下出现，避免平时占地方 -->
        <div v-if="media.selecting && !inSettings" class="selbar">
          <span class="sel-count">
            已选 {{ media.selected.length }} / {{ media.visibleItems().length }}
          </span>
          <button class="btn btn-ghost" @click="media.toggleSelectAll()">
            {{ allPicked ? "取消全选" : "全选" }}
            <span class="sc">{{ fmt(settings.shortcutSelectAll) }}</span>
          </button>
          <button
            class="btn btn-ghost"
            :disabled="!media.visibleItems().length"
            title="把当前可见条目的选中状态整个翻过来"
            @click="media.invertSelection()"
          >
            反选
            <span class="sc">{{ fmt(settings.shortcutInvert) }}</span>
          </button>
          <button
            class="btn btn-ghost"
            :disabled="!media.selected.length || scrapingSelected"
            title="按当前顺序逐条刮削所选（确认所选刮削结果）"
            @click="onScrapeSelected"
          >
            {{ scrapingSelected ? "刮削中…" : "刮削所选" }}
            <span class="sc">{{ fmt(settings.shortcutConfirm) }}</span>
          </button>
          <button
            class="btn btn-ghost"
            :disabled="!media.selected.length"
            title="把选中的标记为看过（只写数据库，本地文件不动）"
            @click="playback.markBatch([...media.selected], true)"
          >
            ✓ 标记已看
          </button>
          <button
            class="btn btn-ghost"
            :disabled="!media.selected.length"
            title="取消选中条目的看过标记"
            @click="playback.markBatch([...media.selected], false)"
          >
            ↺ 取消已看
          </button>
          <button
            class="btn btn-danger"
            :disabled="!media.selected.length"
            @click="confirmDeleteSelected"
          >
            删除所选
          </button>
          <button class="btn btn-ghost" @click="exitSelecting">
            退出选择
            <span class="sc">{{ fmt(settings.shortcutClear) }}</span>
          </button>
        </div>

        <!-- 聚合模式提示条：复用 `selbar` 的样式，不新造一套 class。
             进来第一件事就是告诉用户"拖到另一张封面上"——这条路径以前藏在
             把手 / Alt 后面，没人找得到。 -->
        <div v-if="media.grouping && !inSettings" class="selbar">
          <span>
            聚合模式：勾选多张后拖动任意一张到目标封面上即可合并；拖到顶部「放回顶层」区域解除分组。
          </span>
          <span class="sel-count">已选 {{ media.selected.length }}</span>
          <button class="btn btn-ghost" @click="exitGrouping()">退出聚合</button>
        </div>
      </div>

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

      <p v-if="media.toast" class="toast">{{ media.toast }}</p>
      <p v-if="playback.toast" class="toast">{{ playback.toast }}</p>
      <p v-if="media.error" class="err">{{ media.error }}</p>

      <MediaDetail
        v-if="media.selectedId && !inSettings"
        @edit="editTarget = $event"
      />
      <SettingsPanel v-else-if="inSettings" />
      <!-- 资源目录库：一份文件系统视角的持久清单（不是媒体条目清单）。
           与封面墙并列而不是嵌在它里面 —— 它读的是 `resource_assets` 表，
           数据源与筛选语义都和 `mediaStore` 不同，混在一起两边都会打架。
           `v-if` 放在封面墙**前面**：目录库与「全部媒体」是同一个 activeView，
           只有 `inCatalog` 为真时才让位，否则封面墙照旧。 -->
      <LibraryCatalog v-else-if="inCatalog" />
      <!-- 兜底分支：非设置页、且没打开详情时一律走封面墙。
           剧集层级不再单独占一个视图——`parent_id` 这层关系仍在库里，
           由封面墙按它聚合出分组，所以撤掉栏目不等于撤掉分组能力。
           `:manual-order-groups` 是组内次序的判据，由这里传入而不是让封面墙自己
           `useSettingsStore()`：它是「纯展示 + 手势」组件，保持无 store 依赖才好
           单独挂载测试。手工排过的组按库内 sort_order，其余跟随顶栏排序。 -->
      <!-- 展开态记忆：`:collapsed-groups` 传入、`@collapsed-change` 回写。
           封面墙挂在 `v-if/v-else` 链上，点开详情页会被**卸载**，组件内的
           `expanded` / `collapsedByUser` 随之销毁；不落库的话返回主页时组件重建，
           「新分组自动展开」的 watch 会把一切重新摊开 —— 就是用户报的
           「回到主页没有留存我最后一次的操作」。
           传值与回写都归 settingsStore，组件保持无 store 依赖（便于单测）。 -->
      <PosterGrid
        v-else
        ref="wall"
        :items="media.visibleItems()"
        :selectable="media.selecting || media.grouping"
        :selected="media.selected"
        :grouping="media.grouping"
        :playback="playback.map"
        :manual-order-groups="settings.manualOrderGroups"
        :collapsed-groups="settings.collapsedGroups"
        @collapsed-change="settings.setCollapsedGroups"
        @open="media.openDetail"
        @play="onGridPlay"
        @select="(p) => media.selectWithMods(p.id, p.mods)"
        @select-many="onSelectMany"
        @move="onGridMove"
      />
    </main>

    <MediaEditDialog
      v-if="editTarget !== undefined"
      :initial="editTarget"
      :parent-id="editParentId"
      @close="editTarget = undefined; editParentId = undefined"
    />
    <ImportFolderDialog v-if="showImport" @close="showImport = false" />
  </div>
</template>

<style scoped>
.layout { display: flex; height: 100vh; }
.content { flex: 1; overflow-y: auto; padding: calc(var(--space) * 5) calc(var(--space) * 7) calc(var(--space) * 10); }

/*
 * 顶栏 + 多选工具条装在同一个 sticky 容器里。
 *
 * 为什么不让它们各自 sticky：两块都 `top: 0` 会叠在一起互相盖住；
 * 给工具条算一个 `top: <顶栏高度>` 又会在顶栏折行时错位。装进同一个容器，
 * 高度是自算的，怎么折行都对。
 */
.stickytop {
  position: sticky; top: 0; z-index: 6;
  display: flex; flex-direction: column; gap: calc(var(--space) * 3);
  padding: calc(var(--space) * 3) 0; margin-bottom: calc(var(--space) * 5);
  background: var(--bg);
}
.topbar {
  display: flex; align-items: center; gap: 10px; flex-wrap: wrap;
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
  display: flex; align-items: center; gap: 10px; flex-wrap: wrap; margin: 0;
  padding: calc(var(--space) * 2) calc(var(--space) * 3); border-radius: var(--radius-s);
  background: var(--surface); border: 1px solid var(--border);
}
.sel-count { flex: 1; font-size: var(--fs-13); color: var(--text-2); }
/* 按钮上标出当前快捷键；未绑定时 `fmt` 返回空串，这个 span 也就塌成 0 宽 */
.selbar .sc {
  margin-left: 6px; padding: 1px 5px; border-radius: 3px;
  font-size: var(--fs-12); color: var(--muted);
  background: var(--surface-2); border: 1px solid var(--border);
}

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
