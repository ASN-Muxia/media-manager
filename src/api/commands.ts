export type MediaType = "movie" | "tv";

export interface MediaItem {
  id: number; type: MediaType; file_path: string; file_name: string;
  title: string; year: number | null; tmdb_id: number | null;
  season: number | null; episode: number | null; poster_path: string | null;
  backdrop_path: string | null; overview: string | null; vote_average: number | null;
  genre: string | null;
}

export interface ScrapeResult { ok: boolean; error?: string; item?: MediaItem }

/** 与 Rust 端 `commands::settings::Settings` 一一对应。 */
export interface Settings {
  tmdbKey: string;
  mediaDirs: string[];
  potPlayerPath: string;
}

/** 数据层自检信息（Rust 端 `db_info` 命令）。 */
export interface DbInfo { tables: string[] }

export async function invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke<T>(cmd, args);
}

/* ---------- 模块② 设置持久化 ---------- */

export function getSettings(): Promise<Settings> {
  return invoke<Settings>("get_settings");
}

export function setSettings(settings: Settings): Promise<void> {
  return invoke<void>("set_settings", { settings });
}

export function dbInfo(): Promise<DbInfo> {
  return invoke<DbInfo>("db_info");
}
