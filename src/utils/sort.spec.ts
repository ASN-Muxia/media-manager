import { describe, expect, it } from "vitest";
import {
  buildHanOrder, buildNameKeys, compareItems, compareNameKey, hanCollatorUsable,
  identified, nameKeyOf, normalizeSortBy, normalizeSortDir, rawTitleOf,
  sortMediaItems, type HanOrder, type NameKey, type SortableItem,
} from "./sort";

/**
 * 排序内核的用例：只测纯函数，不牵扯 Pinia / Tauri。
 *
 * 每一个 `it` 都对应一条产品语义（数字最前、未识别沉底、倒序不反转分区…），
 * 语义一变这里就红——这是这套排序规则的**可执行说明文档**。
 */

/** 本运行时 ICU 是否提供 `zh` 拼音序（决定汉字相关用例的期望值）。 */
const HAN_OK = hanCollatorUsable();

function mk(over: Partial<SortableItem> = {}): SortableItem {
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

/** 一批条目按规则排完后取 id 序列（默认 name/asc）。 */
function idsOf(
  list: SortableItem[],
  by: Parameters<typeof sortMediaItems>[1] = "name",
  dir: Parameters<typeof sortMediaItems>[2] = "asc",
  han?: HanOrder,
): number[] {
  return sortMediaItems(list, by, dir, han).map((i) => i.id);
}

describe("sort: 归一化与取串", () => {
  it("非法 / 缺失的排序键与方向一律回落默认", () => {
    expect(normalizeSortBy("year")).toBe("year");
    expect(normalizeSortBy("added")).toBe("added");
    expect(normalizeSortBy("foo")).toBe("name");
    expect(normalizeSortBy("")).toBe("name");
    expect(normalizeSortBy(null)).toBe("name");
    expect(normalizeSortBy(undefined)).toBe("name");

    expect(normalizeSortDir("desc")).toBe("desc");
    expect(normalizeSortDir("DESC")).toBe("asc");
    expect(normalizeSortDir("")).toBe("asc");
    expect(normalizeSortDir(42)).toBe("asc");
  });

  it("取串优先级：display_title → title → original_title → file_name（去扩展名）", () => {
    // display_title 就是卡片上看到的字，它优先
    expect(rawTitleOf(mk({ display_title: "显示名", title: "库内名" }))).toBe("显示名");
    // 没有 display_title 才用 title
    expect(rawTitleOf(mk({ display_title: "  ", title: "库内名" }))).toBe("库内名");
    // 再退到 original_title
    expect(
      rawTitleOf(mk({ display_title: "", title: "", original_title: "The Matrix" })),
    ).toBe("The Matrix");
    // 最后才是 file_name，且去掉扩展名
    expect(
      rawTitleOf(mk({ display_title: "", title: "", file_name: "Inception.2010.mkv" })),
    ).toBe("Inception.2010");
    // 全空就是空串，不是 undefined
    expect(rawTitleOf(mk({ display_title: "", title: "   " }))).toBe("");
  });
});

describe("sort: 名称键", () => {
  it("类别序：数字 < 拉丁 < 汉字 < 其它文字", () => {
    const list = [
      mk({ id: 4, display_title: "Я русский" }),
      mk({ id: 3, display_title: "卧虎藏龙" }),
      mk({ id: 2, display_title: "Zebra" }),
      mk({ id: 1, display_title: "9 Souls" }),
    ];
    expect(idsOf(list)).toEqual([1, 2, 3, 4]);
  });

  it("中英混排样本：2012 → Avatar → 阿甘正传 → 宙斯之子", () => {
    const list = [
      mk({ id: 4, display_title: "宙斯之子" }),
      mk({ id: 2, display_title: "Avatar" }),
      mk({ id: 1, display_title: "2012" }),
      mk({ id: 3, display_title: "阿甘正传" }),
    ];
    const ids = idsOf(list);
    expect(ids.slice(0, 2)).toEqual([1, 2]);
    expect(ids.slice(2)).toEqual(HAN_OK ? [3, 4] : [4, 3]);
  });

  it("汉字顺序：拼音可用时按全拼，降级时回落码点（都不乱序）", () => {
    const list = [
      "战狼", "叶问", "白日梦想家", "阿甘正传", "卧虎藏龙",
    ].map((t, i) => mk({ id: i + 1, display_title: t }));
    const names = sortMediaItems(list, "name", "asc").map((i) => i.display_title);
    if (HAN_OK) {
      expect(names).toEqual(["阿甘正传", "白日梦想家", "卧虎藏龙", "叶问", "战狼"]);
    } else {
      // 码点序：叶(53F6) < 卧(5367) < 战(6218) < 白(767D) < 阿(963F)
      expect(names).toEqual(["叶问", "卧虎藏龙", "战狼", "白日梦想家", "阿甘正传"]);
    }
  });

  it("大小写不敏感、重音字母折叠", () => {
    const a = nameKeyOf(mk({ display_title: "Éclair" }), new Map());
    const b = nameKeyOf(mk({ display_title: "eclair" }), new Map());
    expect(compareNameKey(a, b)).toBe(0);

    // `É` 折成 `e` 后应排在 `f` 之前，而不是按原始码点掉到 `Z` 后面
    const list = [mk({ id: 1, display_title: "Fargo" }), mk({ id: 2, display_title: "Éclair" })];
    expect(idsOf(list)).toEqual([2, 1]);
  });

  it("标点 / 空白 / `·` / Emoji 不参与比较", () => {
    const plain = nameKeyOf(mk({ display_title: "The Matrix" }), new Map());
    const dotted = nameKeyOf(mk({ display_title: "The.Matrix" }), new Map());
    const spaced = nameKeyOf(mk({ display_title: "  The  Matrix  " }), new Map());
    const emoji = nameKeyOf(mk({ display_title: "🎬 The Matrix" }), new Map());
    const mid = nameKeyOf(mk({ display_title: "The·Matrix" }), new Map());

    expect(compareNameKey(plain, dotted)).toBe(0);
    expect(compareNameKey(plain, spaced)).toBe(0);
    expect(compareNameKey(plain, emoji)).toBe(0);
    expect(compareNameKey(plain, mid)).toBe(0);
  });

  it("数字按字符逐位比较：`10` 在 `9` 之前（不做数值解析）", () => {
    const list = [mk({ id: 1, display_title: "9" }), mk({ id: 2, display_title: "10" })];
    expect(idsOf(list)).toEqual([2, 1]);
  });

  it("前缀相同则短者在前", () => {
    const short = nameKeyOf(mk({ display_title: "ab" }), new Map());
    const long = nameKeyOf(mk({ display_title: "abc" }), new Map());
    expect(compareNameKey(short, long)).toBeLessThan(0);
    expect(compareNameKey(long, short)).toBeGreaterThan(0);
    expect(compareNameKey(short, short)).toBe(0);
  });

  it("拼音秩是现算的：没有汉字就是空表，降级时也是空表", () => {
    expect(buildHanOrder(["Avatar", "2012"]).size).toBe(0);
    if (HAN_OK) {
      const han = buildHanOrder(["阿", "白", "宙"]);
      expect(han.get("阿")! < han.get("白")!).toBe(true);
      expect(han.get("白")! < han.get("宙")!).toBe(true);
    }
  });

  it("汉字在空秩表下回落 Unicode 码点（= 降级路径）", () => {
    const empty: HanOrder = new Map();
    const ye = nameKeyOf(mk({ display_title: "叶问" }), empty);
    const a = nameKeyOf(mk({ display_title: "阿甘" }), empty);
    // 叶 U+53F6 < 阿 U+963F
    expect(compareNameKey(ye, a)).toBeLessThan(0);
  });
});

describe("sort: 分区（未识别沉底）", () => {
  it("identified 认三条：类型 ∈ {movie, tv} ∧ 标题非空 ∧ year 非 null", () => {
    expect(identified(mk({ type: "movie", year: 2000 }))).toBe(true);
    expect(identified(mk({ type: "tv", year: 2000 }))).toBe(true);
    // J1：unknown 与任何非预期取值都不算已识别
    expect(identified(mk({ type: "unknown", year: 2000 }))).toBe(false);
    expect(identified(mk({ type: "whatever", year: 2000 }))).toBe(false);
    // J2：连标题都没有（display_title 空且 title 空）
    expect(identified(mk({ display_title: "", title: "", year: 2000 }))).toBe(false);
    // display_title 为空但 title 有值 → 仍算有标题
    expect(identified(mk({ display_title: "", title: "X", year: 2000 }))).toBe(true);
    // J3：缺年份
    expect(identified(mk({ type: "movie", year: null }))).toBe(false);
  });

  it("未识别条目一律排在已识别之后", () => {
    const list = [
      mk({ id: 1, type: "unknown", display_title: "A", year: 2000 }),
      mk({ id: 2, type: "movie", display_title: "Z", year: 2000 }),
      mk({ id: 3, type: "movie", display_title: "Y", year: null }),
    ];
    expect(idsOf(list)).toEqual([2, 1, 3]);
  });

  it("倒序**不**反转分区：未识别仍在最末", () => {
    const list = [
      mk({ id: 1, type: "unknown", display_title: "A", year: 2000 }),
      mk({ id: 2, type: "movie", display_title: "B", year: 2000 }),
      mk({ id: 3, type: "movie", display_title: "C", year: 2000 }),
    ];
    const asc = idsOf(list, "name", "asc");
    const desc = idsOf(list, "name", "desc");
    expect(asc).toEqual([2, 3, 1]);
    expect(desc).toEqual([3, 2, 1]);
    // 两个方向下最后一条都是未识别那条
    expect(asc[asc.length - 1]).toBe(1);
    expect(desc[desc.length - 1]).toBe(1);
  });
});

describe("sort: 时间键", () => {
  const list = (): SortableItem[] => [
    mk({ id: 1, display_title: "B", year: 2000, added_at: 30 }),
    mk({ id: 2, display_title: "A", year: 2010, added_at: 10 }),
    mk({ id: 3, display_title: "C", year: 2010, added_at: 20 }),
    mk({ id: 4, display_title: "无年份", year: null, added_at: 5 }),
    mk({ id: 5, type: "unknown", display_title: "D", year: 1990, added_at: 40 }),
  ];

  it("上映年份：升序旧→新，同年按名称键", () => {
    expect(idsOf(list(), "year", "asc")).toEqual([1, 2, 3, 5, 4]);
  });

  it("上映年份：降序新→旧，同年仍按名称键升序（兜底链不翻转）", () => {
    expect(idsOf(list(), "year", "desc")).toEqual([2, 3, 1, 5, 4]);
  });

  it("缺年份的条目在**任何方向**都排分区末尾", () => {
    for (const dir of ["asc", "desc"] as const) {
      const ids = idsOf(list(), "year", dir);
      // id 4 缺年份 → 无论方向都是最后一条
      expect(ids[ids.length - 1]).toBe(4);
    }
  });

  it("添加时间：升序早→晚 / 降序晚→早", () => {
    const l = [
      mk({ id: 1, display_title: "A", year: 2000, added_at: 30 }),
      mk({ id: 2, display_title: "B", year: 2000, added_at: 10 }),
      mk({ id: 3, display_title: "C", year: 2000, added_at: 20 }),
    ];
    expect(idsOf(l, "added", "asc")).toEqual([2, 3, 1]);
    expect(idsOf(l, "added", "desc")).toEqual([1, 3, 2]);
  });
});

describe("sort: 全序与不可变性", () => {
  it("主键完全相同时由 id 定序，且正序 / 倒序结果一致", () => {
    const l = [
      mk({ id: 5, display_title: "Same", year: 2000, added_at: 1 }),
      mk({ id: 3, display_title: "Same", year: 2000, added_at: 1 }),
    ];
    expect(idsOf(l, "name", "asc")).toEqual([3, 5]);
    expect(idsOf(l, "name", "desc")).toEqual([3, 5]);
  });

  it("compareItems 是反对称的，且分区差异优先于一切", () => {
    const a = mk({ id: 1, type: "movie", display_title: "A", year: 2000 });
    const b = mk({ id: 2, type: "unknown", display_title: "A", year: 2000 });
    const han: HanOrder = new Map();
    const keys: Map<number, NameKey> = buildNameKeys([a, b], han);
    expect(compareItems(a, b, "name", "asc", han, keys)).toBeLessThan(0);
    expect(compareItems(b, a, "name", "asc", han, keys)).toBeGreaterThan(0);
    // 倒序也翻不过分区这一关
    expect(compareItems(a, b, "name", "desc", han, keys)).toBeLessThan(0);
  });

  it("sortMediaItems 不修改入参，且同样输入结果稳定", () => {
    const src = [
      mk({ id: 3, display_title: "C", year: 2000 }),
      mk({ id: 1, display_title: "A", year: 2000 }),
      mk({ id: 2, type: "unknown", display_title: "B", year: 2000 }),
    ];
    const snapshot = src.map((i) => ({ ...i }));
    const out = sortMediaItems(src, "name", "asc");

    expect(out).not.toBe(src);
    // 入参内容与顺序都没被动过
    expect(src).toEqual(snapshot);
    expect(src.map((i) => i.id)).toEqual([3, 1, 2]);
    // 再排一次结果一致（全序，不含随机性）
    expect(idsOf(src, "name", "asc")).toEqual(out.map((i) => i.id));
    expect(out.map((i) => i.id)).toEqual([1, 3, 2]);
  });
});
