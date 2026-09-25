<script setup lang="ts">
import { onMounted, ref, watch } from "vue";
import Sidebar from "./components/Sidebar.vue";
import PosterGrid from "./components/PosterGrid.vue";
import SettingsPanel from "./components/SettingsPanel.vue";
import { useSettingsStore } from "./stores/settingsStore";
import { invoke } from "./api/commands";
import type { ViewKey } from "./types";

const activeView = ref<ViewKey>("all");
const items = ref<any[]>([]);

async function loadItems(): Promise<void> {
  if (activeView.value === "settings") return;
  try {
    items.value = await invoke<any[]>("get_media_list", { type: activeView.value });
  } catch {
    // 该命令由模块③/⑤ 提供；在此之前保持空列表，避免启动即报错。
    items.value = [];
  }
}

watch(activeView, loadItems);

onMounted(async () => {
  const settings = useSettingsStore();
  await settings.load();
  await loadItems();
});
</script>

<template>
  <div class="layout">
    <Sidebar v-model="activeView" />
    <main class="content">
      <SettingsPanel v-if="activeView === 'settings'" />
      <PosterGrid v-else :items="items" />
    </main>
  </div>
</template>

<style scoped>
.layout { display: flex; height: 100vh; }
.content { flex: 1; overflow-y: auto; padding: calc(var(--space) * 4); }
</style>
