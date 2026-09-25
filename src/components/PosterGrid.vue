<script setup lang="ts">
import PosterCard from "./PosterCard.vue";
import type { MediaItem } from "../api/commands";

defineProps<{ items: MediaItem[] }>();
const emit = defineEmits<{ (e: "open", id: number): void }>();
</script>

<template>
  <div v-if="items.length" class="grid">
    <PosterCard
      v-for="it in items"
      :key="it.id"
      :item="it"
      @open="(id) => emit('open', id)"
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
}
.empty { padding: calc(var(--space) * 16) 0; text-align: center; color: var(--muted); }
.empty .big { margin: 0 0 8px; font-size: 16px; color: var(--text); }
.empty .hint { margin: 0; font-size: 13px; }
</style>
