<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from "vue";
import PosterCard from "./PosterCard.vue";
import { useSwipeSelect } from "../composables/useSwipeSelect";
import { buildTree, canDrop, flattenTree, type FlatRow } from "../utils/tree";
// 起手阈值与触摸长按时长**复用滑动多选那一套**：两个手势的"按下 → 移动"
// 判定必须一致，否则同一段位移在两种模式下会有不同的触发时机。
import { DRAG_THRESHOLD_PX, LONG_PRESS_MS } from "../utils/dragSelect";
import type { MediaItem, MediaPlayback, MediaTreeNode } from "../api/commands";

const props = defineProps<{
  items: MediaItem[];
  /** 多选模式（顶栏「选择」开关）：勾选态由父级持有，这里只转发。 */
  selectable?: boolean;
  selected?: number[];
  /**
   * 是否**显式声明**"现在处于聚合模式"（顶栏「聚合」开关）。
   *
   * ⚠️ 聚合早已不依赖它 —— 默认状态下「按住海报拖到另一张海报上」本来就能用
   * （见下面「指针驱动的拖放」一节）。它现在只影响两件外观：把手常显、
   * 分组提示条。拖动**从来不需要**先打开任何模式。
   */
  grouping?: boolean;
  /** 观看状态（media_id → 摘要）；不传就不画角标。 */
  playback?: Record<number, MediaPlayback> | null;
  /**
   * 已被用户**手工排过序**的聚合 id 列表（`settingsStore.manualOrderGroups`）。
   *
   * 由 `App.vue` 传进来而不是在本组件里 `useSettingsStore()`：封面墙是纯展示 +
   * 手势组件（依赖只有 `utils/tree` 与两个 composable），保持「无 store 依赖」
   * 才能单独挂载测试、也才不会在这里埋下第二个真相来源。
   *
   * 不传 = 一组都没手工排过 → 组内一律按列表下标（顶栏排序算出来的先后）。
   */
  manualOrderGroups?: number[];
  /**
   * 被用户**亲手折叠**过的分组 id（`settingsStore.collapsedGroups`）。
   *
   * 为什么需要它：本组件挂在 `App.vue` 的 `v-if/v-else` 链上，点开详情页会被
   * **卸载**，下面那两个 ref 随之销毁。没有这份外部记忆，返回时组件重建、
   * 「新分组自动展开」的 watch 会把用户亲手收起的分组又摊开 —— 正是用户报的
   * 「回到主页没有留存我最后一次的操作」。
   *
   * 与 `manualOrderGroups` 同样是 prop 而非 `useSettingsStore()`：封面墙要能
   * 单独 `mount(Grid, { props })` 测试，不引入 store 依赖。
   * 不传 = 没有任何分组被折叠过 → 全部按默认展开。
   */
  collapsedGroups?: number[];
}>();
const emit = defineEmits<{
  (e: "open", id: number): void;
  /**
   * 双击卡片 = 直接播放。
   *
   * 本组件**不决定播哪一集**：那条规则（上次播放过的那一集 > 第一个未看集）
   * 要读库，放在后端 `play_episode` 里，详情页的「继续播放」也走同一套。
   * 这里只把「用户双击了谁」如实交上去。
   */
  (e: "play", id: number): void;
  /**
   * 多选模式下的点击（携带修饰键）。
   *
   * **从 `id: number` 换成对象是有意的**：Shift 区间与 Ctrl 单张切换都取决于
   * 修饰键，而 Shift 的语义还需要"上一次点的哪张"（anchor）—— 那是**父级**的
   * 状态（勾选集合本来就归 `mediaStore` 持有），所以这里只如实上报
   * 「点了谁、按着什么键」，由父级用 `utils/selection.ts` 算出新集合。
   */
  (e: "select", payload: { id: number; mods: { shift?: boolean; ctrl?: boolean; meta?: boolean } }): void;
  /** 滑动多选：松手时**一次性**提交整批结果（不逐条 select）。 */
  (e: "select-many", ids: number[]): void;
  /**
   * 拖拽聚合 / 解除分组：`parentId` 为 `null` 表示挂回顶层。
   *
   * 与树视图走**同一条** `moveNodes`（同一套 `parent_id`），这里只负责把
   * 落点算出来交上去。
   */
  (e: "move", payload: { ids: number[]; parentId: number | null; index: number | null }): void;
  /**
   * 折叠集合变化（供父级落库，跨卸载留存）。
   *
   * 上报的是**完整**的折叠名单而不是某一项：组件这边真正的真相来源是 `expanded`
   * 这个 Set（点一下、一键展开、一键收起都会动它），把结果整体交上去最不容易
   * 与父级那份状态失配，也天然处理了「一键展开后折叠名单应清空」。
   */
  (e: "collapsed-change", ids: number[]): void;
}>();

/* ---------------- 滑动多选 ---------------- */
/* 在卡片上按下并拖动即可连续勾选；触摸下需要先长按（否则会抢掉页面滚动手势）。
   细节与边界约定都在 `composables/useSwipeSelect.ts` 与 `utils/dragSelect.ts`。 */

const grid = ref<HTMLElement | null>(null);

/* ---------------- 分组（层级）---------------- */
/* 分组就是条目上的 `parent_id` 这一层关系（专门的层级视图 `SeriesTree` 已经删掉，
   没有第二套结构），封面墙只是把同一份关系用"文件夹"的方式画出来：
   父卡片带展开 / 折叠，展开后子项紧跟其后，中间用整行的分组条隔开。 */

/** 展开的节点 id。用 Set 但每次整体替换，保证 Vue 能感知变化。 */
const expanded = ref<Set<number>>(new Set());
/**
 * 用户手动折叠过的节点：新出现的分组**不再**自动展开它。
 *
 * 初值直接取 `props.collapsedGroups`（上次离开时的折叠名单）。为什么必须在**这里**
 * 就位、而不是等下面那个 watch 去补：紧跟着的「新分组自动展开」watch 带
 * `immediate: true`，setup 期间就会跑一遍；那时若这份集合还是空的，它会把用户
 * 亲手收起来的分组统统摊开 —— 再补就晚了（展开已经发生，没有"记得他收过"的证据）。
 */
const collapsedByUser = ref<Set<number>>(new Set(props.collapsedGroups ?? []));

/**
 * 用户是否已经**亲手**动过展开态。
 *
 * 用来解决一个真实的时序竞争：设置是**异步**加载的（`App.onMounted` 里的 IPC），
 * 封面墙很可能先挂载、`collapsedGroups` 后到。若无条件接受后到的 prop，
 * 用户在设置到达前做的那次折叠会被一个更旧的库值覆盖掉（表现为"刚收起来又开了"）。
 * 所以一旦用户动过手，就以本地为准，不再接受外部回填。
 */
let userTouched = false;

