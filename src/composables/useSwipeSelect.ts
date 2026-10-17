/**
 * 滑动多选（在选项区域按下并拖动，连续选中划过的项）。
 *
 * 这个文件只做**DOM 侧的编排**，纯计算部分全在 `utils/dragSelect.ts`：
 * - 触发判定（鼠标即时拖 / 触摸长按）
 * - 命中哪一行、幂等地改状态、结束后导出结果
 * - 快速滑动要补齐被跳过的项、划到列表外要夹到首尾项
 * - 贴边自动滚动的速度
 *
 * 之所以拆开：这些约定（起点决定方向、划过不翻转、只在边缘才滚）埋在 `.vue`
 * 里没法单测；而 DOM 与计时器又没法脱离浏览器验证。各管一段，两边都能被测到。
 *
 * 三条对外承诺：
 * 1. **拖动期间绝不修改外部状态** —— 只在本地维护一份"预览集合"供实时高亮；
 * 2. **松手时统一提交一次** —— 一次赋值、一次渲染，不破坏原来的单击 / 单选逻辑；
 * 3. **不与滚动打架** —— 触摸下先长按，手指先动就当滚动放弃（见 `dragTrigger`）。
 */

import { onBeforeUnmount, onMounted, ref, watch, type Ref } from "vue";
import {
  applyDragRange, applyDragTo, beginDragSelection, clampIndex, dragTrigger,
  DRAG_THRESHOLD_PX, edgeScrollSpeed, finishDragSelection, LONG_PRESS_MS,
  type DragSelection, type PointerKind,
} from "../utils/dragSelect";

export interface SwipeSelectOptions {
  /** 列表根元素：事件委托挂在这里。 */
  root: Ref<HTMLElement | null>;
  /** 当前已选中的 id。**拖动期间不会被修改**，只作为预览的基线。 */
  selected: () => number[];
  /** 当前可见 id 的显示顺序：用于过滤掉已不在界面上的残留项。 */
  order: () => number[];
  /** 松手时统一提交（只调用一次）。 */
  commit: (ids: number[]) => void;
  /** 可选项元素的选择器，元素上必须带 `data-id`。 */
  itemSelector?: string;
  /** 这些控件上的按下不起手拖动（否则按钮会被点两次）。 */
  blockSelector?: string;
}

/**
 * 滑动结束后的这一次 click 要吞掉：不然拖完一松手，浏览器紧接着派发的
 * click 会落到某张卡上，又走一遍"打开详情 / 勾选"。
 */
const CLICK_SWALLOW_MS = 400;

/**
 * 几何命中测试。
 *
 * 兜底是必须的：`document.elementFromPoint` 依赖布局引擎，jsdom（以及任何
 * 没有布局支撑的环境）根本没有这个方法 —— 直接调用会 `TypeError`，
 * 把整条滑动链路一起炸掉。取不到就返回 `null`，交给调用方退化。
 */
export function elementAtPoint(x: number, y: number): Element | null {
  if (typeof document.elementFromPoint !== "function") return null;
  try {
    return document.elementFromPoint(x, y);
  } catch {
    return null;
  }
}

/**
 * 这一次按下要不要被 `blockSelector` 拦掉。
 *
 * 关键约定：**只有选项元素内部的控件才算被拦，选项元素自身不算**。
 * 把选项自己的标签写进 `blockSelector`（比如卡片根元素本身就是 `button`，
 * 又写了 `button`）会让拖动**永远起不来** —— 这类错误极其隐蔽，
 * 症状是"滑动完全没反应"，所以在这里挡掉而不是留给使用者去猜。
 */
export function pressBlocked(
  target: Element | null | undefined,
  itemSel: string,
  blockSel: string | undefined,
): boolean {
  if (!target) return false;
  if (!blockSel) return false;
  const blocked = target.closest?.(blockSel);
  if (!blocked) return false;
  const item = target.closest?.(itemSel);
  if (!item) return true;
  return blocked !== item && item.contains(blocked);
}

/** 贴边自动滚动的句柄。 */
export interface EdgeScrollHandle {
  /** 从 `el` 往上找可滚动的祖先并记住它（每次按下时调一次）。 */
  attach(el: HTMLElement | null): void;
  /** 每收到一次移动就调一次：贴边则启动滚动循环。 */
  update(x: number, y: number): void;
  /** 停止滚动循环（松手 / 卸载）。 */
  stop(): void;
}

/**
 * 贴边自动滚动：拖到列表上下边缘时自动滚，好让手指能选到视野外的项。
 *
 * 速度由 `edgeScrollSpeed` 决定（纯计算，已单测）；这里只负责"滚谁"和
 * "每滚一帧回调一次"（调用方拿回调重新做一次命中测试，于是视野外的项
 * 会被陆续选中）。`findScrollParent` 找不到可滚祖先时（列表整体可见、
 * 或环境没有布局）全部退化成空操作。
 */
