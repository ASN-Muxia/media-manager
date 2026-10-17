<script setup lang="ts">
import { computed, onBeforeUnmount } from "vue";
import { imageUrl, type MediaItem } from "../api/commands";
import { formatScore, typeIcon, typeLabel } from "../constants/mediaType";

const props = defineProps<{
  item: MediaItem;
  /** 多选模式下：整张卡的点击变成「勾选 / 取消」，不再进详情。 */
  selectable?: boolean;
  selected?: boolean;
  /** 是否已看（由 `playbackStore` 给出）。有集数进度时以 `progress` 为准，这个只兜底电影。 */
  watched?: boolean;
  /** 剧集进度文本，如 `3/25`；电影为空。 */
  progress?: string;
}>();
/** 多选模式下点击时携带的修饰键（Shift = 区间，Ctrl / Cmd = 单张切换）。 */
export interface SelectMods {
  shift?: boolean;
  ctrl?: boolean;
  meta?: boolean;
}
const emit = defineEmits<{
  (e: "open", id: number): void;
  (e: "select", payload: { id: number; mods: SelectMods }): void;
  /** 双击 = 直接播放这一条（剧集播「上次那一集」，见后端 `play_episode`）。 */
  (e: "play", id: number): void;
}>();

const poster = computed(() => imageUrl(props.item.poster_path, "w342"));
const isTv = computed(() => props.item.type === "tv");
/** 类型标签：判定不出来的显示「待确认」，不再冒充电影。文案统一来自 `constants/mediaType`。 */
const badge = computed(() => typeLabel(props.item.type));
const placeholderIcon = computed(() => typeIcon(props.item.type));
const score = computed(() => formatScore(props.item.vote_average));

/** 观看三态：`none` 未观看 / `watching` 观看中 / `done` 已看完。 */
type SeenState = "none" | "watching" | "done";
/** 从 `w/N` 文本里解析出的集数。 */
interface EpisodeCount {
  seen: number;
  total: number;
}

/**
 * 解析 `3/25` 这种集数进度；解析不出来（空串、电影没集数、格式不对）返回 null。
 * 只认纯数字 + 斜杠，避免把「第 3 季」之类将来可能出现的文案误判成进度。
 */
function parseProgress(text: string | undefined): EpisodeCount | null {
  if (!text) return null;
  const m = /^(\d+)\s*\/\s*(\d+)$/.exec(text.trim());
  if (!m) return null;
  const seen = Number(m[1]);
  const total = Number(m[2]);
  if (total <= 0) return null;
  return { seen, total };
}

/** 集数进度（`progress` 的唯一解析结果），电影为 null。 */
const counts = computed(() => parseProgress(props.progress));

/**
 * 观看三态 —— **全组件唯一的数据源**。
 *
 * `progress` 与 `watched` 两个入口在这里一次性归并，模板和其它 computed 只认这一个结果，
 * 不再各自判断一遍，从根上避免「整部已看但角标显示 0/25」这类数值错位。
 */
const seenState = computed<SeenState>(() => {
  const c = counts.value;
  // 有集数进度时一律以它为准，不看 `watched`：
  // 后端「整部标记 OR 所有集已看」是或关系，整部标记过之后又新扫进来一集的条目，
  // 会同时带 watched=true 和 25/26。这时按 watched 判会把角标高亮成「看过」，
  // 而用户实际还有集没看 —— 那是假信息。
  if (c) {
    // `0/25` 属于未观看：一集没看就不该显示进度数字。
    if (c.seen <= 0) return "none";
    return c.seen >= c.total ? "done" : "watching";
  }
  // 没有集数（电影）：整部标记是唯一依据；没标记就是未观看。
  return props.watched ? "done" : "none";
});

/**
 * 角标文案：剧集显示集级进度（`3/25`，追完就是 `25/25`），电影只显示「看过」。
 *
 * 空串 = 不显示。追完仍显示数字是有意的：一眼能看出「这部追完了」。
 */
const seenText = computed(() => {
  if (seenState.value === "none") return "";
  const c = counts.value;
  if (c && c.seen > 0) return `${c.seen}/${c.total}`;
  return "看过";
});

/** 角标悬浮说明：两态各一句，不说「看了多少分钟」——口径是集，不是秒。 */
const seenTitle = computed(() =>
  seenState.value === "done"
    ? "已看完（集级记录，非播放进度）"
    : "观看中（集级记录，非播放进度）",
);

/**
 * 单击「稍等一下再打开详情」的等待窗口（毫秒）。
 *
 * 为什么必须等：**双击的第二下要落在同一张卡片上**才能派发 `dblclick`。
 * 而单击是立刻 `emit("open")` 的，`App.vue` 里 `MediaDetail` 是 `v-if`、
 * 封面墙是 `v-else` —— 详情一开，网格当场卸载，第二下就落在了详情页上，
 * `dblclick` 永远等不到。所以单击先挂起，双击来了就取消它。
 *
 * 250ms 是浏览器判定双击的常用窗口：再短会把"手慢一点的双击"当成两次单击，
 * 再长则点开详情的迟滞肉眼可见。
 */
