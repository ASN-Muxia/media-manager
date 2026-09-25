<script setup lang="ts">
/**
 * 设置面板：元数据、媒体目录、播放器、扫描与维护。
 * 仅做了布局/排版美化（分组卡片 + 统一变量），交互逻辑不变。
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

    <div class="panel">
      <h3 class="panel-title">元数据与刮削</h3>
      <div class="field">
        <label>TMDB API Key</label>
        <input v-model="store.tmdbKey" type="password" placeholder="粘贴 TMDB v3 API Key" />
        <p class="panel-hint">不填也能用，只是没有海报与简介。可在设置后回列表点「批量刮削」。</p>
      </div>
    </div>

    <div class="panel">
      <h3 class="panel-title">媒体目录</h3>
      <div class="field">
        <div class="dirs">
          <div v-for="(d, i) in store.mediaDirs" :key="d + i" class="dir">
            <span class="path">{{ d }}</span>
            <button class="mini" @click="store.removeDir(i)">移除</button>
          </div>
          <p v-if="!store.mediaDirs.length" class="panel-hint">尚未添加媒体目录</p>
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
        <p class="panel-hint">递归扫描目录下的视频文件；同一部剧请放在同一目录下以获得正确分组。</p>
      </div>
    </div>

    <div class="panel">
      <h3 class="panel-title">播放器</h3>
      <div class="field">
        <label>PotPlayer 路径</label>
        <div class="add">
          <input v-model="store.potPlayerPath" placeholder="留空自动探测常见安装位置" />
          <button class="mini" @click="browsePlayer">浏览…</button>
        </div>
        <p class="panel-hint">
          {{ probing ? "探测中…" : player ? `当前探测到：${player}` : "未检测到 PotPlayer，将回退到系统默认播放器" }}
          <button class="mini inline" @click="probePlayer">重新探测</button>
        </p>
      </div>
    </div>

    <div class="toolbar">
      <button class="btn btn-primary" :disabled="store.saving" @click="onSave">
        {{ store.saving ? "保存中…" : "保存" }}
      </button>
      <button class="btn btn-ghost" @click="onReload">重新加载</button>
      <button class="btn btn-ghost" :disabled="media.scanning" @click="onScan">
        {{ media.scanning ? "扫描中…" : "保存并扫描媒体库" }}
      </button>
    </div>

    <p v-if="status" class="status">{{ status }}</p>

    <div class="panel">
      <h3 class="panel-title">最近扫描记录</h3>
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
      <p v-else class="panel-hint">暂无扫描记录</p>
    </div>

    <div class="panel">
      <h3 class="panel-title">维护与诊断</h3>
      <div class="toolbar">
        <button class="btn btn-ghost" @click="onOpenDataDir">打开数据目录</button>
        <button class="btn btn-danger" @click="onClear">清空媒体库（保留设置）</button>
      </div>
      <div class="diag">
        <p v-if="tables.length" class="panel-hint">数据表：{{ tables.join(" · ") }}</p>
        <p v-if="pageUrl" class="panel-hint">
          页面来源：{{ pageUrl }}
          <span v-if="pageUrl.includes('localhost:')" class="warn">
            （dev 模式：需先启动 vite dev server；正常使用请用 start.bat 构建）
          </span>
        </p>
        <p v-if="dbPath" class="panel-hint">
          数据库：{{ dbPath }}<span v-if="schemaVersion !== null">（结构版本 v{{ schemaVersion }}）</span>
        </p>
      </div>
    </div>
  </section>
</template>

<style scoped>
.settings { max-width: 760px; display: flex; flex-direction: column; gap: calc(var(--space) * 5); }
h2 { margin: 0 0 calc(var(--space) * 1); font-size: var(--fs-20); font-weight: 650; }

.field { display: flex; flex-direction: column; gap: calc(var(--space) * 2); }
label { color: var(--muted); font-size: var(--fs-13); font-weight: 500; }

.dirs { display: flex; flex-direction: column; gap: calc(var(--space) * 2); }
.dir {
  display: flex; align-items: center; justify-content: space-between; gap: 8px;
  padding: 8px 12px; border-radius: var(--radius-s);
  background: var(--surface-2); border: 1px solid var(--border);
}
.path { font-size: var(--fs-13); word-break: break-all; }

.add { display: flex; gap: 8px; align-items: center; }

.toolbar { display: flex; flex-wrap: wrap; align-items: center; gap: 10px; }

.mini {
  padding: 7px 12px; background: var(--surface-2); color: var(--text);
  border: 1px solid var(--border); font-size: var(--fs-12); flex: none;
}
.mini:hover { background: var(--border-strong); }
.mini.inline { margin-left: 8px; padding: 3px 9px; }
.status { margin: 0; font-size: var(--fs-13); color: var(--muted); }
.warn { color: #ffb454; }

.hist { width: 100%; border-collapse: collapse; font-size: var(--fs-12); }
.hist th, .hist td { padding: 8px 10px; text-align: left; border-bottom: 1px solid var(--border); }
.hist th { color: var(--muted); font-weight: 500; }
.hist tbody tr:last-child td { border-bottom: 0; }

.diag { display: flex; flex-direction: column; gap: 4px; margin-top: calc(var(--space) * 3); }
</style>
