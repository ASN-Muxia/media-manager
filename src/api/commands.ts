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
  /** 刮削前由原始文件名派生的标题（英文原名等写法），检索要用它匹配 */
  original_title?: string | null;
  /** 分组键（通常是所在目录名），检索要能按目录名命中 */
  group_key?: string | null;
  /** `scan`（扫描归纳出来的）/ `manual`（手动新建的 Series） */
  source?: string;
  /** 用户确认过分组后锁住，二次校正不再调整 */
  locked?: boolean;
  /** 父节点 id（`null` = 顶层）。封面墙的拖拽聚合分组靠它判断归属。 */
  parent_id?: number | null;
  /** 直接子级条数；> 0 时卡片给「展开 / 折叠」控件。 */
  child_count?: number;
  /**
   * 同一父节点内的**次序**（库里缺值按 0）。
   *
   * ⚠️ 它不是列表先后：列表先后是用户选的展示排序（默认按拼音名），
   * `sort_order` 才是同级内用户手工调出来的次序。要按同级次序排的地方
   * （详情页「聚合成员」的 ↑ / ↓）必须用它，不能拿 `orderedItems` 的下标顶替，
   * 否则后端按库内序算出的新序与旧序一样，点了看不出变化。
   */
  sort_order?: number;
  /**
   * 条目在全表层级里的深度（顶层 = 0），**由后端用全表边表算出来**。
   *
   * 封面墙的拖拽放行判定必须用它：前端自己数深度时只看得见当前筛选到的
   * 条目，父节点一旦被筛掉就会被当成"走到了根"→ 深度偏小 → 界面画「可放置」
   * 并把请求发出去，后端却回「层级最多 4 层」。缺失（老后端 / 测试桩）才退回
   * 前端自数，见 `PosterGrid.vue::depthOf`。
   */
  depth?: number;
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
  /**
   * 被用户**关掉**的数据源 id 列表（如 `["douban"]`）。
   *
   * 与「Key 留空」不同：留空是「没配置」，这里是「配了但不用」。
   * 只认注册表认识的 id（见 `listProviders`）；空数组 = 全部启用。
   * 关掉一个源**不会**让后面的源提前（权威顺序固定）。
   */
  disabledProviders: string[];

  // ── 手动匹配的筛选偏好 ──
  /** 模糊匹配开关（关掉就只剩「完全一致」与「包含」） */
  matchFuzzy: boolean;
  /** 年份容差：0 = 必须同一年，1 = 前后一年也算命中 */
  matchYearTolerance: number;
  /** 排除词（逗号 / 换行分隔）：候选标题里出现任一个就整条剔除 */
  matchBlacklist: string;
  /** 一次最多拉多少条候选（TMDB 每页 20 条） */
  matchLimit: number;

  // ── 快捷键（组合键字符串，留空 = 不绑定） ──
  /** 全选当前可见条目 */
  shortcutSelectAll: string;
  /** 反选 */
  shortcutInvert: string;
  /** 确认所选（列表：刮削所选；候选面板：应用所选） */
  shortcutConfirm: string;
  /** 清空选择 */
  shortcutClear: string;

  // ── 列表排序（展示层偏好，排序计算在前端 `utils/sort.ts`）──
  /** 排序键：`name` 名称（拼音）/ `year` 上映·首播年份 / `added` 添加时间 */
  sortBy: string;
  /** 排序方向：`asc` 正序 / `desc` 倒序 */
  sortDir: string;
  /**
   * 已被用户**手工排过序**的聚合（分组）id 列表。
   *
   * 只回答「这一组的次序是用户亲手定的吗」，与 `MediaItem.sort_order` 的数值
   * **无关**：`sort_order` 会被任何一次 `move_nodes_sync`（含在封面墙上拖动
   * 聚合）整组重编号成 `0..n-1`，所以「组里存在 > 0 的值」根本不能证明用户排过。
   * 列表里的分组 → 按库内 `sort_order` 显示；不在列表里 → 按顶栏排序偏好显示。
   */
  manualOrderGroups: number[];
  /**
   * 被用户**亲手折叠**过的分组 id 列表（封面墙的展开态记忆）。
   *
   * 为什么不能只放在 `PosterGrid` 的 `ref` 里：封面墙挂在 `App.vue` 的
   * `v-if/v-else` 链上，点开详情页会把它**卸载**，两个展开态 ref 随之销毁；
   * 返回时组件重建，「新分组自动展开」那段 watch 又把一切摊开 —— 表现就是
   * 「我上次明明收起来了，回来又全开了」。落库才能跨卸载存活。
   *
   * 只记**折叠**、不记展开：展开的分组会随刷新不断新增，存它等于存一份只增
   * 不减、越删越脏的名单；折叠天然是少数派，语义也正是「否决了自动展开」。
   */
  collapsedGroups: number[];
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
  /** 原标题（电影 `original_title` / 剧集 `original_name`）：多语言匹配靠它接住。 */
  original_title?: string | null;
  original_name?: string | null;
  /** 匹配打分（`search_tmdb` 才会带；详情 / 缓存路径没有）。 */
  match_score?: number | null;
  /** 命中原因，例如「片名完全一致」「年份一致」。 */
  match_reasons?: string[] | null;
}

