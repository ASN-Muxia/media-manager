<script setup lang="ts">
/**
 * 设置面板：媒体目录、TMDB Key、PotPlayer 路径、扫描与维护。
 * 目录选择器（原生 dialog 插件）留待后续版本。
 */
import { onMounted, ref } from "vue";
import { open } from "@tauri-apps/plugin-dialog";
import { dbInfo, detectPlayer, openDataDir, readReadyMarker, runtimeInfo } from "../api/commands";
import { useSettingsStore } from "../stores/settingsStore";
import { useMediaStore } from "../stores/mediaStore";

const store = useSettingsStore();
const media = useMediaStore();

const newDir = ref("");
const status = ref("");
const tables = ref<string[]>([]);
const player = ref<string | null>(null);
const probing = ref(false);
const schemaVersion = ref<number | null>(null);
const dbPath = ref("");
const pageUrl = ref("");

function fmtTime(ts: number | null): string {
  if (!ts) return "-";
  return new Date(ts * 1000).toLocaleString();
}

onMounted(async () => {
  if (!store.loaded) await store.load();
  await media.loadHistory();
  try {
    tables.value = (await dbInfo()).tables;
  } catch {
    tables.value = [];
  }
  try {
    const rt = await runtimeInfo();
    schemaVersion.value = rt.schemaVersion;
    dbPath.value = rt.dbPath;
    tables.value = rt.tables;
  } catch {
    /* 忽略：诊断信息拿不到不影响使用 */
  }
  try {
    const marker = await readReadyMarker();
    const line = (marker ?? "").split("\n").find((l) => l.startsWith("url="));
    pageUrl.value = line ? line.slice(4) : "";
  } catch {
    /* 同上 */
  }
  void probePlayer();
});

function addDir(): void {
  const d = newDir.value.trim();
  if (!d) return;
  store.addDir(d);
  newDir.value = "";
}

/** 原生目录选择器。 */
async function browseDir(): Promise<void> {
  status.value = "";
  try {
    const picked = await open({ directory: true, multiple: false, title: "选择媒体目录" });
    if (typeof picked === "string" && picked) store.addDir(picked);
  } catch (e) {
    status.value = "❌ 打开目录选择器失败：" + String(e);
  }
}