/**
 * 渲染时怎么给每一行合成 `buildTree` 要的 `sort_order`。
 *
 * 规则只有一条，**按父级**判断：
 * - 某个父底下的这一组**确实被用户手工排过**（父 id 在 `manualOrderGroups` 里）
 *   → 用它自己的 `it.sort_order`（缺值退回列表下标）：那是详情页 ↑↓ 写进库的
 *   次序，也是后端 `move_nodes_sync` 的 `(sort_order, id)` 同一把尺子；
 * - 其余一切（**含顶层**）→ 用**列表下标** `i`：列表先后就是顶栏排序设置算出来的
 *   先后，组内次序于是自动跟随顶栏排序。
 *
 * 为什么不给顶层第二种可能：顶层本来就不存在「手工次序」这个概念（↑↓ 只作用于
 * 某个聚合**内部**的成员），而实测真实库里 `sort_order` 常是全局递增的流水号
 * （0,3,4,5…63）—— 一旦把顶层也交给 DB 值，整面墙的顺序就恒等于 `sort_order`，
 * 用户选的「名称 / 年份 / 添加时间」被整个覆盖，表现就是「排序功能失效」。
 *
 * ⚠️ 判据换成显式标记是**修一个真实缺陷**：旧代码用「这一桶里存在 `sort_order > 0`」
 * 当「被手工排过」的启发式，但任何一次 `move_nodes_sync`（含在墙上拖动聚合）都会把
 * 目标同级整组重编号成 `0..n-1`，实测真实库里连从没排过的组都是 0,1,2,…
 * （14135 = 0..29、14150 = 0,2,3,4,5,18,19,20）—— 「存在 > 0」恒为真，
 * 于是组内次序永远跟着库里那份历史重编号走，顶栏排序根本不生效。
 *
 * 保留 `?? i` 这条兜底：`sort_order` 在 `MediaItem` 上是可选的（老后端 / 测试桩
 * 可能整批不给），缺值时退回列表下标，稳定且不抛错。
 */
const rowsSortOrder = computed<number[]>(() => {
  const manual = new Set(props.manualOrderGroups ?? []);
  return props.items.map((it, i) => {
    const parent = it.parent_id ?? null;
    return parent !== null && manual.has(parent) ? (it.sort_order ?? i) : i;
  });
});

/**
 * 把 `MediaItem` 补成 `MediaTreeNode`。
 *
 * `utils/tree` 的建树与放置校验只认这个形状（树视图用的是同一套类型），
 * 与其在封面墙里再写一份建树逻辑，不如补字段复用——两处规则必须一致，
 * 否则「拖进去」在墙上看着能放、后端却拒绝。
 */
const nodes = computed<MediaTreeNode[]>(() => {
  const byId = new Map<number, MediaItem>();
  for (const it of props.items) byId.set(it.id, it);
  /*
   * ⚠️ 这只是**兜底**：层级以 `it.depth`（后端用**全表**边表算出来的）为准。
   * 自己数会偏小 —— `byId` 只有当前筛选到的条目，父节点被筛掉时循环提前
   * 结束，等于把"父不在列表里"当成走到了根。而 `utils/tree.ts::canDrop` 拿
   * 这个深度算层数上限，偏小就会**放行后端必拒的放置**（界面画「可放置」、
   * 请求也发出去了，后端回「层级最多 4 层」）—— 方向是单向的：只会多放行，
   * 不会反过来拦合法的。只有老后端 / 测试桩没给 `depth` 时才走这里。
   */
  const depthOf = (it: MediaItem): number => {
    let d = 0;
    let cur = it.parent_id ?? null;
    const seen = new Set<number>([it.id]);
    while (cur !== null && cur !== undefined && !seen.has(cur) && byId.has(cur)) {
      seen.add(cur); // 数据成环时兜住，别死循环
      d += 1;
      cur = byId.get(cur)?.parent_id ?? null;
    }
    return d;
  };
  return props.items.map((it, i) => ({
    id: it.id,
    parent_id: it.parent_id ?? null,
    // 合成规则见 `rowsSortOrder`：手工排过的组用库内 `sort_order`，其余（含顶层）
    // 一律用列表下标 —— 也就是顶栏排序设置算出来的先后。
    sort_order: rowsSortOrder.value[i],
    type: it.type,
    title: it.title,
    display_title: it.display_title,
    year: it.year ?? null,
    poster_path: it.poster_path ?? null,
    episode_count: it.episode_count ?? 0,
    child_count: it.child_count ?? 0,
    // 后端给的真实层级优先（全表父链，与放置校验同一套规则）；缺失才自数
    depth: it.depth ?? depthOf(it),
    source: it.source ?? "scan",
    locked: it.locked ?? false,
  }));
});

/** 摊平成可渲染的行；折叠的节点其子级不出现在结果里。 */
const rows = computed<FlatRow[]>(() => flattenTree(buildTree(nodes.value), expanded.value));

/**
 * 「这一行之后要画一条闭合分割线」的行 id 集合。
 *
 * ## 为什么必须有它（这是一个真实的视觉 bug，已用真实浏览器量过）
 * 网格是 `repeat(auto-fill, minmax(min(--card-min,100%),1fr))` 的多列布局，
 * `.cell` 与 `.groupbar` 都只是普通的 grid item。于是「展开 A」之后，
 * **A 的子项会和紧跟其后的顶层项目 B 挤在同一行**里：
 *
 * ```
 * cell  id=1   y=0    x=0     ← 顶层 A（展开）
 * groupbar     y=302          ← A 的「子项」提示条（跨整行）
 * cell  id=11  y=334  x=0     ← A 的子项
 * cell  id=12  y=334  x=185   ← A 的子项
 * cell  id=2   y=334  x=370   ← ⚠️ 顶层 B 混进了 A 的子项那一行
 * ```
 *
 * 用户看到的就是「展开一个项目，后面紧紧跟着的那个项目**也一起被展开了**」。
 * 但 `expanded` 集合其实**完全正确**（点 A 只加 A —— 有单测钉住）——
 * 问题出在布局：B 被画在 A 的子项旁边，看上去就属于 A。
 *
 * ## 修法
 * 在每个「已展开分组」的**最后一个后代**之后插一个跨整行的元素：
 * ① 它自己画成一条分割线（用户要的「展开之后用一条分割线和其他项目分开」）；
 * ② 跨整行 = 强制换行 —— 后面的顶层项目从此**一定另起一行**，
 *    不会被误读成这个分组的子项。
 *
 * 只对**最外层**（`depth === 0`）的展开分组算：嵌套分组各自的子项都落在
 * 同一个外层分组里，再画多条线只会把墙切碎。
 */
const closingGroupAfter = computed<Set<number>>(() => {
  const out = new Set<number>();
  const rs = rows.value;
  for (let i = 0; i < rs.length; i += 1) {
    const r = rs[i];
    if (r.depth !== 0 || !r.expanded || !r.hasChildren) continue;
    // 这个顶层分组的后代就是紧随其后、depth > 0 的那一段；记下最后一个
    let last = -1;
    for (let j = i + 1; j < rs.length && rs[j].depth > 0; j += 1) last = j;
    if (last >= 0) out.add(rs[last].node.id);
  }
  return out;
});

