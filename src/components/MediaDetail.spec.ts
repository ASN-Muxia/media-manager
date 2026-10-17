import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { mount, type VueWrapper } from "@vue/test-utils";
import { nextTick } from "vue";
import { createPinia, setActivePinia } from "pinia";
import MediaDetail from "./MediaDetail.vue";
import { useMediaStore } from "../stores/mediaStore";
import { LONG_PRESS_MS } from "../utils/dragSelect";
import {
  doubanApply, getMediaCategories, getMediaTags, importFolderInto,
} from "../api/commands";
import type { Episode, MediaItem } from "../api/commands";

/**
 * MediaDetail 的组件级用例，只盯「滑动连续多选」这一条链路。
 *
 * 纯状态机的边界（幂等 / 顺序 / 可见性过滤）在 `utils/dragSelect.spec.ts`；
 * 这里验证的是它**真的接上了 DOM** —— 按下起手、划过换行、松手收尾，
 * 以及复选框与行内按钮那两条路径没有被拖动抢走。
 *
 * 后端命令整体 mock：单测不碰真实 Tauri。
 */
/** 原生目录选择器的桩：组件里由它拿路径，用例里直接给结果。 */
const dialog = vi.hoisted(() => ({ open: vi.fn(async () => null as string | null) }));
vi.mock("@tauri-apps/plugin-dialog", () => dialog);

// 工厂是「首次导入时才执行」的，那时 MEDIA / ONE_SEASON 已初始化，可以放心引用。
vi.mock("../api/commands", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../api/commands")>();
  return {
    ...actual,
    listCategories: vi.fn(async () => []),
    listTags: vi.fn(async () => []),
    getMediaCategories: vi.fn(async () => []),
    getMediaTags: vi.fn(async () => []),
    getMediaList: vi.fn(async () => []),
    getScanHistory: vi.fn(async () => []),
    getMediaDetail: vi.fn(async () => ({ media: MEDIA, episodes: ONE_SEASON })),
    importFolderInto: vi.fn(async () => ({
      added: 0, skipped: 25, failed: 0, linked: 0,
      repointed: 25, droppedShells: 1, message: "25 集归入「西游记」",
    })),
    doubanApply: vi.fn(async () => ({
      ok: true, mediaId: 1, tmdbId: null, title: "西游记", episodes: 0, message: "ok",
    })),
  };
});

const MEDIA: MediaItem = {
  id: 1,
  type: "tv",
  title: "西游记",
  year: 1986,
  tmdb_id: null,
  poster_path: null,
  backdrop_path: null,
  overview: null,
  vote_average: null,
  genre: null,
  file_path: null,
  file_name: null,
  episode_count: 4,
  added_at: 1,
  display_title: "西游记",
};

function ep(id: number, episode: number, season = 1): Episode {
  return {
    id,
    media_id: 1,
    season,
    episode,
    title: null,
    file_path: `D:\\西游记\\${episode}.mkv`,
    file_name: `${episode}.mkv`,
    file_size: null,
    added_at: 1,
  };
}

/** 单季 4 集 —— 够跑完「框选一片 / 抹掉一片 / 跨行幂等」。 */
const ONE_SEASON: Episode[] = [ep(11, 1), ep(12, 2), ep(13, 3), ep(14, 4)];
/** 两季 —— 用来确认跨季拖动也走同一份扁平顺序。 */
const TWO_SEASONS: Episode[] = [ep(11, 1, 1), ep(12, 2, 1), ep(21, 1, 2), ep(22, 2, 2)];

/** `elementFromPoint` 的替身：jsdom 没有布局引擎，几何命中只能靠桩。 */
const hit = { el: null as Element | null };

let wrappers: VueWrapper[] = [];

function mountDetail(episodes: Episode[] = ONE_SEASON): VueWrapper {
  const store = useMediaStore();
  store.selectedId = 1;
  store.detail = {
    media: { ...MEDIA, episode_count: episodes.length },
    episodes,
  };
  const w = mount(MediaDetail, { attachTo: document.body });
  wrappers.push(w);
  return w;
}

/**
 * 造一个指针事件。
 *
 * 不用 `wrapper.trigger` 是因为它依赖 jsdom 的 PointerEvent 实现，
 * 而 `pointerType` / `button` 恰好是本组件分支的依据 —— 手写更像真实浏览器，
 * 也让用例与 jsdom 的版本无关。
 */
