/**
 * 列表排序内核（**纯函数，零 import**）。
 *
 * 存在的理由：把「排序」这件最容易长成分散在组件里的 `if` 的事情，收敛成
 * 一组可单测的纯函数。它不认识 Pinia、不认识 Tauri、也不碰任何 DOM ——
 * 输入一个数组，返回一个**新**数组。
 *
 * 三条贯穿全文件的硬规矩（改动前请先读）：
 *
 * 1. **分区在前、方向在后**：最终顺序恒为 `sort(已识别) ++ sort(未识别)`。
 *    倒序**只在各分区内部生效**，绝不可 `reverse()` 整个数组或某个分区
 *    —— `reverse` 会把主键相等的一批也翻转，破坏兜底链。
 * 2. **方向只取反主键**：兜底链 `名称键 → year（null = +∞）→ id` **恒升序**，
 *    这样「同年同名的几条」在正序 / 倒序下相对顺序一致，用户可预测。
 * 3. **分区判据与排序键解耦**：`identified()` 只看 `type` / 标题 / `year`，
 *    与当前按什么排序无关。切换排序方式时条目不会在分区之间来回跳。
 *
 * 拼音怎么来的：用 ECMAScript 标准库 `Intl.Collator("zh")`（CLDR 的 zh
 * 默认 collation 就是全拼序，随 Node / Chromium 的 ICU 一起分发，**不是
 * 第三方依赖、不是数据表、不需要联网**）。注意别用 `zh-Hant` / `zh-TW`，
 * 那两个是笔画序。ICU 不可用时汉字回落 Unicode 码点（= 升级前的行为）。
 */

/** 排序键：`name` 名称（拼音）/ `year` 上映·首播年份 / `added` 添加时间。 */
export type SortBy = "name" | "year" | "added";

/** 排序方向：`asc` 正序 / `desc` 倒序。 */
export type SortDir = "asc" | "desc";

/** 合法排序键（白名单；不在名单里的一律回落默认）。 */
export const SORT_BY_VALUES: readonly SortBy[] = ["name", "year", "added"];

/** 合法排序方向（白名单）。 */
export const SORT_DIR_VALUES: readonly SortDir[] = ["asc", "desc"];

/** 默认排序键：与后端 `ORDER BY m.title COLLATE NOCASE ASC` 同向，升级不跳变。 */
export const DEFAULT_SORT_BY: SortBy = "name";

/** 默认排序方向。 */
export const DEFAULT_SORT_DIR: SortDir = "asc";

/**
 * 排序所需的最小字段集（结构子集，`MediaItem` 天然满足）。
 *
 * `type` 只**读值**比较（`movie` / `tv`），绝不在此重写类型判定逻辑——
 * 类型判定的唯一权威是后端 `core/mediatype.rs`。
 */
export interface SortableItem {
  id: number;
  type: string;
  title: string;
  display_title: string;
  year?: number | null;
  added_at: number;
  original_title?: string | null;
  file_name?: string | null;
}

/** 权重流里的字符类别序：数字(1) < 拉丁字母(2) < 汉字(3) < 其它文字(4)。 */
export const CAT_DIGIT = 1;
export const CAT_LATIN = 2;
export const CAT_HAN = 3;
export const CAT_OTHER = 4;

/** 汉字 → 拼音秩（越小越靠前）。**空表 = 拼音不可用，汉字回落码点。** */
export type HanOrder = ReadonlyMap<string, number>;

/**
 * 名称键：扁平序列 `[类别, 秩, 类别, 秩, …]`。
 *
 * 摊平成 `number[]` 是为了让比较变成纯整数比较——排序期间不再调用 ICU，
 * 一次排序是微秒级而不是毫秒级。
 */
export interface NameKey {
  readonly stream: readonly number[];
}

