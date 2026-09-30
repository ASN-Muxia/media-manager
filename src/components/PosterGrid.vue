<script setup lang="ts">
import { ref } from "vue";
import PosterCard from "./PosterCard.vue";
import { useSwipeSelect } from "../composables/useSwipeSelect";
import type { MediaItem } from "../api/commands";

const props = defineProps<{
  items: MediaItem[];
  /** 多选模式（顶栏「选择」开关）：勾选态由父级持有，这里只转发。 */
  selectable?: boolean;
  selected?: number[];
}>();
const emit = defineEmits<{
  (e: "open", id: number): void;
  (e: "select", id: number): void;
  /** 滑动多选：松手时**一次性**提交整批结果（不逐条 select）。 */
  (e: "select-many", ids: number[]): void;
}>();

/* ---------------- 滑动多选 ---------------- */
/* 在卡片上按下并拖动即可连续勾选；触摸下需要先长按（否则会抢掉页面滚动手势）。
   细节与边界约定都在 `composables/useSwipeSelect.ts` 与 `utils/dragSelect.ts`。 */

const grid = ref<HTMLElement | null>(null);

const swipe = useSwipeSelect({
  root: grid,
  selected: () => props.selected ?? [],
  order: () => props.items.map((i) => i.id),
  commit: (ids) => emit("select-many", ids),
  itemSelector: "[data-id]",
  // ⚠️ 这里**不能**把 `button` 写进 blockSelector：卡片根元素本身就是 button，
  // 写了就永远起不来拖动。卡片内部也没有别的交互控件，所以留空。
  blockSelector: "input, a",
});

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
</script>

<template>
  <div
    v-if="items.length"
    ref="grid"
    class="grid"
    :class="{ dragging: swipe.dragging.value }"
    @pointerdown="swipe.onPointerDown"
    @pointermove="swipe.onPointerMove"
  >
    <PosterCard
      v-for="it in items"
      :key="it.id"
      :item="it"
      :selectable="props.selectable"
      :selected="picked(it.id)"
      @open="(id) => emit('open', id)"
      @select="(id) => emit('select', id)"
    />
  </div>
  <div v-else class="empty">
    <p class="big">媒体库还是空的</p>
    <p class="hint">请到「设置」添加媒体目录，然后点顶部「扫描媒体库」。</p>
  </div>
</template>

<style scoped>
.grid {
  display: grid; gap: calc(var(--space) * 5);
  grid-template-columns: repeat(auto-fill, minmax(150px, 1fr));
  /* 允许竖向滚动（触摸下长按进入多选后，滚动由 touchmove 的 preventDefault 接管） */
  touch-action: pan-y;
}
/* 拖动期间禁掉文字选择：不然一路划过去会把标题选蓝 */
.grid.dragging { user-select: none; }
.empty { padding: calc(var(--space) * 16) 0; text-align: center; color: var(--muted); }
.empty .big { margin: 0 0 8px; font-size: 16px; color: var(--text); }
.empty .hint { margin: 0; font-size: 13px; }
</style>