function pointer(
  target: EventTarget,
  type: string,
  extra: Record<string, unknown> = {},
): void {
  const init: PointerEventInit = {
    bubbles: true,
    cancelable: true,
    button: 0,
    pointerType: "mouse",
    clientX: 0,
    clientY: 0,
    ...(extra as PointerEventInit),
  };
  let ev: Event;
  if (typeof PointerEvent === "function") {
    ev = new PointerEvent(type, init);
  } else {
    // jsdom 没有 PointerEvent：退回 Event，再把剩下几个字段贴上去。
    // `bubbles` / `cancelable` 在 Event 上是只读访问器，只能交给构造函数，
    // 不能走 Object.assign。
    const { bubbles, cancelable, ...rest } = init;
    ev = Object.assign(new Event(type, { bubbles, cancelable }), rest);
  }
  target.dispatchEvent(ev);
}

/** 派发事件并等一次渲染 —— Vue 的 DOM 更新是异步的，不等就断言会看到旧状态。 */
async function fire(
  target: EventTarget,
  type: string,
  extra: Record<string, unknown> = {},
): Promise<void> {
  pointer(target, type, extra);
  await nextTick();
}

/** 从 DOM 反查选中了哪些行（顺序 = 显示顺序）。 */
function picked(w: VueWrapper): number[] {
  return w.findAll("li.picked").map((n) => Number(n.attributes("data-ep-id")));
}

function rows(w: VueWrapper) {
  return w.findAll("li[data-ep-id]");
}

/** 真实等待（触摸长按用的是真计时器，这条链路用假时间容易和 Vue 调度打架）。 */
function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

/**
 * 给元素装一个假的布局矩形。
 *
 * jsdom 没有布局引擎，`getBoundingClientRect()` 恒为全 0 —— 而"划到列表外要
 * 夹到首/末行"必须依赖矩形，所以只能在用例里把矩形桩上去。
 */
function stubRect(el: Element, top: number, bottom: number): void {
  Object.defineProperty(el, "getBoundingClientRect", {
    configurable: true,
    value: () => ({
      top, bottom, left: 0, right: 400, width: 400, height: bottom - top,
      x: 0, y: top, toJSON: () => ({}),
    }),
  });
}

function buttonByText(w: VueWrapper, text: string) {
  const b = w.findAll(".ep-bar button").find((n) => n.text().includes(text));
  if (!b) throw new Error(`工具条里没有「${text}」按钮`);
  return b;
}

beforeEach(() => {
  setActivePinia(createPinia());
  hit.el = null;
  // jsdom 没有 elementFromPoint（也没有布局），统一换成可编程的桩
  Object.defineProperty(document, "elementFromPoint", {
    configurable: true,
    writable: true,
    value: () => hit.el,
  });
});

afterEach(() => {
  for (const w of wrappers) w.unmount();
  wrappers = [];
  document.body.innerHTML = "";
});

