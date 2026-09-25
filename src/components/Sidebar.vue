<script setup lang="ts">
import type { ViewKey } from "../types";

defineProps<{ modelValue: ViewKey }>();
const emit = defineEmits<{ (e: "update:modelValue", v: ViewKey): void }>();
const entries: { key: "all" | "movie" | "tv"; label: string }[] = [
  { key: "all", label: "📁 全部" },
  { key: "movie", label: "🎬 电影" },
  { key: "tv", label: "📺 剧集" },
];
</script>

<template>
  <aside class="sidebar">
    <button
      v-for="e in entries"
      :key="e.key"
      class="item"
      :class="{ active: e.key === modelValue }"
      @click="emit('update:modelValue', e.key)"
    >{{ e.label }}</button>
    <button
      class="item settings"
      :class="{ active: modelValue === 'settings' }"
      @click="emit('update:modelValue', 'settings')"
    >⚙️ 设置</button>
  </aside>
</template>

<style scoped>
.sidebar { width: 160px; background: var(--surface); padding: calc(var(--space) * 4) 0; }
.item { display: block; width: 100%; padding: 12px 16px; border: 0; background: none;
  color: var(--muted); text-align: left; cursor: pointer; }
.item.active { color: var(--text); background: rgba(255,255,255,.06); }
.item.settings { margin-top: auto; }
</style>