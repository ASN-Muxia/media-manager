<script setup lang="ts">
import type { ViewKey } from "../types";

defineProps<{ modelValue: ViewKey }>();
const emit = defineEmits<{ (e: "update:modelValue", v: ViewKey): void }>();
const entries: { key: "all" | "movie" | "tv"; label: string; icon: string }[] = [
  { key: "all", label: "全部", icon: "📁" },
  { key: "movie", label: "电影", icon: "🎬" },
  { key: "tv", label: "剧集", icon: "📺" },
];
</script>

<template>
  <aside class="sidebar">
    <div class="brand">影视库</div>
    <nav class="nav">
      <button
        v-for="e in entries"
        :key="e.key"
        class="item"
        :class="{ active: e.key === modelValue }"
        @click="emit('update:modelValue', e.key)"
      ><span class="ico">{{ e.icon }}</span>{{ e.label }}</button>
    </nav>
    <button
      class="item settings"
      :class="{ active: modelValue === 'settings' }"
      @click="emit('update:modelValue', 'settings')"
    ><span class="ico">⚙️</span>设置</button>
  </aside>
</template>

<style scoped>
.sidebar {
  width: 176px; flex: none; display: flex; flex-direction: column;
  padding: calc(var(--space) * 3) calc(var(--space) * 2) calc(var(--space) * 3);
  background: var(--bg-soft); border-right: 1px solid var(--border);
}
.brand {
  padding: calc(var(--space) * 2) calc(var(--space) * 3);
  margin-bottom: calc(var(--space) * 2);
  font-size: var(--fs-15); font-weight: 650; letter-spacing: 0.5px;
  color: var(--text);
}
.nav { display: flex; flex-direction: column; gap: 2px; }
.item {
  position: relative; display: flex; align-items: center; gap: 10px; width: 100%;
  padding: 9px 12px; border: 0; border-radius: var(--radius-s);
  background: none; color: var(--muted); text-align: left; font-size: var(--fs-14);
  transition: background 0.15s ease, color 0.15s ease;
}
.item:hover { background: rgba(255, 255, 255, 0.05); color: var(--text); }
.item.active { color: var(--text); background: var(--surface-2); font-weight: 600; }
.item.active::before {
  content: ""; position: absolute; left: 0; top: 20%; bottom: 20%; width: 3px;
  border-radius: 3px; background: var(--accent);
}
.item.active .ico { filter: none; }
.ico { font-size: 15px; filter: grayscale(0.4); }
.item.settings { margin-top: auto; }
</style>
