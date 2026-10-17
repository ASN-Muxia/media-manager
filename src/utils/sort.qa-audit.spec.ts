/**
 * QA 独立审计用例（严过关 / Edward）——**与 `sort.spec.ts` 无依赖**，
 * 全部断言直接来自用户原始需求，用于独立复核排序功能是否真的能用。
 *
 * 覆盖：A ICU 红线、B 未识别分区恒序、C 倒序禁 reverse、F 边界与错误路径。
 */
import { describe, expect, it } from "vitest";
import {
  buildHanOrder, hanCollatorUsable, identified, rawTitleOf, sortMediaItems,
  type SortableItem,
} from "./sort";

type By = Parameters<typeof sortMediaItems>[1];
type Dir = Parameters<typeof sortMediaItems>[2];

/** 本运行时 ICU 是否提供 `zh` 拼音序。 */
const HAN_OK = hanCollatorUsable();

function m(over: Partial<SortableItem> = {}): SortableItem {
  return {
    id: 1,
    type: "movie",
    title: "T",
    display_title: "T",
    year: 2000,
    added_at: 1,
    original_title: null,
    file_name: null,
    ...over,
  };
}

/** 像 mediaStore 一样把 `han` 现算出来传进去（这是真实调用形态）。 */
function ids(list: readonly SortableItem[], by: By = "name", dir: Dir = "asc"): number[] {
  const han = buildHanOrder(list.map(rawTitleOf));
  return sortMediaItems(list, by, dir, han).map((i) => i.id);
}

function names(list: readonly SortableItem[], by: By = "name", dir: Dir = "asc"): string[] {
  const han = buildHanOrder(list.map(rawTitleOf));
  return sortMediaItems(list, by, dir, han).map((i) => i.display_title);
}

/* ==================== A. ICU 红线 R1：汉字不得排到拉丁字母之前 ==================== */

describe("QA-A ICU 红线：拉丁必须排在汉字之前", () => {
  /** 用户明确要求的样本：`2012 → Avatar → 阿甘正传 → 宙斯之子`。 */
  const sample = (): SortableItem[] => [
    m({ id: 4, display_title: "宙斯之子", year: 2014, added_at: 4 }),
    m({ id: 2, display_title: "Avatar", year: 2009, added_at: 2 }),
    m({ id: 1, display_title: "2012", year: 2009, added_at: 1 }),
    m({ id: 3, display_title: "阿甘正传", year: 1994, added_at: 3 }),
  ];

  it("证据：裸用 Intl.Collator('zh') 整串比较确实会把汉字排到拉丁之前（红线存在）", () => {
    const raw = new Intl.Collator("zh").compare("Avatar", "阿甘正传");
    // 只要这条断言成立，就说明「直接拿 Collator 比整串标题」的实现必然踩线
    expect(raw).toBe(1);
  });

  it("A1 sortBy=name / dir=asc 时完整顺序为 2012 → Avatar → 阿甘正传 → 宙斯之子", () => {
    expect(names(sample())).toEqual(["2012", "Avatar", "阿甘正传", "宙斯之子"]);
  });

  it("A2 Avatar 必须排在 阿甘正传 之前（以索引差为准，不依赖拼音可用性）", () => {
    const out = names(sample());
    const avatar = out.indexOf("Avatar");
    const agan = out.indexOf("阿甘正传");
    expect(avatar).toBeGreaterThanOrEqual(0);
    expect(agan).toBeGreaterThanOrEqual(0);
    expect(avatar).toBeLessThan(agan);
  });

  it("A3 dir=desc 是把整个展示反过来：宙斯之子 → 阿甘正传 → Avatar → 2012", () => {
    expect(names(sample(), "name", "desc")).toEqual([
      "宙斯之子", "阿甘正传", "Avatar", "2012",
    ]);
  });

  it("A4 即使 han 表为空（ICU 降级路径）也必须是同样的 2012 → Avatar 顺序", () => {
    const out = sortMediaItems(sample(), "name", "asc", new Map());
    const ns = out.map((i) => i.display_title);
    expect(ns.indexOf("2012")).toBeLessThan(ns.indexOf("Avatar"));
    expect(ns.indexOf("Avatar")).toBeLessThan(ns.indexOf("阿甘正传"));
  });

  /**
   * 汉字内部必须是**拼音序**而不是 Unicode 码点序。
   * 码点序应为 二(4E8C) < 冬(51AC) < 安(5B89) < 春(6625) < 波(6CE2)，
   * 拼音序应为 an < bo < chun < dong < er —— 两者完全不同，能证明拼音真的生效。
   */
  it("A5 汉字内部按拼音排序（与码点序可区分）", () => {
    const list = ["二", "冬", "安", "春", "波"].map((t, i) =>
      m({ id: i + 1, display_title: t, title: t, year: 2000 }));
    if (HAN_OK) {
      expect(names(list)).toEqual(["安", "波", "春", "冬", "二"]);
    } else {
      expect(names(list)).toEqual(["二", "冬", "安", "春", "波"]);
    }
  });

  it("A6 同一批字在空 han 表下走码点序（降级路径自洽）", () => {
    const list = ["二", "冬", "安", "春", "波"].map((t, i) =>
      m({ id: i + 1, display_title: t, title: t, year: 2000 }));
    const out = sortMediaItems(list, "name", "asc", new Map()).map((i) => i.display_title);
    expect(out).toEqual(["二", "冬", "安", "春", "波"]);
  });
});