describe("MediaDetail 滑动连续多选", () => {
  it("按住拖动划过一片，连续选中", async () => {
    const w = mountDetail();
    const r = rows(w);

    await fire(r[0].element, "pointerdown");
    await fire(r[1].element, "pointermove");
    await fire(r[2].element, "pointermove");

    // 松手之前就该看到效果，用户不用等抬手
    expect(picked(w)).toEqual([11, 12, 13]);
    expect(w.text()).toContain("已选 3");

    await fire(r[2].element, "pointerup");
    expect(picked(w)).toEqual([11, 12, 13]);
  });

  it("起点是已选中的行，整段拖动都是取消", async () => {
    const w = mountDetail();
    const r = rows(w);
    await buttonByText(w, "全选").trigger("click");
    expect(picked(w)).toEqual([11, 12, 13, 14]);

    await fire(r[1].element, "pointerdown");
    await fire(r[2].element, "pointermove");
    await fire(r[2].element, "pointerup");

    expect(picked(w)).toEqual([11, 14]);
  });

  it("同一行反复划过只写一次（否则手抖会把刚选上的又取消）", async () => {
    const w = mountDetail();
    const r = rows(w);

    await fire(r[0].element, "pointerdown");
    for (let i = 0; i < 5; i++) await fire(r[1].element, "pointermove");
    await fire(r[1].element, "pointerup");

    expect(picked(w)).toEqual([11, 12]);
  });

  it("松手之后再划过不再改变选择", async () => {
    const w = mountDetail();
    const r = rows(w);

    await fire(r[0].element, "pointerdown");
    await fire(r[1].element, "pointermove");
    await fire(r[1].element, "pointerup");
    await fire(r[2].element, "pointermove");

    expect(picked(w)).toEqual([11, 12]);
  });

  it("只在行体上按下即松手，等于切换这一行", async () => {
    const w = mountDetail();
    const r = rows(w);

    await fire(r[3].element, "pointerdown");
    await fire(r[3].element, "pointerup");

    expect(picked(w)).toEqual([14]);
  });

  it("跨季拖动走同一份扁平顺序", async () => {
    const w = mountDetail(TWO_SEASONS);
    expect(w.findAll(".season")).toHaveLength(2);
    const r = rows(w);

    // 第 1 季最后一集 → 第 2 季第一集
    await fire(r[1].element, "pointerdown");
    await fire(r[2].element, "pointermove");
    await fire(r[2].element, "pointerup");

    expect(picked(w)).toEqual([12, 21]);
  });

  it("划过行与行之间的空白不改变任何东西", async () => {
    const w = mountDetail();
    const r = rows(w);

    await fire(r[0].element, "pointerdown");
    await fire(w.find(".episodes").element, "pointermove"); // 落在容器本身
    await fire(r[0].element, "pointerup");

    expect(picked(w)).toEqual([11]);
  });

  it("复选框仍然只切换自己那一行", async () => {
    const w = mountDetail();
    const cb = rows(w)[1].find("input[type=checkbox]");
    await cb.setValue(true);
    expect(picked(w)).toEqual([12]);

    await cb.setValue(false);
    expect(picked(w)).toEqual([]);
  });

  it("从行内按钮上按下不会起手拖动", async () => {
    const w = mountDetail();
    const playBtn = rows(w)[0].find("button");

    await fire(playBtn.element, "pointerdown");
    await fire(rows(w)[2].element, "pointermove");
    await fire(rows(w)[2].element, "pointerup");

    expect(picked(w)).toEqual([]);
  });

  /**
   * 触摸这一条链路上钉了三件事，缺一件就会在真机上出事：
   *   1. 长按成立前手指一动 = 要滚页面，**绝不**能抢成框选（旧实现在这里就把行勾上了）；
   *   2. 长按成立后才起手，且必须走几何命中 —— 浏览器对触摸有隐式指针捕获，
   *      拖动期间 `event.target` 会一直停在起点那一行；
   *   3. 快速滑动时中间被跳过的行要补齐（旧实现只会写下指针正下方那一行）。
   */
  it("触摸长按后用几何命中定位行，并补齐被跳过的行", async () => {
    const w = mountDetail();
    const r = rows(w);

    await fire(r[0].element, "pointerdown", { pointerType: "touch", clientX: 0, clientY: 0 });
    hit.el = r[1].element;
    await fire(r[0].element, "pointermove", { pointerType: "touch", clientX: 0, clientY: 40 });
    expect(picked(w)).toEqual([]); // 长按没成立就移动 → 判定为滚页面，完全不介入

    await fire(r[0].element, "pointerup", { pointerType: "touch", clientX: 0, clientY: 40 });

    await fire(r[0].element, "pointerdown", { pointerType: "touch", clientX: 0, clientY: 0 });
    await sleep(LONG_PRESS_MS + 60);
    await nextTick();
    expect(picked(w)).toEqual([11]); // 长按成立：起手那一行立即生效

    hit.el = r[3].element; // 手指其实已经滑到第 4 行，中间两行被跳过了
    await fire(r[0].element, "pointermove", { pointerType: "touch", clientX: 0, clientY: 300 });
    await fire(r[0].element, "pointerup", { pointerType: "touch", clientX: 0, clientY: 300 });

    expect(picked(w)).toEqual([11, 12, 13, 14]);
  });

  /** 规则：快速滑动（事件稀疏、跨多项跳变）要补齐被跳过的项。 */
  it("鼠标快速滑动跨过多行，中间被跳过的行要补上", async () => {
    const w = mountDetail();
    const r = rows(w);

    // 全程只给一次移动事件，指针已经落到第 4 行
    await fire(r[0].element, "pointerdown", { clientX: 0, clientY: 0 });
    await fire(r[3].element, "pointermove", { clientX: 0, clientY: 300 });
    await fire(r[3].element, "pointerup", { clientX: 0, clientY: 300 });

    // 只处理"指针正下方那一项"的话这里会是 [11, 14]
    expect(picked(w)).toEqual([11, 12, 13, 14]);
  });

  /**
   * 规则：划到列表**首尾之外**不能漏掉端点，也不能越界报错。
   * 兜底顺序：先断言没有布局信息时什么都不做（退化不改旧行为），
   * 再断言装上矩形后末行被带上。
   */
  it("划到最后一行之外的空白仍然带上最后一行（端点不漏）", async () => {
    const plain = mountDetail();
    const pr = rows(plain);
    await fire(pr[1].element, "pointerdown", { clientX: 0, clientY: 50 });
    await fire(plain.find(".episodes").element, "pointermove", { clientX: 0, clientY: 400 });
    await fire(plain.find(".episodes").element, "pointerup", { clientX: 0, clientY: 400 });
    // 没有布局信息（jsdom 的常态）→ 划出列表不改变选择
    expect(picked(plain)).toEqual([12]);

    const w = mountDetail();
    stubRect(w.find(".episodes").element, 0, 200);
    const r = rows(w);
    await fire(r[1].element, "pointerdown", { clientX: 0, clientY: 50 });
    await fire(w.find(".episodes").element, "pointermove", { clientX: 0, clientY: 400 });
    await fire(w.find(".episodes").element, "pointerup", { clientX: 0, clientY: 400 });
    expect(picked(w)).toEqual([12, 13, 14]);
  });

  /** 规则：滑动结束后的那次 click 要吞掉，否则会误触发行内控件。 */
  it("拖动后紧随的那次 click 被吞掉", async () => {
    const w = mountDetail();
    const r = rows(w);

    await fire(r[0].element, "pointerdown", { clientX: 0, clientY: 0 });
    await fire(r[2].element, "pointermove", { clientX: 0, clientY: 200 });
    await fire(r[2].element, "pointerup", { clientX: 0, clientY: 200 });

    const ev = new Event("click", { bubbles: true, cancelable: true });
    r[2].element.dispatchEvent(ev);
    expect(ev.defaultPrevented).toBe(true);
    expect(picked(w)).toEqual([11, 12, 13]);
  });

  /** 规则同上，但**原地按下松手不算拖动**，click 语义必须原样保留。 */
  it("原地按下松手不算拖动，click 不被吞", async () => {
    const w = mountDetail();
    const r = rows(w);

    await fire(r[0].element, "pointerdown", { clientX: 0, clientY: 0 });
    await fire(r[0].element, "pointerup", { clientX: 0, clientY: 0 });

    const ev = new Event("click", { bubbles: true, cancelable: true });
    r[0].element.dispatchEvent(ev);
    expect(ev.defaultPrevented).toBe(false);
    expect(picked(w)).toEqual([11]);
  });

  it("切换条目会丢弃进行中的拖动并清空勾选", async () => {
    const w = mountDetail();
    const r = rows(w);

    await fire(r[0].element, "pointerdown");
    expect(picked(w)).toEqual([11]);

    useMediaStore().selectedId = 2;
    await nextTick();
    expect(picked(w)).toEqual([]);

    // 拖动状态也必须一起丢掉，否则旧行号会被后面的移动事件重新写回来
    await fire(r[1].element, "pointermove");
    expect(picked(w)).toEqual([]);
  });

  it("拖动中给列表加标记（禁掉文字选择）", async () => {
    const w = mountDetail();
    const r = rows(w);

    expect(w.find(".episodes").classes()).not.toContain("dragging");
    await fire(r[0].element, "pointerdown");
    expect(w.find(".episodes").classes()).toContain("dragging");
    await fire(r[0].element, "pointerup");
    expect(w.find(".episodes").classes()).not.toContain("dragging");
  });

  /* ==================== 复核补测：他没覆盖的真实边界 ==================== */
  /* 补的是「真机上会出事、但用例里没人钉」的口子：非左键、pointercancel、
     拖动中数据源被改、松手在容器外、以及卸载时长按定时器 / 边缘滚动收没收干净。 */

  /** 规则：**只有左键**才起手拖动（右键是上下文菜单，不能被抢成框选）。 */
  it("右键（非左键）按下不起手拖动", async () => {
    const w = mountDetail();
    const r = rows(w);

    await fire(r[0].element, "pointerdown", { button: 2, clientX: 0, clientY: 0 });
    await fire(r[2].element, "pointermove", { button: 2, clientX: 0, clientY: 200 });
    await fire(r[2].element, "pointerup", { button: 2, clientX: 0, clientY: 200 });

    expect(picked(w)).toEqual([]);
  });

  /**
   * 规则：手势被系统打断（`pointercancel`）要**回滚到起手前**，不能把半截选择留下 ——
   * cancel 的语义是"当作没发生过"，只有 `pointerup` 才提交。
   * 同时要收干净：不留 dragging 标记，之后的移动事件也不能再写选择。
   *
   * ⚠️ 这条断言在第 3 轮被**有意改写**（不是为了让测试变绿而放宽）：
   * 原断言 `[11, 12, 13]` 编码的是"pointercancel 也提交"这一**错误语义**，
   * 真机上会出现"手指划到一半被来电打断，那半截却被当成最终选择写进
   * 「删除所选」"。现断言 `[]` = 回滚。同一动作走 pointerup 仍然提交，
   * 用下面的对照断言钉住，防止有人把两条路径又合并回一个处理函数。
   */
  it("pointercancel 回滚到起手前，不留半截选择（pointerup 才提交）", async () => {
    const w = mountDetail();
    const r = rows(w);

    await fire(r[0].element, "pointerdown", { clientX: 0, clientY: 0 });
    await fire(r[2].element, "pointermove", { clientX: 0, clientY: 200 });
    expect(picked(w)).toEqual([11, 12, 13]); // 拖动中实时可见
    expect(w.find(".episodes").classes()).toContain("dragging");

    await fire(r[2].element, "pointercancel", { clientX: 0, clientY: 200 });
    expect(w.find(".episodes").classes()).not.toContain("dragging");
    expect(picked(w)).toEqual([]); // 回滚：半截选择不能留下

    // 收尾之后再划过不该有任何作用
    await fire(r[3].element, "pointermove", { clientX: 0, clientY: 300 });
    expect(picked(w)).toEqual([]);

    // 对照：同一串动作走 pointerup 是要提交的
    const w2 = mountDetail();
    const r2 = rows(w2);
    await fire(r2[0].element, "pointerdown", { clientX: 0, clientY: 0 });
    await fire(r2[2].element, "pointermove", { clientX: 0, clientY: 200 });
    await fire(r2[2].element, "pointerup", { clientX: 0, clientY: 200 });
    expect(picked(w2)).toEqual([11, 12, 13]);
  });

  /** 规则：松手**常常发生在列表之外**，pointerup 挂在 document 上才收得到。 */
  it("松手发生在列表之外（document 级 pointerup）仍正常收尾", async () => {
    const w = mountDetail();
    const r = rows(w);

    await fire(r[0].element, "pointerdown", { clientX: 0, clientY: 0 });
    await fire(r[1].element, "pointermove", { clientX: 0, clientY: 100 });
    await fire(document, "pointerup", { clientX: 900, clientY: 900 });

    expect(picked(w)).toEqual([11, 12]);
    // 收尾后再划过不再改变选择
    await fire(r[3].element, "pointermove", { clientX: 0, clientY: 300 });
    expect(picked(w)).toEqual([11, 12]);
  });

  /**
   * 规则：拖动**中途数据源被改**（换季重渲染 / 删集后重渲染）不能写出
   * 已经不在界面上的行，也不能因为 `lastIndex` 过期而越界。
   */
  it("拖动途中单集列表被改短，结果收敛到新列表且不越界", async () => {
    const w = mountDetail();
    const r = rows(w);

    await fire(r[0].element, "pointerdown", { clientX: 0, clientY: 0 });
    await fire(r[2].element, "pointermove", { clientX: 0, clientY: 200 });

    // 数据源变成 2 集：旧的第 3 行（13）已经不在界面上了
    const store = useMediaStore();
    store.detail = {
      media: { ...MEDIA, episode_count: 2 },
      episodes: [ep(11, 1), ep(12, 2)],
    };
    await nextTick();
    const r2 = rows(w);
    expect(r2).toHaveLength(2);

    await fire(r2[1].element, "pointermove", { clientX: 0, clientY: 100 });
    await fire(r2[1].element, "pointerup", { clientX: 0, clientY: 100 });

    expect(picked(w)).toEqual([11, 12]);
  });

  /**
   * 规则：卸载时**长按定时器必须被清掉**，边缘滚动的 rAF 也必须停 ——
   * 否则组件已经不在了，定时器还会把一次 `beginDragSelection` 打出来。
   */
  it("拖动中卸载：长按定时器被清掉、边缘滚动被停掉", async () => {
    const setT = vi.spyOn(window, "setTimeout");
    const clearT = vi.spyOn(window, "clearTimeout");
    try {
      const w = mountDetail();
      const r = rows(w);
      await fire(r[0].element, "pointerdown", { pointerType: "touch", clientX: 0, clientY: 0 });

      const idx = setT.mock.calls.findIndex((call) => call[1] === LONG_PRESS_MS);
      expect(idx).toBeGreaterThanOrEqual(0); // 确实排了长按定时器
      const timerId = setT.mock.results[idx].value as unknown;

      w.unmount();
      wrappers = wrappers.filter((x) => x !== w);
      expect(clearT).toHaveBeenCalledWith(timerId);
      void r;
    } finally {
      setT.mockRestore();
      clearT.mockRestore();
    }
  });

  /**
   * 规则：触摸下必须拦掉原生滚动，否则长按进入多选后一拖页面跟着一起滚。
   * 详情页这条是模板里声明式挂的（`@touchmove`），所以换数据集也不会丢。
   */
  it("触摸拖动期间拦掉原生滚动", async () => {
    const w = mountDetail();
    const r = rows(w);

    await fire(r[0].element, "pointerdown", { pointerType: "touch", clientX: 0, clientY: 0 });
    await sleep(LONG_PRESS_MS + 60);
    await nextTick();

    const tm = new Event("touchmove", { bubbles: true, cancelable: true });
    r[0].element.dispatchEvent(tm);
    expect(tm.defaultPrevented).toBe(true);

    await fire(r[0].element, "pointerup", { pointerType: "touch", clientX: 0, clientY: 0 });
  });

  /**
   * 规则：`pointercancel` 之后**不装 click 吞除**。
   * 系统打断不会再带来那次尾随 click，装了反而会吞掉用户随后的真实点击 ——
   * 详情页这里尤其要紧，行内全是「播放 / 目录 / ✕」这类真的会做事的按钮。
   * 对照段走 `pointerup`，钉住"只有松手才吞"。
   */
  it("pointercancel 不装 click 吞除（对照：pointerup 会吞）", async () => {
    // ① cancel 路径
    const w = mountDetail();
    const r = rows(w);
    await fire(r[0].element, "pointerdown", { clientX: 0, clientY: 0 });
    await fire(r[2].element, "pointermove", { clientX: 0, clientY: 200 });
    await fire(r[2].element, "pointercancel", { clientX: 0, clientY: 200 });

    const afterCancel = new Event("click", { bubbles: true, cancelable: true });
    r[2].element.dispatchEvent(afterCancel);
    expect(afterCancel.defaultPrevented).toBe(false);

    // ② 对照：走 pointerup 且真的拖动过 → 那次 click 要被吞
    const w2 = mountDetail();
    const r2 = rows(w2);
    await fire(r2[0].element, "pointerdown", { clientX: 0, clientY: 0 });
    await fire(r2[2].element, "pointermove", { clientX: 0, clientY: 200 });
    await fire(r2[2].element, "pointerup", { clientX: 0, clientY: 200 });

    const afterUp = new Event("click", { bubbles: true, cancelable: true });
    r2[2].element.dispatchEvent(afterUp);
    expect(afterUp.defaultPrevented).toBe(true);
  });

  /**
   * 规则：`pointercancel` 的回滚必须**只回滚这一次拖动**，
   * 起手之前就勾选好的行不能跟着一起没。
   * （`dragSnapshot` 是在 `beginEpDrag` 里拍的，拍早/拍晚都会在这里露馅。）
   */
  it("pointercancel 只回滚本次拖动，起手前已勾选的行保留", async () => {
    const w = mountDetail();
    const r = rows(w);

    // 先手工勾上第 4 行（不走拖动）
    await rows(w)[3].find("input[type=checkbox]").setValue(true);
    expect(picked(w)).toEqual([14]);

    // 再从第 1 行起手拖到第 3 行，然后被系统打断
    await fire(r[0].element, "pointerdown", { clientX: 0, clientY: 0 });
    await fire(r[2].element, "pointermove", { clientX: 0, clientY: 200 });
    // 注意：`picked()` 读的是 DOM，所以是**显示顺序**不是勾选顺序
    expect(picked(w)).toEqual([11, 12, 13, 14]);

    await fire(r[2].element, "pointercancel", { clientX: 0, clientY: 200 });
    expect(picked(w)).toEqual([14]); // 只剩起手前就有的那一行
  });
});

