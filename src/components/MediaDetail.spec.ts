import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { mount, type VueWrapper } from "@vue/test-utils";
import { nextTick } from "vue";
import { createPinia, setActivePinia } from "pinia";
import MediaDetail from "./MediaDetail.vue";
import { useMediaStore } from "../stores/mediaStore";
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
   * 浏览器对触摸有隐式指针捕获，拖动期间 `event.target` 会一直停在起点那一行，
   * 所以触摸必须走几何命中 —— 这条用例就是钉住这个分支。
   */
  it("触摸拖动用几何命中定位行，不认被捕获的 target", async () => {
    const w = mountDetail();
    const r = rows(w);

    await fire(r[0].element, "pointerdown", { pointerType: "touch" });
    hit.el = r[3].element; // 手指其实已经滑到第 4 行了
    await fire(r[0].element, "pointermove", { pointerType: "touch" });
    await fire(r[0].element, "pointerup", { pointerType: "touch" });

    expect(picked(w)).toEqual([11, 14]);
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
