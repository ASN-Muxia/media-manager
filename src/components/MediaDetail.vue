<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from "vue";
import {
  getMediaCategories, getMediaList, getMediaTags, imageUrl, setMediaCategories,
  setMediaTags, tvdbApply,
  type Category, type Episode, type MediaItem, type Tag,
} from "../api/commands";
import { open } from "@tauri-apps/plugin-dialog";
import { askConfirm } from "../utils/confirm";
import { useMediaStore } from "../stores/mediaStore";
import { useSettingsStore } from "../stores/settingsStore";
import { useCategoryStore } from "../stores/categoryStore";
import { usePlaybackStore } from "../stores/playbackStore";
import { formatScore, typeIcon, typeLabel } from "../constants/mediaType";
import {
  applyDragRange, applyDragTo, beginDragSelection, clampIndex, dragTrigger,
  DRAG_THRESHOLD_PX, finishDragSelection, LONG_PRESS_MS, toggleOne,
  type DragSelection,
} from "../utils/dragSelect";
// 只复用这几个"没有 Vue 生命周期"的纯编排件，不是为了把详情页改成封面墙那套：
// 详情页的行是 `<li>`（没有 click 语义、按下即切换），与卡片（`<button>` + click）
// 语义不同，强行共用一套会把"按下即切换这一行"弄丢。
import { elementAtPoint, pressBlocked, useEdgeScroll } from "../composables/useSwipeSelect";
import CollapsibleSection from "./CollapsibleSection.vue";
import TmdbMatchDialog from "./TmdbMatchDialog.vue";

const store = useMediaStore();
const settings = useSettingsStore();
const catStore = useCategoryStore();
const pbStore = usePlaybackStore();

/* ---------------- 观看状态 / 续播（v6） ---------------- */
/* 只写数据库，不碰视频文件。连播也是把多个路径直接交给播放器，
   不生成任何临时播放列表文件。 */

/** 当前条目的观看摘要（没拉回来就是 null，界面按「未看」渲染）。 */
const pb = computed(() => (store.selectedId ? pbStore.of(store.selectedId) : null));

/** 「看到 S1E3」这样的文案；没看过就没有。 */
const seenAt = computed(() => {
  const p = pb.value;
  if (!p || p.lastSeason == null || p.lastEpisode == null) return "";
  return `S${String(p.lastSeason).padStart(2, "0")}E${String(p.lastEpisode).padStart(2, "0")}`;
});

/**
 * 继续播放。
 *
 * 语义是「播这个条目上次看到的地方」——**不传 episodeId**，由后端
 * `play_episode` 统一决定（上次真正播放过的那一集 > 第一个未看集）。
 * 与封面墙的双击播放走同一条命令、同一套判据，两处不会给出不同答案。
 *
 * 注意这不等于秒级续播：外部播放器不回传进度，拿不到「第 3 分 12 秒」这种精度。
 */
async function resume(): Promise<void> {
  const d = detail.value;
  if (!d) return;
  await pbStore.playDefault(d.media.id);
}

/** 从续播点开始连播后续所有集。 */
async function playContinuous(): Promise<void> {
  const d = detail.value;
  if (!d) return;
  await pbStore.play(d.media.id, pb.value?.resumeEpisodeId ?? null, true);
}

/** 切换「看过 / 没看过」（整部）。 */
async function toggleWatched(): Promise<void> {
  const d = detail.value;
  if (!d) return;
  await pbStore.mark(d.media.id, !pb.value?.watched);
}

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

/** 本次按下是否真的拖动过：只有拖动过才需要吞掉松手后那次 click。 */
let dragMoved = false;
/** 起手前的勾选快照：`pointercancel` 时靠它回滚。 */
let dragSnapshot: number[] = [];
/** 上一次命中的行在 `visibleEpIds()` 里的下标：用来补齐快速滑动跳过的行。 */
let lastIndex: number | null = null;
/** 触摸：按下但长按尚未成立（这时手指一动就判定为"用户要滚页面"）。 */
let pending = false;
let pendingId: number | null = null;
let startX = 0;
let startY = 0;
let startAt = 0;
let longPressTimer = 0;

/** 列表根元素：贴边滚动的矩形基准 + 尾随 click 的拦截点都挂在它上面。 */
const epRoot = ref<HTMLElement | null>(null);

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
/** 可选的行的选择器（`pressBlocked` 要保证它自身不会被上面的选择器拦掉）。 */
const EP_ITEM_SELECTOR = "li[data-ep-id]";

/** 某元素所属行在可见顺序里的下标；拿不到（划过按钮、行间空隙）就返回 -1。 */
function rowIndexOf(el: Element | null | undefined): number {
  const raw = (el?.closest(EP_ITEM_SELECTOR) as HTMLElement | null)?.dataset.epId;
  if (!raw) return -1;
  const id = Number(raw);
  if (!Number.isFinite(id)) return -1;
  return visibleEpIds().indexOf(id);
}

/** 几何命中测试，仅触摸路径用（实现与封面墙共用一份：见 `elementAtPoint`）。 */
function elementAt(x: number, y: number): Element | null {
  return elementAtPoint(x, y);
}

/**
 * 取当前指针下的行。
 *
 * 鼠标走 `event.target`（事件委托，最快，也不依赖布局信息）；
 * 触摸必须走几何命中 —— 浏览器对触摸有**隐式指针捕获**，
 * 拖动期间 `event.target` 会一直停在起点那一行上，用它会把整段都算成第一行。
 */
function rowIndexAt(ev: PointerEvent): number {
  const el: Element | null =
    ev.pointerType === "touch"
      ? elementAt(ev.clientX, ev.clientY)
      : (ev.target as Element | null);
  return rowIndexOf(el);
}

/**
 * 指针跑到列表之外（上方 / 下方的空白）时，把它夹到首行 / 末行 ——
 * 否则往列表外多划一截，端点那一行反而选不上。
 *
 * **没有布局信息时一律返回 -1**（jsdom 的 `rect.height` 恒为 0），
 * 保持"划过行间空白什么都不做"的旧行为。
 */
function clampRowIndexAt(clientY: number): number {
  const el = epRoot.value;
  if (!el) return -1;
  const r = el.getBoundingClientRect();
  if (r.height <= 0) return -1;
  const n = visibleEpIds().length;
  if (n === 0) return -1;
  if (clientY <= r.top) return 0;
  if (clientY >= r.bottom) return n; // 交给 clampIndex 夹成末行
  return -1;
}

