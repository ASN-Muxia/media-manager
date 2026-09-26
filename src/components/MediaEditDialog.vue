<script setup lang="ts">
/**
 * 新建 / 编辑资源对话框（两种模式共用一个组件）。
 *
 * - **新建**（`initial === null`）：名称 + 类型（电影 / 剧集）+ 年份 / 流派 / 简介，
 *   外加可选的分类与标签；走 `create_media`，与扫描结果写同一张 `media` 表。
 * - **编辑**（`initial` 为现有条目）：改名 / 改类型 / 年份 / 流派 / 简介；
 *   分类与标签仍在详情页维护（那里是即点即存的）。
 *
 * 两种模式成功后都由 store 重载列表——新条目 / 改动立刻反映到
 * 「全部」「电影」「剧集」及对应的分类栏目里。
 */
import { computed, ref } from "vue";
import { useCategoryStore } from "../stores/categoryStore";
import { useMediaStore } from "../stores/mediaStore";
import type { MediaItem, MediaType } from "../api/commands";

const props = defineProps<{ initial: MediaItem | null }>();
const emit = defineEmits<{ (e: "close"): void }>();

const media = useMediaStore();
const cat = useCategoryStore();

const isEdit = computed(() => props.initial !== null);

const title = ref(props.initial?.title ?? "");
const mediaType = ref<MediaType>(props.initial?.type ?? "movie");
const year = ref<number | null>(props.initial?.year ?? null);
const genre = ref(props.initial?.genre ?? "");
const overview = ref(props.initial?.overview ?? "");
// 仅新建模式使用：分类与标签可在创建时一并挂上
const picked = ref<number[]>([]);
const tagText = ref("");

const busy = ref(false);
const err = ref("");

function toggle(id: number): void {
  const i = picked.value.indexOf(id);
  if (i >= 0) picked.value.splice(i, 1);
  else picked.value.push(id);
}

function tagNames(): string[] {
  return tagText.value
    .split(/[,，、\s]+/)
    .map((s) => s.trim())
    .filter(Boolean);
}

/**
 * 「待确认」选项只在**编辑**一个判定不出来的条目时出现。
 *
 * 场景：扫描时证据不足（没有季集标记、目录里也没有同伴），条目被标成待确认；
 * 用户进来可能只是想改个名字，不该被逼着瞎选一个类型——所以允许保持原状。
 */
const allowUnknown = computed(() => props.initial?.type === "unknown");

/** 剧集改成电影时给用户一句提醒：单集不会被删，只是不再以「剧集」呈现。 */
const typeChangeHint = computed(() => {
  const it = props.initial;
  if (!it) return "";
  if (it.type === "unknown") {
    return "扫描时证据不足，没能判定出类型；可以手动指定，也可以保持待确认。";
  }
  if (it.type === "tv" && mediaType.value === "movie" && it.episode_count > 0) {
    return `该剧集下有 ${it.episode_count} 条单集记录，改成电影后单集会保留，但不再按季集展示。`;
  }
  return "";
});

async function submit(): Promise<void> {
  err.value = "";
  if (!title.value.trim()) {
    err.value = "请填写名称";
    return;
  }
  busy.value = true;
  try {
    const done = isEdit.value
      ? await media.updateItem({
          id: props.initial!.id,
          title: title.value,
          mediaType: mediaType.value,
          year: year.value,
          genre: genre.value,
          overview: overview.value,
        })
      : await media.createItem({
          title: title.value,
          mediaType: mediaType.value,
          year: year.value,
          genre: genre.value,
          overview: overview.value,
          categoryIds: picked.value,
          tagNames: tagNames(),
        });
    if (done) {
      // 分类 / 标签的计数可能变了，刷新侧栏
      await cat.load();
      emit("close");
    } else {
      err.value = media.error ?? (isEdit.value ? "更新失败" : "新建失败");
    }
  } finally {
    busy.value = false;
  }
}
</script>

