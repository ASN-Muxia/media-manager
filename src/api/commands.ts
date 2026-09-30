/**
 * 媒体类型。
 *
 * `unknown` = **证据不足，判定不了**。它是个正常状态而不是错误：宁可让用户
 * 自己在「待确认」里手动改，也不再像老版本那样一律塞进电影。
 */
export type MediaType = "movie" | "tv" | "unknown";

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
  /** 展示用标题：同名剧集会带目录名后缀（`剧名 · 目录名`） */
  display_title: string;
  /** `scan`（扫描归纳出来的）/ `manual`（手动新建的 Series） */
  source?: string;
  /** 用户确认过分组后锁住，二次校正不再调整 */
  locked?: boolean;
}

/** 分类（左侧导航的大栏目）。**全部来自数据库，前端没有任何硬编码枚举。** */
export interface Category {
  id: number;
  name: string;
  /** `manual`（手动建）/ `auto`（自动识别补出来的） */
  kind: string;
  /** 来源：`manual` / `tmdb` / `folder` / `rule` */
  origin: string;
  sortOrder: number;
  /** 该分类下有多少条影视 */
  seriesCount: number;
}

/** 标签（自由输入，用于筛选）。 */
export interface Tag {
  id: number;
  name: string;
  usageCount: number;
}

/** 从 TMDB 类型同步分类的结果。 */
export interface SyncResult {
  created: number;
  linked: number;
  scanned: number;
}

/** 大模型接入预设（设置页下拉框用）。 */
export interface LlmPreset {
  label: string;
  baseUrl: string;
  model: string;
  visionModel: string;
  visionBaseUrl: string;
}

/** 大模型解析出的结构化字段。 */
export interface LlmFields {
  title: string;
  year: number | null;
  season: number | null;
  episode: number | null;
  resolution: string | null;
  releaseSource: string | null;
}

/** 手动应用 TheTVDB 的结果。 */
export interface TvdbApplyResult {
  tvdbId: number;
  episodes: number;
  genre: string;
}

/** OMDb 查到的信息。 */
export interface OmdbInfo {
  title: string;
  year: string;
  rating: number | null;
  genre: string;
  plot: string;
}

/** 豆瓣查到的信息（中日文元数据兜底）。 */
export interface DoubanInfo {
  doubanId: string;
  title: string;
  originalTitle: string;
  year: number | null;
  directors: string;
  actors: string;
  genre: string;
  releaseDate: string;
  rating: number | null;
  overview: string;
  poster: string;
  /** 剧集集数（电影为 0）。 */
  episodes: number;
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
  /** 以下字段由单集级刮削回填 */
  tmdb_id?: number | null;
  overview?: string | null;
  still_path?: string | null;
  air_date?: string | null;
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
  /** 自定义域名解析，hosts 文件格式：`IP 域名`（一行一条）。 */
  hosts: string[];
  /** 元数据语言（`zh-CN` / `en-US` …），留空按后端默认处理。 */
  tmdbLanguage: string;
  /** TheTVDB API Key（剧集季 / 集结构；可选） */
  tvdbKey: string;
  /** Fanart.tv API Key（补充海报 / 背景；可选） */
  fanartKey: string;
  /** OMDb API Key（IMDb 评分补充；可选） */
  omdbKey: string;
  /** 豆瓣刮削总开关。**默认关闭** —— 豆瓣非官方接口且涉第三方条款，由用户自行决定 */
  doubanEnabled: boolean;
  /** 豆瓣 Cookie（可选，带登录态可降低验证码概率） */
  doubanCookie: string;
  /**
   * 扫描 / 导入完成后自动为**还没有海报**的条目跑一遍刮削（默认关闭）。
   * 开启后仍受 TMDb / 豆瓣各自开关约束；没配 TMDb Key 时豆瓣承担主要工作。
   */
  autoScrapeAfterScan: boolean;
  /** 大模型脏文件名解析总开关（**可选增强层**，默认关闭） */
  llmEnabled: boolean;
  llmBaseUrl: string;
  llmApiKey: string;
  llmModel: string;
  /** 视觉模型名；留空表示不启用视觉兜底 */
  llmVisionModel: string;
  llmVisionBaseUrl: string;
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
  /** 判定不出类型的文件数 */
  unknown: number;
  addedMovies: number;
  addedSeries: number;
  addedEpisodes: number;
  /** 新入库的「待确认」条数 */
  addedUnknown: number;
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
  /** 本次同步到的单集条数 */
  episodes: number;
  message: string;
}