const swipe = useSwipeSelect({
  root: grid,
  selected: () => props.selected ?? [],
  // 顺序取**渲染顺序**（摊平后的行序）：DOM 里卡片就是这个顺序，
  // 滑动的"补齐中间被跳过的项"必须按看得见的顺序算，否则会补错一批。
  order: () => rows.value.map((r) => r.node.id),
  commit: (ids) => emit("select-many", ids),
  itemSelector: "[data-id]",
  // ⚠️ 这里**不能**把 `button` 写进 blockSelector：卡片根元素本身就是 button，
  // 写了就永远起不来拖动。真正要拦的是浮在卡片上的那几个控件——
  // 它们是按钮，按上去是"点按钮"，不是"起手拖动"。
  blockSelector: "input, a, .ops",
});

/** id → 原始条目（行里只留了树需要的那几个字段，画卡片还得用完整的）。 */
const itemById = computed<Map<number, MediaItem>>(
  () => new Map(props.items.map((it) => [it.id, it])),
);

/**
 * 取某一行的原始条目。
 *
 * 理论上一定取得到（行就是从 `items` 里摊出来的）；真缺了也不能把
 * `undefined` 塞给 `PosterCard`，所以兜一条空条目。
 */
function itemOf(id: number): MediaItem {
  const it = itemById.value.get(id);
  if (it) return it;
  return {
    id,
    type: "unknown",
    title: "",
    year: null,
    tmdb_id: null,
    poster_path: null,
    backdrop_path: null,
    overview: null,
    vote_average: null,
    genre: null,
    file_path: null,
    file_name: null,
    episode_count: 0,
    added_at: 0,
    display_title: "",
  };
}

/**
 * 新出现的分组自动展开。
 *
 * 用户刚把两部片拖成一组，转头发现墙上只剩一张卡——他会以为片子没了。
 * 手动折叠过的（`collapsedByUser`）不再自动展开，否则「折叠」这个动作没意义。
 */
watch(
  nodes,
  (list) => {
    const next = new Set(expanded.value);
    let changed = false;
    for (const n of list) {
      if (n.child_count > 0 && !collapsedByUser.value.has(n.id) && !next.has(n.id)) {
        next.add(n.id);
        changed = true;
      }
    }
    if (changed) expanded.value = next;
  },
  { immediate: true },
);

function toggleExpand(id: number): void {
  const next = new Set(expanded.value);
  const byUser = new Set(collapsedByUser.value);
  if (next.has(id)) {
    next.delete(id);
    byUser.add(id);
  } else {
    next.add(id);
    byUser.delete(id);
  }
  expanded.value = next;
  collapsedByUser.value = byUser;
  userTouched = true;
  reportCollapsed();
}

/**
 * 把「现在哪些分组是折叠的」上报给父级落库。
 *
 * 口径：当前**有子级**且**不在展开集合**里的分组。
 * ⚠️ 必须用 `groups` 过滤而不是直接倒 `expanded` 的补集 —— 折叠名单会跨会话留着，
 * 一个分组如果**已经被删掉**（或本次筛选下不可见），它不该继续占着名单；
 * 只上报"当下真实存在的折叠分组"，名单才不会越积越脏。
 */
function reportCollapsed(): void {
  emit("collapsed-change", groups.value.filter((id) => !expanded.value.has(id)));
}

/** 有子级的节点 id（「全部展开」的作用范围，也是展开态按钮的可见前提）。 */
const groups = computed(() => nodes.value.filter((n) => n.child_count > 0).map((n) => n.id));

/**
 * 外部折叠名单**后到**时回填（设置是异步加载的，封面墙常先挂载）。
 *
 * 只在用户还没动过手（`!userTouched`）时接受回填：用户已经亲手调过展开态，
 * 再来一个更旧/更晚的库值覆盖他，就会表现成"刚收起来又自己开了"。
 *
 * 回填后要把 `expanded` 对齐一次 —— 收起名单里的分组若是当展开着，得真的合上。
 */
watch(
  () => props.collapsedGroups,
  (ids) => {
    if (userTouched) return;
    const restored = new Set(ids ?? []);
    collapsedByUser.value = restored;
    const next = new Set(expanded.value);
    let changed = false;
    // 名单里说"收着"的，若当前是展开的，就合上
    for (const id of restored) {
      if (next.delete(id)) changed = true;
    }
    // 名单里没有、当前有子级、又还没展开的 → 按默认展开（与自动展开同一语义）
    for (const id of groups.value) {
      if (!restored.has(id) && !next.has(id)) {
        next.add(id);
        changed = true;
      }
    }
    if (changed) expanded.value = next;
  },
);

/** 当前处于展开态的分组数（父级据此决定「全部展开」要不要置灰）。 */
const expandedCount = computed(
  () => groups.value.filter((id) => expanded.value.has(id)).length,
);

/**
 * 一键展开所有分组。
 *
 * ⚠️ 必须**同时清空 `collapsedByUser`**：那个集合是"用户手动折叠过、别再自动展开"
 * 的记忆。一键展开是用户的明确指令，若留着它，下一次列表刷新（`items` 一变就触发
 * 那段 watch）会把这些分组又收回去 —— 用户会觉得按钮"点了没用"。
 */
function expandAll(): void {
  expanded.value = new Set(groups.value);
  collapsedByUser.value = new Set();
  userTouched = true;
  reportCollapsed();
}

/**
 * 一键收起所有分组。
 *
 * 这里**要**把每一个分组记进 `collapsedByUser`：收起之后列表刷新会重新触发
 * 自动展开那段 watch，不记的话刚收起的分组马上又自己弹开。
 */
function collapseAll(): void {
  expanded.value = new Set();
  collapsedByUser.value = new Set(groups.value);
  userTouched = true;
  reportCollapsed();
}

/*
 * 把展开 / 收起能力交给父级（顶栏那两个按钮在 App.vue 里）。
 *
 * 为什么不把按钮画在封面墙上：封面墙还会出现在「详情页返回」「空库」等状态，
 * 而这两个按钮属于**列表工具条**，只该跟其它列表操作（排序 / 筛选）放在一起。
 */
defineExpose({ expandAll, collapseAll, expandedCount, groupCount: computed(() => groups.value.length) });

/**
 * 某一项当前是否呈现为"已选"。
 *
 * 拖动期间以**预览集合**为准（实时高亮，且反向划过会立刻取消）；
 * 不拖动时以外部传入的 `selected` 为准 —— 原来的点击 / 单选逻辑完全不变。
 */
function picked(id: number): boolean {
  if (swipe.dragging.value) return swipe.preview.value.has(id);
  return props.selected?.includes(id) ?? false;
}

/**
 * 进度文本：`3/25` 这种。电影总集数为 0，返回空串（角标退回「看过」）。
 *
 * 全看完时也不隐藏 —— 角标变成 `25/25`，用户一眼能看出「这部追完了」。
 */
function progressOf(id: number): string {
  const p = props.playback?.[id];
  if (!p || p.totalEpisodes <= 0) return "";
  return `${p.watchedEpisodes}/${p.totalEpisodes}`;
}