/** 原生文件选择器：挑 PotPlayer 可执行文件。 */
async function browsePlayer(): Promise<void> {
  status.value = "";
  try {
    const picked = await open({
      multiple: false,
      title: "选择 PotPlayer 可执行文件",
      filters: [{ name: "可执行文件", extensions: ["exe"] }],
    });
    if (typeof picked === "string" && picked) {
      store.potPlayerPath = picked;
      await probePlayer();
    }
  } catch (e) {
    status.value = "❌ 打开文件选择器失败：" + String(e);
  }
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

async function probePlayer(): Promise<void> {
  probing.value = true;
  try {
    player.value = await detectPlayer();
  } catch {
    player.value = null;
  } finally {
    probing.value = false;
  }
}

/** 先保存设置再扫描，避免用了旧目录。 */
async function onScan(): Promise<void> {
  status.value = "";
  await onSave();
  await media.scan();
  status.value = media.toast;
}

async function onClear(): Promise<void> {
  status.value = "";
  await media.clearAll();
  status.value = media.toast;
}

/** 打开数据库所在目录（排查问题用）。 */
async function onOpenDataDir(): Promise<void> {
  try {
    const p = await openDataDir();
    status.value = `📂 已打开数据目录：${p}`;
  } catch (e) {
    status.value = "❌ " + String(e);
  }
}
</script>

<template>
  <section class="settings">
    <h2>设置</h2>

    <div class="field">
      <label>TMDB API Key（模块④ 刮削需要）</label>
      <input v-model="store.tmdbKey" type="password" placeholder="粘贴 TMDB v3 API Key" />
    </div>

    <div class="field">
      <label>媒体目录（递归扫描视频文件）</label>
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
          placeholder="例如 D:\Movies 或 E:\电视剧"
          @keyup.enter="addDir"
        />
        <button class="mini" @click="addDir">添加</button>
        <button class="mini" @click="browseDir">浏览…</button>
      </div>
    </div>

    <div class="field">
      <label>PotPlayer 路径（留空自动探测常见安装位置）</label>
      <div class="add">
        <input v-model="store.potPlayerPath" placeholder="例如 D:\PotPlayer\PotPlayerMini64.exe" />
        <button class="mini" @click="browsePlayer">浏览…</button>
      </div>
      <p class="hint">
        {{ probing ? "探测中…" : player ? `当前探测到：${player}` : "未检测到 PotPlayer，将回退到系统默认播放器" }}
        <button class="mini inline" @click="probePlayer">重新探测</button>
      </p>
    </div>

    <div class="actions">
      <button class="primary" :disabled="store.saving" @click="onSave">
        {{ store.saving ? "保存中…" : "保存" }}
      </button>
      <button class="mini" @click="onReload">重新加载</button>
      <button class="mini" :disabled="media.scanning" @click="onScan">
        {{ media.scanning ? "扫描中…" : "保存并扫描媒体库" }}
      </button>
    </div>

    <p v-if="status" class="status">{{ status }}</p>

    <div class="field">
      <label>最近扫描记录</label>
      <table v-if="media.scanHistory.length" class="hist">
        <thead>
          <tr><th>时间</th><th>范围</th><th>发现</th><th>新增</th><th>状态</th></tr>
        </thead>
        <tbody>
          <tr v-for="h in media.scanHistory" :key="h.id">
            <td>{{ fmtTime(h.finished_at ?? h.started_at) }}</td>
            <td>{{ h.path }}</td>
            <td>{{ h.found }}</td>
            <td>{{ h.added }}</td>
            <td>{{ h.status }}</td>
          </tr>
        </tbody>
      </table>
      <p v-else class="empty">暂无扫描记录</p>
    </div>

    <div class="field">
      <label>维护</label>
      <div class="actions">
        <button class="mini" @click="onOpenDataDir">打开数据目录</button>
        <button class="mini danger" @click="onClear">清空媒体库（保留设置）</button>
      </div>
      <p v-if="tables.length" class="hint">数据表：{{ tables.join(" · ") }}</p>
      <p v-if="pageUrl" class="hint">
        页面来源：{{ pageUrl }}
        <span v-if="pageUrl.includes('localhost:')" class="warn">
          （dev 模式：需先启动 vite dev server；正常使用请用 start.bat 构建）
        </span>
      </p>
      <p v-if="dbPath" class="hint">
        数据库：{{ dbPath }}<span v-if="schemaVersion !== null">（结构版本 v{{ schemaVersion }}）</span>
      </p>
    </div>
  </section>
</template>

<style scoped>
.settings { max-width: 720px; display: flex; flex-direction: column; gap: calc(var(--space) * 6); }
h2 { margin: 0; font-size: 20px; }
.field { display: flex; flex-direction: column; gap: calc(var(--space) * 2); }
label { color: var(--muted); font-size: 13px; }
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
.hint { margin: 0; color: var(--muted); font-size: 12px; word-break: break-all; }
.warn { color: #ffb454; }
.add { display: flex; gap: 8px; }
.actions { display: flex; flex-wrap: wrap; align-items: center; gap: 10px; }
button { cursor: pointer; border: 0; border-radius: 6px; font-size: 13px; }
.primary { padding: 9px 18px; background: var(--accent); color: #fff; }
.primary:disabled { opacity: .6; cursor: default; }
.mini { padding: 6px 12px; background: rgba(255,255,255,.08); color: var(--text); }
.mini.inline { margin-left: 8px; padding: 3px 8px; font-size: 12px; }
.mini.danger { background: rgba(229,9,20,.16); color: #ff6b73; }
.mini:disabled { opacity: .6; cursor: default; }
.status { margin: 0; font-size: 13px; color: var(--muted); }

.hist { width: 100%; border-collapse: collapse; font-size: 12px; }
.hist th, .hist td {
  padding: 6px 8px; text-align: left; border-bottom: 1px solid rgba(255,255,255,.07);
}
.hist th { color: var(--muted); font-weight: 500; }
</style>