/** 手动匹配的筛选条件（与后端的 `MatchOptions` 对应）。 */
export interface MatchFilter {
  /** 本地原标题 / 别名，用于多语言比对。 */
  originalTitle?: string | null;
  /** 季数：剧集用它推断首播年份。 */
  season?: number | null;
  /** 一次最多拉多少条候选。 */
  limit?: number | null;
  /** 模糊匹配开关。 */
  fuzzy?: boolean | null;
  /** 年份容差（0 = 必须同一年）。 */
  yearTolerance?: number | null;
  /** 排除词：候选标题里出现任一个就整条剔除。 */
  blacklist?: string | null;
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
  /** 父节点 id；不传表示建在顶层 */
  parentId?: number | null;
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
  /** 从别的剧集改挂到已存在剧集下的单集数（老数据自愈） */
  repointed: number;
  /** 清理掉的空剧集条目数 */
  droppedShells: number;
  /** 散落条目（电影 / 待确认）被收编成单集的条数 */
  adopted: number;
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

/**
 * 把一个文件夹的视频**指定**归到某条剧集下（不靠文件名推断）。
 *
 * 用于「文件名与目录名都不含剧名」、推断必然失败的情况
 * （如 `86版西游记.01.mkv`、`XYJ.01.mkv`）。
 */
export function importFolderInto(
  mediaId: number,
  path: string,
  categoryId?: number | null,
): Promise<ImportFolderResult> {
  return invoke<ImportFolderResult>("import_folder_into", {
    mediaId,
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
    parentId: input.parentId ?? null,
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

/**
 * 搜索 TMDB 候选。
 *
 * 返回类型与旧版一致（`TmdbSearchItem[]`），只是按匹配度排好了序、
 * 并带上了 `match_score` / `match_reasons`。
 */
export function searchTmdb(
  mediaType: MediaType,
  query: string,
  year?: number | null,
  filter?: MatchFilter | null,
): Promise<TmdbSearchItem[]> {
  return invoke<TmdbSearchItem[]>("search_tmdb", {
    mediaType,
    query,
    year: year ?? null,
    originalTitle: filter?.originalTitle ?? null,
    season: filter?.season ?? null,
    limit: filter?.limit ?? null,
    fuzzy: filter?.fuzzy ?? null,
    yearTolerance: filter?.yearTolerance ?? null,
    blacklist: filter?.blacklist ?? null,
  });
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

/* ---------------------------- 层级（分组） ---------------------------- */
/*
 * 层级**不再单独占一个栏目/视图**：`parent_id` 这层关系留在数据里，
 * 由封面墙按它聚合出分组（拖拽聚合），详情页里管理成员与顺序。
 * 所以这里不再有"取整棵树"的入口——建树是 `utils/tree.ts` 的纯函数，
 * 喂给它的是封面墙那份 `MediaItem` 补出来的节点。
 */

/**
 * 分组树上的一个节点。
 *
 * 现在由封面墙 / 详情页用 `MediaItem` 现补出来（`utils/tree` 只认这个形状），
 * 后端也不再单独给"取整棵树"的接口——一份数据只在前端建一次树，
 * 避免两端各拼一份导致顺序不一致。
 */
export interface MediaTreeNode {
  id: number;
  parent_id: number | null;
  sort_order: number;
  type: MediaType;
  title: string;
  /** 同名剧集带目录名后缀，与海报墙一致 */
  display_title: string;
  year: number | null;
  poster_path: string | null;
  episode_count: number;
  child_count: number;
  /** 顶层 = 0 */
  depth: number;
  source: string;
  locked: boolean;
}

/** 层级操作的结果（含撤销栈剩余步数，前端据此禁用「撤销」）。 */
export interface TreeOpResult {
  message: string;
  undo_depth: number;
}

/** 允许的层级数（与 Rust `core::tree::MAX_DEPTH` 一致）。 */
export const TREE_MAX_DEPTH = 4;

/**
 * 把一批条目搬到目标节点下（聚合 / 解除分组 / 组内排序，全走这一条）。
 *
 * `targetParentId` 为 `null` 表示顶层；`index` 为目标同级中的位置（省略 = 末尾）。
 * 子孙会跟着一起搬；非法放置（自身 / 子孙 / 超层）由后端拒绝并给出原因。
 *
 * 电影与剧集走**完全相同**的一条路径：后端 `load_edges` 不按类型过滤，
 * 所以两端行为、交互、数据结构天然一致。
 */
export function moveMediaNodes(args: {
  ids: number[];
  targetParentId: number | null;
  index?: number | null;
}): Promise<TreeOpResult> {
  return invoke<TreeOpResult>("move_media_nodes", {
    ids: args.ids,
    targetParentId: args.targetParentId,
    index: args.index ?? null,
  });
}

/** 撤销最后一次层级操作（移动 / 聚合 / 解除分组 / 组内排序）。 */
export function undoTreeOp(): Promise<TreeOpResult> {
  return invoke<TreeOpResult>("undo_tree_op");
}

/* -------------------- 观看状态 / 续播 / 连播（v6） -------------------- */

/**
 * 一个条目的观看摘要。
 *
 * 口径必须说清楚：这是**集级**进度，不是秒级。
 * 本软件调用外部播放器（PotPlayer），播放器进程不会把播放位置回传给我们，
 * 所以「看到第 3 分 12 秒」这种精度本期拿不到——「继续播放」= 播下一个未看集。
 */
export interface MediaPlayback {
  mediaId: number;
  watched: boolean;
  watchedEpisodes: number;
  totalEpisodes: number;
  playCount: number;
  lastPlayedAt: number | null;
  resumeEpisodeId: number | null;
  lastEpisodeId: number | null;
  lastSeason: number | null;
  lastEpisode: number | null;
  /**
   * **上次真正播放过**的那一集（只看播放时间，不看「已看」）。
   *
   * 双击封面播放要的目标：`record_play` 播放后不写 `watched`，所以拿
   * `lastEpisodeId` 会漏掉「播了但没标记已看」——那才是最常见的状态。
   */
  lastPlayedEpisodeId: number | null;
}

/** 连播 / 播放单集的结果。 */
export interface PlayResult {
  player: string;
  files: number;
  mediaId: number;
  episodeId: number | null;
  /** 实际播放那一集的季 / 集号（界面提示用）；电影为 null。 */
  season: number | null;
  episode: number | null;
}

export function getPlayback(mediaId: number): Promise<MediaPlayback> {
  return invoke<MediaPlayback>("get_playback", { mediaId });
}

/** 批量取（列表页一次拿全，避免逐条请求）。 */
export function getPlaybackMany(
  mediaIds: number[],
): Promise<Record<string, MediaPlayback>> {
  return invoke<Record<string, MediaPlayback>>("get_playback_many", { mediaIds });
}

/** 最近播放（「继续观看」用）。 */
export function recentPlayback(limit?: number): Promise<MediaPlayback[]> {
  return invoke<MediaPlayback[]>("recent_playback", { limit: limit ?? null });
}

export function markWatched(args: {
  mediaId: number;
  episodeId?: number | null;
  watched: boolean;
}): Promise<MediaPlayback> {
  return invoke<MediaPlayback>("mark_watched", {
    mediaId: args.mediaId,
    episodeId: args.episodeId ?? null,
    watched: args.watched,
  });
}

export function markWatchedBatch(mediaIds: number[], watched: boolean): Promise<number> {
  return invoke<number>("mark_watched_batch", { mediaIds, watched });
}

/** 手动修正播放位置（秒）。本期不自动采集，但允许用户自己填。 */
export function setPlaybackPosition(args: {
  mediaId: number;
  episodeId?: number | null;
  positionSecs?: number | null;
  durationSecs?: number | null;
}): Promise<MediaPlayback> {
  return invoke<MediaPlayback>("set_playback_position", {
    mediaId: args.mediaId,
    episodeId: args.episodeId ?? null,
    positionSecs: args.positionSecs ?? null,
    durationSecs: args.durationSecs ?? null,
  });
}

export function clearPlayback(mediaId: number): Promise<MediaPlayback> {
  return invoke<MediaPlayback>("clear_playback", { mediaId });
}

/**
 * 播放某一集；`continuous` 为真时从这一集开始连播。
 *
 * 连播**不生成临时播放列表文件**——直接把多个路径作为参数交给播放器，
 * 避免在用户的媒体目录里留下任何东西。
 */
export function playEpisode(args: {
  mediaId: number;
  episodeId?: number | null;
  continuous?: boolean;
}): Promise<PlayResult> {
  return invoke<PlayResult>("play_episode", {
    mediaId: args.mediaId,
    episodeId: args.episodeId ?? null,
    continuous: args.continuous ?? false,
  });
}

/* -------------------- NFO 互通 / 备份（v6） -------------------- */

export interface ExportResult {
  written: number;
  skipped: number;
  failed: number;
  message: string;
}

export interface ImportResult {
  scanned: number;
  filled: number;
  reclassified: number;
  unmatched: number;
  message: string;
}

/**
 * 导出 NFO。
 *
 * `targetDir` 省略时写在媒体文件旁边（Kodi / tMM 的 sidecar 约定）。
 * `overwrite` 默认 false：已存在的 NFO 一律跳过，绝不覆盖用户或别的软件写的。
 */
export function exportNfo(args: {
  /** 留空 = 导出全库；传了 id 就只导这几条。 */
  mediaIds?: number[];
  targetDir?: string | null;
  overwrite?: boolean;
}): Promise<ExportResult> {
  return invoke<ExportResult>("export_nfo", {
    mediaIds: args.mediaIds ?? [],
    targetDir: args.targetDir ?? null,
    overwrite: args.overwrite ?? false,
  });
}

/** 从一个目录导入 NFO（只回填空字段，不覆盖已有值）。 */
export function importNfo(dir: string): Promise<ImportResult> {
  return invoke<ImportResult>("import_nfo", { dir });
}

/** 备份数据库到指定路径（目标已存在则拒绝，绝不覆盖）。 */
export function backupDatabase(dest: string): Promise<string> {
  return invoke<string>("backup_database", { dest });
}

/* -------------------- 维护 / 诊断（v6） -------------------- */

export interface LibraryHealth {
  dbPath: string;
  schemaVersion: number;
  tables: string[];
  /** FTS5 检索索引是否可用（SQLite 可能没编进去，此时检索回落到 LIKE）。 */
  ftsEnabled: boolean;
  cacheRows: number;
  playbackRows: number;
  ffprobeAvailable: boolean;
  os: string;
  arch: string;
}

export function libraryHealth(): Promise<LibraryHealth> {
  return invoke<LibraryHealth>("library_health");
}

/** 清理元数据缓存；不传 provider 则清空全部。 */
export function clearMetadataCache(provider?: string | null): Promise<number> {
  return invoke<number>("clear_metadata_cache", { provider: provider ?? null });
}

export function purgeExpiredCache(): Promise<number> {
  return invoke<number>("purge_expired_cache");
}

/** provider 注册表（顺序即权威顺序）。 */
export interface ProviderEntry {
  id: string;
  label: string;
  provides: string;
  defaultOn: boolean;
}

export function listProviders(): Promise<{ order: string[]; items: ProviderEntry[] }> {
  return invoke<{ order: string[]; items: ProviderEntry[] }>("list_providers");
}

/* -------------------- 资源目录库（增量对比） -------------------- */

/**
 * 资源目录库里的一条记录（与 Rust `CatalogEntry` 对应，后端 camelCase 序列化）。
 *
 * 它是**文件系统视角**的清单，和 `media` 表不是一回事：文件还在不在、大小与
 * 修改时间有没有变，都看这里。`mediaId` / `displayTitle` 只是尽力关联到媒体库，
 * 关联不上就是 `null`（界面显示「未关联」），不代表这条记录有问题。
 */
export interface CatalogEntry {
  id: number;
  /** 绝对路径 */
  path: string;
  fileName: string;
  /** 相对所在扫描根的目录；根目录下的文件是空串 */
  relDir: string;
  rootPath: string;
  /** 字节数 */
  size: number;
  /** 修改时间（Unix 秒）；取不到时为 null */
  mtime: number | null;
  /** `present` 文件还在 / `missing` 上次见过、这次没找到 */
  status: "present" | "missing";
  /** 关联到的媒体条目 id；关联不上为 null */
  mediaId: number | null;
  displayTitle: string | null;
  mediaType: string | null;
}

/** 一次「检测资源库」的结果（界面直接展示这几个数字）。 */
export interface CatalogSyncResult {
  /** 开始 / 结束时间（Unix 秒） */
  startedAt: number;
  finishedAt: number;
  /** 参与的扫描根数量 */
  roots: number;
  /** 本次遍历到的文件数 */
  scanned: number;
  /** 新入库的条数 */
  added: number;
  /** 指纹（大小 / 修改时间）变了的条数 */
  updated: number;
  /** 指纹没变、原样保留的条数 */
  unchanged: number;
  /** 本次判定为「文件已不在」的条数 */
  missing: number;
  /** 上次丢失、这次又找回来的条数 */
  restored: number;
  message: string;
}

/** 与上一次检测的差异（后端 `diff_resource_catalog`）。 */
export interface CatalogDiff {
  added: CatalogEntry[];
  changed: CatalogEntry[];
  missing: CatalogEntry[];
  /** 当前在库总数 */
  present: number;
  /** 上一次检测时间（Unix 秒）；从未检测过为 null */
  lastSyncAt: number | null;
}

/** 资源目录库的查询条件（与 `getMediaList` 的 `MediaQuery` 同一写法）。 */
export interface CatalogQuery {
  /** 最多返回多少条；不传 = 全部 */
  limit?: number | null;
  /** 只看在库 / 已丢失；不传 = 都要 */
  status?: "present" | "missing" | null;
  /** 文件名的模糊匹配 */
  keyword?: string | null;
}

/**
 * 检测资源库：遍历各扫描根，把结果**增量**写进后端的 `resource_snapshots` 表。
 *
 * 「增量」= 与上一次快照逐条比对：指纹没变的只更新「最近见到」的时间，变了的写
 * 新指纹，这次没见到的标记为丢失。**只读文件系统**，不动媒体库里的条目。
 * 跑完返回这一轮的统计，界面直接展示。
 */
export function syncResourceCatalog(): Promise<CatalogSyncResult> {
  return invoke<CatalogSyncResult>("sync_resource_catalog");
}

/** 读资源目录库清单（筛选在 SQL 层做，前端不二次筛选）。 */
export function getResourceCatalog(query: CatalogQuery = {}): Promise<CatalogEntry[]> {
  return invoke<CatalogEntry[]>("get_resource_catalog", {
    limit: query.limit ?? null,
    status: query.status ?? null,
    keyword: query.keyword?.trim() || null,
  });
}

/** 与上一次检测对比：新增 / 变化 / 丢失 三组条目（「定位旧资源」靠它）。 */
export function diffResourceCatalog(): Promise<CatalogDiff> {
  return invoke<CatalogDiff>("diff_resource_catalog");
}