function watchedOf(id: number): boolean {
  return props.playback?.[id]?.watched ?? false;
}

/* ---------------- 剧集聚合：指针驱动的拖放 ---------------- */
/*
 * 为什么**不用** HTML5 原生 DnD（这是本次修复的结论，有真实浏览器证据）：
 *
 * 1. 原生拖拽必须由一个 `draggable` 元素起手，而 `draggable` 只能在
 *    `pointerdown` 里决定（浏览器是在随后的 mousemove 里才认拖拽源的）。
 *    于是「卡片恒可拖」与「按住划过 = 滑动多选」在**同一个手势**上直接冲突，
 *    老实现只能二选一 —— 默认状态下卡片被设成不可拖，用户看到的
 *    就是"拖不动"；要让拖拽能用就必须先进顶栏那个「聚合」模式。
 * 2. 原生拖拽一旦启动，浏览器就掐断 pointer 事件流并补一次 `pointercancel`，
 *    滑动多选当场失灵。这两个能力在鼠标下天然互斥，靠阈值猜不出来。
 * 3. **光标完全由浏览器判**：`<img>` 天生可拖、标题文字可选，按住它们拖起来的
 *    是图片 / 文本拖拽 —— 载荷是图片地址或一段文字，鼠标上画的正是那个"禁止"
 *    图标，放下什么都不发生。它跟"目标拒绝接收"长得一模一样。
 * 4. 落点反馈也被浏览器接管：非法落点只能画禁止光标，给不出"为什么不行"。
 *
 * 改成指针驱动的自绘拖拽之后：
 * - 起手判定归自己（`DRAG_THRESHOLD_PX`），**不需要任何 draggable 元素**
 *   （模板上留一个都会让浏览器重新有机会接管手势、补一次 `pointercancel`）；
 * - 落点用 `elementFromPoint` 命中，光标 / 高亮 / 画面上的原因全由界面给，
 *   非法落点照旧给文字原因（`canDrop` 的 `reason`），不再是"目标不可接收"；
 * - 触摸也一并受益：HTML5 原生拖拽在触摸下压根起不来，指针驱动没有这个问题
 *   （触摸走"长按起拖"，与滑动多选对触摸的同一条约定）。
 *
 * ⚠️ 手势的分配见下面 `selectMode` 那一段：「滑动多选」与「拖动聚合」是
 * **同一个手势**（左键按下 → 移动 → 松手），几何上没有判据能分开它们 ——
 * 只能靠一个明确的用户意图，也就是顶栏的「选择」模式。
 */

/** 「放下但被拒」那条提示存活多久（毫秒）。到点必须自己消失，不能常驻。 */
const REJECT_HINT_MS = 3000;

/** 正在拖拽的 id 集合；`null` = 没在拖。跨出原卡片之后才非空。 */
const dragIds = ref<number[] | null>(null);
/**
 * 手势起手阶段（按下之后、还没跨出原卡片）的候选。
 *
 * 这一阶段**完全按滑动多选走**（逐字保留上一轮的行为），只有指针明确跨到
 * 别的卡片时才改判成拖拽聚合并把它交给 `dragIds`。
 */
const dragArm = ref<number[] | null>(null);
/** 这一次手势的起点卡片 id（判断"跨出去了没有"）。 */
const dragOrigin = ref<number | null>(null);
/** 拖拽中的指针位置（跟手预览用）。 */
const pointer = ref({ x: 0, y: 0 });
/** 当前悬停的目标卡片 id；`null` = 不在任何卡片上。 */
const overId = ref<number | null>(null);
/** `overId` 处的判定原因（空串 = 可放）。 */
const overReasonText = ref("");
/** 指针是否在「放回顶层」区上面（拖到这里 = 解除分组）。 */
const overRoot = ref(false);
/**
 * 「刚放下的那一次被拒」的原因。
 *
 * ⚠️ 只在**确实发生了非法放置之后**才非空，且**必须自动消失**：
 * 放下之后这一轮就结束了，原因如果跟着一直留着，用户会看到一条没有对应
 * 拖拽、也没有任何禁用状态的"凭空禁止提示"。所以它活一小会儿再自己收掉。
 */
const rejectReason = ref("");
/**
 * 被拒的那一次针对哪个目标（`null` = 「放回顶层」那条）。
 *
 * 与 `rejectReason` **同生共死**：红框必须跟红字一起过期。
 */
const rejectId = ref<number | null>(null);
let rejectTimer = 0;

/** 起手位置与触摸长按计时器（见下面的手势仲裁）。 */
let pressX = 0;
let pressY = 0;
let touchTimer = 0;

function clearReject(): void {
  if (rejectTimer) {
    window.clearTimeout(rejectTimer);
    rejectTimer = 0;
  }
  rejectReason.value = "";
  rejectId.value = null;
}

/**
 * 界面上那条禁止提示的文字。
 *
 * 显示条件（两者都是"确实处于不可用状态"）：
 * 1. **拖拽进行中**且当前悬停的目标非法 —— 实时反馈，挪到合法目标上立刻收掉；
 * 2. **刚放下**的那一次被拒 —— 短暂告知原因，到点自动消失。
 *
 * 除此之外一律为空：没在拖、也没有刚被拒的放置时，绝不渲染禁止态。
 */
const overReason = computed(() => {
  if (!dragIds.value) return rejectReason.value;
  return overReasonText.value;
});

/**
 * 指针下面的元素。
 *
 * 兜底是必须的：`document.elementFromPoint` 依赖布局引擎，jsdom 没有它 ——
 * 取不到就返回 `null`，调用方退化（单测里靠派发事件流走，不依赖几何）。
 */
function elementAtPoint(x: number, y: number): Element | null {
  if (typeof document.elementFromPoint !== "function") return null;
  try {
    return document.elementFromPoint(x, y);
  } catch {
    return null;
  }
}

/** 命中指针下面的格子；不在任何卡片上返回 `null`。 */
function cellAt(x: number, y: number): { id: number; el: HTMLElement } | null {
  const el = elementAtPoint(x, y);
  const cell = el?.closest?.(".cell") as HTMLElement | null;
  if (!cell) return null;
  const id = Number(cell.dataset.id);
  if (!Number.isFinite(id)) return null;
  return { id, el: cell };
}

/**
 * 本组件自己的 `.wall`。
 *
 * 落点区与跟手预览都在它里面查，而不是 `document.querySelector` ——
 * 一页上可能同时挂着不止一块网格（详情页的成员列表也是复用同一套逻辑），
 * 用全局查询会命中别人的那一块。
 */
function wallEl(): HTMLElement | null {
  return grid.value?.closest(".wall") ?? null;
}

/**
 * 「放回顶层」区：拖拽期间才渲染，是 `.wall` 里的第一个子元素。
 *
 * ⚠️ 它是 `position: fixed` 的一条顶部横幅 —— **不能用静态布局**：
 * 它在拖拽开始时插入，静态布局会把整个网格往下顶，指针下的卡片当场移走，
 * 落点判定跟着漂。固定定位不参与布局，插进来也不会挪动任何一张卡。
 */