<template>
  <div class="mask" @click.self="emit('close')">
    <div class="dialog">
      <h3>{{ isEdit ? "编辑资源" : "新建资源" }}</h3>

      <label class="field">
        <span>名称 *</span>
        <input v-model="title" placeholder="例如：沙丘 / 绝命毒师" @keyup.enter="submit" />
      </label>

      <div class="field">
        <span>类型 *</span>
        <div class="type-row">
          <label class="radio" :class="{ on: mediaType === 'movie' }">
            <input v-model="mediaType" type="radio" value="movie" />
            🎬 电影
          </label>
          <label class="radio" :class="{ on: mediaType === 'tv' }">
            <input v-model="mediaType" type="radio" value="tv" />
            📺 剧集
          </label>
          <label v-if="allowUnknown" class="radio" :class="{ on: mediaType === 'unknown' }">
            <input v-model="mediaType" type="radio" value="unknown" />
            ❓ 待确认
          </label>
        </div>
        <p v-if="typeChangeHint" class="warn">{{ typeChangeHint }}</p>
      </div>

      <label class="field">
        <span>年份</span>
        <input v-model.number="year" type="number" placeholder="2021" />
      </label>

      <label class="field">
        <span>流派（用 / 分隔）</span>
        <input v-model="genre" placeholder="剧情 / 科幻" />
      </label>

      <label class="field">
        <span>简介</span>
        <textarea v-model="overview" rows="3" placeholder="一句话介绍这部作品（可选）"></textarea>
      </label>

      <template v-if="!isEdit">
        <div class="field">
          <span>分类（可多选，也可稍后再归）</span>
          <div class="chips">
            <button
              v-for="c in cat.categories"
              :key="c.id"
              class="chip"
              :class="{ on: picked.includes(c.id) }"
              @click="toggle(c.id)"
            >{{ c.name }}</button>
            <span v-if="!cat.categories.length" class="hint">还没有分类</span>
          </div>
        </div>

        <label class="field">
          <span>标签（逗号或空格分隔）</span>
          <input v-model="tagText" placeholder="看过 收藏 4K" />
        </label>
      </template>

      <p v-if="err" class="err">{{ err }}</p>

      <div class="actions">
        <button class="btn ghost" @click="emit('close')">取消</button>
        <button class="btn primary" :disabled="busy" @click="submit">
          {{ busy ? "保存中…" : isEdit ? "保存" : "创建" }}
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
  width: 480px; max-width: calc(100vw - 32px); max-height: calc(100vh - 48px);
  display: flex; flex-direction: column; gap: calc(var(--space) * 3);
  padding: calc(var(--space) * 5); overflow-y: auto;
  border-radius: var(--radius-l); background: var(--surface);
  border: 1px solid var(--border);
}
h3 { margin: 0; font-size: var(--fs-16); font-weight: 650; }
.field { display: flex; flex-direction: column; gap: 6px; font-size: var(--fs-13); color: var(--muted); }
.field input, .field textarea {
  padding: 8px 11px; border-radius: var(--radius-s); font-size: var(--fs-13);
  border: 1px solid var(--border); background: var(--surface-2); color: var(--text);
  outline: none; font-family: inherit; resize: vertical;
}
.field input:focus, .field textarea:focus { border-color: var(--accent); }

.type-row { display: flex; gap: 8px; }
.radio {
  flex: 1; display: flex; align-items: center; justify-content: center; gap: 6px;
  padding: 8px 0; border-radius: var(--radius-s); cursor: pointer; font-size: var(--fs-13);
  border: 1px solid var(--border); background: var(--surface-2); color: var(--muted);
}
.radio.on { border-color: var(--accent); color: var(--text); background: var(--accent-soft); }
.radio input { display: none; }
.warn { margin: 0; font-size: var(--fs-12); color: var(--score); }

.chips { display: flex; flex-wrap: wrap; gap: 6px; }
.chip {
  padding: 4px 10px; border-radius: 999px; cursor: pointer; font-size: var(--fs-12);
  border: 1px solid var(--border); background: var(--surface-2); color: var(--muted);
}
.chip.on { background: var(--accent); color: #fff; border-color: transparent; }
.hint { font-size: var(--fs-12); }
.err { margin: 0; font-size: var(--fs-12); color: var(--danger); }
.actions { display: flex; justify-content: flex-end; gap: 10px; }
.btn {
  padding: 8px 16px; border-radius: var(--radius-s); cursor: pointer; font-size: var(--fs-13);
  background: var(--surface-2); color: var(--text); border: 1px solid var(--border);
}
.btn.primary { background: var(--accent); color: #fff; border-color: transparent; }
.btn:disabled { opacity: .6; cursor: default; }
</style>