/** 批量刮削的进度（Rust 端 `scrape-progress` 事件载荷）。 */
export interface ScrapeProgress {
  running: boolean;
  finished: boolean;
  cancelled: boolean;
  total: number;
  done: number;
  ok: number;
  failed: number;
  /** 当前正在处理的条目标题。 */
  current: string;
  message: string;
}

/** 启动批量刮削的返回值：命令不等结果，只说明「起没起来」。 */
export interface ScrapeStart {
  started: boolean;
  total: number;
  noKey: boolean;
  message: string;
}

/** 扫描进度（Rust 端 `scan-progress` 事件载荷）。 */
export interface ScanProgress {
  running: boolean;
  finished: boolean;
  cancelled: boolean;
  /** 阶段说明：`遍历目录` / `写入数据库` / `完成` / `已取消` / `出错` */
  phase: string;
  /** 已发现的视频文件数 */
  found: number;
  /** 已入库条数 */
  imported: number;
  /** 待入库总数（进入入库阶段后才确定） */
  total: number;
  current: string;
  message: string;
  /** 结束时的统计；进行中为 null */
  stats: ScanStats | null;
}

/** 启动扫描的返回值：命令不等结果，只说明「起没起来」。 */
export interface ScanStart {
  started: boolean;
  message: string;
}

// 直接复用 Tauri 的 invoke（插件已静态依赖 @tauri-apps/api，
// 这里再包一层动态 import 只会让打包告警且无收益）。
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export { invoke };
export type { UnlistenFn };

/**
 * 订阅批量刮削进度事件。
 *
 * 刮削在后台线程跑，进度/结束都通过事件回传，这样主线程（界面）始终空闲。
 * 返回值用于取消订阅。
 */
export function onScrapeProgress(cb: (p: ScrapeProgress) => void): Promise<UnlistenFn> {
  return listen<ScrapeProgress>("scrape-progress", (e) => cb(e.payload));
}

/**
 * 订阅扫描进度事件。
 *
 * 扫描同样在后台线程跑（遍历大目录可能要几十秒），界面靠这个事件更新进度条。
 * 返回值用于取消订阅。
 */
export function onScanProgress(cb: (p: ScanProgress) => void): Promise<UnlistenFn> {
  return listen<ScanProgress>("scan-progress", (e) => cb(e.payload));
}

/**
 * 拼图片地址。
 *
 * 两种输入都吃：
 * - TMDb 的相对路径（`/abc.jpg`）→ 拼上 `image.tmdb.org`；
 * - Fanart.tv / TheTVDB 的**完整 URL** → 原样返回。
 * 后者是补图时写进去的，若不识别前缀就会拼出一个坏地址。
 */
