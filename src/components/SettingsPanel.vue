<script setup lang="ts">
/**
 * 设置面板（模块② 最小可用版：验证设置持久化打通）。
 * 模块⑧ 再补：原生目录选择器、主题、语言等。
 */
import { onMounted, ref } from "vue";
import { useSettingsStore } from "../stores/settingsStore";
import { dbInfo } from "../api/commands";

const store = useSettingsStore();
const newDir = ref("");
const status = ref("");
const tables = ref<string[]>([]);

onMounted(async () => {
  if (!store.loaded) await store.load();
  try {
    tables.value = (await dbInfo()).tables;
  } catch {
    tables.value = [];
  }
});

function addDir(): void {
  const d = newDir.value.trim();
  if (!d) return;
  store.addDir(d);
  newDir.value = "";
}

async function onSave(): Promise<void> {
  status.value = "";
  try {
    await store.save();
    status.value = "✅ 已保存到本地数据库";
  } catch {
    status.value = "❌ 保存失败：" + (store.error ?? "未知错误");
  }
}

async function onReload(): Promise<void> {
  status.value = "";
  await store.load();
  status.value = "↻ 已从数据库重新加载";
}
</script>

<template>
  <section class="settings">
    <h2>设置</h2>

    <div class="field">
      <label>TMDB API Key</label>
      <input v-model="store.tmdbKey" type="password" placeholder="粘贴 TMDB v3 API Key" />
    </div>

    <div class="field">
      <label>媒体目录</label>
      <div class="dirs">
        <div v-for="(d, i) in store.mediaDirs" :key="d + i" class="dir">
          <span class="path">{{ d }}</span>
          <button class="mini" @click="store.removeDir(i)">移除</button>
        </div>
        <p v-if="!store.mediaDirs.length" class="empty">尚未添加媒体目录</p>
      </div>
      <div class="add">
        <input
          v-model="newDir"
          placeholder="例如 D:\Movies（模块⑧ 将支持目录选择器）"
          @keyup.enter="addDir"
        />
        <button class="mini" @click="addDir">添加</button>
      </div>
    </div>

    <div class="field">
      <label>PotPlayer 路径</label>
      <input v-model="store.potPlayerPath" placeholder="例如 D:\PotPlayer\PotPlayerMini64.exe" />
    </div>

    <div class="actions">
      <button class="primary" :disabled="store.saving" @click="onSave">
        {{ store.saving ? "保存中…" : "保存" }}
      </button>
      <button class="mini" @click="onReload">重新加载</button>
      <span v-if="status" class="status">{{ status }}</span>
    </div>

    <p v-if="tables.length" class="db">数据表：{{ tables.join(" · ") }}</p>
  </section>
</template>

<style scoped>
.settings { max-width: 640px; }
h2 { margin: 0 0 calc(var(--space) * 6); font-size: 20px; }
.field { margin-bottom: calc(var(--space) * 6); }
label { display: block; margin-bottom: calc(var(--space) * 2); color: var(--muted); font-size: 13px; }
input {
  width: 100%; padding: 8px 10px; border-radius: 6px; font-size: 14px;
  border: 1px solid rgba(255,255,255,.14); background: var(--surface); color: var(--text);
}
.dirs { display: flex; flex-direction: column; gap: calc(var(--space) * 2); }
.dir {
  display: flex; align-items: center; justify-content: space-between; gap: 8px;
  padding: 6px 10px; border-radius: 6px; background: var(--surface);
}
.path { font-size: 13px; word-break: break-all; }
.empty { margin: 0; color: var(--muted); font-size: 13px; }
.add { display: flex; gap: 8px; margin-top: calc(var(--space) * 2); }
.actions { display: flex; align-items: center; gap: 10px; }
button { cursor: pointer; border: 0; border-radius: 6px; font-size: 13px; }
.primary { padding: 9px 18px; background: var(--accent); color: #fff; }
.primary:disabled { opacity: .6; cursor: default; }
.mini { padding: 6px 12px; background: rgba(255,255,255,.08); color: var(--text); }
.status { font-size: 13px; color: var(--muted); }
.db { margin-top: calc(var(--space) * 5); color: var(--muted); font-size: 12px; }
</style>
