<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import Sidebar from "./components/Sidebar.vue";
import PosterGrid from "./components/PosterGrid.vue";
import MediaDetail from "./components/MediaDetail.vue";
import SettingsPanel from "./components/SettingsPanel.vue";
import { useSettingsStore } from "./stores/settingsStore";
import { useMediaStore } from "./stores/mediaStore";
import type { ViewKey } from "./types";

const settings = useSettingsStore();
const media = useMediaStore();
const activeView = ref<ViewKey>("all");

const TITLES: Record<ViewKey, string> = {
  all: "全部媒体",
  movie: "电影",
  tv: "剧集",
  settings: "设置",
};

const inSettings = computed(() => activeView.value === "settings");
const heading = computed(() =>
  media.selectedId && !inSettings.value ? "媒体详情" : TITLES[activeView.value],
);

async function onNav(v: ViewKey): Promise<void> {
  activeView.value = v;
  if (v === "settings") {
    media.closeDetail();
    await media.loadHistory();
    return;
  }
  await media.setFilter(v);
}

onMounted(async () => {
  await settings.load();
  await media.load();
  await media.loadHistory();
});
</script>

<template>
  <div class="layout">
    <Sidebar :model-value="activeView" @update:model-value="onNav" />

    <main class="content">
      <header class="topbar">
        <h1>{{ heading }}</h1>
        <input
          v-if="!inSettings"
          v-model="media.keyword"
          class="search"
          placeholder="搜索标题 / 类型 / 年份"
        />
        <button class="btn btn-primary" :disabled="media.scanning" @click="media.scan()">
          {{ media.scanning ? "扫描中…" : "扫描媒体库" }}
        </button>
        <button class="btn btn-ghost" :disabled="media.scraping" @click="media.scrapeMissing()">
          {{ media.scraping ? "刮削中…" : "批量刮削" }}
        </button>
      </header>

      <p v-if="media.toast" class="toast">{{ media.toast }}</p>
      <p v-if="media.error" class="err">{{ media.error }}</p>

      <MediaDetail v-if="media.selectedId && !inSettings" />
      <SettingsPanel v-else-if="inSettings" />
      <PosterGrid v-else :items="media.visibleItems()" @open="media.openDetail" />
    </main>
  </div>
</template>

<style scoped>
.layout { display: flex; height: 100vh; }
.content { flex: 1; overflow-y: auto; padding: calc(var(--space) * 5) calc(var(--space) * 7) calc(var(--space) * 10); }

.topbar {
  display: flex; align-items: center; gap: 10px; margin-bottom: calc(var(--space) * 5);
  position: sticky; top: 0; z-index: 5; padding: calc(var(--space) * 3) 0;
  background: var(--bg);
}
h1 { flex: 1; margin: 0; font-size: var(--fs-18); font-weight: 650; }
.search {
  width: 240px; padding: 9px 12px; border-radius: var(--radius-s); font-size: var(--fs-13);
  border: 1px solid var(--border); background: var(--surface); color: var(--text);
  outline: none; transition: border-color 0.15s ease;
}
.search:focus { border-color: var(--accent); }
.search::placeholder { color: var(--muted); }

.toast {
  margin: 0 0 calc(var(--space) * 4); padding: 10px 14px; border-radius: var(--radius-s);
  font-size: var(--fs-13); background: var(--surface); border: 1px solid var(--border); color: var(--text-2);
}
.err {
  margin: 0 0 calc(var(--space) * 4); padding: 10px 14px; border-radius: var(--radius-s);
  font-size: var(--fs-13); background: var(--accent-soft); border: 1px solid rgba(229,9,20,.35);
  color: #ff8a90; word-break: break-all;
}
</style>