/* ==================== B. 未识别分区：恒在末尾，与方向无关 ==================== */

describe("QA-B 未识别分区恒在最末", () => {
  /**
   * 已识别：11(星际穿越/2014)、12(Breaking Bad/2008)
   * 未识别：21(类型 unknown)、22(标题为空)、23(year 为 null)
   */
  const mixed = (): SortableItem[] => [
    m({ id: 11, display_title: "星际穿越", title: "星际穿越", year: 2014, added_at: 500 }),
    m({ id: 12, type: "tv", display_title: "Breaking Bad", year: 2008, added_at: 400 }),
    m({ id: 21, type: "unknown", display_title: "阿 Unknown", year: 2010, added_at: 10 }),
    m({ id: 22, type: "movie", display_title: "", title: "", year: 2010, added_at: 20 }),
    m({ id: 23, type: "movie", display_title: "无年份片", year: null, added_at: 30 }),
  ];

  const IDENTIFIED = [11, 12];
  const UNIDENTIFIED = [21, 22, 23];

  it("B0 identified 三条判据各自成立", () => {
    expect(identified(m({ type: "unknown", year: 2000 }))).toBe(false);
    expect(identified(m({ type: "movie", display_title: "", title: "", year: 2000 }))).toBe(false);
    expect(identified(m({ type: "movie", year: null }))).toBe(false);
    expect(identified(m({ type: "tv", display_title: "X", year: 1999 }))).toBe(true);
  });

  /** 3 个排序键 × 2 个方向，共 6 种组合，未识别分区都必须在最后。 */
  it("B1 所有 6 种（键 × 方向）组合下，未识别三条恒排在已识别两条之后", () => {
    for (const by of ["name", "year", "added"] as const) {
      for (const dir of ["asc", "desc"] as const) {
        const out = ids(mixed(), by, dir);
        const head = out.slice(0, 2);
        const tail = out.slice(2);
        expect(head.slice().sort(), `${by}/${dir} 头部`).toEqual(IDENTIFIED);
        expect(tail.slice().sort(), `${by}/${dir} 尾部`).toEqual(UNIDENTIFIED);
        // 更强的一条：任取一条已识别都排在任一条未识别之前
        for (const i of IDENTIFIED) {
          for (const u of UNIDENTIFIED) {
            expect(out.indexOf(i), `${by}/${dir}: ${i} 应早于 ${u}`).toBeLessThan(out.indexOf(u));
          }
        }
      }
    }
  });

  it("B2 倒序不会把未识别分区翻到最前（最后一条永远是未识别）", () => {
    for (const by of ["name", "year", "added"] as const) {
      for (const dir of ["asc", "desc"] as const) {
        const out = ids(mixed(), by, dir);
        expect(UNIDENTIFIED, `${by}/${dir}`).toContain(out[out.length - 1]);
      }
    }
  });

  it("B3 未识别分区内部仍按当前规则有序（名称 asc / desc）", () => {
    const list: SortableItem[] = [
      m({ id: 31, type: "unknown", display_title: "C-未知", year: 2000, added_at: 1 }),
      m({ id: 32, type: "unknown", display_title: "A-未知", year: 2000, added_at: 2 }),
      m({ id: 33, type: "unknown", display_title: "B-未知", year: 2000, added_at: 3 }),
    ];
    expect(ids(list, "name", "asc")).toEqual([32, 33, 31]);
    expect(ids(list, "name", "desc")).toEqual([31, 33, 32]);
  });

  it("B4 未识别分区内部按年份规则有序，且缺年份者沉到分区自己的末尾", () => {
    const list: SortableItem[] = [
      m({ id: 41, type: "unknown", display_title: "甲", year: 2010, added_at: 1 }),
      m({ id: 42, type: "unknown", display_title: "乙", year: null, added_at: 2 }),
      m({ id: 43, type: "unknown", display_title: "丙", year: 2000, added_at: 3 }),
    ];
    expect(ids(list, "year", "asc")).toEqual([43, 41, 42]);
    expect(ids(list, "year", "desc")).toEqual([41, 43, 42]);
  });

  it("B5 缺 year 的条目在年份排序下，正序 / 倒序都在末尾", () => {
    const list = mixed();
    const asc = ids(list, "year", "asc");
    const desc = ids(list, "year", "desc");
    expect(asc[asc.length - 1]).toBe(23);
    expect(desc[desc.length - 1]).toBe(23);
  });
});