export function imageUrl(path: string | null | undefined, size = "w342"): string | null {
  const p = (path ?? "").trim();
  if (!p) return null;
  if (/^https?:\/\//i.test(p)) return p;
  return `https://image.tmdb.org/t/p/${size}${p}`;
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

/** 在资源管理器中打开应用数据目录。 */
export function openDataDir(): Promise<string> {
  return invoke<string>("open_data_dir");
}

/** 运行时诊断信息（数据库位置、结构版本等）。 */
export interface RuntimeInfo {
  dbPath: string;
  schemaVersion: number;
  tables: string[];
  os: string;
  arch: string;
}

export function runtimeInfo(): Promise<RuntimeInfo> {
  return invoke<RuntimeInfo>("runtime_info");
}

/** 前端启动自检标记（内容含页面 URL，可判断是 dev 模式还是内嵌资源模式）。 */
export function readReadyMarker(): Promise<string | null> {
  return invoke<string | null>("read_ready_marker");
}

/** 读取前端错误日志尾部（白屏排查用）。 */
export function readErrorLog(limit = 4000): Promise<string> {
  return invoke<string>("read_error_log", { limit });
}

/* ---------------------- 模块③ 扫描与列表 ---------------------- */

/**
 * 启动扫描（后台执行，立即返回；进度走 `onScanProgress`）。
 *
 * 遍历大目录要几十秒甚至更久，放在主线程会让界面整个卡住，
 * 所以这里和批量刮削一样：只负责起任务，结果由事件回传。
 */
export function scanLibrary(roots?: string[]): Promise<ScanStart> {
  return invoke<ScanStart>("scan_library", roots ? { roots } : {});
}

/** 取消正在进行的扫描（遍历阶段生效；入库阶段不中断）。 */
export function cancelScan(): Promise<boolean> {
  return invoke<boolean>("cancel_scan");
}

/** 列表查询条件。分类 / 标签 / 关键字 / 未分类都在 SQL 层过滤，前端不二次筛选。 */
export interface MediaQuery {
  mediaType?: "all" | MediaType;
  /** 只取该分类下的条目 */
  categoryId?: number | null;
  /** 只取带该标签的条目 */
  tagId?: number | null;
  /** 标题 / 原始标题 / 文件名的模糊匹配 */
  keyword?: string | null;
  /** 只看还没归类的条目 */
  unclassified?: boolean;
}

export function getMediaList(query: MediaQuery = {}): Promise<MediaItem[]> {
  return invoke<MediaItem[]>("get_media_list", {
    mediaType: query.mediaType ?? "all",
    categoryId: query.categoryId ?? null,
    tagId: query.tagId ?? null,
    keyword: query.keyword?.trim() || null,
    unclassified: query.unclassified ?? false,
  });
}

/**
 * 系列电影合集（派生视图：由电影标题算出来，不改数据）。
 *
 * 成员默认按上映时间升序；`duplicateCount` 是被折叠掉的「同片多版本」条数。
 */
export interface MovieCollection {
  key: string;
  name: string;
  movies: CollectionMovie[];
  duplicateCount: number;
  startYear: number | null;
  endYear: number | null;
}

/** 合集里的一部电影。 */
export interface CollectionMovie {
  id: number;
  title: string;
  year: number | null;
  poster_path: string | null;
  file_path: string | null;
}

/** 系列电影合集列表。空数组表示库里还没有能凑成合集的系列。 */
export function getMovieCollections(): Promise<MovieCollection[]> {
  return invoke<MovieCollection[]>("get_movie_collections");
}

/** 重新识别存量数据的结果（界面直接展示这几个数字）。 */
export interface ReclassifyResult {
  /** 参与重新判定的条目数（媒体行 + 单集行） */
  scanned: number;
  changed: number;
  /** 修正为剧集的条数 */
  toTv: number;
  /** 修正为电影的条数 */
  toMovie: number;
  /** 标为待确认的条数 */
  toUnknown: number;
  /** 用户锁定而跳过的条数 */
  skippedLocked: number;
  message: string;
}

/**
 * 对库里已有的条目重新跑一遍类型判定。
 *
 * 老版本只有「有季集号才算剧集」一条规则，把大量剧集判成了电影。升级后
 * 不用清空重扫，跑一次这个就能修正：电影 → 剧集会搬进单集表并挂到同名剧集下。
 * 传 `ids` 只重判这几条；不传就是全库。
 */
export function reclassifyMedia(ids?: number[] | null): Promise<ReclassifyResult> {
  return invoke<ReclassifyResult>("reclassify_media", { ids: ids ?? null });
}

/** 手动新建资源的输入（电影 / 剧集都行，与扫描结果同表）。 */
export interface CreateMediaInput {
  title: string;
  mediaType: MediaType;
  year?: number | null;
  genre?: string | null;
  overview?: string | null;
  categoryIds?: number[];
  tagNames?: string[];
}

/** 从文件夹导入的结果（界面直接展示这几个数字）。 */
export interface ImportFolderResult {
  /** 本次真正入库的文件数（电影 + 单集） */
  added: number;
  /** 库里已有、被跳过的文件数 */
  skipped: number;
  /** 入库失败的文件数 */
  failed: number;
  /** 挂到目标栏目下的影视条数 */
  linked: number;
  message: string;
}

/**
 * 从本地文件夹导入。
 *
 * 复用扫描那套解析 / 分组 / 入库流程，**重复文件自动跳过**；
 * 传 `categoryId` 时把导入的条目一并归入该栏目。
 */
export function importFolder(path: string, categoryId?: number | null): Promise<ImportFolderResult> {
  return invoke<ImportFolderResult>("import_folder", {
    path,
    categoryId: categoryId ?? null,
  });
}

/** 编辑资源基本信息的输入（这几个字段全量覆盖；留空 = 清空）。 */
export interface UpdateMediaInput {
  id: number;
  title: string;
  mediaType: MediaType;
  year?: number | null;
  genre?: string | null;
  overview?: string | null;
}

/** 手动新建一条资源（电影或剧集）。 */
export function createMedia(input: CreateMediaInput): Promise<MediaItem> {
  return invoke<MediaItem>("create_media", {
    title: input.title,
    mediaType: input.mediaType,
    year: input.year ?? null,
    genre: input.genre ?? null,
    overview: input.overview ?? null,
    categoryIds: input.categoryIds ?? null,
    tagNames: input.tagNames ?? null,
  });
}

/** 更新一条资源的基本信息（名称 / 类型 / 年份 / 流派 / 简介）。 */
export function updateMedia(input: UpdateMediaInput): Promise<MediaItem> {
  return invoke<MediaItem>("update_media", {
    id: input.id,
    title: input.title,
    mediaType: input.mediaType,
    year: input.year ?? null,
    genre: input.genre ?? null,
    overview: input.overview ?? null,
  });
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

/**
 * 批量删除媒体（单事务）。
 *
 * 只删 `ids` 里的条目，返回**实际删除条数**——界面用它提示「已删除 N 条」，
 * 而不是假设「传进去几个就删了几个」。
 */
export function deleteMediaBatch(ids: number[]): Promise<number> {
  return invoke<number>("delete_media_batch", { ids });
}

/** 批量删除单集（只删传入的 id，不影响其他剧目）。返回实际删除条数。 */
export function deleteEpisodes(ids: number[]): Promise<number> {
  return invoke<number>("delete_episodes", { ids });
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

/** 启动批量刮削（后台执行，立即返回；进度走 `onScrapeProgress`）。 */
export function scrapeAll(limit = 50): Promise<ScrapeStart> {
  return invoke<ScrapeStart>("scrape_all", { limit });
}

/** 取消正在进行的批量刮削。 */
export function cancelScrape(): Promise<boolean> {
  return invoke<boolean>("cancel_scrape");
}

/** 测试 TMDB 连通性（验证 API Key 与 hosts 配置是否生效）。 */
export function tmdbTest(): Promise<string> {
  return invoke<string>("tmdb_test");
}

export function hasMetadata(id: number): Promise<boolean> {
  return invoke<boolean>("has_metadata", { id });
}

/* --------------------- 分类与标签（左侧导航） --------------------- */
/* 分类全部来自数据库：前端不保留任何栏目枚举，界面上看到什么由数据决定。 */

export function listCategories(): Promise<Category[]> {
  return invoke<Category[]>("list_categories");
}

/** 新建分类；同名（忽略大小写）已存在时返回已有的那条。 */
export function createCategory(name: string, origin = "manual"): Promise<Category> {
  return invoke<Category>("create_category", { name, origin });
}

export function renameCategory(id: number, name: string): Promise<void> {
  return invoke<void>("rename_category", { id, name });
}

export function deleteCategory(id: number): Promise<void> {
  return invoke<void>("delete_category", { id });
}

export function listTags(): Promise<Tag[]> {
  return invoke<Tag[]>("list_tags");
}

export function createTag(name: string): Promise<Tag> {
  return invoke<Tag>("create_tag", { name });
}

export function renameTag(id: number, name: string): Promise<void> {
  return invoke<void>("rename_tag", { id, name });
}

export function deleteTag(id: number): Promise<void> {
  return invoke<void>("delete_tag", { id });
}

/** 某条影视所属的分类。 */
export function getMediaCategories(mediaId: number): Promise<Category[]> {
  return invoke<Category[]>("get_media_categories", { mediaId });
}

/** 全量覆盖某条影视的分类。 */
export function setMediaCategories(mediaId: number, categoryIds: number[]): Promise<Category[]> {
  return invoke<Category[]>("set_media_categories", { mediaId, categoryIds });
}

export function getMediaTags(mediaId: number): Promise<Tag[]> {
  return invoke<Tag[]>("get_media_tags", { mediaId });
}

/** 用**名字**全量覆盖标签；不存在的名字会自动建出来。 */
export function setMediaTags(mediaId: number, names: string[]): Promise<Tag[]> {
  return invoke<Tag[]>("set_media_tags", { mediaId, names });
}

/** 把库里所有条目的 TMDB 类型补成分类（历史数据迁移用）。 */
export function syncCategoriesFromTmdb(): Promise<SyncResult> {
  return invoke<SyncResult>("sync_categories_from_tmdb");
}

/* --------------------- 其他元数据 provider --------------------- */

/** 手动绑定 TheTVDB 剧集并按它的季 / 集结构回填单集。 */
export function tvdbApply(mediaId: number, tvdbId?: number | null): Promise<TvdbApplyResult> {
  return invoke<TvdbApplyResult>("tvdb_apply", { mediaId, tvdbId: tvdbId ?? null });
}

/** 查询 OMDb（评分补充）。 */
export function omdbLookup(title: string, year?: number | null): Promise<OmdbInfo> {
  return invoke<OmdbInfo>("omdb_lookup", { title, year: year ?? null });
}

/** 搜索豆瓣候选（设置页试检索 / 详情页手动匹配）。 */
export function searchDouban(query: string): Promise<DoubanInfo[]> {
  return invoke<DoubanInfo[]>("search_douban", { query });
}

/** 用豆瓣刮削指定条目（TMDb 已有结果时只补缺失，否则整条接管）。 */
export function doubanApply(
  mediaId: number,
  query?: string | null,
  year?: number | null,
): Promise<ScrapeOutcome> {
  return invoke<ScrapeOutcome>("douban_apply", {
    mediaId,
    query: query ?? null,
    year: year ?? null,
  });
}

/** 测试豆瓣连通性。成功返回可读文案；匹配失败 / 网络异常都作为异常抛出。 */
export function doubanTest(): Promise<string> {
  return invoke<string>("douban_test");
}

/** 大模型接入预设（推荐的排第一）。 */
export function llmPresets(): Promise<LlmPreset[]> {
  return invoke<LlmPreset[]>("llm_presets");
}

/** 让大模型解析一个脏文件名（**只产出检索词，不替代元数据 API**）。 */
export function llmParseName(fileName: string): Promise<LlmFields> {
  return invoke<LlmFields>("llm_parse_name", { fileName });
}

/**
 * 视觉模型兜底：仅在文件名完全不可推断时后端才放行。
 * `imageDataUrl` 可以是一帧画面的 data URL，也可以是能直接访问的图片地址。
 */
export function llmParseImage(fileName: string, imageDataUrl: string): Promise<LlmFields> {
  return invoke<LlmFields>("llm_parse_image", { fileName, imageDataUrl });
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
