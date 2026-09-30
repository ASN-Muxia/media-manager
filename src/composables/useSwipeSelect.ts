/**
 * 滑动多选（在选项区域按下并拖动，连续选中划过的项）。
 *
 * 这个文件只做**DOM 侧的编排**，纯计算部分全在 `utils/dragSelect.ts`：
 * - 触发判定（鼠标即时拖 / 触摸长按）
 * - 命中哪一行、幂等地改状态、结束后导出结果
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

import { onBeforeUnmount, onMounted, ref, type Ref } from "vue";
import {
  applyDragTo, beginDragSelection, dragTrigger, DRAG_THRESHOLD_PX, edgeScrollSpeed,
  finishDragSelection, LONG_PRESS_MS, type DragSelection, type PointerKind,
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
  let lastX = 0;
  let lastY = 0;

  let longPressTimer = 0;
  let rafId = 0;
  let swallowClick: ((ev: Event) => void) | null = null;
  let swallowTimer = 0;
  /** 可滚动的祖先容器（边缘自动滚动就滚它）。 */
  let scrollEl: HTMLElement | null = null;

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

  function clearTimer(): void {
    if (longPressTimer) {
      clearTimeout(longPressTimer);
      longPressTimer = 0;
    }
  }

  function stopScroll(): void {
    if (rafId) {
      cancelAnimationFrame(rafId);
      rafId = 0;
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

  /** 命中指针下的那一项并更新预览。用几何命中，鼠标和触摸走同一条路径。 */
  function hitAt(x: number, y: number): void {
    if (!state) return;
    const row = document.elementFromPoint(x, y)?.closest(itemSel) as HTMLElement | null;
    const raw = row?.dataset.id;
    if (!raw) return;
    const id = Number(raw);
    if (!Number.isFinite(id)) return;
    // 幂等：同一项反复划过只写一次，只有真的变了才刷新预览
    if (applyDragTo(state, id)) preview.value = new Set(state.ids);
  }

  /** 贴边自动滚动；每滚一帧重新命中一次，这样能选到视野外的项。 */
  function tick(): void {
    rafId = 0;
    if (!state || !scrollEl) return;
    const r = scrollEl.getBoundingClientRect();
    const v = edgeScrollSpeed(lastY, r.top, r.bottom);
    if (v === 0) return;
    const before = scrollEl.scrollTop;
    scrollEl.scrollTop += v;
    if (scrollEl.scrollTop !== before) hitAt(lastX, lastY);
    rafId = requestAnimationFrame(tick);
  }

  function maybeAutoScroll(): void {
    if (rafId || !state || !scrollEl) return;
    const r = scrollEl.getBoundingClientRect();
    if (edgeScrollSpeed(lastY, r.top, r.bottom) === 0) return;
    rafId = requestAnimationFrame(tick);
  }

  function begin(id: number): void {
    clearTimer();
    state = beginDragSelection(o.selected(), id);
    pending = false;
    dragging.value = true;
    preview.value = new Set(state.ids);
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
    if (o.blockSelector && el.closest(o.blockSelector)) return;
    const row = el.closest(itemSel) as HTMLElement | null;
    const raw = row?.dataset.id;
    if (!raw) return;
    const id = Number(raw);
    if (!Number.isFinite(id)) return;

    releaseSwallow();
    pending = true;
    pendingId = id;
    moved = false;
    startX = lastX = ev.clientX;
    startY = lastY = ev.clientY;
    startAt = ev.timeStamp;
    scrollEl = findScrollParent(o.root.value);

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
    lastX = ev.clientX;
    lastY = ev.clientY;
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
      hitAt(ev.clientX, ev.clientY);
      maybeAutoScroll();
    }
  }

  /**
   * 松手（或手势被系统取消）：统一提交一次。
   *
   * 拖动期间外部状态**一次都没被改过**，所以取消 / 中断天然是干净的 ——
   * 这也是"结束后统一提交"最大的好处。
   */
  function end(): void {
    clearTimer();
    stopScroll();
    const wasDragging = state !== null;
    if (!state) {
      abort();
      return;
    }
    const ids = finishDragSelection(state, o.order());
    state = null;
    pendingId = null;
    dragging.value = false;
    preview.value = new Set();
    if (wasDragging) armSwallow();
    o.commit(ids);
  }

  /** 触摸下必须拦掉原生滚动：指针事件的 preventDefault 不管用，得拦 touchmove。 */
  function onTouchMove(ev: TouchEvent): void {
    if (state && ev.cancelable) ev.preventDefault();
  }

  onMounted(() => {
    const el = o.root.value;
    el?.addEventListener("touchmove", onTouchMove, { passive: false });
    // 松手常常发生在列表之外，所以这两个挂 document
    document.addEventListener("pointerup", end);
    document.addEventListener("pointercancel", end);
  });

  onBeforeUnmount(() => {
    const el = o.root.value;
    el?.removeEventListener("touchmove", onTouchMove);
    document.removeEventListener("pointerup", end);
    document.removeEventListener("pointercancel", end);
    releaseSwallow();
    clearTimer();
    stopScroll();
    state = null;
  });

  return { dragging, preview, onPointerDown, onPointerMove, end };
}