function rootZoneEl(): HTMLElement | null {
  return wallEl()?.querySelector(".rootzone") as HTMLElement | null;
}

/** 根据指针位置更新落点状态（高亮 / 原因 / 是否在「放回顶层」区上）。 */
function updateDropTarget(x: number, y: number): void {
  const ids = dragIds.value;
  if (!ids) return;

  const rz = rootZoneEl();
  if (rz) {
    const r = rz.getBoundingClientRect();
    if (r.height > 0 && y <= r.bottom && x >= r.left && x <= r.right) {
      overId.value = null;
      overReasonText.value = "";
      overRoot.value = true;
      return;
    }
  }
  overRoot.value = false;

  const hit = cellAt(x, y);
  if (!hit) {
    overId.value = null;
    overReasonText.value = "";
    return;
  }
  // 只写悬停态：它是实时的，挪到合法目标上就该立刻把红字收掉。
  // 这里**不写** `rejectReason`（那个只在"真的放下去却被拒"时才出现）。
  const check = canDrop(nodes.value, ids, hit.id, "inside");
  overId.value = hit.id;
  overReasonText.value = check.reason;
}

/** 记一次"放下但被拒"的原因，并在 `REJECT_HINT_MS` 后自动收掉。 */
function flashReject(reason: string, id: number | null): void {
  clearReject();
  rejectReason.value = reason;
  rejectId.value = id;
  rejectTimer = window.setTimeout(clearReject, REJECT_HINT_MS);
}

/**
 * 跟手预览的文案：单个显示片名，多个显示条数。
 *
 * 指针驱动的拖拽**没有**浏览器自带的拖影，这一块必须自己画 ——
 * 否则用户按住一拖，屏幕上除了落点高亮之外没有任何"东西正被搬着"的反馈。
 */
const dragLabel = computed(() => {
  const ids = dragIds.value;
  if (!ids || !ids.length) return "";
  if (ids.length > 1) return `${ids.length} 项`;
  return itemOf(ids[0]).display_title || `#${ids[0]}`;
});

/* ---------------- 手势：谁在什么时候拿到这一次按下 ---------------- */

/**
 * 「选择」模式（顶栏「选择」开关）：**滑动多选只在它里面生效**。
 *
 * ⚠️ 这是本次修复唯一一处对外的行为收敛，必须说清楚为什么：
 *
 * 「按住卡片横向划过 = 连续勾选」与「按住卡片拖到另一张上 = 聚合」是**同一个
 * 手势**（都是左键按下 → 移动 → 松手），没有任何几何判据能把两者分开 ——
 * 实测把"跨到别的卡片就改判成拖拽"接上之后，滑动多选的整条链路（划过
 * 卡片 1→2→3）恰好就是"跨了好几次卡片"，会被当场改判成拖拽。
 * 所以二者只能靠**一个明确的用户意图**来分辨，而那个意图界面里本来就有：
 * 顶栏的「选择」。
 *
 * 交给指针驱动的自绘拖拽之后，这一次的分配是：
 * - **默认 / 聚合模式**：拖动 = 聚合（用户要的"不用先进任何模式"）；
 * - **选择模式**：拖动 = 连续勾选（上一轮交付的滑动多选，逻辑未改）。
 *
 * 代价说清楚：想在默认模式下"拖过一串卡片来勾选"就必须先进「选择」。
 * 这是必要的取舍 —— 同一个手势不可能既是"搬走"又是"勾选"，
 * 老实现选的是"勾选"，于是默认模式永远拖不动（用户报的那条）。
 *
 * 这也是 Windows 资源管理器的模型：在条目上按住一拖就是"搬动"，
 * 而"框选一批"是一个明确进入的选择态。
 */
const selectMode = computed(() => !!props.selectable && !props.grouping);

/** 这一次按下在网格上是否有效（非选择模式下驱动拖拽）。 */
function beginPress(ev: PointerEvent): void {
  const target = ev.target as Element | null;
  const cell = target?.closest?.(".cell") as HTMLElement | null;
  // 卡片上的浮层控件（展开 / 折叠 / 解除分组）是按钮，不是拖拽起手。
  if (!cell || target?.closest?.(".ops")) return;
  const id = Number(cell.dataset.id);
  if (!Number.isFinite(id)) return;
  // 拖的是**已勾选**的卡片 = 整批一起搬；否则只搬这一张。
  // 判据只看 `selected`（勾选集合由父级持有），不看 `selectable`：
  // 聚合不必先进「选择」模式，要求先勾选等于多加一道隐藏步骤。
  // 🔴 **按住 Shift / Ctrl / Cmd = 用户明确在做"多选"，这一下绝不起拖。**
  //
  // 不然就是用户报的那个 bug：**在聚合模式里用 Shift 点选，会把组里的电影拖出来**。
  // 原因是聚合模式下 `selectable = selecting || grouping` 为 true（点击要能选中），
  // 而 `selectMode = selectable && !grouping` 又是 false（拖动 = 搬运）。
  // 两个条件叠在一起：Shift 点选会把卡片放进 `selected`，接着只要手一动超过
  // `DRAG_THRESHOLD_PX`（6px，鼠标下极易触发）就被判成拖拽，松手落到哪张卡上
  // 就整批 `move` 过去 —— 表现就是"用 Shift 多选时，聚合里的电影被拖出来了"。
  //
  // 这是 Windows 资源管理器的模型：**修饰键 = 选择，不按修饰键的拖动 = 搬动**。
  // 选择模式的滑动多选（按住划过连续勾选）不受影响 —— 那条路径在 `selectMode`
  // 分支里就走掉了，根本到不了这里。
  //
  // 触摸没有修饰键，不受影响（它仍走长按起拖）。
  if (ev.shiftKey || ev.ctrlKey || ev.metaKey) {
    dragArm.value = null;
    dragOrigin.value = null;
    return;
  }

  const chosen = props.selected ?? [];
  const batch = chosen.length > 1 && chosen.includes(id);
  dragArm.value = batch ? [...chosen] : [id];
  dragOrigin.value = id;
  pressX = ev.clientX;
  pressY = ev.clientY;
  pointer.value = { x: ev.clientX, y: ev.clientY };
  clearTouchTimer();
  if (ev.pointerType === "touch") {
    // 触摸：先长按，指针才归拖拽 —— 否则一按一动就被判成拖动，
    // 页面再也滚不动（与滑动多选对触摸的同一条约定）。
    touchTimer = window.setTimeout(() => {
      touchTimer = 0;
      startDrag(ev.clientX, ev.clientY);
    }, LONG_PRESS_MS);
  }
}

/** 按下这一刻的候选正式变成一次拖拽。 */
function startDrag(x: number, y: number): void {
  if (dragIds.value || !dragArm.value) return;
  dragIds.value = dragArm.value;
  dragArm.value = null;
  updateDropTarget(x, y);
}

function clearTouchTimer(): void {
  if (touchTimer) {
    window.clearTimeout(touchTimer);
    touchTimer = 0;
  }
}