export function useEdgeScroll(onFrame: (x: number, y: number) => void): EdgeScrollHandle {
  let scrollEl: HTMLElement | null = null;
  let rafId = 0;
  let lastX = 0;
  let lastY = 0;

  /** 向上找第一个真正能竖向滚动的祖先 —— 调用方不必把这个容器传进来。 */
  function findScrollParent(el: HTMLElement | null): HTMLElement | null {
    let n = el?.parentElement ?? null;
    while (n) {
      const oy = getComputedStyle(n).overflowY;
      if ((oy === "auto" || oy === "scroll") && n.scrollHeight > n.clientHeight) return n;
      n = n.parentElement;
    }
    return null;
  }

  function stop(): void {
    if (rafId) {
      cancelAnimationFrame(rafId);
      rafId = 0;
    }
  }

  /** 每滚一帧重新命中一次，这样能选到视野外的项。 */
  function tick(): void {
    rafId = 0;
    if (!scrollEl) return;
    const r = scrollEl.getBoundingClientRect();
    const v = edgeScrollSpeed(lastY, r.top, r.bottom);
    if (v === 0) return;
    const before = scrollEl.scrollTop;
    scrollEl.scrollTop += v;
    if (scrollEl.scrollTop !== before) onFrame(lastX, lastY);
    rafId = requestAnimationFrame(tick);
  }

  return {
    attach(el: HTMLElement | null): void {
      stop();
      scrollEl = findScrollParent(el);
    },
    update(x: number, y: number): void {
      lastX = x;
      lastY = y;
      if (rafId || !scrollEl) return;
      const r = scrollEl.getBoundingClientRect();
      if (edgeScrollSpeed(y, r.top, r.bottom) === 0) return;
      rafId = requestAnimationFrame(tick);
    },
    stop,
  };
}