const DBLCLICK_MS = 250;

let openTimer = 0;

function cancelPendingOpen(): void {
  if (openTimer) {
    window.clearTimeout(openTimer);
    openTimer = 0;
  }
}

/**
 * 点击卡片。
 *
 * 多选模式下把**修饰键**一起交上去 —— Shift（区间）/ Ctrl·Cmd（单张切换）
 * 的语义判定在 `utils/selection.ts` 里（纯函数、可单测），这里只负责把
 * `MouseEvent` 上的三个布尔翻译过去。组件自己不判断选中集合，仍然由父级持有。
 *
 * 多选模式下**不延迟**：勾选要即时反馈，而且那时双击没有播放语义。
 */
function onClick(ev?: MouseEvent): void {
  if (props.selectable) {
    emit("select", {
      id: props.item.id,
      mods: {
        shift: !!ev?.shiftKey,
        ctrl: !!ev?.ctrlKey,
        meta: !!ev?.metaKey,
      },
    });
    return;
  }
  // 键盘触发的 click（Enter / Space）在浏览器里带 `detail === 0`，鼠标点击是 1、2、3…
  // 递增。键盘不可能「双击」，不该让用户白等一个窗口。
  //
  // ⚠️ 判据必须写成 `detail === 0`（严格等于），不能写 `!ev.detail`：
  // 测试里 `trigger("click")` 造出来的 `MouseEvent` 也是 `detail === 0`，
  // 宽松判据会让整条延迟逻辑在单测里静默失效（用例仍然"绿"，但测的是另一条路径）。
  // 真实使用中这条分支只命中键盘，是安全的。
  if (ev && ev.detail === 0) {
    cancelPendingOpen();
    emit("open", props.item.id);
    return;
  }
  cancelPendingOpen();
  openTimer = window.setTimeout(() => {
    openTimer = 0;
    emit("open", props.item.id);
  }, DBLCLICK_MS);
}

/**
 * 双击卡片 = 直接播放。
 *
 * 三条必须守住：
 * 1. **多选模式下不播**：那时双击的语义是「勾一下再勾一下」，播出去等于
 *    在勾选途中突然弹出播放器。判据与单击完全一致（`selectable`）。
 * 2. **把挂起的那次单击取消掉**：否则「双击播放」之后还会弹出一层层详情页。
 * 3. **只发事件、不自己调 store**：卡片是无 store 依赖的纯展示组件
 *    （它的 spec 全是 `mount(Card, { props })` 直挂），播放由父级接。
 */
function onDblClick(): void {
  if (props.selectable) return;
  cancelPendingOpen();
  emit("play", props.item.id);
}

onBeforeUnmount(cancelPendingOpen);
</script>

