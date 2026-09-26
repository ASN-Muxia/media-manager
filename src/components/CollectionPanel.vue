<script setup lang="ts">
/**
 * 系列电影合集。
 *
 * 数据**不是**库里的一张表，而是后端按「系列基名」从电影标题算出来的视图
 * （见 `src-tauri/src/collection.rs`）：
 * - 聚合依据：砍掉冒号副标题与尾部续作标记后的系列基名（`沙丘 2` / `Dune: Part Two` → 同一系列）；
 * - 去重：`(标题, 年份)` 相同的多个版本只占一个位次，其余计入 `duplicateCount`；
 * - 集内排序：按上映时间升序，同年按片名，没有年份的排最后。
 *
 * 这里只负责展示与「点开某一部」，不修改任何数据。
 */
import { computed } from "vue";
import { imageUrl, type MovieCollection } from "../api/commands";

const props = defineProps<{
  collections: MovieCollection[];
  loading?: boolean;
}>();
const emit = defineEmits<{ (e: "open", id: number): void }>();

/** 关键字过滤放到父级；这里只按年份跨度给一句说明。 */
const totalMovies = computed(() =>
  props.collections.reduce((n, c) => n + c.movies.length, 0),
);

function span(c: MovieCollection): string {
  if (c.startYear && c.endYear && c.startYear !== c.endYear) {
    return `${c.startYear} – ${c.endYear}`;
  }
  return c.startYear ? String(c.startYear) : "年份未知";
}
</script>

<template>
  <div class="wrap">
    <p v-if="loading" class="hint">正在计算系列合集…</p>

    <div v-else-if="collections.length" class="head">
      <span class="sum">{{ collections.length }} 个系列 · {{ totalMovies }} 部电影</span>
      <span class="hint">同一系列按上映时间排序；同片多版本已折叠。</span>
    </div>

    <section v-for="c in collections" :key="c.key" class="coll">
      <header class="coll-head">
        <h2>{{ c.name }}</h2>
        <span class="meta">
          {{ span(c) }} · {{ c.movies.length }} 部
          <template v-if="c.duplicateCount"> · 折叠 {{ c.duplicateCount }} 个重复版本</template>
        </span>
      </header>

      <div class="row">
        <button
          v-for="m in c.movies"
          :key="m.id"
          class="card"
          :title="m.title"
          @click="emit('open', m.id)"
        >
          <div class="poster">
            <img v-if="imageUrl(m.poster_path, 'w185')" :src="imageUrl(m.poster_path, 'w185') ?? ''" :alt="m.title" loading="lazy" />
            <div v-else class="ph"><span>🎬</span></div>
          </div>
          <span class="title">{{ m.title }}</span>
          <span class="year">{{ m.year ?? "—" }}</span>
        </button>
      </div>
    </section>

    <div v-if="!loading && !collections.length" class="empty">
      <p class="big">还没有能凑成合集的系列</p>
      <p class="hint">
        同一系列 / IP 的电影（如「沙丘」「沙丘 2」）会在这里自动归到一起，
        至少两部才成系列。
      </p>
    </div>
  </div>
</template>

<style scoped>
.wrap { display: flex; flex-direction: column; gap: calc(var(--space) * 6); }
.head { display: flex; align-items: baseline; gap: 12px; flex-wrap: wrap; }
.sum { font-size: var(--fs-14); font-weight: 600; color: var(--text); }

.coll { display: flex; flex-direction: column; gap: calc(var(--space) * 3); }
.coll-head { display: flex; align-items: baseline; gap: 12px; flex-wrap: wrap; }
.coll-head h2 { margin: 0; font-size: var(--fs-15); font-weight: 650; }
.coll-head .meta { font-size: var(--fs-12); color: var(--muted); }

.row {
  display: grid; gap: calc(var(--space) * 4);
  grid-template-columns: repeat(auto-fill, minmax(120px, 1fr));
}
.card {
  display: flex; flex-direction: column; gap: 4px; padding: 0; border: 0;
  background: none; color: inherit; cursor: pointer; text-align: left;
}
.poster {
  aspect-ratio: 2 / 3; width: 100%; overflow: hidden; border-radius: var(--radius-s);
  background: var(--surface); border: 1px solid var(--border);
  transition: transform .18s ease, box-shadow .18s ease;
}
.card:hover .poster { transform: translateY(-3px); box-shadow: var(--shadow); }
.poster img { width: 100%; height: 100%; object-fit: cover; display: block; }
.ph {
  width: 100%; height: 100%; display: flex; align-items: center; justify-content: center;
  font-size: 22px; opacity: .7;
}
.title {
  font-size: var(--fs-12); line-height: 1.35; color: var(--text);
  overflow: hidden; text-overflow: ellipsis; white-space: nowrap;
}
.year { font-size: var(--fs-11); color: var(--muted); }

.empty { padding: calc(var(--space) * 16) 0; text-align: center; color: var(--muted); }
.empty .big { margin: 0 0 8px; font-size: 16px; color: var(--text); }
.hint { margin: 0; font-size: var(--fs-12); color: var(--muted); }
</style>