/** 汉字（CJK 统一汉字及其扩展区）。 */
const HAN_RE = /\p{Script=Han}/u;
/** 组合记号（NFD 拆出来的重音等，去掉即可把 `é` 折成 `e`）。 */
const MARK_RE = /\p{M}/gu;
/** 半角数字（全角数字已由 NFKC 折成半角）。 */
const DIGIT_RE = /[0-9]/;
/** 拉丁小写字母（已去重音、已转小写）。 */
const LATIN_RE = /[a-z]/;
/** 不参与比较的字符：空白 / 标点 / 符号（含 Emoji、括号、`·`）。 */
const SKIP_RE = /^[\s\p{P}\p{S}\p{C}]$/u;

/**
 * 模块级 `zh` 排序器单例。
 *
 * 只建一次：`Intl.Collator` 构造要加载 CLDR 数据，放进比较器里每比一次
 * 建一个是灾难。注意 locale 用 **"zh"**（拼音序），不是 `zh-Hant`（笔画序）。
 */
const ZH = new Intl.Collator("zh");

/** `hanCollatorUsable()` 的缓存（模块加载后只判一次）。 */
let usableCache: boolean | null = null;
/** 降级提示只打一次，不刷屏。 */
let warnedDegraded = false;

/**
 * 把任意输入收进白名单：非法 / 缺失 / 拼错一律回落默认。
 *
 * 脏值绝不能流进比较器——它只会让顺序变得不可解释。
 */
export function normalizeSortBy(v: unknown): SortBy {
  return SORT_BY_VALUES.includes(v as SortBy) ? (v as SortBy) : DEFAULT_SORT_BY;
}

/** 同 `normalizeSortBy`，方向版。 */
export function normalizeSortDir(v: unknown): SortDir {
  return SORT_DIR_VALUES.includes(v as SortDir) ? (v as SortDir) : DEFAULT_SORT_DIR;
}

/** 去掉文件名扩展名（`Movie.mkv` → `Movie`；`..` 这种点开头的不动）。 */
function stripExt(name: string): string {
  const dot = name.lastIndexOf(".");
  return dot > 0 ? name.slice(0, dot) : name;
}

/**
 * 名称取串优先级：`display_title` → `title` → `original_title` → `file_name`。
 *
 * 用 `display_title` 打头是因为它就是卡片上显示的字——用它排才不会出现
 * 「看到的字和排出来的位置对不上」。trim 后为空视为空，继续往下取。
 */
export function rawTitleOf(it: SortableItem): string {
  const candidates: Array<string | null | undefined> = [
    it.display_title,
    it.title,
    it.original_title,
    it.file_name ? stripExt(it.file_name) : null,
  ];
  for (const c of candidates) {
    const s = (c ?? "").trim();
    if (s) return s;
  }
  return "";
}

/**
 * `zh` 拼音排序在本运行时是否可用（ICU 自检）。
 *
 * 判据取三组跨越整个字母表的汉字：阿(a) < 白(bai) < 宙(zhou)、卧(wo) < 无(wu)。
 * 三者同时成立才说明这是**拼音序**而不是码点序或笔画序。
 */
export function hanCollatorUsable(): boolean {
  if (usableCache === null) {
    usableCache =
      ZH.compare("阿", "白") < 0 &&
      ZH.compare("白", "宙") < 0 &&
      ZH.compare("卧", "无") < 0;
    if (!usableCache && !warnedDegraded) {
      warnedDegraded = true;
      console.warn(
        '[sort] 当前运行时的 Intl.Collator("zh") 不是拼音序，' +
          "汉字已回落 Unicode 码点序（等同升级前的行为）",
      );
    }
  }
  return usableCache;
}

/**
 * 用**当前数据集里实际出现的汉字**现算拼音秩。
 *
 * 秩只在同一份数据内有效：数据变了必须重算（所以调用方把它挂在只依赖
 * `items` 的 `computed` 上）。因为是现算的，覆盖率恒为 100%，
 * 不存在「表外汉字」这个概念。
 *
 * ICU 不可用时返回**空表**，汉字一律回落码点。
 */
