/**
 * 侧边栏 / 主视图的视图键。
 *
 * 之所以是 `string` 而不是联合类型：**分类来自数据库**，运行时才知道有哪些，
 * 写成联合类型就等于把栏目写死在代码里（需求明确禁止硬编码枚举）。
 *
 * 键的构成：
 * - `all` / `movie` / `tv` / `unknown` / `settings`：内置入口（媒体类型快捷筛选，不是分类）
 * - `collections`：系列电影合集（由电影标题算出来的派生视图）
 * - `cat:<id>`：某个分类（id 来自 `categories` 表）
 * - `tag:<id>`：某个标签（id 来自 `tags` 表）
 * - `unclassified`：还没归类的条目
 */
export type ViewKey = string;

/** 内置视图键（不随数据变化的那几个）。 */
export type BuiltinView =
  | "all"
  | "movie"
  | "tv"
  | "unknown"
  | "collections"
  | "settings";

export const VIEW_ALL: ViewKey = "all";
export const VIEW_MOVIE: ViewKey = "movie";
export const VIEW_TV: ViewKey = "tv";
/** 判定不出类型的条目（不再被静默归成电影）。 */
export const VIEW_UNKNOWN: ViewKey = "unknown";
/** 系列电影合集。 */
export const VIEW_COLLECTIONS: ViewKey = "collections";
export const VIEW_SETTINGS: ViewKey = "settings";
export const VIEW_UNCLASSIFIED: ViewKey = "unclassified";

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
  "collections",
  "settings",
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