/**
 * 「换条目」的那个 watch 里藏过一个静默 bug：`immediate: true` 会在 setup 期间
 * **同步**跑一次回调，而回调引用的 `tagInput` / `catMsg` 当时还声明在它下面，
 * 于是回调一进到那两行就抛 TDZ 错 —— Vue 只把错误收进告警，**后面几行不再执行**，
 * 表现为「进入详情不加载分类」和「单集勾选跨剧目残留」。这两条用例把
 * 「回调必须跑到底」钉住，只靠拖动用例是抓不住前半截的。
 */
describe("MediaDetail 进入详情 / 换条目", () => {
  it("进入详情会加载该条目的分类与标签", async () => {
    mountDetail();
    await vi.waitFor(() => {
      expect(getMediaCategories).toHaveBeenCalledWith(1);
      expect(getMediaTags).toHaveBeenCalledWith(1);
    });
  });

  it("换条目会重新加载新条目的分类与标签", async () => {
    const w = mountDetail();
    await vi.waitFor(() => expect(getMediaCategories).toHaveBeenCalledWith(1));

    useMediaStore().selectedId = 2;
    await vi.waitFor(() => expect(getMediaCategories).toHaveBeenCalledWith(2));
    // 勾选也必须跟着清掉，两条都在同一个回调里
    expect(picked(w)).toEqual([]);
  });
});

