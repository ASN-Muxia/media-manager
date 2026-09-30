/**
 * 连续多选（拖动划过）的纯状态机。
 *
 * 抽成独立模块的原因：这段逻辑有一堆"边界情况下应该怎样"的约定
 * （起点决定方向、划过不重复写入、拖出容器再拖回来、空集合…），
 * 埋在 `.vue` 里没法单测。这里只做状态计算，不碰 DOM —— DOM 事件由组件负责，
 * 于是两边都能各自被验证。
 *
 * 交互约定（与资源管理器 / 相册的习惯一致）：
 * - **起点决定方向**：按下时若该行**未选中**，整段拖动都是"选中"；
 *   若**已选中**，整段拖动都是"取消选中"。这样一次拖动就能完成
 *   "框选一片"或"抹掉一片"，不用额外的修饰键。
 * - **只写一次**：同一行被反复划过不会来回翻转（幂等），
 *   否则拖动时手抖就会把已选中的又取消掉。
 * - 拖动结束后返回的数组**保持传入时的顺序**，新加的排在后面 ——
 *   顺序稳定，用户看到的「已选 N」不会跳来跳去。
 */

/** 拖动选择的进行中状态。 */
export interface DragSelection {
  /** 是否仍处于拖动中。 */
  readonly active: boolean;
  /**
   * 本次拖动要写成的目标状态：`true` = 划过即选中，`false` = 划过即取消。
   * 由起点那一行按下时的选中状态取反得到。
   */
  readonly target: boolean;
  /** 拖动期间累积的选中集合（`active` 为 false 后不应再读写）。 */
  readonly ids: Set<number>;
}

/**
 * 开始一次拖动选择。
 *
 * @param selected 当前已选中的 id 列表（顺序会被保留）
 * @param startId  按下那一行的 id
 */
export function beginDragSelection(
  selected: readonly number[],
  startId: number,
): DragSelection {
  const target = !selected.includes(startId);
  const ids = new Set(selected);
  if (target) ids.add(startId);
  else ids.delete(startId);
  return { active: true, target, ids };
}

/**
 * 把拖动应用到某一行（划过时调用）。
 *
 * 返回是否**真的发生了变化** —— 调用方据此决定要不要刷新界面，
 * 避免每划过一行都触发一次重渲染。
 */
export function applyDragTo(state: DragSelection, id: number): boolean {
  if (!state.active) return false;
  const had = state.ids.has(id);
  const want = state.target;
  if (had === want) return false; // 已经是目标状态，幂等
  if (want) state.ids.add(id);
  else state.ids.delete(id);
  return true;
}

/**
 * 结束拖动，导出最终的选中列表。
 *
 * 两条约定：
 * - **保留选择发生的先后顺序**（先选的在前面），与单点切换那条路径一致，
 *   这样「已选 N」的计数在拖动过程中不会因为顺序变化而看起来在跳。
 * - **过滤掉已不在当前列表里的行**：选中集合可能残留着已经不存在的单集
 *   （刚被删掉、或详情页换了一部剧的瞬间）。不过滤的话，用户会删到
 *   **界面上看不见的条目** —— 这与项目里「切换视图必须清空多选」是同一条安全约定。
 *   代价是 `order` 为空时结果必然为空，这也是正确的：没有可见的行就没什么可选。
 */
export function finishDragSelection(state: DragSelection, order: readonly number[]): number[] {
  const visible = new Set(order);
  const out: number[] = [];
  for (const id of state.ids) {
    if (visible.has(id)) out.push(id);
  }
  return out;
}

/**
 * 把一个 id 的选中状态改掉（复选框点击走的路径，不走拖动）。
 *
 * 保留在模块里是为了让"拖动"与"单击/复选框"两条路径共用同一套顺序约定。
 */
export function toggleOne(selected: readonly number[], id: number): number[] {
  const i = selected.indexOf(id);
  if (i >= 0) {
    const next = [...selected];
    next.splice(i, 1);
    return next;
  }
  return [...selected, id];
}

/* ==================== 触发判定与边缘滚动（纯计算，可单测） ==================== */
/* 下面两个函数是「滑动多选」里最容易在改动中被悄悄改坏的部分：
   一个是"这次按下到底算不算拖动"，一个是"贴边滚动该多快"。
   它们只吃数字、只吐数字，所以能脱开 DOM 与计时器来验证。 */

/** 超过这个位移（像素）才认为用户是在拖动，而不是手抖 / 点击。 */
export const DRAG_THRESHOLD_PX = 6;
/** 触摸下要按住这么久（毫秒）才进入滑动多选。 */
export const LONG_PRESS_MS = 350;
/** 距滚动容器上下边缘多少像素内开始自动滚动。 */
export const EDGE_ZONE_PX = 56;
/** 自动滚动的最大速度（像素 / 帧）。 */
export const EDGE_MAX_SPEED = 16;

/** 指针种类（与 `PointerEvent.pointerType` 对齐，这里只区分是否触摸）。 */
export type PointerKind = "mouse" | "touch" | "pen";

/**
 * "这次按下到底算不算拖动"。
 *
 * - **鼠标 / 触控笔**：拖动**即时生效** —— 鼠标在卡片上拖不会和页面滚动打架，
 *   等长按反而别扭。没超过阈值就还是普通点击。
 * - **触摸**：必须**长按**到 `LONG_PRESS_MS` 且期间没怎么移动才进入；
 *   一旦在长按成立前就移动了 —— 那是用户要滚页面，直接放弃（`abort`），
 *   绝不能把滚动手势抢成框选。
 *
 * @param kind   指针种类
 * @param moved  是否已超过 `DRAG_THRESHOLD_PX`
 * @param heldMs 已按住多久
 */
export function dragTrigger(
  kind: PointerKind,
  moved: boolean,
  heldMs: number,
  longPressMs: number = LONG_PRESS_MS,
): "drag" | "wait" | "abort" {
  if (kind === "touch") {
    if (moved) return "abort"; // 手指先动了 = 想滚页面
    return heldMs >= longPressMs ? "drag" : "wait";
  }
  return moved ? "drag" : "wait";
}

/**
 * 贴边自动滚动的速度：靠近上边缘返回负值（向上滚），靠近下边缘返回正值。
 *
 * `0` 表示不在边缘区、不用滚。速度按"贴得多近"线性加大，指针跑到容器外面
 * （距离 ≤ 0）时取最大速度 —— 手指拖出界了也应该继续滚，否则选不到下面的项。
 */
export function edgeScrollSpeed(
  pointerY: number,
  top: number,
  bottom: number,
  zone: number = EDGE_ZONE_PX,
  max: number = EDGE_MAX_SPEED,
): number {
  const up = speedAt(pointerY - top, zone, max);
  if (up !== 0) return -up;
  return speedAt(bottom - pointerY, zone, max);
}

function speedAt(distance: number, zone: number, max: number): number {
  if (zone <= 0 || distance >= zone) return 0;
  const ratio = (zone - Math.max(0, distance)) / zone; // 越贴边越接近 1
  return Math.max(1, Math.round(max * ratio));
}