/* ==================== C. 倒序禁 Array.reverse() ==================== */

describe("QA-C 倒序只取反主键，兜底链不翻转", () => {
  it("C1 同年的一批在倒序下仍按名称升序（reverse() 会把它翻成 C→B→A）", () => {
    const list: SortableItem[] = [
      m({ id: 51, display_title: "A", year: 2000, added_at: 1 }),
      m({ id: 52, display_title: "B", year: 2000, added_at: 2 }),
      m({ id: 53, display_title: "C", year: 2000, added_at: 3 }),
    ];
    expect(ids(list, "year", "desc")).toEqual([51, 52, 53]);
    expect(ids(list, "year", "asc")).toEqual([51, 52, 53]);
  });

  it("C2 添加时间倒序：主键反转，主键相同时仍由 id 升序定序", () => {
    const list: SortableItem[] = [
      m({ id: 61, display_title: "X", year: 2000, added_at: 900 }),
      m({ id: 62, display_title: "Y", year: 2000, added_at: 900 }),
      m({ id: 63, display_title: "Z", year: 2000, added_at: 100 }),
    ];
    expect(ids(list, "added", "desc")).toEqual([61, 62, 63]);
    expect(ids(list, "added", "asc")).toEqual([63, 61, 62]);
  });

  it("C3 主键完全相同（同名同年同时）时由 id 定序，正倒序一致", () => {
    const list: SortableItem[] = [
      m({ id: 75, display_title: "Same", title: "Same", year: 2000, added_at: 1 }),
      m({ id: 73, display_title: "Same", title: "Same", year: 2000, added_at: 1 }),
      m({ id: 71, display_title: "Same", title: "Same", year: 2000, added_at: 1 }),
    ];
    for (const dir of ["asc", "desc"] as const) {
      expect(ids(list, "name", dir)).toEqual([71, 73, 75]);
      expect(ids(list, "year", dir)).toEqual([71, 73, 75]);
      expect(ids(list, "added", dir)).toEqual([71, 73, 75]);
    }
  });
});

/* ==================== F. 边界与错误路径 ==================== */

