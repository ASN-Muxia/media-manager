<script setup lang="ts">
/**
 * 设置面板：元数据、媒体目录、播放器、扫描与维护。
 * 仅做了布局/排版美化（分组卡片 + 统一变量），交互逻辑不变。
 */
import { onMounted, ref } from "vue";
import { open, save } from "@tauri-apps/plugin-dialog";
import { askConfirm } from "../utils/confirm";
import {
  backupDatabase, clearMetadataCache, dbInfo, detectPlayer, doubanTest, exportNfo,
  importNfo, libraryHealth, listProviders, llmParseName, llmPresets, omdbLookup,
  openDataDir, purgeExpiredCache, readReadyMarker, runtimeInfo, searchDouban,
  tmdbTest,
  type LibraryHealth, type LlmPreset, type ProviderEntry,
} from "../api/commands";
import { useSettingsStore } from "../stores/settingsStore";
import { useMediaStore } from "../stores/mediaStore";
import { useCategoryStore } from "../stores/categoryStore";
import {
  comboFromEvent, formatCombo, normalizeCombo,
  SHORTCUT_ACTIONS, SHORTCUT_LABELS, type ShortcutAction,
} from "../utils/shortcuts";

const store = useSettingsStore();
const media = useMediaStore();
const catalog = useCategoryStore();

const newDir = ref("");
const status = ref("");
const tables = ref<string[]>([]);
const player = ref<string | null>(null);
const probing = ref(false);
const testing = ref(false);
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
  void catalog.load();
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

