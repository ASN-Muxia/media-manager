export type MediaType = "movie" | "tv";

/** 媒体列表项 / 详情主记录（与 Rust `MediaListItem` 对应）。 */
export interface MediaItem {
  id: number;
  type: MediaType;
  title: string;
  year: number | null;
  tmdb_id: number | null;
  poster_path: string | null;
  backdrop_path: string | null;
  overview: string | null;
  vote_average: number | null;
  genre: string | null;
  file_path: string | null;
  file_name: string | null;
  episode_count: number;
  added_at: number;
}

export interface Episode {
  id: number;
  media_id: number;
  season: number;
  episode: number;
  title: string | null;
  file_path: string | null;
  file_name: string | null;
  file_size: number | null;
  added_at: number;
}

export interface MediaDetail {
  media: MediaItem;
  episodes: Episode[];
}

/** 与 Rust `commands::settings::Settings` 一一对应。 */
export interface Settings {
  tmdbKey: string;
  mediaDirs: string[];
  potPlayerPath: string;
}

/** 数据层自检信息（Rust 端 `db_info` 命令）。 */
export interface DbInfo { tables: string[] }

/** 扫描统计（Rust 端 camelCase 序列化）。 */
export interface ScanStats {
  roots: number;
  scanned: number;
  movies: number;
  series: number;
  episodes: number;
  addedMovies: number;
  addedSeries: number;
  addedEpisodes: number;
  skipped: number;
  failed: number;
}

export interface ScanHistoryItem {
  id: number;
  path: string;
  started_at: number;
  finished_at: number | null;
  status: string;
  found: number;
  added: number;
  message: string | null;
}

export interface TmdbSearchItem {
  id: number;
  title?: string | null;
  name?: string | null;
  overview?: string | null;
  poster_path?: string | null;
  backdrop_path?: string | null;
  vote_average?: number | null;
  release_date?: string | null;
  first_air_date?: string | null;
}

export interface ScrapeOutcome {
  ok: boolean;
  mediaId: number;
  tmdbId: number | null;
  title: string | null;
  message: string;
}

export interface ScrapeSummary {
  total: number;
  ok: number;
  failed: number;
  noKey: boolean;
}

export async function invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke<T>(cmd, args);
}

/** 拼 TMDB 图片地址。 */
export function imageUrl(path: string | null | undefined, size = "w342"): string | null {
  return path ? `https://image.tmdb.org/t/p/${size}${path}` : null;
}

/** 搜索结果里取显示标题。 */
export function tmdbTitle(it: TmdbSearchItem): string {
  return it.title || it.name || "(无标题)";
}

/** 搜索结果里取年份。 */
export function tmdbYear(it: TmdbSearchItem): number | null {
  const d = it.release_date || it.first_air_date || "";
  const y = Number(d.slice(0, 4));
  return Number.isFinite(y) && y > 0 ? y : null;
}

/* ---------------------- 模块② 设置持久化 ---------------------- */

export function getSettings(): Promise<Settings> {
  return invoke<Settings>("get_settings");
}

export function setSettings(settings: Settings): Promise<void> {
  return invoke<void>("set_settings", { settings });
}

export function dbInfo(): Promise<DbInfo> {
  return invoke<DbInfo>("db_info");
}

/* ---------------------- 模块③ 扫描与列表 ---------------------- */

export function scanLibrary(roots?: string[]): Promise<ScanStats> {
  return invoke<ScanStats>("scan_library", roots ? { roots } : {});
}

export function getMediaList(mediaType: "all" | "movie" | "tv" = "all"): Promise<MediaItem[]> {
  return invoke<MediaItem[]>("get_media_list", { mediaType });
}

export function getMediaDetail(id: number): Promise<MediaDetail> {
  return invoke<MediaDetail>("get_media_detail", { id });
}

export function getEpisodes(mediaId: number): Promise<Episode[]> {
  return invoke<Episode[]>("get_episodes", { mediaId });
}

export function deleteMedia(id: number): Promise<void> {
  return invoke<void>("delete_media", { id });
}

export function clearLibrary(): Promise<void> {
  return invoke<void>("clear_library");
}

export function getScanHistory(limit = 20): Promise<ScanHistoryItem[]> {
  return invoke<ScanHistoryItem[]>("get_scan_history", { limit });
}

/* ------------------------ 模块④ 刮削 ------------------------ */

export function searchTmdb(
  mediaType: MediaType,
  query: string,
  year?: number | null,
): Promise<TmdbSearchItem[]> {
  return invoke<TmdbSearchItem[]>("search_tmdb", { mediaType, query, year: year ?? null });
}

export function scrapeMedia(id: number, tmdbId?: number | null): Promise<ScrapeOutcome> {
  return invoke<ScrapeOutcome>("scrape_media", { id, tmdbId: tmdbId ?? null });
}

export function scrapeAll(limit = 50): Promise<ScrapeSummary> {
  return invoke<ScrapeSummary>("scrape_all", { limit });
}

export function hasMetadata(id: number): Promise<boolean> {
  return invoke<boolean>("has_metadata", { id });
}

/* ------------------------ 模块⑦ 播放 ------------------------ */

export function detectPlayer(): Promise<string | null> {
  return invoke<string | null>("detect_player");
}

export function playMedia(path: string): Promise<string> {
  return invoke<string>("play_media", { path });
}

export function revealInExplorer(path: string): Promise<void> {
  return invoke<void>("reveal_in_explorer", { path });
}