export function buildHanOrder(titles: Iterable<string>): HanOrder {
  if (!hanCollatorUsable()) return new Map<string, number>();
  const chars = new Set<string>();
  for (const t of titles) {
    for (const ch of t) {
      if (HAN_RE.test(ch)) chars.add(ch);
    }
  }
  const sorted = [...chars].sort((a, b) => ZH.compare(a, b));
  const order = new Map<string, number>();
  sorted.forEach((ch, i) => order.set(ch, i));
  return order;
}

/** 归一化：`NFKC`（全角→半角、`ß`→`ss`）→ `NFD` 去组合记号（`é`→`e`）→ 小写。 */
function normalizeText(s: string): string {
  return s
    .normalize("NFKC")
    .normalize("NFD")
    .replace(MARK_RE, "")
    .toLowerCase();
}

/**
 * 生成名称键：归一化后逐**码点**扫描，映射成「类别 + 秩」的扁平流。
 *
 * 空白 / 标点 / 符号 / Emoji / `·` 直接跳过，不参与比较——否则
 * `The Matrix` 和 `The.Matrix` 会因为一个标点排到两个地方去。
 * 数字按**字符逐位**比较（`10` 在 `9` 之前），不做数值解析（自然序 = P1-2）。
 */
export function nameKeyOf(it: SortableItem, han: HanOrder): NameKey {
  const text = normalizeText(rawTitleOf(it));
  const stream: number[] = [];
  for (const ch of text) {
    if (DIGIT_RE.test(ch)) {
      stream.push(CAT_DIGIT, ch.codePointAt(0) ?? 0);
      continue;
    }
    if (LATIN_RE.test(ch)) {
      stream.push(CAT_LATIN, ch.codePointAt(0) ?? 0);
      continue;
    }
    if (HAN_RE.test(ch)) {
      // 表里没有（ICU 降级时整张表都是空的）→ 回落 Unicode 码点
      stream.push(CAT_HAN, han.get(ch) ?? ch.codePointAt(0) ?? 0);
      continue;
    }
    if (SKIP_RE.test(ch)) continue;
    stream.push(CAT_OTHER, ch.codePointAt(0) ?? 0);
  }
  return { stream };
}

/** 批量预计算名称键，避免排序过程中反复扫描同一条标题。 */
export function buildNameKeys(
  items: readonly SortableItem[],
  han: HanOrder,
): Map<number, NameKey> {
  const keys = new Map<number, NameKey>();
  for (const it of items) keys.set(it.id, nameKeyOf(it, han));
  return keys;
}

/**
 * 名称键升序：逐 token 比类别 → 比秩；前缀相同则**短者在前**。
 */
export function compareNameKey(a: NameKey, b: NameKey): number {
  const n = Math.min(a.stream.length, b.stream.length);
  for (let i = 0; i < n; i += 1) {
    const d = a.stream[i] - b.stream[i];
    if (d !== 0) return d < 0 ? -1 : 1;
  }
  const rest = a.stream.length - b.stream.length;
  return rest === 0 ? 0 : rest < 0 ? -1 : 1;
}

/**
 * 分区因子：J1 类型 ∈ {movie, tv} ∧ J2 标题非空 ∧ J3 `year` 非 null。
 *
 * 三条全中才算「已识别」。与排序键**无关**（K6）——所以按名称排序时
 * 缺年份的条目也照沉底，换来的是「切换排序方式时条目不在分区间跳变」。
 *
 * 只看 `type` 的**值**，不重新判定类型（约束 B4）。
 */
export function identified(it: SortableItem): boolean {
  if (it.type !== "movie" && it.type !== "tv") return false;
  if (!rawTitleOf(it)) return false;
  return it.year !== null && it.year !== undefined;
}

/** 取名称键（缓存表里没有就现算一次，保证函数自洽）。 */
function keyOf(
  it: SortableItem,
  han: HanOrder,
  keys: ReadonlyMap<number, NameKey>,
): NameKey {
  return keys.get(it.id) ?? nameKeyOf(it, han);
}

function compareByKey(
  a: SortableItem,
  b: SortableItem,
  han: HanOrder,
  keys: ReadonlyMap<number, NameKey>,
): number {
  return compareNameKey(keyOf(a, han, keys), keyOf(b, han, keys));
}

