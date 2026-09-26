import { describe, expect, it } from "vitest";
import {
  categoryView, parseView, tagView, VIEW_ALL, VIEW_COLLECTIONS, VIEW_UNKNOWN,
  VIEW_UNCLASSIFIED,
} from "./types";

/**
 * 视图键的解析是左侧导航的枢纽：分类 / 标签的 id 来自数据库，
 * 一旦解析错了就会切到莫名其妙的栏目（或整个空白列表），所以单独锁住。
 */
describe("types: 视图键", () => {
  it("内置入口按原样解析", () => {
    expect(parseView("all")).toEqual({ kind: "builtin", key: "all" });
    expect(parseView("movie")).toEqual({ kind: "builtin", key: "movie" });
    expect(parseView("tv")).toEqual({ kind: "builtin", key: "tv" });
    expect(parseView("settings")).toEqual({ kind: "builtin", key: "settings" });
  });

  it("分类与标签视图带出 id", () => {
    expect(parseView(categoryView(7))).toEqual({ kind: "category", id: 7 });
    expect(parseView(tagView(12))).toEqual({ kind: "tag", id: 12 });
    expect(categoryView(7)).toBe("cat:7");
    expect(tagView(12)).toBe("tag:12");
  });

  it("未分类是独立的一类", () => {
    expect(parseView(VIEW_UNCLASSIFIED)).toEqual({ kind: "unclassified" });
  });

  /**
   * 类型判定加了「证据不足」这个出口，界面上必须有个地方能看到它们，
   * 否则这些条目就和老版本一样被无声地当成电影了。
   */
  it("待确认与系列合集都是内置入口", () => {
    expect(parseView(VIEW_UNKNOWN)).toEqual({ kind: "builtin", key: "unknown" });
    expect(parseView(VIEW_COLLECTIONS)).toEqual({ kind: "builtin", key: "collections" });
  });

  /** 认不出来的键不能让界面崩掉，回落到全部列表 */
  it("未知键回落到全部", () => {
    expect(parseView("does-not-exist")).toEqual({ kind: "builtin", key: "all" });
    expect(parseView("cat:abc")).toEqual({ kind: "builtin", key: "all" });
    expect(parseView("")).toEqual({ kind: "builtin", key: "all" });
  });

  it("分类 id 与内置键不会互相撞车", () => {
    expect(parseView(categoryView(0))).toEqual({ kind: "category", id: 0 });
    expect(categoryView(3)).not.toBe(VIEW_ALL);
  });
});