export function useSwipeSelect(o: SwipeSelectOptions) {
  /** 是否正在拖动（用于给容器加样式：禁掉文字选择）。 */
  const dragging = ref(false);
  /** 拖动中的预览集合：**假如现在松手**，会被选中的那些 id。仅供高亮。 */
  const preview = ref<Set<number>>(new Set());

  const itemSel = o.itemSelector ?? "[data-id]";

  let state: DragSelection | null = null;
  /** 按下但还没判定成拖动。 */
  let pending = false;
  let pendingId: number | null = null;
  let startX = 0;
  let startY = 0;
  let startAt = 0;
  let moved = false;
  /** 上一次命中的项在 `order()` 里的下标：用来补齐快速滑动跳过的项。 */
  let lastIndex: number | null = null;

  let longPressTimer = 0;
  let swallowClick: ((ev: Event) => void) | null = null;
  let swallowTimer = 0;
  /** 当前挂着 touchmove 监听的元素（根节点会被 v-if 摘掉，要能重新挂）。 */
  let touchEl: HTMLElement | null = null;

  function clearTimer(): void {
    if (longPressTimer) {
      clearTimeout(longPressTimer);
      longPressTimer = 0;
    }
  }

  function releaseSwallow(): void {
    if (swallowTimer) {
      clearTimeout(swallowTimer);
      swallowTimer = 0;
    }
    if (swallowClick && o.root.value) {
      o.root.value.removeEventListener("click", swallowClick, true);
      swallowClick = null;
    }
  }

  /** 装一个一次性的捕获阶段 click 拦截，把拖动末尾那次 click 吃掉。 */
  function armSwallow(): void {
    releaseSwallow();
    const el = o.root.value;
    if (!el) return;
    const h = (ev: Event) => {
      ev.stopPropagation();
      ev.preventDefault();
      releaseSwallow();
    };
    swallowClick = h;
    el.addEventListener("click", h, true);
    // 兜底：万一这次手势没有产生 click（比如触摸被系统接管），别把拦截留到下次
    swallowTimer = window.setTimeout(releaseSwallow, CLICK_SWALLOW_MS);
  }

  /**
   * 解出某个元素所属项在 `order()` 里的下标；不是可选项返回 null。
   *
   * ⚠️ `closest` 必须用可选调用：`ev.target` **不保证**是 Element ——
   * 事件派发在 `document` / `window` 上时 target 就是它们（没有 `closest`），
   * 直接调会抛 `TypeError` 把整条滑动链路炸掉。取不到就当"没命中"。
   */
  function indexOfElement(el: Element | null | undefined): number | null {
    const row = el?.closest?.(itemSel) as HTMLElement | null;    const raw = row?.dataset.id;
    if (!raw) return null;
    const id = Number(raw);
    if (!Number.isFinite(id)) return null;
    const i = o.order().indexOf(id);
    return i >= 0 ? i : null;
  }

  /** 纯几何命中（自动滚动每滚一帧时没有事件可用，走这条）。 */
  function indexAtPoint(x: number, y: number): number | null {
    return indexOfElement(elementAtPoint(x, y));
  }

  /**
   * 本次移动命中了哪一项。
   *
   * 鼠标没有**隐式指针捕获**，`event.target` 就是指针下的元素 —— 所以几何
   * 取不到时（没有布局、或压根没有 `elementFromPoint`）用 `event.target` 兜底；
   * 触摸必须走几何命中，否则 `event.target` 会一直停在起点那一项上。
   */
  function indexAt(ev: PointerEvent): number | null {
    const byGeo = indexAtPoint(ev.clientX, ev.clientY);
    if (byGeo !== null) return byGeo;
    if (ev.pointerType === "touch") return null;
    return indexOfElement(ev.target as Element | null);
  }

  /**
   * 指针跑到列表之外（上方 / 下方的空白）时，把它夹到首项或末项 ——
   * 否则往列表外多划一截，端点那一项反而选不上。
   *
   * **没有布局信息时一律返回 null**（`rect.height <= 0`），保持
   * "划过行间空白什么都不做"的旧行为，不因为环境退化而误判。
   */
  function clampOutside(clientY: number): number | null {
    const el = o.root.value;
    if (!el) return null;
    const r = el.getBoundingClientRect();
    if (r.height <= 0) return null;
    if (clientY <= r.top) return 0;
    if (clientY >= r.bottom) return o.order().length; // 交给 clampIndex 夹成末项
    return null;
  }

  /**
   * 把某一项写进预览集合 —— 连同上一次命中项之间的**整段**一起写，
   * 这样快速滑动时被跳过的项不会漏掉（见 `applyDragRange`）。
   */
  function applyIndex(index: number): void {
    if (!state) return;
    const order = o.order();
    const clamped = clampIndex(index, order.length);
    if (clamped < 0) return;
    const changed = lastIndex === null || lastIndex === clamped
      ? applyDragTo(state, order[clamped])
      : applyDragRange(state, order, lastIndex, clamped);
    lastIndex = clamped;
    if (changed) preview.value = new Set(state.ids);
  }

  /** 滚一帧（或一次移动）之后重新命中一次。 */
  function hitAt(x: number, y: number): void {
    const i = indexAtPoint(x, y);
    if (i !== null) applyIndex(i);
  }

  /** 贴边自动滚动；每滚一帧重新命中一次，这样能选到视野外的项。 */
  const edge = useEdgeScroll(hitAt);

  function begin(id: number): void {
    clearTimer();
    state = beginDragSelection(o.selected(), id);
    pending = false;
    dragging.value = true;
    preview.value = new Set(state.ids);
    // 起手那一项就是"上一次命中"，后面跨项时从它开始补
    lastIndex = o.order().indexOf(id);
    if (lastIndex < 0) lastIndex = null;
  }

  /** 放弃这一次按下（手指是在滚页面 / 按在了按钮上）。 */
  function abort(): void {
    clearTimer();
    pending = false;
    pendingId = null;
  }

  function onPointerDown(ev: PointerEvent): void {
    if (ev.pointerType === "mouse" && ev.button !== 0) return;
    const el = ev.target as Element | null;
    if (!el) return;
    const row = el.closest?.(itemSel) as HTMLElement | null;
    const raw = row?.dataset.id;
    if (!raw) return;
    const id = Number(raw);
    if (!Number.isFinite(id)) return;
    // 行内的控件（按钮 / 复选框）上的按下不起手拖动，否则会被点两次；
    // 但**选项元素自身**不算被拦 —— 否则拖动永远起不来（见 pressBlocked）
    if (pressBlocked(el, itemSel, o.blockSelector)) return;

    releaseSwallow();
    pending = true;
    pendingId = id;
    moved = false;
    lastIndex = null;
    startX = ev.clientX;
    startY = ev.clientY;
    startAt = ev.timeStamp;
    edge.attach(o.root.value);

    // 触摸走长按：到点还没移动就进入；期间移动了会在 onPointerMove 里被判成滚动
    if (ev.pointerType === "touch") {
      longPressTimer = window.setTimeout(() => {
        longPressTimer = 0;
        if (pending && !moved && pendingId !== null) begin(pendingId);
      }, LONG_PRESS_MS);
    }
  }

  function onPointerMove(ev: PointerEvent): void {
    if (!pending && !state) return;
    if (!moved) {
      moved = Math.hypot(ev.clientX - startX, ev.clientY - startY) >= DRAG_THRESHOLD_PX;
    }
    if (!state && pending && pendingId !== null) {
      const kind = ev.pointerType as PointerKind;
      // 触摸下"先动了"就是滚动，直接放弃，不要抢用户的滚动手势
      const verdict = dragTrigger(kind, moved, ev.timeStamp - startAt);
      if (verdict === "drag") begin(pendingId);
      else if (verdict === "abort") {
        abort();
        return;
      }
    }
    if (state) {
      const idx = indexAt(ev) ?? clampOutside(ev.clientY);
      if (idx !== null) applyIndex(idx);
      edge.update(ev.clientX, ev.clientY);
    }
  }

  /**
   * 松手：统一提交一次。
   *
   * 拖动期间外部状态**一次都没被改过**，所以提交只在这里发生一次。
   */
  function end(): void {
    clearTimer();
    edge.stop();
    if (!state) {
      abort();
      return;
    }
    const ids = finishDragSelection(state, o.order());
    state = null;
    pendingId = null;
    lastIndex = null;
    dragging.value = false;
    preview.value = new Set();
    armSwallow();
    o.commit(ids);
  }

  /**
   * 手势被系统打断（`pointercancel`）：**丢弃，不提交**。
   *
   * 真机上手指划到一半会被来电 / 系统返回手势 / 侧滑 / 切后台打断，此时派发的是
   * `pointercancel` 而不是 `pointerup`。把它当成"松手"就会把**半截选择**写回外部状态 ——
   * 用户没做完这个动作，结果却被当成最终选择。`cancel` 的语义是"当作没发生过"。
   *
   * 代价很小：拖动期间外部状态一次都没被改过，丢掉本地预览就等于完整回滚。
   */
  function cancel(): void {
    clearTimer();
    abort();
    edge.stop();
    if (!state) return;
    state = null;
    pendingId = null;
    lastIndex = null;
    dragging.value = false;
    preview.value = new Set();
  }

  /**
   * 触摸下必须拦掉原生滚动：指针事件的 preventDefault 不管用，得拦 touchmove。
   *
   * **注册由本模块独占**（`bindTouch`），不导出给调用方：
   * 曾经同时走"模板声明式 `@touchmove`"和"模块内 `bindTouch`"两条路 ——
   * Vue 注册的是它自己的 invoker 包装函数，和这里的 `onTouchMove` **不是同一个引用**，
   * DOM 按 (type, callback, capture) 去重时不会合并，于是**每个 touchmove 跑两次**。
   * 今天它只是一句幂等的 preventDefault，看不出问题；哪天里面加了非幂等逻辑
   * （记录指针位置、累计偏移…）就会静默跑两遍。所以只留一条注册路径。
   */
  function onTouchMove(ev: TouchEvent): void {
    if (state && ev.cancelable) ev.preventDefault();
  }

  /**
   * 把 touchmove 监听挂到真正的根节点上（**唯一的注册入口**）。
   *
   * 坑：根节点常常是 `v-if` 出来的（封面墙在**空库时根本不渲染** `.grid`）。
   * 首次挂载时 `o.root.value` 是 null，`el?.` 会静默跳过，而 `onMounted`
   * 不会再跑第二次 —— 等扫描完数据来了、网格渲染出来，监听**永远补不上**，
   * 表现为"触摸下长按进入多选后一拖，页面跟着一起滚"。所以必须跟着根节点重挂。
   *
   * 留在模块内而不是交给调用方声明式绑定的理由：这个坑对**每一个**调用方都成立，
   * 不该要求谁记得写 `@touchmove` 才安全（Bug 1 就是这么来的）。
   */
  function bindTouch(el: HTMLElement | null): void {
    if (touchEl === el) return;
    if (touchEl) touchEl.removeEventListener("touchmove", onTouchMove);
    touchEl = el;
    el?.addEventListener("touchmove", onTouchMove, { passive: false });
  }

  function unbindTouch(): void {
    if (touchEl) touchEl.removeEventListener("touchmove", onTouchMove);
    touchEl = null;
  }

  onMounted(() => {
    bindTouch(o.root.value);
    // 松手常常发生在列表之外，所以这两个挂 document
    document.addEventListener("pointerup", end);
    // 系统打断走 cancel：丢弃，不能把半截选择提交上去
    document.addEventListener("pointercancel", cancel);
  });

  // 根节点可能被 v-if 摘掉再挂回来：监听要跟着它走（`flush: "post"` 保证元素已渲染）
  watch(o.root, (el) => bindTouch(el), { flush: "post" });

  onBeforeUnmount(() => {
    unbindTouch();
    document.removeEventListener("pointerup", end);
    document.removeEventListener("pointercancel", cancel);
    releaseSwallow();
    clearTimer();
    edge.stop();
    state = null;
  });

  // 注意：**不导出** onTouchMove —— 注册由 bindTouch 独占，避免与调用方的
  // 声明式绑定重复（两份 callback 引用不同，DOM 不会去重，会跑两遍）。
  return { dragging, preview, onPointerDown, onPointerMove, end, cancel };
}
