/**
 * 侧边栏 / 主视图的视图键。
 *
 * 之所以是 `string` 而不是联合类型：**分类来自数据库**，运行时才知道有哪些，
 * 写成联合类型就等于把栏目写死在代码里（需求明确禁止硬编码枚举）。
 *
 * 键的构成：
 * - `all` / `movie` / `tv` / `unknown` / `settings`：内置入口（媒体类型快捷筛选，不是分类）
 * - `cat:<id>`：某个分类（id 来自 `categories` 表）
 * - `tag:<id>`：某个标签（id 来自 `tags` 表）
 * - `unclassified`：还没归类的条目
 *
 * 注：剧集层级**不是**一个栏目 —— 层级（`parent_id`）保留在数据里，
 * 由封面墙以「聚合分组」呈现，不再单独占一个视图键。
 */
export type ViewKey = string;

/** 内置视图键（不随数据变化的那几个）。 */
export type BuiltinView =
  | "all"
  | "movie"
  | "tv"
  | "unknown"
  | "settings"
  | "catalog";

export const VIEW_ALL: ViewKey = "all";
export const VIEW_MOVIE: ViewKey = "movie";
export const VIEW_TV: ViewKey = "tv";
/** 判定不出类型的条目（不再被静默归成电影）。 */
export const VIEW_UNKNOWN: ViewKey = "unknown";
export const VIEW_SETTINGS: ViewKey = "settings";
export const VIEW_UNCLASSIFIED: ViewKey = "unclassified";
/**
 * 资源目录库：一份**持久化**的磁盘资源清单（不是媒体条目清单）。
 *
 * 与其它内置入口的区别：它不走 mediaStore 的列表查询，而是直接读后端的
 * resource_assets 表。存在的理由是「每次检测与增量添加时方便对比与定位旧资源」——
 * 媒体条目只在「识别得出来」时才存在，被挪走 / 改名 / 判不出类型的文件在条目层面
 * 就消失了，资源清单则一直留着它们最后一次出现的位置。
 */
export const VIEW_CATALOG: ViewKey = "catalog";

/** 分类视图键：`cat:<id>`。 */
export function categoryView(id: number): ViewKey {
  return `cat:${id}`;
}

/** 标签视图键：`tag:<id>`。 */
export function tagView(id: number): ViewKey {
  return `tag:${id}`;
}

export type ParsedView =
  | { kind: "builtin"; key: BuiltinView }
  | { kind: "category"; id: number }
  | { kind: "tag"; id: number }
  | { kind: "unclassified" };

const BUILTINS: BuiltinView[] = [
  "all",
  "movie",
  "tv",
  "unknown",
  "settings",
  "catalog",
];

/** 把视图键拆成可判别的结构，避免各处再手写字符串切割。 */
export function parseView(v: ViewKey): ParsedView {
  if (v === VIEW_UNCLASSIFIED) return { kind: "unclassified" };
  if (v.startsWith("cat:")) {
    const id = Number(v.slice(4));
    return Number.isFinite(id) ? { kind: "category", id } : { kind: "builtin", key: "all" };
  }
  if (v.startsWith("tag:")) {
    const id = Number(v.slice(4));
    return Number.isFinite(id) ? { kind: "tag", id } : { kind: "builtin", key: "all" };
  }
  const key = BUILTINS.includes(v as BuiltinView) ? (v as BuiltinView) : "all";
  return { kind: "builtin", key };
}
