<script setup lang="ts">
/**
 * 从本地文件夹导入影视。
 *
 * 流程：**选目录 → 选目标栏目（可选）→ 导入**。
 * 解析 / 分组 / 入库复用扫描那一套（后端 `import_folder`），所以
 * 文件名识别、剧集聚合规则与「扫描媒体库」完全一致；**重复文件自动跳过**
 * （入库是 `ON CONFLICT(file_path) DO NOTHING`），不会把同一个文件录两遍。
 *
 * 失败时把后端给的原因原样显示在这里——「导入失败」这种空话对用户没有帮助。
 */
import { computed, ref } from "vue";
import { open } from "@tauri-apps/plugin-dialog";
import { useCategoryStore } from "../stores/categoryStore";
import { useMediaStore } from "../stores/mediaStore";
import type { ImportFolderResult } from "../api/commands";

const emit = defineEmits<{ (e: "close"): void }>();

const media = useMediaStore();
const cat = useCategoryStore();

const path = ref("");
/** 目标栏目 id；空串表示「不归类」（只入库）。 */
const categoryId = ref<number | null>(null);
const busy = ref(false);
const err = ref("");
const result = ref<ImportFolderResult | null>(null);

const canSubmit = computed(() => path.value.trim().length > 0 && !busy.value);

/** 原生目录选择器（与设置页的媒体目录用的是同一个插件入口）。 */
async function browse(): Promise<void> {
  err.value = "";
  try {
    const picked = await open({
      directory: true,
      multiple: false,
      title: "选择要导入的文件夹",
    });
    if (typeof picked === "string" && picked) path.value = picked;
  } catch (e) {
    err.value = "打开目录选择器失败：" + String(e);
  }
}

async function submit(): Promise<void> {
  err.value = "";
  result.value = null;
  if (!path.value.trim()) {
    err.value = "请先选择要导入的文件夹";
    return;
  }
  busy.value = true;
  try {
    const r = await media.importFolder(path.value.trim(), categoryId.value);
    if (r) {
      result.value = r;
      // 导入可能新建 / 充实了分类，刷新侧栏计数
      await cat.load();
      if (r.added === 0 && r.failed === 0) return; // 全是重复项或空目录，留在页面上说明原因
      emit("close");
    } else {
      err.value = media.error ?? "导入失败";
    }
  } finally {
    busy.value = false;
  }
}
</script>

<template>
  <div class="mask" @click.self="emit('close')">
    <div class="dialog">
      <h3>从文件夹导入</h3>

      <label class="field">
        <span>文件夹 *</span>
        <div class="row">
          <input v-model="path" placeholder="例如 D:\影视\绝命毒师" />
          <button class="btn ghost nowrap" @click="browse">浏览…</button>
        </div>
      </label>

      <label class="field">
        <span>导入到栏目（可选）</span>
        <select v-model="categoryId">
          <option :value="null">不归类（只入库）</option>
          <option v-for="c in cat.categories" :key="c.id" :value="c.id">
            {{ c.name }}（{{ c.seriesCount }}）
          </option>
        </select>
      </label>

      <p class="hint">
        递归读取目录下的视频文件，按现有规则解析剧名 / 季 / 集并聚合；
        库里已存在的相同文件会被自动跳过。
      </p>

      <!-- 导入结果：数字比「成功」两个字有用 -->
      <div v-if="result" class="result">
        <p class="msg">{{ result.message }}</p>
        <ul class="nums">
          <li>新增 {{ result.added }}</li>
          <li>跳过 {{ result.skipped }}</li>
          <li v-if="result.failed">失败 {{ result.failed }}</li>
          <li v-if="result.linked">归入栏目 {{ result.linked }}</li>
        </ul>
      </div>

      <p v-if="err" class="err">{{ err }}</p>

      <div class="actions">
        <button class="btn ghost" @click="emit('close')">
          {{ result ? "关闭" : "取消" }}
        </button>
        <button class="btn primary" :disabled="!canSubmit" @click="submit">
          {{ busy ? "导入中…" : "开始导入" }}
        </button>
      </div>
    </div>
  </div>
</template>

<style scoped>
.mask {
  position: fixed; inset: 0; z-index: 40; display: flex; align-items: center;
  justify-content: center; background: rgba(0, 0, 0, .55);
}
.dialog {
  width: 520px; max-width: calc(100vw - 32px); display: flex; flex-direction: column;
  gap: calc(var(--space) * 3); padding: calc(var(--space) * 5);
  border-radius: var(--radius-l); background: var(--surface);
  border: 1px solid var(--border);
}
h3 { margin: 0; font-size: var(--fs-16); font-weight: 650; }
.field { display: flex; flex-direction: column; gap: 6px; font-size: var(--fs-13); color: var(--muted); }
.field input, .field select {
  padding: 8px 11px; border-radius: var(--radius-s); font-size: var(--fs-13);
  border: 1px solid var(--border); background: var(--surface-2); color: var(--text); outline: none;
}
.field input:focus, .field select:focus { border-color: var(--accent); }
.row { display: flex; gap: 8px; }
.row input { flex: 1; min-width: 0; }
.nowrap { white-space: nowrap; }
.hint { margin: 0; font-size: var(--fs-12); color: var(--muted); line-height: 1.6; }

.result {
  display: flex; flex-direction: column; gap: 6px; padding: calc(var(--space) * 3);
  border-radius: var(--radius-s); background: var(--surface-2); border: 1px solid var(--border);
}
.result .msg { margin: 0; font-size: var(--fs-13); color: var(--text); }
.nums { display: flex; flex-wrap: wrap; gap: 12px; margin: 0; padding: 0; list-style: none; }
.nums li { font-size: var(--fs-12); color: var(--muted); }

.err { margin: 0; font-size: var(--fs-12); color: var(--danger); word-break: break-all; }
.actions { display: flex; justify-content: flex-end; gap: 10px; }
.btn {
  padding: 8px 16px; border-radius: var(--radius-s); cursor: pointer; font-size: var(--fs-13);
  background: var(--surface-2); color: var(--text); border: 1px solid var(--border);
}
.btn.primary { background: var(--accent); color: #fff; border-color: transparent; }
.btn:disabled { opacity: .6; cursor: default; }
</style>
