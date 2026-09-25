<script setup lang="ts">
import { computed, ref, watch } from "vue";
import {
  imageUrl, searchTmdb, tmdbTitle, tmdbYear, type Episode, type TmdbSearchItem,
} from "../api/commands";
import { useMediaStore } from "../stores/mediaStore";

const store = useMediaStore();

/** 刮削过的用 TMDB 集名，未刮削的退回文件名（更可读）。 */
function epTitle(e: Episode): string {
  const scraped = Boolean(e.still_path || e.overview);
  return scraped
    ? e.title || e.file_name || "(未命名)"
    : e.file_name || e.title || "(未命名)";
}

const detail = computed(() => store.detail);
const poster = computed(() => imageUrl(detail.value?.media.poster_path, "w342"));
const backdrop = computed(() => imageUrl(detail.value?.media.backdrop_path, "w1280"));
const score = computed(() =>
  detail.value?.media.vote_average ? detail.value.media.vote_average.toFixed(1) : null,
);

/** 单集按季分组。 */
const seasons = computed(() => {
  const eps = detail.value?.episodes ?? [];
  const map = new Map<number, typeof eps>();
  for (const e of eps) {
    const arr = map.get(e.season) ?? [];
    arr.push(e);
    map.set(e.season, arr);
  }
  return [...map.entries()]
    .sort((a, b) => a[0] - b[0])
    .map(([season, list]) => ({ season, list }));
});

/* ---------------- TMDB 手动匹配 ---------------- */
const showMatch = ref(false);
const q = ref("");
const searching = ref(false);
const results = ref<TmdbSearchItem[]>([]);
const matchMsg = ref("");

watch(
  () => store.selectedId,
  () => {
    showMatch.value = false;
    results.value = [];
    q.value = "";
    matchMsg.value = "";
  },
);

async function doSearch(): Promise<void> {
  const d = detail.value;
  if (!d) return;
  const query = q.value.trim() || d.media.title;
  if (!query) return;
  searching.value = true;
  matchMsg.value = "";
  results.value = [];
  try {
    results.value = await searchTmdb(d.media.type, query, d.media.year);
    if (!results.value.length) matchMsg.value = "没有找到结果，换个关键词试试";
  } catch (e) {
    matchMsg.value = String(e);
  } finally {
    searching.value = false;
  }
}

function pick(it: TmdbSearchItem): void {
  const d = detail.value;
  if (!d) return;
  showMatch.value = false;
  store.scrape(d.media.id, it.id);
}
</script>

<template>
  <article v-if="detail" class="detail">
    <button class="back" @click="store.closeDetail()">← 返回列表</button>

    <header class="head" :style="backdrop ? { backgroundImage: `url(${backdrop})` } : {}">
      <div class="shade"></div>
      <div class="head-inner">
        <img v-if="poster" class="poster" :src="poster" :alt="detail.media.title" />
        <div v-else class="poster ph">{{ detail.media.type === "tv" ? "📺" : "🎬" }}</div>
        <div class="info">
          <h2>{{ detail.media.title }}</h2>
          <p class="line">
            <span class="tag">{{ detail.media.type === "tv" ? "剧集" : "电影" }}</span>
            <span v-if="detail.media.year">{{ detail.media.year }}</span>
            <span v-if="detail.media.genre">{{ detail.media.genre }}</span>
            <span v-if="score" class="score">★ {{ score }}</span>
            <span v-if="detail.media.episode_count">共 {{ detail.media.episode_count }} 集</span>
          </p>
          <p class="overview">{{ detail.media.overview || "暂无简介（可用下方「手动匹配 TMDB」补全）" }}</p>
          <p v-if="detail.media.file_name" class="file" :title="detail.media.file_path ?? ''">
            {{ detail.media.file_name }}
          </p>
        </div>
      </div>
    </header>

    <div class="actions">
      <button
        v-if="detail.media.type === 'movie'"
        class="btn primary"
        @click="store.play(detail.media.file_path)"
      >▶ 播放</button>
      <button class="btn" @click="store.scrape(detail.media.id)">
        {{ detail.media.tmdb_id ? "重新刮削" : "刮削元数据" }}
      </button>
      <button class="btn" @click="showMatch = !showMatch">手动匹配 TMDB</button>
      <button
        v-if="detail.media.file_path"
        class="btn ghost"
        @click="store.reveal(detail.media.file_path)"
      >在文件夹中显示</button>
      <button class="btn danger" @click="store.remove(detail.media.id)">从库中移除</button>
    </div>

    <section v-if="showMatch" class="match">
      <div class="match-row">
        <input
          v-model="q"
          :placeholder="`搜索词（默认：${detail.media.title}）`"
          @keyup.enter="doSearch"
        />
        <button class="btn" :disabled="searching" @click="doSearch">
          {{ searching ? "搜索中…" : "搜索" }}
        </button>
      </div>
      <p v-if="matchMsg" class="match-msg">{{ matchMsg }}</p>
      <ul v-if="results.length" class="results">
        <li v-for="it in results" :key="it.id">
          <img
            v-if="imageUrl(it.poster_path, 'w92')"
            :src="imageUrl(it.poster_path, 'w92')!"
            alt=""
          />
          <div class="r-info">
            <span class="r-title">{{ tmdbTitle(it) }}</span>
            <span class="r-sub">
              <span v-if="tmdbYear(it)">{{ tmdbYear(it) }}</span>
              <span v-if="it.vote_average">★ {{ it.vote_average.toFixed(1) }}</span>
              <span class="r-id">TMDB #{{ it.id }}</span>
            </span>
          </div>
          <button class="btn small" @click="pick(it)">应用</button>
        </li>
      </ul>
    </section>

    <section v-if="seasons.length" class="episodes">
      <div v-for="s in seasons" :key="s.season" class="season">
        <h3>第 {{ s.season }} 季</h3>
        <ul>
          <li v-for="e in s.list" :key="e.id">
            <img
              v-if="imageUrl(e.still_path, 'w92')"
              class="ep-thumb"
              :src="imageUrl(e.still_path, 'w92')!"
              alt=""
            />
            <span class="ep-no">E{{ String(e.episode).padStart(2, "0") }}</span>
            <span class="ep-main">
              <span class="ep-title" :title="e.file_name ?? ''">{{ epTitle(e) }}</span>
              <span v-if="e.air_date" class="ep-date">{{ e.air_date }}</span>
            </span>
            <button class="btn small" @click="store.play(e.file_path)">▶ 播放</button>
            <button class="btn small ghost" @click="store.reveal(e.file_path)">目录</button>
          </li>
        </ul>
      </div>
    </section>
    <section v-else-if="detail.media.type === 'tv'" class="episodes empty">
      暂无单集记录，请先扫描媒体库。
    </section>
  </article>
