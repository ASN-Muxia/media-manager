/**
 * 多选点击语义的**纯函数**。
 *
 * 抽成独立模块的原因：Shift 区间、Ctrl 单张切换、普通切换这三条路径的
 * 差别全在"一次点击之后选中集合与锚点各是什么"，而锚点又有一个很容易被
 * 改错的约定（Shift 之后**不动**）。这段逻辑埋在 `PosterGrid.vue` 里
 * 没法单测 —— 组件只管把 DOM 事件翻译成 `ClickMods`，这里只吐结果集合。
 *
 * 三条交互约定（与资源管理器 / Windows 文件管理器一致）：
 * - **普通点击**：toggle 单张（已选则取消、未选则加入），**不是**单选替换 ——
 *   现有封面墙就是"点一下切换一张"，这里保持不变，否则用户点第二张时
 *   会把第一张丢掉。锚点更新为被点的那张。
 * - **Ctrl / Cmd + 点击**：同样是单张 toggle，语义上与普通点击等价，
 *   之所以单独给修饰键留位置，是为了不让 Ctrl 点击掉进区间分支；
 *   锚点也更新为被点的那张（用户刚点过谁，谁就是接下来的起点）。
 * - **Shift + 点击**：以**传入的 anchor** 为起点取 `order` 里的一整段，
 *   **并入**当前选中（不是替换 —— 替换会把这轮多选之前辛苦选中的都抹掉），
 *   随后**锚点保持不变**：连续 Shift 时每次都从同一个起点重算区间，
 *   这正是文件管理器的手感（同一次多选里"原点"不该漂移）。
 *
 * 一个必须说清的副作用：并入是**只增不减**的。所以连续 Shift 只能扩大
 * 或保持区间，缩小要靠 Ctrl+点击逐张取消，或者由调用方在按 Shift 前
 * 记住一份基础集合。这是"并入"这条契约本身的推论，不是实现偷懒。
 *
 * ⚠️ 本模块是纯函数：不 import Vue / DOM / store，也**绝不原地修改**传入的数组。
 *    调用方（组件）拿到返回值后自行赋值给响应式状态。
 */

/** 一次点击带的修饰键。`ctrl` 与 `meta` 视为同一个意思（Windows / macOS）。 */
export type ClickMods = { shift?: boolean; ctrl?: boolean; meta?: boolean };

/**
 * 普通点击：切换单个（已选则取消，未选则加入）。
 *
 * 顺序约定与 `utils/dragSelect.ts` 一致：**保留选择发生的先后顺序**，
 * 新加入的排在末尾。这样"已选 N"的列表与拖动多选那条路径不会互相打架。
 */
export function toggleOne(selected: number[], id: number): number[] {
  const i = selected.indexOf(id);
  if (i < 0) return [...selected, id];
  const next = [...selected];
  next.splice(i, 1);
  return next;
}

/**
 * Shift+点击：把 order 里 `[anchor..target]` 这一整段**并入** base。
 *
 * 细节与理由：
 * - **去重**：base 里已有的（含 order 之外的残留项）不重复写入，
 *   Set 判重是 O(1)，避免集合因为反复 Shift 抖动。
 * - **顺序稳定**：先 `base` 原有项（保持它们原本的先后），再多出的区间项
 *   按 `order` 的顺序追加 —— 顺序可断言，界面上的"已选"列表也不会跳。
 * - **区间方向**：anchor 在下标上位于 target 之后时，取两者之间的段，
 *   也就是"反向拖选"同样成立（用户先点下面再 Shift 点上面）。
 * - **同名 / 重复 id**：`order` 里同一个 id 出现多次时，起点只认
 *   **首次出现的下标**（`indexOf`），否则重复项会让区间莫名变长或变短。
 * - **残留项**：base 里存在 order 之外的 id 时**保留**它们 ——
 *   裁剪是调用方的责任（列表被过滤 / 换页时才知道哪些不该留），
 *   这里擅自裁剪会把"界面没显示但仍在选"的项悄悄吞掉。
 * - **构不成区间**（anchor 为 null、anchor 或 target 不在 order 里、
 *   order 为空）时原样返回去重后的 base 副本：退化成什么由 `resolveClick`
 *   决定，这里不抛错、不返回 undefined。
 */
export function extendRange(
  order: number[],
  base: number[],
  anchor: number | null,
  target: number,
): number[] {
  const out = dedupe(base);
  const bounds = rangeBounds(order, anchor, target);
  if (!bounds) return out;
  const seen = new Set(out);
  for (let i = bounds[0]; i <= bounds[1]; i += 1) {
    const id = order[i];
    if (seen.has(id)) continue;
    seen.add(id);
    out.push(id);
  }
  return out;
}

/**
 * 一次点击的完整决策（plain / shift / ctrl+plain）。
 *
 * 返回的 `anchor` 是**下一次 Shift 的起点**：
 * - Shift 且区间成立 → 锚点保持传入值不变（见文件头说明）；
 * - 其余情况（普通点击、Ctrl/Cmd 点击，以及"构不成区间"而退化的 Shift）
 *   → 都走 toggleOne，锚点更新为被点的那张。
 *
 * 退化规则（逐条都有用例钉着）：
 * - anchor 为 `null` 或不在 `order` 里 → 没有起点，按普通切换处理；
 * - 被点的 id 不在 `order` 里（列表刚过滤过、或点了刚被删的那张）→
 *   同样按普通切换处理，**不得抛错也不得返回 undefined**；
 * - `order` 为空 → 同样退化，选中集合仍能正常 toggle。
 *
 * `shift` 与 `ctrl` 同时按下属于契约未定义的组合，这里让 **shift 优先**：
 * 区间语义更强，且不会把用户想选的一片误换成"只切换一张"。
 */
export function resolveClick(input: {
  order: number[];
  selected: number[];
  anchor: number | null;
  id: number;
  mods: ClickMods;
}): { selected: number[]; anchor: number | null } {
  const { order, selected, anchor, id, mods } = input;
  if (mods.shift === true && rangeBounds(order, anchor, id)) {
    return { selected: extendRange(order, selected, anchor, id), anchor };
  }
  return { selected: toggleOne(selected, id), anchor: id };
}

/* ==================== 内部工具 ==================== */

/** 去掉重复 id，保留首次出现的顺序（纯函数，返回新数组）。 */
function dedupe(ids: number[]): number[] {
  const seen = new Set<number>();
  const out: number[] = [];
  for (const id of ids) {
    if (seen.has(id)) continue;
    seen.add(id);
    out.push(id);
  }
  return out;
}

/**
 * 求 `[anchor..target]` 在 order 里的闭区间下标（升序返回）。
 *
 * 任何一端对不上就返回 `null`，由调用方决定怎么退化 ——
 * 把"构不成区间"表达成 null 而不是抛错，是因为这些情况在真实交互里
 * 很常见（刚切过筛选、列表还没渲染），不该让一次点击炸掉界面。
 */
function rangeBounds(
  order: number[],
  anchor: number | null,
  target: number,
): [number, number] | null {
  if (anchor === null) return null;
  const a = order.indexOf(anchor);
  if (a < 0) return null;
  const t = order.indexOf(target);
  if (t < 0) return null;
  return a <= t ? [a, t] : [t, a];
}
