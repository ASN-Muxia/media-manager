import { describe, expect, it } from "vitest";
import { imageUrl, tmdbTitle, tmdbYear } from "./commands";

/**
 * 这里只测**纯函数**：它们不依赖 Tauri 运行时，是命令层里最容易 regressions
 * 也最该被锁住的部分（拼图片地址、从搜索结果里取标题和年份）。
 */
describe("commands: 图片地址", () => {
  it("有路径时拼出完整的 TMDB 图片地址", () => {
    expect(imageUrl("/poster.jpg")).toBe("https://image.tmdb.org/t/p/w342/poster.jpg");
    expect(imageUrl("/back.jpg", "w1280")).toBe("https://image.tmdb.org/t/p/w1280/back.jpg");
  });

  it("空路径返回 null（界面据此显示占位块）", () => {
    expect(imageUrl(null)).toBeNull();
    expect(imageUrl(undefined)).toBeNull();
    expect(imageUrl("")).toBeNull();
  });

  /**
   * Fanart.tv / TheTVDB 补图写进来的是**完整 URL**，
   * 这里若不识别前缀就会拼出一个坏地址（`https://image.tmdb.org/...https://...`）。
   */
  it("完整 URL 原样返回（补图来源不是 TMDB）", () => {
    const abs = "https://assets.fanart.tv/fanart/movies/1/movieposter/x.jpg";
    expect(imageUrl(abs)).toBe(abs);
    expect(imageUrl(abs, "w1280")).toBe(abs);
    expect(imageUrl("http://a/b.jpg")).toBe("http://a/b.jpg");
  });
});

describe("commands: 搜索结果取值", () => {
  it("电影取 title，剧集取 name", () => {
    expect(tmdbTitle({ id: 1, title: "盗梦空间" })).toBe("盗梦空间");
    expect(tmdbTitle({ id: 2, name: "绝命毒师" })).toBe("绝命毒师");
  });

  it("两个都没有时给出占位，不返回空串", () => {
    expect(tmdbTitle({ id: 3 })).toBe("(无标题)");
    expect(tmdbTitle({ id: 4, title: "" })).toBe("(无标题)");
  });

  it("年份取 release_date / first_air_date 的前四位", () => {
    expect(tmdbYear({ id: 1, release_date: "2010-07-16" })).toBe(2010);
    expect(tmdbYear({ id: 2, first_air_date: "2008-01-20" })).toBe(2008);
  });

  it("日期缺失或不可解析时返回 null", () => {
    expect(tmdbYear({ id: 1 })).toBeNull();
    expect(tmdbYear({ id: 2, release_date: "" })).toBeNull();
    expect(tmdbYear({ id: 3, release_date: "未知" })).toBeNull();
  });
});