/** 贴边自动滚动：每滚一帧用最后的指针位置重算一次命中（触摸只能用几何）。 */
const edge = useEdgeScroll((x, y) => applyRowIndex(rowIndexOf(elementAt(x, y))));

/**
 * 把某一行写进拖动状态 —— 连同上一次命中行之间的**整段**一起写，
 * 这样快速滑动时被跳过的行不会漏掉（见 `applyDragRange`）。
 */
function applyRowIndex(index: number): void {
  if (!dragState || index < 0) return;
  const order = visibleEpIds();
  const clamped = clampIndex(index, order.length);
  if (clamped < 0) return;
  const changed = lastIndex === null || lastIndex === clamped
    ? applyDragTo(dragState, order[clamped])
    : applyDragRange(dragState, order, lastIndex, clamped);
  lastIndex = clamped;
  // 幂等：同一行反复划过只写一次，只在真的变了时才刷新界面
  if (changed) syncFromDrag();
}

/** 真正起手一次拖动。 */
function beginEpDrag(id: number): void {
  clearLongPress();
  pending = false;
  pendingId = null;
  // 起手前的快照：系统打断时要靠它回滚（这里的 epSelected 是**实时**写的，
  // 不像封面墙那样只在松手时提交一次，所以回滚必须显式做）
  dragSnapshot = [...epSelected.value];
  dragState = beginDragSelection(epSelected.value, id);
  dragging.value = true;
  const i = visibleEpIds().indexOf(id);
  lastIndex = i >= 0 ? i : null;
  syncFromDrag();
}

function onEpPointerDown(ev: PointerEvent, id: number): void {
  if (ev.button !== 0) return;
  const target = ev.target as Element | null;
  if (!target) return;
  // 行内控件上的按下不起手拖动；行自身永远不算"被拦"（见 pressBlocked）
  if (pressBlocked(target, EP_ITEM_SELECTOR, EP_BLOCKED_SELECTOR)) return;
  ev.preventDefault(); // 不然一路划过去会把标题选蓝 / 触发原生拖拽

  dragMoved = false;
  lastIndex = null;
  startX = ev.clientX;
  startY = ev.clientY;
  startAt = ev.timeStamp;
  edge.attach(epRoot.value);

  if (ev.pointerType === "touch") {
    // 触摸必须先长按：手指先动 = 想滚页面，绝不能抢成框选（见 dragTrigger）
    pending = true;
    pendingId = id;
    clearLongPress();
    longPressTimer = window.setTimeout(() => {
      longPressTimer = 0;
      if (pending && pendingId !== null) beginEpDrag(pendingId);
    }, LONG_PRESS_MS);
    return;
  }
  // 鼠标即时起手：按下即切换这一行（历史语义，不要改成"必须移动 6px"）
  beginEpDrag(id);
}

function onEpPointerMove(ev: PointerEvent): void {
  if (pending) {
    const moved = Math.hypot(ev.clientX - startX, ev.clientY - startY) >= DRAG_THRESHOLD_PX;
    const verdict = dragTrigger("touch", moved, ev.timeStamp - startAt);
    if (verdict === "abort") abortPending(); // 手指先动了 → 让页面滚
    else if (verdict === "drag" && pendingId !== null) beginEpDrag(pendingId);
    return;
  }
  if (!dragState) return;
  if (Math.hypot(ev.clientX - startX, ev.clientY - startY) >= DRAG_THRESHOLD_PX) dragMoved = true;
  const idx = rowIndexAt(ev);
  applyRowIndex(idx >= 0 ? idx : clampRowIndexAt(ev.clientY));
  edge.update(ev.clientX, ev.clientY);
}

/** 触摸下必须拦掉原生滚动：指针事件的 preventDefault 不管用，得拦 touchmove。 */
function onEpTouchMove(ev: TouchEvent): void {
  if (dragState && ev.cancelable) ev.preventDefault();
}

function clearLongPress(): void {
  if (longPressTimer) {
    clearTimeout(longPressTimer);
    longPressTimer = 0;
  }
}

/** 放弃这一次按下（手指是在滚页面）。 */
function abortPending(): void {
  clearLongPress();
  pending = false;
  pendingId = null;
}

/** 松手后那次 click 的一次性拦截（只有真的拖动过才装）。 */
const CLICK_SWALLOW_MS = 400;
let swallowClick: ((ev: Event) => void) | null = null;
let swallowTimer = 0;

function releaseSwallow(): void {
  if (swallowTimer) {
    clearTimeout(swallowTimer);
    swallowTimer = 0;
  }
  if (swallowClick && epRoot.value) {
    epRoot.value.removeEventListener("click", swallowClick, true);
    swallowClick = null;
  }
}

function armSwallow(): void {
  releaseSwallow();
  const el = epRoot.value;
  if (!el) return;
  const h = (ev: Event): void => {
    ev.stopPropagation();
    ev.preventDefault();
    releaseSwallow();
  };
  swallowClick = h;
  el.addEventListener("click", h, true);
  swallowTimer = window.setTimeout(releaseSwallow, CLICK_SWALLOW_MS);
}

function endDrag(): void {
  abortPending();
  edge.stop();
  if (!dragState) return;
  syncFromDrag();
  dragState = null;
  lastIndex = null;
  dragging.value = false;
  // 只有真的拖动过才吞 click：原地按下松手仍然交还给普通的点击语义
  if (dragMoved) armSwallow();
  dragMoved = false;
}

/**
 * 手势被系统打断（`pointercancel`）：**回滚到起手前**，绝不提交半截选择。
 *
 * 真机上手指划到一半会被来电 / 系统返回手势 / 侧滑 / 切后台打断，派发的是
 * `pointercancel`。这里如果按"松手"处理，用户没做完的那半截就会被当成最终
 * 选择写进 `epSelected` —— 而 `epSelected` 直接驱动「删除所选」，后果比
 * 封面墙更重。`cancel` 的语义就是"当作没发生过"。
 */
function cancelDrag(): void {
  abortPending();
  edge.stop();
  if (!dragState) return;
  dragState = null;
  lastIndex = null;
  dragMoved = false;
  dragging.value = false;
  epSelected.value = dragSnapshot; // 回滚
}

/** 丢弃进行中的拖动（不提交、不回滚）：换条目 / 卸载时用 —— 勾选由调用方处置。 */
function abortDrag(): void {
  abortPending();
  edge.stop();
  dragState = null;
  lastIndex = null;
  dragMoved = false;
  dragging.value = false;
}

onMounted(() => {
  document.addEventListener("pointerup", endDrag);
  // 系统打断走 cancel：回滚，不提交
  document.addEventListener("pointercancel", cancelDrag);
});