/** 用当前填写的 hosts 试连一次 TMDB，验证配置是否真的生效。 */
async function onTestConnection(): Promise<void> {
  testing.value = true;
  try {
    // 先落库再测，避免测的是「没保存的旧配置」。
    await store.save();
    status.value = "✅ " + (await tmdbTest());
  } catch (e) {
    status.value = "❌ 连接失败：" + String(e);
  } finally {
    testing.value = false;
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

/* ---------------- 可选 provider 与大模型增强 ---------------- */

const presets = ref<LlmPreset[]>([]);
/** 「试解析」的输入与输出（直接看到模型把脏文件名翻成了什么）。 */
const probeName = ref("");
const probeResult = ref<string>("");
const probingLlm = ref(false);
const omdbTitle = ref("");
const omdbResult = ref("");
const doubanQuery = ref("");
const doubanResult = ref("");
const probingDouban = ref(false);

onMounted(async () => {
  try {
    presets.value = await llmPresets();
  } catch {
    presets.value = [];
  }
});

function onPreset(): void {
  const label = presetLabel.value;
  const p = presets.value.find((x) => x.label === label);
  if (p) store.applyLlmPreset(p);
}

const presetLabel = ref("");

/** 手动试一次脏文件名解析：验证配置是否真的通，也看看模型认得准不准。 */
async function onLlmProbe(): Promise<void> {
  probeResult.value = "";
  if (!probeName.value.trim()) {
    probeResult.value = "请先填一个文件名";
    return;
  }
  probingLlm.value = true;
  try {
    await store.save();
    const f = await llmParseName(probeName.value.trim());
    probeResult.value =
      `片名：${f.title || "（未识别）"}｜年份：${f.year ?? "-"}｜` +
      `季：${f.season ?? "-"}｜集：${f.episode ?? "-"}｜` +
      `分辨率：${f.resolution ?? "-"}｜来源：${f.releaseSource ?? "-"}`;
  } catch (e) {
    probeResult.value = "❌ " + String(e);
  } finally {
    probingLlm.value = false;
  }
}

/** 试一次 OMDb，确认 Key 有效。 */
async function onOmdbProbe(): Promise<void> {
  omdbResult.value = "";
  if (!omdbTitle.value.trim()) return;
  try {
    const r = await omdbLookup(omdbTitle.value.trim());
    omdbResult.value = r.rating
      ? `《${r.title}》IMDb ${r.rating.toFixed(1)}（${r.year}）`
      : `《${r.title}》无 IMDb 评分`;
  } catch (e) {
    omdbResult.value = "❌ " + String(e);
  }
}

/** 试一次豆瓣，确认开关与网络都通。 */
async function onDoubanProbe(): Promise<void> {
  doubanResult.value = "";
  probingDouban.value = true;
  try {
    doubanResult.value = "✅ " + (await doubanTest());
  } catch (e) {
    doubanResult.value = "❌ " + String(e);
  } finally {
    probingDouban.value = false;
  }
}

/** 按片名搜豆瓣，展示命中的字段，用户能直接看出刮削会写入什么。 */
async function onDoubanSearch(): Promise<void> {
  doubanResult.value = "";
  if (!doubanQuery.value.trim()) return;
  probingDouban.value = true;
  try {
    const list = await searchDouban(doubanQuery.value.trim());
    if (list.length === 0) {
      doubanResult.value = `未找到「${doubanQuery.value.trim()}」的匹配条目`;
      return;
    }
    const first = list[0];
    const bits = [
      `${first.title}${first.year ? `（${first.year}）` : ""}`,
      first.rating != null ? `评分 ${first.rating.toFixed(1)}` : "暂无评分",
      first.directors ? `导演 ${first.directors}` : "",
      first.genre || "",
      first.poster ? "有海报" : "无海报",
    ].filter(Boolean);
    doubanResult.value = `命中 ${list.length} 条 · ${bits.join("｜")}`;
  } catch (e) {
    doubanResult.value = "❌ " + String(e);
  } finally {
    probingDouban.value = false;
  }
}

/** 把已有条目的 TMDB 类型一次性补成分类（老数据迁移用）。 */
async function onSyncCategories(): Promise<void> {
  status.value = "";
  const r = await catalog.syncFromTmdb();
  status.value = r
    ? `✅ ${catalog.notice}`
    : "❌ " + (catalog.error ?? "同步失败");
}

/** 先保存设置再扫描，避免用了旧目录。 */
async function onScan(): Promise<void> {
  status.value = "";
  await onSave();
  // 扫描在后台线程跑，命令立即返回；结果由 scan-progress 事件回传后写进 toast。
  await media.scan();
  status.value = media.toast || "扫描已开始…";
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

/* ---------------- 数据互通与备份（v6） ---------------- */
/* 这一整块对外只写两种东西：`.nfo` 旁车文件、一份数据库副本。
   视频文件从头到尾是只读的 —— 不转码、不重编码、不裁剪、不覆盖。 */

const health = ref<LibraryHealth | null>(null);
const providers = ref<ProviderEntry[]>([]);
const shareBusy = ref(false);
const shareMsg = ref("");
/** 是否覆盖已存在的 NFO：默认**关**，避免冲掉用户或 Kodi / tMM 写的文件。 */
const nfoOverwrite = ref(false);

onMounted(async () => {
  try {
    health.value = await libraryHealth();
  } catch {
    health.value = null;
  }
  try {
    providers.value = (await listProviders()).items;
  } catch {
    providers.value = [];
  }
});

/**
 * 导出 NFO。
 *
 * `targetDir` 为空表示写在视频文件旁边（Kodi / tMM 的 sidecar 约定）——
 * 那是**新建**一个 `.nfo`，视频文件本身一个字节都不动。
 */
async function runExport(targetDir: string | null): Promise<void> {
  if (shareBusy.value) return;
  shareMsg.value = "";
  if (
    nfoOverwrite.value &&
    !(await askConfirm("已勾选「覆盖已有 NFO」：同名的旧 NFO 会被重写。确定继续？", "导出 NFO"))
  ) {
    return;
  }
  shareBusy.value = true;
  try {
    // 不传 ids 由后端决定范围：这里导出全部条目
    const r = await exportNfo({ targetDir, overwrite: nfoOverwrite.value });
    shareMsg.value =
      `✅ ${r.message}（写出 ${r.written} · 跳过 ${r.skipped} · 失败 ${r.failed}）`;
  } catch (e) {
    shareMsg.value = "❌ " + String(e);
  } finally {
    shareBusy.value = false;
  }
}

/** 导出到视频文件旁（sidecar）。 */
async function onExportSidecar(): Promise<void> {
  await runExport(null);
}

/** 导出到用户指定的目录（不想在媒体目录里留文件时用）。 */
async function onExportToDir(): Promise<void> {
  const picked = await open({ directory: true, multiple: false, title: "选择 NFO 输出目录" });
  const dir = Array.isArray(picked) ? picked[0] : picked;
  if (!dir) return;
  await runExport(String(dir));
}

/** 从一个目录导入 NFO：只回填空字段，已有值一律不覆盖。 */
async function onImportNfo(): Promise<void> {
  if (shareBusy.value) return;
  const picked = await open({ directory: true, multiple: false, title: "选择包含 NFO 的目录" });
  const dir = Array.isArray(picked) ? picked[0] : picked;
  if (!dir) return;
  shareMsg.value = "";
  shareBusy.value = true;
  try {
    const r = await importNfo(String(dir));
    shareMsg.value =
      `✅ ${r.message}（读到 ${r.scanned} · 回填 ${r.filled} · 改判 ${r.reclassified} · 未匹配 ${r.unmatched}）`;
    await media.load();
  } catch (e) {
    shareMsg.value = "❌ " + String(e);
  } finally {
    shareBusy.value = false;
  }
}

/**
 * 备份数据库。
 *
 * 走 `VACUUM INTO`：得到一份一致的快照，不锁库、不改原库；
 * 目标文件**已存在时后端直接拒绝**，绝不覆盖用户已有的备份。
 */
async function onBackup(): Promise<void> {
  if (shareBusy.value) return;
  const dest = await save({
    title: "把数据库备份到…",
    defaultPath: "media-manager-backup.db",
    filters: [{ name: "SQLite 数据库", extensions: ["db"] }],
  });
  if (!dest) return;
  shareMsg.value = "";
  shareBusy.value = true;
  try {
    const p = await backupDatabase(dest);
    shareMsg.value = `✅ 已备份到：${p}`;
  } catch (e) {
    shareMsg.value = "❌ " + String(e);
  } finally {
    shareBusy.value = false;
  }
}

/** 清空元数据缓存（刮削结果）；留空 provider 表示全部清。 */
async function onClearCache(): Promise<void> {
  if (shareBusy.value) return;
  shareMsg.value = "";
  shareBusy.value = true;
  try {
    const n = await clearMetadataCache(null);
    shareMsg.value = `✅ 已清除 ${n} 条元数据缓存（下次刮削会重新联网取）`;
  } catch (e) {
    shareMsg.value = "❌ " + String(e);
  } finally {
    shareBusy.value = false;
  }
}

/** 只清过期的缓存（7 天 TTL），没过期的留着继续用。 */
async function onPurgeCache(): Promise<void> {
  if (shareBusy.value) return;
  shareMsg.value = "";
  shareBusy.value = true;
  try {
    const n = await purgeExpiredCache();
    shareMsg.value = `✅ 已清理 ${n} 条过期缓存`;
  } catch (e) {
    shareMsg.value = "❌ " + String(e);
  } finally {
    shareBusy.value = false;
  }
}

/* ---------------- 快捷键（可自定义） ---------------- */

const shortcutActions = SHORTCUT_ACTIONS.map((key) => ({ key, label: SHORTCUT_LABELS[key] }));

function rawShortcut(a: ShortcutAction): string {
  switch (a) {
    case "selectAll":
      return store.shortcutSelectAll;
    case "invert":
      return store.shortcutInvert;
    case "confirm":
      return store.shortcutConfirm;
    case "clear":
      return store.shortcutClear;
  }
}

function setShortcut(a: ShortcutAction, v: string): void {
  switch (a) {
    case "selectAll":
      store.shortcutSelectAll = v;
      break;
    case "invert":
      store.shortcutInvert = v;
      break;
    case "confirm":
      store.shortcutConfirm = v;
      break;
    case "clear":
      store.shortcutClear = v;
      break;
  }
}

/** 输入框里显示成人类可读的形式；未绑定就留空，让 placeholder 提示用户按键。 */
function shortcutValue(a: ShortcutAction): string {
  const raw = rawShortcut(a).trim();
  return raw ? formatCombo(raw) : "";
}

/**
 * 焦点在输入框里时**直接按组合键**即录入。
 *
 * 解析不出来（比如只按了一个 Shift）就原样不动——否则用户想按 `Ctrl+Shift+A`，
 * 按下 Ctrl 的那一刻就被记成 `Ctrl` 了。
 */
function onShortcutKey(a: ShortcutAction, e: KeyboardEvent): void {
  const combo = comboFromEvent(e);
  const norm = combo ? normalizeCombo(combo) : null;
  if (!norm) return;
  setShortcut(a, formatCombo(norm));
}

function clearShortcut(a: ShortcutAction): void {
  setShortcut(a, "");
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
      <div class="field">
        <label>元数据语言</label>
        <select v-model="store.tmdbLanguage" class="select">
          <option v-for="o in store.languageOptions" :key="o.value" :value="o.value">
            {{ o.label }}（{{ o.value }}）
          </option>
        </select>
        <p class="panel-hint">
          决定刮削回来的标题、简介与类型名的语言（TMDB 的 <code>language</code> 参数）。
          改完后需重新刮削已有条目才会更新。
        </p>
      </div>
      <p class="panel-hint split">
        <strong>数据源分工</strong>：TMDb 负责片名 / 简介 / 评分 / 图（权威）；
        TheTVDB 负责剧集的季 / 集结构；Fanart.tv 在 TMDb 缺图时补海报与背景；
        OMDb 只补 IMDb 评分；豆瓣负责<strong>中文元数据兜底</strong>。
        上面四项都是<strong>可选</strong>的，留空就整段跳过，不影响主链路。
      </p>
      <div class="field">
        <label>TheTVDB API Key（可选 · 剧集季 / 集结构）</label>
        <input v-model="store.tvdbKey" type="password" placeholder="用于补 TMDb 缺失的单集结构" />
      </div>
      <div class="field">
        <label>Fanart.tv API Key（可选 · 补充海报 / 背景）</label>
        <input v-model="store.fanartKey" type="password" placeholder="TMDb 没给图时才去取" />
      </div>
      <div class="field">
        <label>OMDb API Key（可选 · IMDb 评分补充）</label>
        <div class="add">
          <input v-model="store.omdbKey" type="password" placeholder="omdbapi.com 的 Key" />
          <input v-model="omdbTitle" class="narrow" placeholder="片名" @keyup.enter="onOmdbProbe" />
          <button class="mini" @click="onOmdbProbe">测试</button>
        </div>
        <p v-if="omdbResult" class="panel-hint">{{ omdbResult }}</p>
      </div>
    </div>

    <div class="panel">
      <h3 class="panel-title">
        豆瓣刮削
        <span class="badge">中文兜底 · 默认关闭</span>
      </h3>
      <p class="panel-hint split">
        豆瓣<strong>没有官方开放 API</strong>，这里走的是它的公开检索与移动端接口，
        存在随时变动或限流的风险，因此默认<strong>关闭</strong>，由你自行决定是否启用。
        启用后：TMDb <strong>没命中</strong>时，豆瓣会<strong>整条接管</strong>
        （写入片名 / 年份 / 导演 / 演员 / 类型 / 上映日期 / 评分 / 简介 / 海报）；
        TMDb <strong>命中</strong>时，豆瓣只补它没给的中文字段，已有的一律不覆盖。
      </p>
      <div class="field">
        <label class="switch">
          <input v-model="store.doubanEnabled" type="checkbox" />
          <span>启用豆瓣刮削</span>
        </label>
      </div>
      <div class="field">
        <label class="switch">
          <input v-model="store.autoScrapeAfterScan" type="checkbox" />
          <span>扫描 / 导入完成后自动刮削</span>
        </label>
        <p class="panel-hint">
          开启后，每次<strong>扫描媒体库</strong>或<strong>从文件夹导入</strong>结束，
          系统会自动给<strong>还没有海报的条目</strong>跑一遍刮削（TMDb 优先，
          豆瓣兜底）——只碰新条目，已经刮好的不会重刷，单次最多 50 条。
          进度走同一个「刮削」提示，随时可以在设置页取消。
        </p>
      </div>
      <div class="field">
        <label>豆瓣 Cookie（可选 · 可降低验证码概率）</label>
        <input
          v-model="store.doubanCookie"
          type="password"
          placeholder="留空也能用；浏览器登录豆瓣后复制 Cookie 可提高稳定性"
        />
      </div>
      <div class="field">
        <label>连通性 / 检索测试</label>
        <div class="add">
          <input
            v-model="doubanQuery"
            class="narrow"
            placeholder="片名，如 沙丘"
            @keyup.enter="onDoubanSearch"
          />
          <button class="mini" :disabled="probingDouban" @click="onDoubanSearch">搜豆瓣</button>
          <button class="mini" :disabled="probingDouban" @click="onDoubanProbe">测试连接</button>
        </div>
        <p v-if="doubanResult" class="panel-hint">{{ doubanResult }}</p>
      </div>
      <p class="panel-hint split">
        <strong>使用提示</strong>：改完开关与 Cookie 记得先点最下方的「保存设置」。
        刮削入口仍在条目的详情页——「刮削」按钮会按 TMDb → 豆瓣的顺序自动跑完；
        也可以在那里用豆瓣单独重刮一次。「自动刮削」是<strong>可选</strong>的，
        默认关闭；即使打开了，没开豆瓣、也没配 TMDb Key 时它会直接跳过，不会空跑。
      </p>
    </div>

    <div class="panel">
      <h3 class="panel-title">
        大模型脏文件名解析
        <span class="badge">可选增强层</span>
      </h3>
      <p class="panel-hint split">
        默认<strong>关闭</strong>。开启后，只有在正则解析器认不出片名
        （标题为空、只有一两个字符、或整个文件名原样搬过来）时才会调用它，
        用来产出「拿什么去检索」的结构化字段；<strong>真正的标题、简介、评分、海报
        依然全部来自上面的元数据 API</strong>。调用失败会自动回退到正则结果，不会让情况变糟。
        视觉模型只在文件名<strong>完全不可推断</strong>（纯数字 / 纯符号）时才可能用到。
      </p>
      <div class="field">
        <label class="switch">
          <input v-model="store.llmEnabled" type="checkbox" />
          <span>启用大模型解析</span>
        </label>
      </div>
      <div class="field">
        <label>接入预设</label>
        <div class="add">
          <select v-model="presetLabel" class="select" @change="onPreset">
            <option value="">— 选择一项以自动填入 —</option>
            <option v-for="p in presets" :key="p.label" :value="p.label">{{ p.label }}</option>
          </select>
        </div>
        <p class="panel-hint">都走 OpenAI 兼容协议，换地址与模型名即可切换到别的厂商（含本地 Ollama）。</p>
      </div>
      <div class="field">
        <label>接口地址（OpenAI 兼容）</label>
        <input v-model="store.llmBaseUrl" placeholder="https://api.deepseek.com/v1" />
      </div>
      <div class="field">
        <label>API Key（本地 Ollama / vLLM 可留空）</label>
        <input v-model="store.llmApiKey" type="password" />
      </div>
      <div class="field">
        <label>文本模型</label>
        <input v-model="store.llmModel" placeholder="留空则用推荐模型 deepseek-chat" />
      </div>
      <div class="field">
        <label>视觉模型（只在文件名完全不可推断时用）</label>
        <input v-model="store.llmVisionModel" placeholder="留空表示不启用视觉兜底" />
      </div>
      <div class="field">
        <label>视觉模型接口地址（与文本不同源时才填）</label>
        <input v-model="store.llmVisionBaseUrl" placeholder="留空则与文本模型同址" />
      </div>
      <div class="field">
        <label>试解析一个文件名</label>
        <div class="add">
          <input v-model="probeName" placeholder="例如 [0724].mkv" @keyup.enter="onLlmProbe" />
          <button class="mini" :disabled="probingLlm" @click="onLlmProbe">
            {{ probingLlm ? "解析中…" : "保存并解析" }}
          </button>
        </div>
        <p v-if="probeResult" class="panel-hint">{{ probeResult }}</p>
      </div>
    </div>

    <div class="panel">
      <h3 class="panel-title">
        数据源开关
        <span class="badge">关掉不等于删 Key</span>
      </h3>
      <p class="panel-hint split">
        这里回答的是「<strong>哪些源参与刮削</strong>」。
        关掉一个源<strong>不会</strong>把它上面填的 Key 删掉，也不会让后面的源顶上来——
        顺序是写死的：<strong>TMDb 权威 → TheTVDB → Fanart.tv → OMDb → 豆瓣兜底</strong>。
        关掉 TMDb 时豆瓣会承担整条刮削，而不是直接失败。
      </p>
      <div class="providers">
        <label v-for="p in providers" :key="p.id" class="prov-row">
          <input
            type="checkbox"
            :checked="store.isProviderOn(p.id)"
            @change="store.toggleProvider(p.id)"
          />
          <span class="prov-name">{{ p.label }}</span>
          <span class="prov-desc">{{ p.provides }}</span>
        </label>
        <p v-if="!providers.length" class="panel-hint">读不到数据源列表（不影响使用）</p>
      </div>
      <p class="panel-hint">改完记得点最下方的「保存」。</p>
    </div>

    <div class="panel">
      <h3 class="panel-title">
        匹配筛选
        <span class="badge">手动匹配 TMDB 时生效</span>
      </h3>
      <p class="panel-hint split">
        这些条件决定「候选怎么排、哪些直接剔除」，用来压低误匹配：
        优先级是<strong>片名完全一致 → 原标题 / 别名一致 → 包含 → 模糊</strong>，
        年份作为修正项叠加。
      </p>
      <div class="field">
        <label class="switch">
          <input v-model="store.matchFuzzy" type="checkbox" />
          <span>模糊匹配（关掉就只剩完全一致与包含）</span>
        </label>
      </div>
      <div class="field">
        <label>年份容差</label>
        <select v-model.number="store.matchYearTolerance" class="select">
          <option :value="0">必须同一年</option>
          <option :value="1">前后 1 年也算命中</option>
          <option :value="2">前后 2 年也算命中</option>
          <option :value="10">不看年份</option>
        </select>
        <p class="panel-hint">年份差超出容差的候选会被扣分，排到后面去。</p>
      </div>
      <div class="field">
        <label>候选上限（TMDB 每页 20 条）</label>
        <select v-model.number="store.matchLimit" class="select">
          <option :value="20">20（只第一页）</option>
          <option :value="60">60（推荐）</option>
          <option :value="120">120</option>
          <option :value="200">200（上限）</option>
        </select>
        <p class="panel-hint">调大能看见更多候选，但检索也会慢一点。</p>
      </div>
      <div class="field">
        <label>排除关键词（黑名单）</label>
        <textarea
          v-model="store.matchBlacklist"
          rows="2"
          placeholder="剧场版, 演唱会, OVA, 幕后花絮"
        ></textarea>
        <p class="panel-hint">
          逗号或换行分隔。候选标题里出现任一个就<strong>整条剔除</strong>，
          连候选列表都不进。
        </p>
      </div>
    </div>

    <div class="panel">
      <h3 class="panel-title">
        快捷键
        <span class="badge">可自定义</span>
      </h3>
      <p class="panel-hint split">
        只对「选择模式」与 TMDB 候选面板生效，输入框里不抢键。
        点一下输入框再<strong>直接按你想用的组合键</strong>即可录入；留空 = 不绑定。
      </p>
      <div
        v-for="a in shortcutActions"
        :key="a.key"
        class="field"
      >
        <label>{{ a.label }}</label>
        <div class="add">
          <input
            :value="shortcutValue(a.key)"
            readonly
            :placeholder="`按下组合键…`"
            @keydown.prevent="onShortcutKey(a.key, $event)"
          />
          <button class="mini" @click="clearShortcut(a.key)">清除</button>
        </div>
      </div>
      <p class="panel-hint">
        写法示例：<code>Ctrl+A</code>、<code>Ctrl+Shift+A</code>、<code>Ctrl+Enter</code>、<code>Esc</code>。
      </p>
    </div>

    <div class="panel">
      <h3 class="panel-title">分类维护</h3>
      <p class="panel-hint split">
        左侧栏目的分类全部存在数据库里，手动建的在设置页之外（左侧导航「＋」）操作。
        这里只做一件事：把已刮削条目的 TMDB 类型一次性补成分类——
        <strong>升级前刮过的老数据</strong>没有分类关联，靠它补齐。
      </p>
      <div class="toolbar">
        <button class="btn btn-ghost" :disabled="catalog.saving" @click="onSyncCategories">
          {{ catalog.saving ? "同步中…" : "从 TMDB 类型同步分类" }}
        </button>
        <span v-if="catalog.categories.length" class="panel-hint">
          当前 {{ catalog.categories.length }} 个分类 · {{ catalog.tags.length }} 个标签
        </span>
      </div>
    </div>

    <div class="panel">
      <h3 class="panel-title">Hosts 解析（自定义域名 → IP）</h3>
      <div class="field">
        <textarea
          v-model="store.hostsText"
          class="hosts"
          rows="4"
          spellcheck="false"
          placeholder="13.224.161.90 api.themoviedb.org&#10;104.16.61.155 image.tmdb.org"
        ></textarea>
        <p class="panel-hint">
          一行一条，格式 <code>IP 域名</code>（反过来写 <code>域名 IP</code> 也能识别），
          <code>#</code> 后面是注释；一行可写多个域名。
          <strong>生效范围</strong>：只作用于本应用自己发出的 HTTP 请求（TMDB 刮削接口），
          不改系统 hosts 文件、不影响其他程序；同时保留原域名做 TLS 校验，证书不会报错。
          海报图由界面组件直接加载，不受这里影响——图片域名若也被墙，请改系统 hosts 文件。
        </p>
        <div class="toolbar">
          <button class="btn btn-ghost" :disabled="testing" @click="onTestConnection">
            {{ testing ? "测试中…" : "保存并测试连接" }}
          </button>
        </div>
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

    <!-- 后台扫描的实时反馈：遍历大目录要几十秒，不能让用户以为卡住了 -->
    <p v-if="media.scanning" class="status">
      扫描中：{{ media.scanProgress?.phase ?? "准备中" }}
      <template v-if="media.scanProgress?.found">
        · 已发现 {{ media.scanProgress.found }} 个视频
      </template>
      <button class="mini inline" @click="media.cancelScan()">停止扫描</button>
    </p>

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
      <h3 class="panel-title">
        数据互通与备份
        <span class="badge">非侵入式 · 不碰视频文件</span>
      </h3>
      <p class="panel-hint split">
        NFO 是 Kodi / tinyMediaManager 通用的<strong>旁车元数据</strong>文件
        （<code>xxx.mkv</code> 旁边一个 <code>xxx.nfo</code>）。
        导出只会<strong>新建</strong>这个文件，视频本身一个字节都不会动；
        默认是<strong>不覆盖</strong>，已经存在的 NFO 直接跳过。
        导入则只回填<strong>空字段</strong>，你已有的标题与海报不会被顶掉。
      </p>
      <div class="field">
        <label class="switch">
          <input v-model="nfoOverwrite" type="checkbox" />
          <span>覆盖已存在的 NFO（默认关闭）</span>
        </label>
      </div>
      <div class="toolbar">
        <button class="btn btn-ghost" :disabled="shareBusy" @click="onExportSidecar">
          {{ shareBusy ? "导出中…" : "导出到视频文件旁" }}
        </button>
        <button class="btn btn-ghost" :disabled="shareBusy" @click="onExportToDir">
          导出到指定目录…
        </button>
        <button class="btn btn-ghost" :disabled="shareBusy" @click="onImportNfo">
          从目录导入 NFO…
        </button>
      </div>
      <div class="toolbar">
        <button class="btn btn-ghost" :disabled="shareBusy" @click="onBackup">
          备份数据库到…
        </button>
        <button class="btn btn-ghost" :disabled="shareBusy" @click="onPurgeCache">
          清理过期缓存
        </button>
        <button class="btn btn-danger" :disabled="shareBusy" @click="onClearCache">
          清空元数据缓存
        </button>
      </div>
      <p class="panel-hint split">
        备份走 SQLite 的 <code>VACUUM INTO</code>，得到一份一致快照，
        <strong>不锁库、不改原库</strong>；目标文件已存在时后端<strong>直接拒绝</strong>，
        不会覆盖你之前的备份。缓存只存元数据 API 的响应（7 天过期），清掉后下次刮削重新联网取。
      </p>
      <p v-if="shareMsg" class="panel-hint">{{ shareMsg }}</p>
      <div class="diag" v-if="providers.length">
        <p class="panel-hint">
          数据源顺序（越靠前越权威）：
          {{ providers.map((p) => p.label).join(" → ") }}
        </p>
      </div>
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
        <p v-if="health" class="panel-hint">
          检索索引：{{ health.ftsEnabled ? "FTS5 已启用" : "未启用（回落到 LIKE 检索）" }}
          · 元数据缓存 {{ health.cacheRows }} 条
          · 观看记录 {{ health.playbackRows }} 条
          · ffprobe {{ health.ffprobeAvailable ? "可用" : "不可用（跳过技术信息探测）" }}
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

.hosts {
  width: 100%; padding: 10px 12px; border-radius: var(--radius-s);
  border: 1px solid var(--border); background: var(--surface-2); color: var(--text);
  font-family: ui-monospace, Consolas, monospace; font-size: var(--fs-13);
  line-height: 1.6; resize: vertical; outline: none;
}
.hosts:focus { border-color: var(--accent); }
.hosts::placeholder { color: var(--muted); }
code {
  padding: 1px 5px; border-radius: 4px; background: var(--surface-2);
  font-family: ui-monospace, Consolas, monospace; font-size: var(--fs-12);
}

.select {
  padding: 9px 12px; border-radius: var(--radius-s); font-size: var(--fs-13);
  border: 1px solid var(--border); background: var(--surface-2); color: var(--text);
  outline: none; max-width: 260px;
}
.select:focus { border-color: var(--accent); }

.panel-hint.split { margin-top: calc(var(--space) * -1); line-height: 1.7; }
.badge {
  margin-left: 8px; padding: 2px 8px; border-radius: 999px; font-size: var(--fs-11);
  font-weight: 500; color: var(--muted); background: var(--surface-2);
  border: 1px solid var(--border);
}
.switch { display: flex; align-items: center; gap: 8px; color: var(--text-2); }
.switch input { width: 15px; height: 15px; accent-color: var(--accent); }
.narrow { max-width: 150px; }

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

/* 数据源开关：一行一个源，勾选框在最左，说明文字在右 */
.providers { display: flex; flex-direction: column; gap: calc(var(--space) * 2); }
.prov-row {
  display: flex; align-items: baseline; gap: 10px; padding: 8px 12px;
  border-radius: var(--radius-s); background: var(--surface-2); border: 1px solid var(--border);
  cursor: pointer;
}
.prov-row input { width: 15px; height: 15px; accent-color: var(--accent); flex: none; }
.prov-name { flex: none; min-width: 84px; font-size: var(--fs-13); color: var(--text); font-weight: 550; }
.prov-desc { font-size: var(--fs-12); color: var(--muted); }
</style>