function onGridPointerDown(ev: PointerEvent): void {
  // 选择模式：整条手势归滑动多选，**不做任何拖拽改判**（见 selectMode 的说明）
  if (selectMode.value) {
    swipe.onPointerDown(ev);
    return;
  }
  beginPress(ev);
}

function onWindowPointerMove(ev: PointerEvent): void {
  pointer.value = { x: ev.clientX, y: ev.clientY };

  // 「选择」模式：整条手势归滑动多选（它自己是幂等的空转，没在拖就直接返回）
  if (selectMode.value) {
    swipe.onPointerMove(ev);
    return;
  }

  if (dragIds.value) {
    updateDropTarget(ev.clientX, ev.clientY);
    return;
  }
  if (!dragArm.value) return;

  if (ev.pointerType === "touch") {
    // ⚠️ 触摸**只能**由长按起拖：移动事件在这里唯一的用途是判断"手指先动了
    // = 用户在滚页面"（与滑动多选对触摸的同一条约定）。
    // 不能让"长按计时器被清掉"之后的任意一次移动就把拖拽点着 —— 那等于
    // 手指一划就抢走页面滚动，而且坐标缺失的移动事件还会把落点判到 (0,0)。
    if (touchTimer && Math.hypot(ev.clientX - pressX, ev.clientY - pressY) >= DRAG_THRESHOLD_PX) {
      clearTouchTimer();
    }
    return;
  }
  if (Math.hypot(ev.clientX - pressX, ev.clientY - pressY) >= DRAG_THRESHOLD_PX) {
    startDrag(ev.clientX, ev.clientY);
  }
}

/**
 * 松手：按当前落点结算。
 *
 * 落点必须**明确**（某张卡片，或「放回顶层」区）。松在空白处 = 取消这一次
 * 拖拽，什么都不发 —— 那是所有拖放界面的通行语义，也避免"手一抖就搬家"。
 */
function onWindowPointerUp(): void {
  clearTouchTimer();
  dragArm.value = null;
  dragOrigin.value = null;
  const ids = dragIds.value;
  if (!ids) return;
  armSwallow();
  const target = overRoot.value ? null : overId.value;
  const explicit = overRoot.value || overId.value !== null;
  dragIds.value = null;
  overId.value = null;
  overReasonText.value = "";
  overRoot.value = false;
  if (!explicit) return;

  const check = canDrop(nodes.value, ids, target, "inside");
  if (check.ok) {
    emit("move", { ids, parentId: target, index: null });
    clearReject(); // 成功落点：不该再留着任何禁止态
  } else {
    // 非法放置什么都不发，只把原因亮一小会儿（见 flashReject）。
    flashReject(check.reason || "这个位置不能放", target);
  }
}

/**
 * 手势被系统打断（`pointercancel`）：**丢弃，不提交**。
 *
 * 与滑动多选同一条约定：cancel 的语义是"当作没发生过"。
 */
function onWindowPointerCancel(): void {
  clearTouchTimer();
  dragArm.value = null;
  dragOrigin.value = null;
  dragIds.value = null;
  overId.value = null;
  overReasonText.value = "";
  overRoot.value = false;
}

/**
 * 拖完松手之后这一次 `click` 要吞掉。
 *
 * 松在自己的卡片上（非法落点 / 只是按了一下又拖回原地）时，浏览器会照常
 * 派发 click，卡片上的"打开详情 / 勾选"就会执行 —— 用户明明是拖了一下。
 * 与滑动多选同一套处理：一次性的捕获阶段拦截，400ms 兜底。
 */
let swallowClick: ((ev: Event) => void) | null = null;
let swallowTimer = 0;

function releaseSwallow(): void {
  if (swallowTimer) {
    window.clearTimeout(swallowTimer);
    swallowTimer = 0;
  }
  if (swallowClick) {
    document.removeEventListener("click", swallowClick, true);
    swallowClick = null;
  }
}

function armSwallow(): void {
  releaseSwallow();
  const h = (ev: Event): void => {
    ev.stopPropagation();
    ev.preventDefault();
    releaseSwallow();
  };
  swallowClick = h;
  document.addEventListener("click", h, true);
  swallowTimer = window.setTimeout(releaseSwallow, 400);
}

/** 解除分组（卡片上的 ⤒ 按钮）：挂回顶层。 */
function ungroup(id: number): void {
  emit("move", { ids: [id], parentId: null, index: null });
}

onMounted(() => {
  // 移动 / 松手常常发生在网格之外（划到窗口顶部、划出列表），所以挂 window。
  // ⚠️ 这里只挂**网格之外**那三个；网格内部的事件由滑动多选自己按 `root`
  // 绑定（它挂的也是 document，见 useSwipeSelect），两边各管各的模式。
  window.addEventListener("pointermove", onWindowPointerMove);
  window.addEventListener("pointerup", onWindowPointerUp);
  window.addEventListener("pointercancel", onWindowPointerCancel);
});

onBeforeUnmount(() => {
  window.removeEventListener("pointermove", onWindowPointerMove);
  window.removeEventListener("pointerup", onWindowPointerUp);
  window.removeEventListener("pointercancel", onWindowPointerCancel);
  releaseSwallow();
  clearReject();
});

/** 这一格的视觉状态（拖拽中 / 可放置 / 非法放置 / 子项）。 */
function cellClass(row: FlatRow): Record<string, boolean> {
  const id = row.node.id;
  const dragging = dragIds.value !== null;
  return {
    child: row.depth > 0,
    dragging: !!dragIds.value && dragIds.value.includes(id),
    over: dragging && overId.value === id && !overReasonText.value,
    // 两个来源都算「这一格现在确实放不进去」：
    // ① 拖拽中悬停在它上面且判定非法（实时，挪开就收）；
    // ② 刚才那一次放下被拒（短暂，到点自己收 —— 见 rejectId）。
    rejected:
      (dragging && overId.value === id && !!overReasonText.value) ||
      (rejectId.value === id && !!rejectReason.value),
  };
}
</script>