onBeforeUnmount(() => {
  document.removeEventListener("pointerup", endDrag);
  document.removeEventListener("pointercancel", cancelDrag);
  releaseSwallow();
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
  const ok = await askConfirm(
    n === 1
      ? "确定删除这条资源？\n\n删除后不可恢复；本地视频文件不会被删除。"
      : `确定删除选中的 ${n} 条资源？\n\n删除后不可恢复；本地视频文件不会被删除。`,
    "删除资源",
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
/** 电影 / 待确认条目名下的多份文件是「版本」，不是「季」（同片多文件归并而来）。 */
const isVersionList = computed(() => detail.value?.media.type !== "tv");
const seasonTitle = (season: number): string =>
  isVersionList.value ? "其它版本 / 文件来源" : `第 ${season} 季`;
const countLabel = (n: number): string =>
  isSingleFile.value ? `共 ${n} 个版本` : `共 ${n} 集`;

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

/* ---------------- 聚合成员（父条目内的成员管理） ---------------- */
/*
 * 封面墙的拖拽聚合与老的剧集层级写的是**同一列** `media.parent_id`，
 * 所以这一块**不区分 `type`**：电影与剧集走同一套代码路径、同一套文案，
 * 判据只有「child_count > 0」或「确实有 parent_id 指向本条目的条目」。
 * 既不因为 `type === "tv"` 才显示，也不假设老数据是空的——老关系要照原样显示出来。
 *
 * ⚠️ 与上面的「单集管理」不是一回事：单集是 `episodes` 表里的行，
 * 成员是 `media` 表里的另一条记录，两边互不相干。
 */

/** 当前条目的 id（详情没打开时是 null）。 */
const groupSelfId = computed<number | null>(() => detail.value?.media.id ?? null);

/**
 * 子成员：凡是 `parent_id` 指向本条目的条目。
 *
 * 默认顺序**跟随顶栏排序设置**；只有用户亲手用 ↑↓ 排过的那些组才按库内 `sort_order`。
 *
 * ⚠️ 判据是**显式标记** `settings.manualOrderGroups`，不是「组里有 `sort_order > 0`」。
 * 后者曾经是这里的启发式，但它是错的：任何一次 `move_nodes_sync`（含在封面墙上
 * 拖动聚合）都会把目标同级整组重编号成 0..n-1，实测真实库里连从没排过的组都是
 * 0,1,2,…（14135 = 0..29、14150 = 0,2,3,4,5,18,19,20）——「存在 > 0」恒为真，
 * 于是顶栏排序永远不生效，用户「看不到变化」。
 *
 * 手工组仍按 `(sort_order, id)`：那是后端 `move_nodes_sync` 里
 * `siblings.sort_by_key(|id| (sort_order, id))` 的同一把尺子，也是 `moveMember`
 * 发出的 `index` 能被后端当成「同级内落点」解释的前提。
 *
 * 非手工组**直接用 `store.orderedItems` 的既定子序列**（`.filter` 保序）：
 * 那正是顶栏排序算出来的顺序，再排一次既多余、又可能与它的分区规则
 * （已识别在前 / 未识别沉底）不一致 —— 那会让成员列表和封面墙的顺序对不上。
 *
 * 过滤口径不变（仍取筛选后的列表）：成员不在当前筛选内时走空态提示，不是把它藏起来。
 */
const members = computed<MediaItem[]>(() => {
  const id = groupSelfId.value;
  if (id === null) return [];
  const kids = store.orderedItems.filter(
    (it) => it.id !== id && (it.parent_id ?? null) === id,
  );
  // 手工排过的组：按库内同级次序（与后端 move_nodes_sync 的 (sort_order,id) 同一把尺子）
  if (settings.manualOrderGroups.includes(id)) {
    return [...kids].sort(
      (a, b) => (a.sort_order ?? 0) - (b.sort_order ?? 0) || a.id - b.id,
    );
  }
  // 没手工排过：跟随顶栏排序偏好（kids 就是 orderedItems 的子序列，保序）
  return kids;
});

/** 后端记的直接子级数。它与 `members.length` 可能不一致（见模板里的空态说明）。 */
const childCount = computed<number>(() => detail.value?.media.child_count ?? 0);

/** 是不是聚合：两个判据取「或」——任一边说是就按聚合渲染，避免残留 / 漏显示。 */
const isGroup = computed<boolean>(() => childCount.value > 0 || members.value.length > 0);

/** 所属聚合（`parent_id` 非空 = 自己也是别人的成员）。 */
const parentId = computed<number | null>(() => detail.value?.media.parent_id ?? null);

/** 父条目可能不在当前列表里（被类型 / 分类筛选挡住，或已被删除），取不到就退回 id。 */
const parentTitle = computed<string>(() => {
  const pid = parentId.value;
  if (pid === null) return "";
  const p = store.orderedItems.find((it) => it.id === pid);
  return p?.display_title || p?.title || `#${pid}`;
});

/**
 * 父条目在**当前列表里**取不到。
 *
 * 判据刻意复用 `parentTitle` 的查找结果（显示成 `#id` 就是取不到），
 * 两边共用一把尺子，不会出现「显示 `#id` 却还能点进去」这种自相矛盾的界面。
 *
 * ⚠️ 这是「不在这个列表里」，**不等于「库里已删除」**：它同样可能只是被类型 /
 * 分类 / 关键字筛选挡住了。所以文案只说到列表为止，不替用户下"已删除"的结论。
 * 库里真有悬挂 `parent_id` 的情况由后端 `heal_orphan_parents` 在开库时清掉。
 */
const parentMissing = computed<boolean>(() => parentTitle.value.startsWith("#"));

/** 当前这一组的次序是不是用户亲手定的（决定要不要给「恢复默认排序」）。 */
const isManualOrder = computed<boolean>(() => {
  const id = groupSelfId.value;
  return id !== null && settings.manualOrderGroups.includes(id);
});

/** 聚合区的提示 / 说明（与 `catMsg` 分开：别把两条不同来源的话挤在一行里）。 */
const groupMsg = ref("");
/** 正在跑层级操作：按钮置灰，避免连点把顺序点乱。 */
const groupBusy = ref(false);
/** 「加入聚合」下拉里当前选中的条目 id。 */
const addCandidate = ref<number | null>(null);

/**
 * 以 `root` 为根的整棵子树 id（含自己）。
 *
 * 用来挡住「把自己 / 自己的子级加进自己」——后端也会拒（`validate_drop`），
 * 但前端不该明知非法还发一次请求。`out` 同时当 visited 用：老数据里若有脏的
 * 父子链（a 的父是 b、b 的父是 a），靠它兜住，不至于死循环。
 */
function subtreeOf(root: number): Set<number> {
  const childrenOf = new Map<number, number[]>();
  for (const it of store.items) {
    const p = it.parent_id ?? null;
    if (p === null) continue;
    const list = childrenOf.get(p) ?? [];
    list.push(it.id);
    childrenOf.set(p, list);
  }
  const out = new Set<number>();
  const stack = [root];
  while (stack.length) {
    const cur = stack.pop() as number;
    if (out.has(cur)) continue;
    out.add(cur);
    for (const c of childrenOf.get(cur) ?? []) stack.push(c);
  }
  return out;
}

/**
 * `id` 的**祖先** id（沿 `parent_id` 上溯，正常数据下不含自己）。
 *
 * 与 `subtreeOf`（向下）配成一对：**环在两个方向都成立** —— 把自己的父 / 祖父
 * 「加入」到自己名下，后端 `validate_drop` 的 `is_descendant` 同样判 `Cycle`，
 * 只挡后代会让下拉里留下一条点了必然失败的出路。
 *
 * 边同样取 `store.items`；`out` 同时当 visited 用：老数据里若有脏环
 * （a 的父是 b、b 的父是 a），靠它兜住 —— 绝不死循环。父节点已被删除
 * （不在 `items` 里）时 `get` 得 `undefined`，按「到顶了」处理。
 */
function ancestorsOf(id: number): Set<number> {
  const parentOf = new Map<number, number | null>();
  for (const it of store.items) parentOf.set(it.id, it.parent_id ?? null);
  const out = new Set<number>();
  let cur = parentOf.get(id) ?? null;
  while (cur !== null && !out.has(cur)) {
    out.add(cur);
    cur = parentOf.get(cur) ?? null;
  }
  return out;
}

/** 可加入的条目：排除自己、自己的后代与祖先（成环的两个方向）以及已在组里的成员。 */
const addCandidates = computed<MediaItem[]>(() => {
  const id = groupSelfId.value;
  if (id === null) return [];
  // 向上（祖先）与向下（后代）都要挡：两个方向都会成环
  const blocked = subtreeOf(id);
  for (const a of ancestorsOf(id)) blocked.add(a);
  const inside = new Set(members.value.map((m) => m.id));
  return store.orderedItems.filter((it) => !blocked.has(it.id) && !inside.has(it.id));
});

/**
 * 重拉列表与详情。
 *
 * 层级操作（`moveNodes` / `undoTree`）内部只刷了层级视图那一份数据，
 * 详情的 `child_count` 与成员列表来自另一份，不重拉就会出现
 * 「已经移出了，列表里还在」的残留。
 */
async function reloadDetail(): Promise<void> {
  await store.load();
  const id = store.selectedId;
  if (id !== null) await store.openDetail(id);
}

/**
 * 跑一次层级操作并在成功后刷新。
 *
 * 失败不用再提示一次：`store.moveNodes` 已经把后端给的原因写进 toast。
 */
async function runTreeOp(op: () => Promise<boolean>): Promise<void> {
  if (groupBusy.value) return;
  groupBusy.value = true;
  groupMsg.value = "";
  try {
    if (await op()) {
      await reloadDetail();
      // 层级变了 → 全量同级序也跟着变：不刷这一份，下一次 ↑↓ 的落点与置灰就会算错
      await loadAllSiblings();
    }
  } finally {
    groupBusy.value = false;
  }
}

/** 刷新（没有层级操作，只是把可能的陈旧数据对齐成真实状态）。 */
async function refreshGroup(): Promise<void> {
  if (groupBusy.value) return;
  groupBusy.value = true;
  groupMsg.value = "";
  try {
    await reloadDetail();
    await loadAllSiblings();
  } finally {
    groupBusy.value = false;
  }
}

/**
/**
 * 让「跟随顶栏排序」的那份默认次序先落地，再把它转成「手工排序」。
 *
 * 为什么非要有这一步（本任务最容易做错的地方）：
 * 组还没被标记时，界面显示的是**顶栏序**，而库里存的还是旧次序，两者通常不同。
 * 若只记标记就往下走，显示会立刻改按库内 `sort_order` 渲染 —— 用户看到的是一
 * 次「点了 ↑ 之后整个列表顺序全乱了」的跳变，正是要避免的缺陷。
 *
 * 顺序**不能颠倒**：
 * ① 先 `moveNodes(当前渲染序, self, null)` —— 后端 `move_nodes_sync` 把这一批按
 *    给定顺序（`roots_of` 保留输入序）重编号成 0..n-1 追加到同级末尾，于是
 *    「用户看到的顺序」变成了库内次序，随后的手工标记**视觉上完全不跳变**；
 * ② 再 `markManualOrder(self)`；
 * ③ 最后才执行原来的 ↑/↓。
 *
 * 返回是否「已就绪、可以继续移动」。失败（或组已经标记过）时：
 * - 已经标记过 → 直接 true（不走任何多余请求）；
 * - 落库失败 → false，**不标记手工**、也不继续：`moveNodes` 已经把原因写进 toast，
 *   这里再补一句 `groupMsg`，用户不必靠猜。
 *
 * ⚠️ 只把**当前渲染序**（`members`）落库，那是筛选后的可见子集。被筛选挡住的成员
 * 不在这批里，`move_nodes_sync` 会保留它们、只重排被搬的这批 —— 这对本用例是
 * 正确行为：用户要求「跟随顶栏排序」时看见的就是这一批，挡住的那部分不该被
 * 卷进这次写库。
 *
 * 调用方的 `groupBusy` 必须**覆盖到这一步**：落库与随后的移动之间不能被连点插进来，
 * 否则第二次点击会拿一个尚未落库的次序去算落点。
 */
async function ensureManualOrder(): Promise<boolean> {
  const self = groupSelfId.value;
  if (self === null) return false;
  if (settings.manualOrderGroups.includes(self)) return true;
  const ids = members.value.map((m) => m.id);
  if (!ids.length) return false;
  // 先落库：把「用户现在看到的顺序」变成库内次序（失败就什么都不做）
  const ok = await store.moveNodes(ids, self, null);
  if (!ok) {
    groupMsg.value = "未能先保存当前显示顺序，没有改动排序（原因见上方提示）";
    return false;
  }
  // ① 重取列表：`store.items` 上的 `sort_order` 还是写库前的那一份。不重取就标记，
  //    接下来的渲染会按**旧值**重排（真实库里旧值与顶栏序是两种不同的排列），
  //    用户当场看到一次跳变 —— 那正是这一步要消灭的东西。
  await store.load();
  // ② 同级序也刚被整体重编号，挂载时那份缓存已经失效；重取一次。
  //    这一步失败就**不标记**（`loadAllSiblings` 已把原因写进 groupMsg）：
  //    库里的次序已经改成用户看到的这一份，显示上不会跳变，下次再点即可。
  if (!(await loadAllSiblings())) return false;
  // ③ 落库成功才标记；反过来会让显示当场跳回旧的库内序。
  settings.markManualOrder(self);
  return true;
}

/**
 * 上移 / 下移一位（`delta` = -1 / +1）。
 *
 * ⚠️ `index` 必须在**全量同级空间**里算，不能拿可见列表的行号：后端
 * `tree.rs::move_nodes_sync` 的 `siblings` 是**全部** `parent_id = 自己` 的行
 * （排除被搬者、按 `(sort_order, id)` 排），`index` 是「插进这个全量列的第几位」。
 * 真实同级序 A B C D、当前只看得见 B / D 时：UI 第 1 行 B 的「↓」若发可见下标 1，
 * 后端把 B 插回第 1 位 = 与移动前一模一样（`before.is_empty()`）→ 用户点了没反应。
 *
 * 落点推导（照 `tree.rs:267-291`）：`final_order = siblings[..at] ++ [mover] ++ siblings[at..]`，
 * 而 `siblings` **排除了 mover**。设 mover 在全量序里是第 `p` 位，把它自己移出后
 * 原本落在它后面的元素各前移一位，目标位就是 `p`，所以：
 * - 往后挪一位（↓）→ 目标位 `p + 1` → `at = p + 1`；
 * - 往前挪一位（↑）→ 目标位 `p - 1` → `at = p - 1`。
 * ⚠️ `p = 0` 时的 `p - 1` **绝不能发**：Rust 侧 `index as usize` 会把 -1 变成
 * 巨大的数、再被夹到 `siblings.len()`，等于把「上移」变成「丢到最下面」。
 * 全量序的首 / 末本来就置灰，这里再兜一层。
 */
async function moveMember(id: number, delta: -1 | 1): Promise<void> {
  if (groupBusy.value) return;
  const self = groupSelfId.value;
  if (self === null) return;
  groupBusy.value = true;
  let all: MediaItem[] | null = null;
  try {
    // ① 默认次序先落地 + 转手工排序（组已标记时是一次空转，不发请求）
    if (!(await ensureManualOrder())) return;
    // ② 之后的落点一律按**全量同级序**算（含被筛选挡住的成员）
    all = allSiblings.value ?? (await loadAllSiblings());
  } finally {
    groupBusy.value = false;
  }
  // 取数失败：`loadAllSiblings` 已经把原因写进 groupMsg —— 这里**一个请求都不发**，
  // 宁可不动，也不拿可见序"猜"一个下标（那正是这条缺陷的成因）
  if (!all) return;
  const p = all.findIndex((m) => m.id === id);
  if (p < 0) {
    groupMsg.value = "这个成员已经不在当前聚合里（可点「刷新」重查）";
    return;
  }
  const at = p + delta;
  if (at < 0 || at > all.length - 1) return; // 全量序的首 / 末：按钮本该置灰，兜底
  await runTreeOp(() => store.moveNodes([id], self, at));
}

/**
 * 「恢复默认排序」：只清掉手工标记，**不搬动任何成员**。
 *
 * 库里那些 `sort_order` 原样留着（它们本来就是历史重编号的产物，不代表用户意图）：
 * 用户下次再手工排一次时，`ensureManualOrder` 会重新以「当前看得见的顺序」落库，
 * 不会因为清标记时动过数据而跳变。
 */
function restoreDefaultOrder(): void {
  const self = groupSelfId.value;
  if (self === null) return;
  groupMsg.value = "";
  settings.clearManualOrder(self);
}

/** 移出这个聚合：挂回顶层。条目本身仍在库里，不是删除。 */
function removeMember(id: number): Promise<void> {
  return runTreeOp(() => store.moveNodes([id], null, null));
}

/** 脱离聚合：把自己挂回顶层。 */
function leaveGroup(): Promise<void> {
  const id = groupSelfId.value;
  if (id === null) return Promise.resolve();
  return runTreeOp(() => store.moveNodes([id], null, null));
}

/**
 * 取全部**真实**子级：不受当前视图筛选影响。
 *
 * ⚠️ 「取消聚合」必须搬走每一个成员的父级关系，所以**不能**拿 `members` ——
 * 那是筛选后的渲染列表（`orderedItems` 只含当前栏目 / 分类 / 标签 / 关键字命中的
 * 条目）。用它会漏掉被挡住的成员：`child_count` 归不了零、聚合区收不起来，
 * 而提示语却说「全部成员」——用户每点一次都漏同一批，等于死循环。
 *
 * ⚠️ `store.items` 同样不行：它也只是「最近一次筛选查询」的结果，在分类 / 标签
 * 视图下根本不是全库。所以这里显式发一次**不带任何筛选**的全量查询，再按
 * `parent_id` 过滤。零后端改动（`getMediaList` 的默认值就是「全部」）。
 *
 * 排序与 `members` 同一把尺子（`(sort_order, id)`）：后端 `roots_of` 保留输入序、
 * 再追加到顶层末尾，所以固定顺序 ⇒ 搬回顶层后的相对次序可复现。
 */
async function allChildren(): Promise<MediaItem[]> {
  const self = groupSelfId.value;
  if (self === null) return [];
  const all = await getMediaList({
    mediaType: "all", categoryId: null, tagId: null, unclassified: false,
  });
  return all
    .filter((it) => it.id !== self && (it.parent_id ?? null) === self)
    .sort((a, b) => (a.sort_order ?? 0) - (b.sort_order ?? 0) || a.id - b.id);
}

/**
 * 全量同级成员的缓存（含被当前筛选挡住的那些），按 `(sort_order, id)`。
 *
 * `null` = 还没取到 / 取失败。**置灰与落点都靠它**：拿不到时一律"什么都不做"
 * （见 `moveMember`）—— 拿可见序猜一个下标正是「点了没反应 / 插错位置」的成因。
 */
const allSiblings = ref<MediaItem[] | null>(null);

/**
 * 取一次全量同级并缓存；失败把原因写进 `groupMsg` 并返回 `null`。
 *
 * 调用方拿到 `null` 就**一个请求都不发**：落点算不出来时"猜"比不做更糟。
 */
async function loadAllSiblings(): Promise<MediaItem[] | null> {
  try {
    const list = await allChildren();
    allSiblings.value = list;
    return list;
  } catch (e) {
    allSiblings.value = null;
    groupMsg.value = "读取全部聚合成员失败，未做任何改动：" + String(e);
    return null;
  }
}

/** 在**全量同级序**里的位置（0-based）；还没取到 / 不在其中返回 -1。 */
function fullIndexOf(id: number): number {
  return allSiblings.value?.findIndex((m) => m.id === id) ?? -1;
}

/**
 * ↑ / ↓ 的置灰一律按**全量序**，可见序只负责渲染。
 *
 * 眼前第一行不一定是同级第一条（它前面可能还压着被筛选挡住的成员），
 * 所以不能再用可见下标 `i === 0` / `i === members.length - 1` 判边界 ——
 * 那会把「前面还有 A，只是没显示」的成员的上移按钮错当成没有了。
 * 全量取数失败时两份都返回 false（两个按钮都可用），点了会给出失败提示、
 * 且**不发**请求：宁可让用户点一下看到原因，也不静默置灰成"永远不能动"。
 */
function isFirstSibling(id: number): boolean {
  return fullIndexOf(id) === 0;
}

function isLastSibling(id: number): boolean {
  const all = allSiblings.value;
  const i = fullIndexOf(id);
  return all !== null && i >= 0 && i === all.length - 1;
}

/**
 * 取消聚合：把**全部**成员一次性挂回顶层。
 *
 * 搬完这条自己就不再是聚合了（`child_count` 归零），成员区随之收起——
 * 所以这一步必须一次搬完，不能留半截。
 *
 * ⚠️ 取数走 `allChildren()` / `loadAllSiblings()`（全库）而不是 `members`（筛选后）：
 * 后者会漏搬，留下一个永远收不起来的「半截聚合」。
 * ⚠️ 取数失败**绝不静默降级成「只搬看得见的」**：那比什么都不做更糟（用户以为
 * 成功了，实际留了一批在下面）。把原因写进 `groupMsg` 并中止，一个请求都不发。
 */
async function disbandGroup(): Promise<void> {
  if (groupBusy.value) return;
  groupBusy.value = true;
  groupMsg.value = "";
  let all: MediaItem[] | null = null;
  try {
    all = await loadAllSiblings();
  } finally {
    groupBusy.value = false;
  }
  if (!all) return; // 取数失败：`loadAllSiblings` 已把原因写进 groupMsg，不发请求
  if (!all.length) {
    // 按钮现在按「记录到的子级数」显示，而 `child_count` 可能已经过期（成员被删 /
    // 被移走）：点了没动静比报错更难查，所以这里必须说话，不能静默 return。
    groupMsg.value = "没有找到实际挂在这个聚合下的成员（记录数可能已过期，可点「刷新」重查）";
    return;
  }
  const ids = all.map((m) => m.id);
  // 文案里的 N 必须是**全量子级数**，不是「当前看得见的 M 个」：
  // 用户授权的是「把这一组解散掉」，说法与实际搬走的条数必须一致。
  const hidden = ids.length - members.value.length;
  const ok = await askConfirm(
    `把这个聚合下的全部 ${ids.length} 个成员挂回顶层？` +
      (hidden > 0 ? `\n（其中有 ${hidden} 个不在当前筛选内，这次会一起搬走）` : "") +
      "\n\n只是取消分组，条目与本地文件都不会被删除。",
    "取消聚合",
  );
  if (!ok) return;
  await runTreeOp(() => store.moveNodes(ids, null, null));
}

/** 把下拉里选中的条目加入这个聚合（追加到末尾）。 */
async function addMember(): Promise<void> {
  const self = groupSelfId.value;
  const id = addCandidate.value;
  if (self === null || id === null) return;
  // 双保险：候选里已经排除了自己、后代与祖先，这里再挡一次成环的两个方向
  // （祖先方向漏挡会做一次必然被后端拒的请求）
  if (id === self || ancestorsOf(self).has(id)) {
    groupMsg.value = "不能把条目加进它自己或它的上级聚合（会形成循环引用）";
    return;
  }
  await runTreeOp(() => store.moveNodes([id], self, null));
  addCandidate.value = null;
}

/** 撤销最近一次层级改动（撤销栈是全局的：封面墙的拖拽分组也在里面）。 */
function undoGroupOp(): Promise<void> {
  return runTreeOp(() => store.undoTree());
}

/** 打开某个成员的详情：同一个弹窗换一条数据，回退由顶部的「← 返回列表」负责。 */
function openMember(id: number): Promise<void> {
  return store.openDetail(id);
}

/** 打开所属聚合的详情。 */
function openParent(): Promise<void> {
  const pid = parentId.value;
  return pid === null ? Promise.resolve() : store.openDetail(pid);
}

/** 成员的信息编辑：复用详情页自己的编辑入口（由 App 打开编辑对话框）。 */
function editMember(m: MediaItem): void {
  emit("edit", m);
}

/* ---------------- TMDB 手动匹配 ---------------- */
/** 候选面板的开关（面板自己是弹窗，这里只管开合与「应用」后的动作）。 */
const showMatch = ref(false);

/* ---------------- 分类与标签 ---------------- */
/* 一条影视可以属于多个分类（多对多），也可以有多个标签。
   分类改动会立刻反映到左侧导航，标签则用于筛选。 */

const allCategories = computed(() => catStore.categories);
const myCategories = ref<Category[]>([]);
const myTags = ref<Tag[]>([]);
const tagInput = ref("");
const catMsg = ref("");
const tvdbBusy = ref(false);
/** 「分类」「标签」两块各自的展开状态，默认展开、互不干涉。 */
const catBlockOpen = ref(true);
const tagBlockOpen = ref(true);

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
  (id) => {
    showMatch.value = false;
    tagInput.value = "";
    catMsg.value = "";
    // 聚合区同样要归零：上一个条目的「待加入」候选 id 对新条目多半不成立；
    // 全量同级也要丢掉——它是**上一个条目**的兄弟序，留着会让 ↑↓ 算错落点。
    groupMsg.value = "";
    addCandidate.value = null;
    allSiblings.value = null;
    abortDrag();
    epSelected.value = [];
    if (id) {
      void pbStore.loadOne(id);
      // 进详情就先取一次全量同级：↑↓ 的置灰与落点都依赖它
      void loadAllSiblings();
    }
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

/** 用户确认了某个 TMDB 候选：走原来那条刮削链路，只是多带一个 tmdb id。 */
function applyTmdb(tmdbId: number): void {
  const d = detail.value;
  if (!d) return;
  store.scrape(d.media.id, tmdbId);
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
            <span v-if="detail.media.episode_count">{{ countLabel(detail.media.episode_count) }}</span>
            <span v-if="seenAt" class="seen">已看到 {{ seenAt }}</span>
            <span v-if="pb?.watched" class="seen">✓ 看过</span>
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
      <!-- 剧集的续播：先播「上次真正播放过的那一集」，没有记录再取下一个未看集。
           与封面墙的双击播放同一条命令（`play_episode` 不传 episodeId）。 -->
      <button
        v-if="!isSingleFile"
        class="btn primary"
        :disabled="!detail.media.episode_count"
        :title="`播放上次看到的那一集${seenAt ? `（${seenAt}）` : ''}；没有播放记录时从第一个未看集开始`"
        @click="resume"
      >▶ 继续播放</button>
      <button
        v-if="!isSingleFile"
        class="btn"
        :disabled="!detail.media.episode_count"
        title="从续播点起连播后续所有集；多个文件直接交给播放器，不生成临时播放列表"
        @click="playContinuous"
      >▶▶ 连播</button>
      <button
        class="btn"
        :title="pb?.watched ? '取消看过标记' : '标记为看过（只写数据库，不动本地文件）'"
        @click="toggleWatched"
      >{{ pb?.watched ? "↺ 取消已看" : "✓ 标记已看" }}</button>
      <button
        v-if="pb?.playCount"
        class="btn ghost"
        title="清除播放次数与续播点"
        @click="pbStore.clear(detail.media.id)"
      >清除观看记录</button>
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

    <p v-if="pbStore.toast" class="match-msg">{{ pbStore.toast }}</p>

    <!-- 归类与打标签：分类同时决定它在左侧导航里出现在哪个栏目下 -->
    <section class="classify">
      <!-- 与侧栏一致：两块各自独立折叠，默认展开 -->
      <CollapsibleSection v-model:open="catBlockOpen" title="分类" class="c-block">
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
      </CollapsibleSection>

      <CollapsibleSection v-model:open="tagBlockOpen" title="标签" class="c-block">
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
      </CollapsibleSection>
      <p v-if="catMsg" class="match-msg">{{ catMsg }}</p>
    </section>

    <!-- 手动匹配：候选面板（可筛选、可勾选、滚动加载），点了「应用」才刮削 -->
    <TmdbMatchDialog
      v-model:open="showMatch"
      :media="detail.media"
      @apply="applyTmdb"
    />

    <!--
      聚合：所属聚合 + 成员管理。电影与剧集同一套（判据只有 child_count / 有无子成员），
      老的剧集层级数据与封面墙的拖拽聚合写的是同一列 parent_id，这里照原样显示。
    -->
    <section v-if="isGroup || parentId !== null" class="group">
      <!-- 自己是别人的成员时：说清楚属于哪个聚合，并给出脱离的出路 -->
      <div v-if="parentId !== null" class="g-parent">
        <span class="g-label">所属聚合</span>
        <button
          v-if="!parentMissing"
          class="g-link"
          title="打开所属聚合的详情"
          @click="openParent"
        >{{ parentTitle }}</button>
        <!--
          取不到父条目时不能只留一个可点的 `#id`：点进去必然失败（后端查不到这条），
          用户拿到一句看不懂的报错。改成不可点的灰字 + 说明，出路是旁边的「脱离聚合」。
        -->
        <span v-else class="g-dead">{{ parentTitle }}（不在这个列表里）</span>
        <button
          class="btn small"
          :disabled="groupBusy"
          title="把自己挂回顶层（只是取消分组，不会删除条目与本地文件）"
          @click="leaveGroup"
        >脱离聚合（挂回顶层）</button>
      </div>
      <p v-if="parentMissing" class="g-hint">
        这个聚合的父条目不在这个列表里（可能已被删除，也可能被当前的类型 / 分类 /
        关键字筛选挡住）。想把它单独放出来，点上面的「脱离聚合」。
      </p>

      <template v-if="isGroup">
        <div class="g-bar">
          <span class="g-count">
            聚合 · 共 {{ members.length }} 个成员<template v-if="childCount !== members.length">
              （记录到 {{ childCount }} 个，当前能看到 {{ members.length }} 个）
            </template>
          </span>
          <!--
            只有用户亲手排过（在 manualOrderGroups 里）才给「恢复默认排序」：
            没排过的组本来就跟随顶栏排序，多一个按钮只会让人以为「我改过什么」。
            它**只清标记**，不搬动任何成员 —— 标题里必须说清楚。
          -->
          <button
            v-if="isManualOrder"
            class="btn small ghost"
            :disabled="groupBusy"
            title="改回按顶栏排序设置显示（不会搬动任何成员）"
            @click="restoreDefaultOrder"
          >↺ 恢复默认排序</button>
          <button
            v-if="childCount > 0 || members.length"
            class="btn small ghost"
            :disabled="groupBusy"
            title="把全部成员（含当前筛选外的）挂回顶层，本条目不再作为聚合"
            @click="disbandGroup"
          >取消聚合</button>
          <button
            class="btn small ghost"
            :disabled="groupBusy || !store.undoDepth"
            title="撤销最近一次层级改动（封面墙的拖拽分组也在同一条撤销栈里）"
            @click="undoGroupOp"
          >↶ 撤销</button>
        </div>

        <!-- 空聚合：child_count 说有子级但当前筛选下一个都看不到，不能渲染成空白 -->
        <p v-if="!members.length" class="g-empty">
          这个聚合暂时没有成员（记录到 {{ childCount }} 个）。
          成员可能已被删除，也可能不在当前筛选范围内 —— 可以「刷新」或换个栏目 / 类型再看；
          也可以直接「取消聚合」，它会把全部成员（含筛选外的）一起挂回顶层。
          <button class="btn small" :disabled="groupBusy" @click="refreshGroup">刷新</button>
        </p>

        <ul v-else class="g-list">
          <li v-for="(m, i) in members" :key="m.id">
            <span class="g-no">{{ i + 1 }}</span>
            <img
              v-if="imageUrl(m.poster_path, 'w92')"
              class="g-thumb"
              :src="imageUrl(m.poster_path, 'w92')!"
              :alt="m.display_title"
            />
            <button class="g-title" :title="m.display_title" @click="openMember(m.id)">
              {{ m.display_title }}
            </button>
            <span class="g-meta">
              {{ typeLabel(m.type) }}<template v-if="m.year"> · {{ m.year }}</template>
            </span>
            <button
              class="btn small ghost"
              :disabled="groupBusy || isFirstSibling(m.id)"
              title="上移一位（按全部成员的真实次序，含当前筛选外的）；本组若正跟随顶栏排序，第一次点击会先把现在看到的顺序存下来"
              @click="moveMember(m.id, -1)"
            >↑</button>
            <button
              class="btn small ghost"
              :disabled="groupBusy || isLastSibling(m.id)"
              title="下移一位（按全部成员的真实次序，含当前筛选外的）；本组若正跟随顶栏排序，第一次点击会先把现在看到的顺序存下来"
              @click="moveMember(m.id, 1)"
            >↓</button>
            <button
              class="btn small"
              :disabled="groupBusy"
              title="修改这个成员的名称 / 类型 / 年份 / 流派 / 简介"
              @click="editMember(m)"
            >✎</button>
            <button
              class="btn small danger"
              :disabled="groupBusy"
              title="移出这个聚合，挂回顶层（不会删除条目与本地文件）"
              @click="removeMember(m.id)"
            >移出</button>
          </li>
        </ul>

        <!-- 加入成员：候选里已排除自己与自己的后代，不会形成环 -->
        <div v-if="addCandidates.length" class="g-add">
          <select v-model="addCandidate" class="g-select">
            <option :value="null">选择要加入的条目…</option>
            <option v-for="c in addCandidates" :key="c.id" :value="c.id">
              {{ c.display_title }}
            </option>
          </select>
          <button
            class="btn small"
            :disabled="groupBusy || addCandidate === null"
            @click="addMember"
          >＋ 加入聚合</button>
        </div>
      </template>

      <p v-if="groupMsg" class="match-msg">{{ groupMsg }}</p>
    </section>

    <section
      v-if="seasons.length"
      class="episodes"
      :class="{ dragging }"
      ref="epRoot"
      @pointerover="onEpPointerMove"
      @pointermove="onEpPointerMove"
      @touchmove="onEpTouchMove"
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
        <h3>{{ seasonTitle(s.season) }}</h3>
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
/* 观看状态：绿色系，与「类型标签」的强调色区分开 */
.seen {
  padding: 2px 8px; border-radius: 4px; font-size: var(--fs-12); font-weight: 600;
  background: rgba(16, 185, 129, .16); color: #10b981;
}
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

.match-msg { margin: 0; font-size: var(--fs-13); color: var(--muted); }

.classify {
  display: flex; flex-direction: column; gap: calc(var(--space) * 4);
  padding: calc(var(--space) * 4); border-radius: var(--radius);
  background: var(--surface); border: 1px solid var(--border);
}
/*
 * 归类卡片里的两块：折叠结构在 `CollapsibleSection.vue`，这里只把标题的字号字重
 * 与间距通过 CSS 变量传进去（scoped 样式穿不进子组件内部）。
 * 颜色只给标题，不写在容器上——否则会把下面 chip / 输入框的文字一起染灰。
 */
.c-block {
  display: flex; flex-direction: column; gap: 8px;
  --cs-head-pad: 0;
  --cs-body-gap: 8px;
  --cs-title-fs: var(--fs-13);
  --cs-title-fw: 600;
  --cs-title-color: var(--muted);
}
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

/* 聚合区：所属聚合一行 + 成员列表。与单集列表同构，但类名独立（两套数据互不相干） */
.group { display: flex; flex-direction: column; gap: calc(var(--space) * 3); }
.g-parent {
  display: flex; align-items: center; gap: 10px; flex-wrap: wrap;
  padding: calc(var(--space) * 2) calc(var(--space) * 3);
  border-radius: var(--radius-s); background: var(--surface); border: 1px solid var(--border);
}
.g-label { flex: none; font-size: var(--fs-12); color: var(--muted); }
/* 所属聚合的名字是个按钮（点了打开那个聚合），所以按链接的样子画 */
.g-link {
  flex: 1; min-width: 0; padding: 0; border: 0; background: none; cursor: pointer;
  font-size: var(--fs-13); color: var(--text); text-align: left;
  overflow: hidden; text-overflow: ellipsis; white-space: nowrap;
}
.g-link:hover { color: var(--accent); text-decoration: underline; }
/* 取不到父条目时画成灰字：它不是链接，点了也进不去 */
.g-dead {
  flex: 1; min-width: 0; font-size: var(--fs-13); color: var(--muted); font-style: italic;
  overflow: hidden; text-overflow: ellipsis; white-space: nowrap;
}
.g-hint {
  margin: 0; padding: 0 calc(var(--space) * 3);
  font-size: var(--fs-12); color: var(--muted); line-height: 1.7;
}
.g-bar {
  display: flex; align-items: center; gap: 10px; padding: calc(var(--space) * 2) calc(var(--space) * 3);
  border-radius: var(--radius-s); background: var(--surface); border: 1px solid var(--border);
}
.g-count { flex: 1; font-size: var(--fs-13); color: var(--text-2); }
.g-empty {
  display: flex; align-items: center; gap: 10px; flex-wrap: wrap; margin: 0;
  font-size: var(--fs-13); color: var(--muted);
}
.g-list { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; gap: 6px; }
.g-list li { display: flex; align-items: center; gap: 12px; padding: 8px 12px; border-radius: var(--radius-s); background: var(--surface); border: 1px solid var(--border); }
.g-no { flex: none; width: 34px; font-size: var(--fs-12); color: var(--muted); font-weight: 600; }
.g-thumb { width: 40px; border-radius: 4px; flex: none; display: block; }
/* 成员标题是按钮（点了打开它的详情），左对齐 + 超长省略 */
.g-title {
  flex: 1; min-width: 0; padding: 0; border: 0; background: none; cursor: pointer;
  font-size: var(--fs-13); color: var(--text); text-align: left;
  overflow: hidden; text-overflow: ellipsis; white-space: nowrap;
}
.g-title:hover { color: var(--accent); }
.g-meta { flex: none; font-size: var(--fs-12); color: var(--muted); }
.g-add { display: flex; align-items: center; gap: 8px; }
.g-select {
  flex: 1; min-width: 180px; padding: 6px 10px; border-radius: var(--radius-s);
  font-size: var(--fs-12); border: 1px solid var(--border);
  background: var(--surface-2); color: var(--text); outline: none;
}
.g-select:focus { border-color: var(--accent); }

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