/**
 * 「添加文件到这部剧」是推断失败时的兜底：`86版西游记.01.mkv` 这类形状
 * 两条推断规则都失效，只能由用户显式指定归属。这里验证按钮真的接上了。
 */
describe("MediaDetail 指定归属与豆瓣刮削", () => {
  function actionButton(w: VueWrapper, text: string) {
    const b = w.findAll(".actions button").find((n) => n.text().includes(text));
    if (!b) throw new Error(`操作区里没有「${text}」按钮`);
    return b;
  }

  it("剧集详情页才有「添加文件到这部剧」", () => {
    expect(actionButton(mountDetail(), "添加文件到这部剧").exists()).toBe(true);
    const movie = mount(MediaDetail, { attachTo: document.body });
    wrappers.push(movie);
    const st = useMediaStore();
    st.selectedId = 9;
    st.detail = { media: { ...MEDIA, type: "movie" }, episodes: [] };
    return nextTick().then(() => {
      expect(
        movie.findAll(".actions button").some((n) => n.text().includes("添加文件到这部剧")),
      ).toBe(false);
    });
  });

  it("选好文件夹后按 id 与路径调用指定归属", async () => {
    const w = mountDetail();
    dialog.open.mockResolvedValueOnce("D:////86版西游记");
    await actionButton(w, "添加文件到这部剧").trigger("click");
    await vi.waitFor(() => {
      expect(importFolderInto).toHaveBeenCalledWith(1, "D:////86版西游记", null);
    });
  });

  it("取消目录选择时什么都不做", async () => {
    const w = mountDetail();
    dialog.open.mockResolvedValueOnce(null);
    await actionButton(w, "添加文件到这部剧").trigger("click");
    await nextTick();
    expect(importFolderInto).not.toHaveBeenCalled();
  });

  it("「用豆瓣刮削」按钮调 doubanApply", async () => {
    const w = mountDetail();
    await actionButton(w, "用豆瓣刮削").trigger("click");
    await vi.waitFor(() => expect(doubanApply).toHaveBeenCalledWith(1, null, null));
  });
});