<template>
  <button
    class="card"
    :class="{ picked: selected, selectable }"
    :data-id="item.id"
    :title="selectable ? '点击勾选 / 取消勾选' : `${item.display_title}（双击直接播放）`"
    @click="onClick($event)"
    @dblclick="onDblClick"
  >
    <div class="poster">
      <span v-if="selectable" class="check" :class="{ on: selected }">{{ selected ? "✓" : "" }}</span>
      <!--
        ⚠️ `:draggable="false"` 是**必须**的，不是可选的美化项。

        \`<img>\` 天生 `draggable`（浏览器默认行为）。拖拽源只允许是 PosterGrid 的
        \`.cell\` 一个 —— 只要图片自己可拖，按住海报一拖就是**图片拖拽**
        （载荷是图片 URL、放下什么都不发生），鼠标上画的正是那个"禁止"图标。
        这里写死 `false` 而不是靠外层在 pointerdown 里现改：那是"按过之后才准"，
        而浏览器在**还没按**的时候就已经把这张图当成可拖源了。
        CSS 里的 `-webkit-user-drag: none` 是同一件事第二道保险
        （WebView2 上 `draggable=false` 偶尔被 `lazy` 图片的默认行为盖回去）。
      -->
      <img
        v-if="poster"
        :src="poster"
        :alt="item.display_title"
        loading="lazy"
        :draggable="false"
      />
      <div v-else class="placeholder">
        <span class="icon">{{ placeholderIcon }}</span>
        <span class="ph-title">{{ item.display_title }}</span>
      </div>
      <span class="badge">{{ badge }}</span>
      <span v-if="isTv && item.episode_count" class="eps">{{ item.episode_count }} 集</span>
      <span v-if="!item.poster_path" class="unscraped" title="尚未刮削">?</span>
      <!-- 观看角标：集级进度（不是秒级），三态统一由 `seenState` 决定画不画、画什么 -->
      <span
        v-if="!selectable && seenText"
        class="seen"
        :class="{ done: seenState === 'done' }"
        :title="seenTitle"
      >{{ seenText }}</span>
    </div>
    <div class="meta">
      <span class="title" :title="item.display_title">{{ item.display_title }}</span>
      <span class="sub">
        <span v-if="item.year">{{ item.year }}</span>
        <span v-if="score" class="score">★ {{ score }}</span>
      </span>
    </div>
  </button>
</template>

<style scoped>
/* 宽度完全交给网格（列宽来自 `--card-min`）：这里不写死任何像素宽，
   min-width: 0 是为了让标题的省略号在窄列下也能生效而不撑破卡片。 */
.card {
  display: flex; flex-direction: column; gap: calc(var(--space) * 2);
  width: 100%; min-width: 0; max-width: 100%;
  padding: 0; border: 0; background: none; color: inherit; cursor: pointer; text-align: left;
  /* 标题 / 年份都是可选中的文字。按住它们一拖，浏览器起的是**文本拖拽**，
     光标同样是禁止图标、放下也什么都不发生 —— 与"目标拒绝接收"长得一模一样。
     拖拽源只该是 PosterGrid 的 `.cell` 一个，文字永远不参与。 */
  user-select: none; -webkit-user-select: none;
}
/* 海报区只锁比例不锁尺寸：任意列宽下都是 2:3，图片 cover 裁切而非拉伸 */
.poster {
  position: relative; aspect-ratio: 2 / 3; width: 100%; overflow: hidden;
  border-radius: var(--radius); background: var(--surface);
  border: 1px solid var(--border);
  transition: transform .18s ease, box-shadow .18s ease;
}
.card:hover .poster { transform: translateY(-3px); box-shadow: var(--shadow); }
/* 多选模式：勾上的卡片给个明确的边框反馈 */
.card.picked .poster { border-color: var(--accent); box-shadow: 0 0 0 2px var(--accent-soft); }
.check {
  position: absolute; top: 6px; left: 6px; z-index: 1; width: 20px; height: 20px;
  display: flex; align-items: center; justify-content: center; border-radius: 50%;
  font-size: 12px; font-weight: 700; color: #fff;
  background: rgba(0, 0, 0, .55); border: 1px solid rgba(255, 255, 255, .6);
}
.check.on { background: var(--accent); border-color: transparent; }
/* 多选模式下左上角让给勾选框，类型标签挪到左下角 */
.card.selectable .badge { top: auto; bottom: 6px; }
/* `-webkit-user-drag: none`：WebView2/Chromium 里图片默认可以被"拖出去"，
   光标就是禁止图标。与模板上的 `:draggable="false"` 是同一件事的两道保险。 */
.poster img {
  width: 100%; height: 100%; object-fit: cover; display: block;
  -webkit-user-drag: none; user-select: none;
}
.placeholder {
  width: 100%; height: 100%; display: flex; flex-direction: column;
  align-items: center; justify-content: center; gap: 8px; padding: 10px; text-align: center;
}
.icon { font-size: 28px; opacity: .8; }
.ph-title { font-size: 12px; color: var(--muted); line-height: 1.35; word-break: break-word; }
/* 四个角标在窄列下不能互相压住：各自最多占半宽，超出省略 */
.badge, .eps, .seen { max-width: calc(50% - 8px); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.badge {
  position: absolute; top: 6px; left: 6px; padding: 2px 6px; border-radius: 4px;
  font-size: 11px; background: rgba(0, 0, 0, .68); color: #fff;
}
.eps {
  position: absolute; bottom: 6px; right: 6px; padding: 2px 6px; border-radius: 4px;
  font-size: 11px; background: rgba(0, 0, 0, .68); color: #fff;
}
/* 观看角标：左下角（右下角是集数，左上角是类型标签）。
   多选模式下左上/左下都让给了勾选框与类型标签，这里直接不画，避免叠在一起。 */
/* 观看中：灰底白字，和「已看完」的绿底拉开区分 */
.seen {
  position: absolute; bottom: 6px; left: 6px; padding: 2px 6px; border-radius: 4px;
  font-size: 11px; font-weight: 600; background: rgba(0, 0, 0, .72); color: #fff;
  border: 1px solid var(--border-strong);
}
.seen.done { background: rgba(16, 185, 129, .92); border-color: transparent; }
.unscraped {
  position: absolute; top: 6px; right: 6px; width: 18px; height: 18px;
  display: flex; align-items: center; justify-content: center; border-radius: 50%;
  font-size: 12px; background: var(--accent); color: #fff;
}
.meta { display: flex; flex-direction: column; gap: 2px; min-width: 0; padding: 0 2px; }
.title { font-size: var(--fs-13); line-height: 1.35; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.sub { display: flex; gap: 8px; font-size: var(--fs-12); color: var(--muted); }
.score { color: var(--score); }
</style>