<template>
  <div v-if="rows.length" class="wall">
    <!-- 拖拽期间才出现的「放回顶层」区：拖到这里 = 解除分组 -->
    <!--
      ⚠️ 固定定位（见样式里的 `.rootzone`）是**必须**的，不是美化：
      它在拖拽开始时才插入，静态布局会把整个网格往下顶、指针下的卡片当场
      移走，落点判定跟着漂 —— 表现就是"拖到顶部，卡片全跳了一下"。
    -->
    <div v-if="dragIds" class="rootzone" :class="{ active: overRoot }">
      拖到这里解除分组（挂回顶层）
    </div>

    <!-- 手势的归属在 onGridPointerDown 里分流（见「手势：谁在什么时候拿到
         这一次按下」）：默认 / 聚合模式归拖拽聚合，「选择」模式归滑动多选。
         跟手与落点在 window 上（见 `onMounted`），指针划到窗口顶部照样收得到。 -->
    <div
      ref="grid"
      class="grid"
      :class="{ dragging: swipe.dragging.value || !!dragIds, grouping: props.grouping }"
      @pointerdown="onGridPointerDown"
    >
      <template v-for="row in rows" :key="row.node.id">
        <!--
          ⚠️ 卡片上**一个`draggable`都不留**：整个拖放已经由指针事件驱动
          （见 script 里「剧集聚合：指针驱动的拖放」一节）。留着它就是又给
          浏览器一次机会，把"按住卡片划过去"认成原生拖拽、顺手补一次
          `pointercancel` 掐断 pointer 事件流 —— 滑动多选当场失灵。
        -->
        <div
          class="cell"
          :class="cellClass(row)"
          :data-id="row.node.id"
          :data-depth="row.depth"
        >
          <PosterCard
            :item="itemOf(row.node.id)"
            :selectable="props.selectable"
            :selected="picked(row.node.id)"
            :watched="watchedOf(row.node.id)"
            :progress="progressOf(row.node.id)"
            @open="(id) => emit('open', id)"
            @select="(p) => emit('select', p)"
            @play="(id) => emit('play', id)"
          />

          <!--
            卡片操作条：展开 / 折叠 + 解除分组。

            ⚠️ **必须走常规文档流，不能浮在海报上**。老实现把它 `position:absolute`
            钉在 `.cell` 的 `bottom: 4px`，而 `.cell` 不只包含海报、还包含下面的
            标题 / 年份 / 评分——于是按钮正好压在年份那一行上，把「7.5」盖成「7.」
            （用户截图就是这条）。海报四角也全被卡片自己的角标占了（左上类型、
            右上未刮削、左下观看、右下集数），没有能安全浮起来的空位。

            改成卡片下方的独立一行，同时解决三件事：
            ① 任何列宽 / 任何角标组合下都不会重叠；
            ② 高度由内容自己撑开，不用按 meta 高度反算偏移量；
            ③ 有按钮的格子只是变高一点，`align-items: start` 保证不影响同排其它卡。
          -->
          <span v-if="row.hasChildren || row.node.parent_id !== null" class="ops">
            <button
              v-if="row.hasChildren"
              class="op op-expand"
              :class="{ open: row.expanded }"
              :aria-expanded="row.expanded"
              :title="row.expanded ? `折叠这 ${row.node.child_count} 个子项` : `展开 ${row.node.child_count} 个子项`"
              @click.stop="toggleExpand(row.node.id)"
            >
              <span class="chev" aria-hidden="true">{{ row.expanded ? "▾" : "▸" }}</span>
              <span class="op-n">{{ row.node.child_count }}</span>
              <span class="op-label">{{ row.expanded ? "折叠" : "展开" }}</span>
            </button>
            <button
              v-if="row.node.parent_id !== null"
              class="op op-ungroup"
              title="解除分组（挂回顶层，条目不会被删除）"
              @click.stop="ungroup(row.node.id)"
            >
              <span class="chev" aria-hidden="true">⤴</span>
              <span class="op-label">移出</span>
            </button>
          </span>

        </div>

        <!-- 分组条：占满整行，说明紧随其后的卡片属于上面那一张。
             子项仍是等宽卡片（缩进会挤压列宽，列宽一乱整个墙就歪了）。 -->
        <div v-if="row.expanded && row.hasChildren" class="groupbar">
          ↳ {{ row.node.display_title }} 的子项
        </div>

        <!-- 闭合分割线：插在这个分组的**最后一个后代**之后。
             ① 画成一条线，满足「展开之后用分割线把这一组和其它项目分开」；
             ② `grid-column: 1 / -1` 让它跨整行 → **强制换行**，
                后面的顶层项目从此另起一行，不会再混进这一组的子项里
                （那正是「展开一个项目，后面那个也跟着展开」的真正成因）。
             说明见 script 里 `closingGroupAfter` 的注释与实测坐标。 -->
        <div
          v-if="closingGroupAfter.has(row.node.id)"
          class="groupend"
          :data-group-end="row.node.id"
        ></div>
      </template>
    </div>

    <!-- 跟手预览：拖拽期间贴在指针右下角，说明"正在搬哪几个"。
         指针事件驱动的拖拽没有浏览器自带的拖影，这一块必须自己画 ——
         不然用户按住一拖，屏幕上除了落点高亮之外**什么反馈都没有**。
         `pointer-events: none` 保证它自己不参与落点命中。 -->
    <span
      v-if="dragIds"
      class="ghost"
      :style="{ left: `${pointer.x + 12}px`, top: `${pointer.y + 12}px` }"
    >{{ dragLabel }}</span>

    <!-- 非法放置的原因：拖到自己 / 子孙 / 超层数，`canDrop` 已经判出来了。
         它是**文字原因**，不是"目标不可接收"—— 能不能放只影响这一次的落点判定。 -->
    <p v-if="overReason" class="reject">{{ overReason }}</p>
  </div>
  <div v-else class="empty">
    <p class="big">媒体库还是空的</p>
    <p class="hint">请到「设置」添加媒体目录，然后点顶部「扫描媒体库」。</p>
  </div>
</template>

<style scoped>
.wall { display: flex; flex-direction: column; gap: calc(var(--space) * 3); }

/*
 * 无级缩放：列数由**容器可用宽度**连续决定，不写死一个断点表。
 * - `auto-fill` + `minmax(…, 1fr)`：能放下几列就放几列，剩下的宽度由各列平分，
 *   于是窗口从窄拖到宽、或系统缩放（100% → 150% → 200% DPI）时列宽是连续变化的，
 *   不会出现裁切 / 重叠 / 错位（每列都是 1fr，不存在半像素累积误差）；
 * - 外面再包一层 `min(…, 100%)` 兜住极窄容器：窗口窄到放不下一张最小卡片时
 *   退回单列 100%，而不是把 150px 硬塞进去把整页顶出横向滚动条；
 * - `--card-min` 由 `main.css` 统一给（卡片内部比例也归它管），这里只引用，
 *   并保留 150px 兜底，免得变量还没定义时布局塌掉。
 */
.grid {
  display: grid; gap: calc(var(--space) * 5);
  grid-template-columns: repeat(auto-fill, minmax(min(var(--card-min, 150px), 100%), 1fr));
  align-items: start;
  /* 允许竖向滚动（触摸下长按进入多选后，滚动由 touchmove 的 preventDefault 接管） */
  touch-action: pan-y;
}
/* 拖动期间禁掉文字选择：不然一路划过去会把标题选蓝 */
.grid.dragging { user-select: none; }

/* 一格 = 一张卡片。定位上下文给浮层控件用；`min-width: 0` 防止长标题把列撑开 */
.cell { position: relative; min-width: 0; }
.cell.dragging { opacity: 0.45; }
/* 子项：整块左侧色条表明归属，卡片宽度与其它列保持一致 */
.cell.child::before {
  content: ""; position: absolute; left: -7px; top: 6px; bottom: 6px; width: 3px;
  border-radius: 2px; background: var(--accent); opacity: .55;
}
/* 落点反馈：合法 = 主色描边，非法 = 红色描边。
   光标那边同样是"能放 = move、不能放 = none"，但**事件永远照常送到** ——
   非法落点不再变成"这个目标不可接收"。 */
