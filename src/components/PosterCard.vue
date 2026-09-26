<script setup lang="ts">
import { computed } from "vue";
import { imageUrl, type MediaItem } from "../api/commands";

const props = defineProps<{
  item: MediaItem;
  /** 多选模式下：整张卡的点击变成「勾选 / 取消」，不再进详情。 */
  selectable?: boolean;
  selected?: boolean;
}>();
const emit = defineEmits<{ (e: "open", id: number): void; (e: "select", id: number): void }>();

const poster = computed(() => imageUrl(props.item.poster_path, "w342"));
const isTv = computed(() => props.item.type === "tv");
/** 类型标签：判定不出来的显示「待确认」，不再冒充电影。 */
const badge = computed(() =>
  props.item.type === "tv" ? "剧集" : props.item.type === "unknown" ? "待确认" : "电影",
);
const placeholderIcon = computed(() =>
  props.item.type === "tv" ? "📺" : props.item.type === "unknown" ? "❓" : "🎬",
);
const score = computed(() =>
  props.item.vote_average ? props.item.vote_average.toFixed(1) : null,
);

function onClick(): void {
  if (props.selectable) emit("select", props.item.id);
  else emit("open", props.item.id);
}
</script>

<template>
  <button
    class="card"
    :class="{ picked: selected, selectable }"
    :title="selectable ? '点击勾选 / 取消勾选' : item.display_title"
    @click="onClick"
  >
    <div class="poster">
      <span v-if="selectable" class="check" :class="{ on: selected }">{{ selected ? "✓" : "" }}</span>
      <img v-if="poster" :src="poster" :alt="item.display_title" loading="lazy" />
      <div v-else class="placeholder">
        <span class="icon">{{ placeholderIcon }}</span>
        <span class="ph-title">{{ item.display_title }}</span>
      </div>
      <span class="badge">{{ badge }}</span>
      <span v-if="isTv && item.episode_count" class="eps">{{ item.episode_count }} 集</span>
      <span v-if="!item.poster_path" class="unscraped" title="尚未刮削">?</span>
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
.card {
  display: flex; flex-direction: column; gap: calc(var(--space) * 2);
  padding: 0; border: 0; background: none; color: inherit; cursor: pointer; text-align: left;
}
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
.poster img { width: 100%; height: 100%; object-fit: cover; display: block; }
.placeholder {
  width: 100%; height: 100%; display: flex; flex-direction: column;
  align-items: center; justify-content: center; gap: 8px; padding: 10px; text-align: center;
}
.icon { font-size: 28px; opacity: .8; }
.ph-title { font-size: 12px; color: var(--muted); line-height: 1.35; word-break: break-word; }
.badge {
  position: absolute; top: 6px; left: 6px; padding: 2px 6px; border-radius: 4px;
  font-size: 11px; background: rgba(0, 0, 0, .68); color: #fff;
}
.eps {
  position: absolute; bottom: 6px; right: 6px; padding: 2px 6px; border-radius: 4px;
  font-size: 11px; background: rgba(0, 0, 0, .68); color: #fff;
}
.unscraped {
  position: absolute; top: 6px; right: 6px; width: 18px; height: 18px;
  display: flex; align-items: center; justify-content: center; border-radius: 50%;
  font-size: 12px; background: var(--accent); color: #fff;
}
.meta { display: flex; flex-direction: column; gap: 2px; padding: 0 2px; }
.title { font-size: var(--fs-13); line-height: 1.35; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.sub { display: flex; gap: 8px; font-size: var(--fs-12); color: var(--muted); }
.score { color: var(--score); }
</style>