/**
 * 主键比较：只算「用户当前选的那一个键」，方向在这里取反。
 *
 * `year` 缺值（null）是**位置规则**而不是方向的一部分：它恒等于 +∞，
 * 所以缺年份的条目在任何方向下都排分区末尾（D6）。
 */
function primaryCompare(
  a: SortableItem,
  b: SortableItem,
  by: SortBy,
  dir: SortDir,
  han: HanOrder,
  keys: ReadonlyMap<number, NameKey>,
): number {
  if (by === "added") {
    const d = a.added_at - b.added_at;
    return d === 0 ? 0 : dir === "desc" ? -(d < 0 ? -1 : 1) : d < 0 ? -1 : 1;
  }
  if (by === "year") {
    const ya = a.year ?? null;
    const yb = b.year ?? null;
    // 两者都缺 → 主键分不出高下，交给兜底链（名称键 → year → id）
    if (ya === null && yb === null) return 0;
    // NULL = +∞：与方向无关，恒在分区末尾
    if (ya === null) return 1;
    if (yb === null) return -1;
    const d = ya - yb;
    return d === 0 ? 0 : dir === "desc" ? -(d < 0 ? -1 : 1) : d < 0 ? -1 : 1;
  }
  const d = compareByKey(a, b, han, keys);
  return d === 0 ? 0 : dir === "desc" ? -d : d;
}

/**
 * 兜底链：**恒升序**，不随方向翻转。
 *
 * `名称键 → year（null = +∞）→ id`。任何两条条目都不会「相等后随机摆放」，
 * 最终一定能由 `id` 定序（全序保证）。
 */
function tiebreak(
  a: SortableItem,
  b: SortableItem,
  han: HanOrder,
  keys: ReadonlyMap<number, NameKey>,
): number {
  const n = compareByKey(a, b, han, keys);
  if (n !== 0) return n;
  const ya = a.year ?? Number.POSITIVE_INFINITY;
  const yb = b.year ?? Number.POSITIVE_INFINITY;
  if (ya !== yb) return ya < yb ? -1 : 1;
  const d = a.id - b.id;
  return d === 0 ? 0 : d < 0 ? -1 : 1;
}

/**
 * 全序比较器。
 *
 * ① 分区：已识别恒在前，**不受 dir 影响**；
 * ② 主键：按 `by` 取（缺 year 者恒沉分区末尾）；
 * ③ 方向：**只**取反主键；
 * ④ 兜底链恒升序。
 */
export function compareItems(
  a: SortableItem,
  b: SortableItem,
  by: SortBy,
  dir: SortDir,
  han: HanOrder,
  keys: ReadonlyMap<number, NameKey>,
): number {
  const ia = identified(a);
  const ib = identified(b);
  if (ia !== ib) return ia ? -1 : 1;
  const s = primaryCompare(a, b, by, dir, han, keys);
  if (s !== 0) return s;
  return tiebreak(a, b, han, keys);
}

/**
 * 唯一入口：分区 → 各区排序 → 拼接。**不修改入参**。
 *
 * `han` 可省略（内部现算）：调用方通常从只依赖 `items` 的 `computed` 里
 * 传进来，避免每次改方向都重建拼音秩。
 */
export function sortMediaItems<T extends SortableItem>(
  items: readonly T[],
  by: SortBy,
  dir: SortDir,
  han?: HanOrder,
): T[] {
  const normBy = normalizeSortBy(by);
  const normDir = normalizeSortDir(dir);
  const order = han ?? buildHanOrder(items.map((it) => rawTitleOf(it)));
  const keys = buildNameKeys(items, order);

  const known: T[] = [];
  const unknown: T[] = [];
  for (const it of items) {
    if (identified(it)) known.push(it);
    else unknown.push(it);
  }

  const cmp = (x: T, y: T): number =>
    compareItems(x, y, normBy, normDir, order, keys);
  known.sort(cmp);
  unknown.sort(cmp);
  return [...known, ...unknown];
}