.cell.over :deep(.card) { outline: 2px solid var(--accent); outline-offset: 2px; border-radius: var(--radius-s); }
.cell.rejected :deep(.card) { outline: 2px solid var(--danger); outline-offset: 2px; border-radius: var(--radius-s); }

/*
 * 卡片操作条：展开 / 折叠 + 解除分组。
 *
 * ⚠️ **不许再改成 `position: absolute`**。老实现浮在海报底边（`bottom: 4px`），
 * 而 `.cell` 包含海报 + 标题 + 年份/评分三块，那个"底边"其实是**卡片最底下**
 * ——按钮直接压在年份行上，把评分盖掉一半（用户截图里 `7.5` 被压成 `7.`）。
 * 海报四角也全被卡片自己的角标占了，没有能安全浮起来的空位。
 *
 * 现在它是我方 meta 下方的**独立一行**：占位由内容自己撑开，不重叠、不用
 * 按 meta 高度反算偏移量，窄列下也能自动换行。
 */
.ops {
  display: flex; flex-wrap: wrap; gap: 6px;
  margin-top: 9px;
  /* 与 PosterCard 的 .meta 对齐（那边有 2px 内边距），文字左缘才在一条线上 */
  padding: 0 2px;
}

/*
 * 单个按钮：胶囊形，逐字可读。
 *
 * 为什么不只画图标：老实现是「▸ 32」「⤒」两个纯符号方块 —— 用户既看不出
 * 那是什么操作，也分不清 32 是"集数"还是"子项数"（卡片右下角刚有一个
 * `32 集`）。带上文字之后按钮自己就把话说完了。
 */
.op {
  display: inline-flex; align-items: center; gap: 4px;
  padding: 3px 9px; border-radius: 999px;
  border: 1px solid var(--border-strong);
  background: var(--surface-2); color: var(--text-2);
  font-size: var(--fs-12); line-height: 1.45; white-space: nowrap;
  cursor: pointer;
  /* 一点投影把它从深色背景上拎起来：它是一条"附加操作"，不是卡片的一部分 */
  box-shadow: 0 1px 2px rgba(0, 0, 0, .35);
  transition: background .15s ease, border-color .15s ease, color .15s ease, transform .1s ease;
}
.op:hover { background: var(--border-strong); color: var(--text); }
.op:active { transform: scale(.97); }
.op:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }

/* 箭头是纯符号，字号跟着文字走（更大会显得虚） */
.op .chev { font-size: 11px; line-height: 1; opacity: .8; }
/* 子项数量：等宽数字，展开 / 折叠切换时按钮宽度不跳 */
.op .op-n { font-variant-numeric: tabular-nums; font-weight: 600; }

/*
 * 默认态（分组是**收起**的）用主色描边：这时候卡片里有内容被藏着，
 * 必须让人一眼看到"这里还能展开"。
 * 展开态退回中性色 —— 子项已经摊在下面了，不需要再喊。
 */
.op-expand { color: var(--accent); border-color: rgba(229, 9, 20, .5); background: var(--accent-soft); }
.op-expand:hover { background: rgba(229, 9, 20, .26); color: var(--accent-hover); border-color: var(--accent); }
.op-expand.open { color: var(--text-2); border-color: var(--border-strong); background: var(--surface-2); }
.op-expand.open:hover { background: var(--border-strong); color: var(--text); }

/* 解除分组：破坏性动作的语义色只在悬停时露出来，平时不抢眼 */
.op-ungroup:hover { color: var(--danger); border-color: rgba(255, 107, 115, .55); background: rgba(255, 107, 115, .12); }

/*
 * 跟手预览：指针驱动的拖拽没有浏览器自带的拖影，这一块自己画。
 * `position: fixed` + `pointer-events: none`：跟手不参与布局、也永远不当落点。
 */
.ghost {
  position: fixed; z-index: 50; pointer-events: none;
  padding: 2px 8px; border-radius: 4px;
  font-size: var(--fs-12); line-height: 1.6; color: #fff; white-space: nowrap;
  background: rgba(0, 0, 0, .78); border: 1px solid rgba(255, 255, 255, .35);
  box-shadow: var(--shadow);
}

/* 分组条：横跨 auto-fill 出来的所有列（`1 / -1` 不必知道到底有几列） */
.groupbar {
  grid-column: 1 / -1; margin: calc(var(--space) * -2) 0;
  padding: 0 2px; font-size: var(--fs-12); color: var(--muted);
  border-top: 1px dashed var(--border);
}

/*
 * 闭合分割线：一个已展开分组的**最后一个后代**之后。
 *
 * 两条作用缺一不可：
 * 1. `grid-column: 1 / -1` —— 跨满所有列。它因此**必然落在新的一行**，
 *    后面的顶层卡片被挤到下一行去。没有这一条，下一个项目会紧贴着
 *    这一组的子项排在同一行（真实浏览器实测：`cell id=2 y=334 x=370`
 *    就混在 `id=11/12 y=334` 那一行里），看上去就像"它也一起展开了"。
 * 2. `border-top` 之外的可见分隔 —— 用实线（`--border-strong`）而不是
 *    分组条那种虚线：虚线表示"以下是这一组的成员"，实线表示"这一组到此为止"，
 *    两者语义不同，画成一样用户分不清哪个是开始、哪个是结束。
 *
 * 高度取自 `--space`：它只是条分割线，不该在墙上占出一整格的高度。
 */
.groupend {
  grid-column: 1 / -1; height: 0;
  margin: calc(var(--space) * 2) 0 calc(var(--space) * -1);
  border-top: 1px solid var(--border-strong);
}

/*
 * 「放回顶层」区：拖拽期间才插入，所以**必须脱离文档流**（fixed）——
 * 静态布局插进来会把整个网格往下顶，指针下的卡片当场移走，落点判定跟着漂。
 * 拖拽期间不需要滚动页面（指针驱动的拖拽跟手），固定在顶部视野里最直接。
 */
.rootzone {
  position: fixed; top: 0; left: 0; right: 0; z-index: 40;
  padding: 8px 12px; text-align: center; pointer-events: none;
  font-size: var(--fs-12); color: var(--muted);
  background: var(--surface); border-bottom: 1px dashed var(--border);
}
.rootzone.active { color: var(--accent); border-color: var(--accent); background: var(--accent-soft); }

.reject {
  margin: 0; padding: 8px 12px; border-radius: var(--radius-s); font-size: var(--fs-12);
  background: rgba(229, 9, 20, .12); border: 1px solid rgba(229, 9, 20, .35); color: #ff8a90;
}

.empty { padding: calc(var(--space) * 16) 0; text-align: center; color: var(--muted); }
.empty .big { margin: 0 0 8px; font-size: 16px; color: var(--text); }
.empty .hint { margin: 0; font-size: 13px; }
</style>