describe("QA-F 边界、脏值与不可变性", () => {
  it("F1 空列表 / 单条都不炸，输出长度守恒", () => {
    expect(sortMediaItems([], "name", "asc")).toEqual([]);
    const one = [m({ id: 9 })];
    expect(ids(one)).toEqual([9]);
  });

  it("F2 全部未识别：顺序仍然确定，不崩", () => {
    const list: SortableItem[] = [
      m({ id: 1, type: "unknown", display_title: "Zed", year: 2000 }),
      m({ id: 2, type: "unknown", display_title: "Alpha", year: 2000 }),
      m({ id: 3, type: "unknown", display_title: "Mid", year: 2000 }),
    ];
    expect(ids(list)).toEqual([2, 3, 1]);
    expect(ids(list, "name", "desc")).toEqual([1, 3, 2]);
  });

  /**
   * 全部缺年份时主键**无从取反**（null 是位置哨兵而非方向的一部分），
   * 于是由兜底链按名称定序 —— 这与 C3「主键相同则两个方向结果一致」是同一条不变量，
   * 不是缺陷。真正该验证的是：只要有人有年份，方向就必须生效。
   */
  it("F3 全部缺年份：主键无法取反，由名称兜底，两个方向一致", () => {
    const list: SortableItem[] = [
      m({ id: 1, display_title: "Zulu", year: null }),
      m({ id: 2, display_title: "Alpha", year: null }),
    ];
    expect(ids(list, "year", "asc")).toEqual([2, 1]);
    expect(ids(list, "year", "desc")).toEqual([2, 1]);
  });

  /** 只要还有人有年份，倒序就必须真的把顺序反过来。 */
  it("F3b 缺年份者与有年份者混排时，年份倒序确实反转且缺年份者恒在末尾", () => {
    const list: SortableItem[] = [
      m({ id: 1, display_title: "Old One", year: 1990, added_at: 1 }),
      m({ id: 2, display_title: "New One", year: 2020, added_at: 2 }),
      m({ id: 3, display_title: "Mid One", year: 2005, added_at: 3 }),
      m({ id: 4, display_title: "No Year", year: null, added_at: 4 }),
    ];
    expect(ids(list, "year", "asc")).toEqual([1, 3, 2, 4]);
    expect(ids(list, "year", "desc")).toEqual([2, 3, 1, 4]);
  });

  it("F4 标题为空串的条目沉底且不崩", () => {
    const list: SortableItem[] = [
      m({ id: 1, display_title: "", title: "", year: 2000, added_at: 1 }),
      m({ id: 2, display_title: "Real", title: "Real", year: 2000, added_at: 2 }),
    ];
    expect(ids(list)).toEqual([2, 1]);
    expect(ids(list, "name", "desc")).toEqual([2, 1]);
  });

  it("F5 纯标点标题不参与比较（键为空流），排序仍然确定", () => {
    const list: SortableItem[] = [
      m({ id: 1, display_title: "！！！", title: "！！！", year: 2000, added_at: 1 }),
      m({ id: 2, display_title: "Real Movie", title: "Real Movie", year: 2000, added_at: 2 }),
    ];
    const out = ids(list);
    expect(out).toHaveLength(2);
    expect(out.slice().sort()).toEqual([1, 2]);
    // 再排一次结果必须一致（无随机性）
    expect(ids(list)).toEqual(out);
  });

  it("F6 纯数字标题按字符逐位比较：`2012` 在 `1999` 之前，`10` 在 `9` 之前", () => {
    const a = [m({ id: 1, display_title: "1999" }), m({ id: 2, display_title: "2012" })];
    expect(ids(a)).toEqual([1, 2]);
    const b = [m({ id: 1, display_title: "9" }), m({ id: 2, display_title: "10" })];
    expect(ids(b)).toEqual([2, 1]);
  });

  it("F7 Emoji 前缀不影响名称比较，且 emoji 版本与普通版本由兜底链稳定定序", () => {
    const list: SortableItem[] = [
      m({ id: 1, display_title: "🎬 The Matrix", year: 2000, added_at: 1 }),
      m({ id: 2, display_title: "The Matrix", year: 2000, added_at: 1 }),
      m({ id: 3, display_title: "Zebra", year: 2000, added_at: 1 }),
    ];
    const out = ids(list);
    expect(out.slice(2)).toEqual([3]);
    expect(out.slice(0, 2).sort()).toEqual([1, 2]);
    expect(ids(list)).toEqual(out);
  });

  it("F8 重音字母折叠到基础字母：`Éclair` 排在 `Fargo` 之前，且与 `eclair` 同键", () => {
    const list: SortableItem[] = [
      m({ id: 1, display_title: "Fargo", year: 2000, added_at: 1 }),
      m({ id: 2, display_title: "Éclair", year: 2000, added_at: 2 }),
    ];
    expect(ids(list)).toEqual([2, 1]);

    const same: SortableItem[] = [
      m({ id: 1, display_title: "Éclair", year: 2000, added_at: 1 }),
      m({ id: 2, display_title: "eclair", year: 2000, added_at: 1 }),
    ];
    // 主键相同 → 由 id 兜底定序，两个方向一致
    expect(ids(same, "name", "asc")).toEqual([1, 2]);
    expect(ids(same, "name", "desc")).toEqual([1, 2]);
  });

  it("F9 排序键 / 方向是脏值时回落默认（等价于 name / asc）", () => {
    const list: SortableItem[] = [
      m({ id: 1, display_title: "Beta", year: 2000, added_at: 1 }),
      m({ id: 2, display_title: "Alpha", year: 2000, added_at: 2 }),
    ];
    const bogus = sortMediaItems(list, "bogus" as By, "sideways" as Dir).map((i) => i.id);
    const nameAsc = sortMediaItems(list, "name", "asc").map((i) => i.id);
    expect(bogus).toEqual(nameAsc);
    expect(bogus).toEqual([2, 1]);
  });

  it("F10 sortMediaItems 不修改入参（入参与元素都冻结也不报错）", () => {
    const list: SortableItem[] = [
      m({ id: 3, display_title: "C", year: 2000, added_at: 3 }),
      m({ id: 1, display_title: "A", year: 2000, added_at: 1 }),
      m({ id: 2, type: "unknown", display_title: "B", year: 2000, added_at: 2 }),
    ];
    list.forEach((o) => Object.freeze(o));
    Object.freeze(list);
    const before = list.map((i) => i.id);

    expect(() => sortMediaItems(list, "name", "desc")).not.toThrow();
    expect(list.map((i) => i.id)).toEqual(before);
    expect(list[0].display_title).toBe("C");
  });

  it("F11 不传 han 参数也能自洽（内部现算拼音秩）", () => {
    const list: SortableItem[] = [
      m({ id: 1, display_title: "宙斯之子", year: 2014 }),
      m({ id: 2, display_title: "Avatar", year: 2009 }),
      m({ id: 3, display_title: "阿甘正传", year: 1994 }),
    ];
    const out = sortMediaItems(list, "name", "asc").map((i) => i.display_title);
    expect(out.indexOf("Avatar")).toBeLessThan(out.indexOf("阿甘正传"));
  });
});
