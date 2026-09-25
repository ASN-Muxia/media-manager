<script setup lang="ts">
import { computed } from "vue";
import { imageUrl, type MediaItem } from "../api/commands";

const props = defineProps<{ item: MediaItem }>();
const emit = defineEmits<{ (e: "open", id: number): void }>();

const poster = computed(() => imageUrl(props.item.poster_path, "w342"));
const isTv = computed(() => props.item.type === "tv");
const score = computed(() =>
  props.item.vote_average ? props.item.vote_average.toFixed(1) : null,
);
</script>

<template>
  <button class="card" :title="item.display_title" @click="emit('open', item.id)">
    <div class="poster">
      <img v-if="poster" :src="poster" :alt="item.display_title" loading="lazy" />
      <div v-else class="placeholder">
        <span class="icon">{{ isTv ? "📺" : "🎬" }}</span>
        <span class="ph-title">{{ item.display_title }}</span>
      </div>
      <span class="badge">{{ isTv ? "剧集" : "电影" }}</span>
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
  border-radius: 8px; background: var(--surface);
  transition: transform .18s ease, box-shadow .18s ease;
}
.card:hover .poster { transform: translateY(-3px); box-shadow: 0 8px 22px rgba(0, 0, 0, .45); }
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
.meta { display: flex; flex-direction: column; gap: 2px; }
.title { font-size: 13px; line-height: 1.3; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.sub { display: flex; gap: 8px; font-size: 12px; color: var(--muted); }
.score { color: #f5c518; }
</style>