</template>

<style scoped>
.detail { display: flex; flex-direction: column; gap: calc(var(--space) * 5); }
.back {
  align-self: flex-start; padding: 6px 0; border: 0; background: none;
  color: var(--muted); font-size: 13px; cursor: pointer;
}
.back:hover { color: var(--text); }

.head {
  position: relative; border-radius: 10px; overflow: hidden; background-color: var(--surface);
  background-size: cover; background-position: center;
}
.shade {
  position: absolute; inset: 0;
  background: linear-gradient(90deg, rgba(20,20,20,.96) 0%, rgba(20,20,20,.84) 55%, rgba(20,20,20,.55) 100%);
}
.head-inner {
  position: relative; display: flex; gap: calc(var(--space) * 5);
  padding: calc(var(--space) * 6); align-items: flex-start;
}
.poster { width: 165px; border-radius: 8px; flex: none; display: block; }
.poster.ph {
  aspect-ratio: 2 / 3; display: flex; align-items: center; justify-content: center;
  font-size: 40px; background: rgba(255,255,255,.06);
}
.info { display: flex; flex-direction: column; gap: 10px; min-width: 0; }
h2 { margin: 0; font-size: 24px; line-height: 1.25; }
.line { display: flex; flex-wrap: wrap; gap: 10px; margin: 0; font-size: 13px; color: var(--muted); }
.tag { padding: 1px 7px; border-radius: 4px; background: var(--accent); color: #fff; font-size: 12px; }
.score { color: #f5c518; }
.overview { margin: 0; font-size: 14px; line-height: 1.7; color: #ddd; max-width: 70ch; }
.file { margin: 0; font-size: 12px; color: var(--muted); word-break: break-all; }

.actions { display: flex; flex-wrap: wrap; gap: 10px; }
.btn {
  padding: 8px 14px; border: 0; border-radius: 6px; cursor: pointer; font-size: 13px;
  background: rgba(255,255,255,.9); color: #111;
}
.btn.primary { background: var(--accent); color: #fff; }
.btn.ghost { background: rgba(255,255,255,.12); color: var(--text); }
.btn.danger { background: rgba(229,9,20,.16); color: #ff6b73; }
.btn.small { padding: 5px 10px; font-size: 12px; }
.btn:disabled { opacity: .6; cursor: default; }

.match { display: flex; flex-direction: column; gap: 10px; }
.match-row { display: flex; gap: 8px; }
.match-row input {
  flex: 1; padding: 8px 10px; border-radius: 6px; font-size: 13px;
  border: 1px solid rgba(255,255,255,.14); background: var(--surface); color: var(--text);
}
.match-msg { margin: 0; font-size: 13px; color: var(--muted); }
.results { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; gap: 8px; }
.results li {
  display: flex; align-items: center; gap: 12px; padding: 8px;
  border-radius: 8px; background: var(--surface);
}
.results img { width: 44px; border-radius: 4px; flex: none; }
.r-info { flex: 1; display: flex; flex-direction: column; gap: 3px; min-width: 0; }
.r-title { font-size: 14px; }
.r-sub { display: flex; gap: 10px; font-size: 12px; color: var(--muted); }
.r-id { opacity: .7; }

.episodes { display: flex; flex-direction: column; gap: calc(var(--space) * 5); }
.episodes.empty { color: var(--muted); font-size: 13px; }
h3 { margin: 0 0 8px; font-size: 15px; color: var(--muted); font-weight: 600; }
.season ul { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; gap: 4px; }
.season li {
  display: flex; align-items: center; gap: 12px; padding: 7px 10px;
  border-radius: 6px; background: var(--surface);
}
.ep-no { flex: none; width: 34px; font-size: 12px; color: var(--muted); }
.ep-thumb { width: 56px; border-radius: 4px; flex: none; display: block; }
.ep-main { flex: 1; display: flex; flex-direction: column; gap: 2px; min-width: 0; }
.ep-title { font-size: 13px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.ep-date { font-size: 11px; color: var(--muted); }
</style>
