import { beforeEach, describe, expect, it, vi } from "vitest";
import { createPinia, setActivePinia } from "pinia";
import { useSettingsStore } from "./settingsStore";

/**
 * 设置 store 的所有 IO 都走 `../api/commands`，这里整体 mock 掉，
 * 只验证「读到的值怎么落到状态里」和「存回去时有没有被清洗」。
 */
const mocks = vi.hoisted(() => ({
  getSettings: vi.fn(),
  setSettings: vi.fn(),
}));

vi.mock("../api/commands", () => mocks);

function seed(over: Partial<Record<string, unknown>> = {}): void {
  mocks.getSettings.mockResolvedValue({
    tmdbKey: "key-1",
    mediaDirs: ["D:\\Movies"],
    potPlayerPath: "C:\\PotPlayer\\PotPlayerMini64.exe",
    hosts: ["1.2.3.4 api.themoviedb.org"],
    tmdbLanguage: "zh-CN",
    ...over,
  });
}

describe("settingsStore", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    seed();
    mocks.setSettings.mockResolvedValue(undefined);
  });

  it("加载后各字段落到状态里", async () => {
    const s = useSettingsStore();
    await s.load();

    expect(s.tmdbKey).toBe("key-1");
    expect(s.mediaDirs).toEqual(["D:\\Movies"]);
    expect(s.hostsText).toBe("1.2.3.4 api.themoviedb.org");
    expect(s.tmdbLanguage).toBe("zh-CN");
    expect(s.loaded).toBe(true);
  });

  /** 老库里没有语言设置（字段是后加的），不能把 undefined 显示到下拉框上。 */
  it("语言缺失时回落 zh-CN", async () => {
    seed({ tmdbLanguage: undefined });
    const s = useSettingsStore();
    await s.load();
    expect(s.tmdbLanguage).toBe("zh-CN");
  });

  it("hosts 文本框按行解析：去空行、去首尾空格、兼容 CRLF", () => {
    const s = useSettingsStore();
    s.hostsText = "1.1.1.1 a.com\r\n\r\n  \n 2.2.2.2   b.com  \n# 注释";
    expect(s.hostsLines()).toEqual(["1.1.1.1 a.com", "2.2.2.2   b.com", "# 注释"]);
  });

  it("保存时把 hosts 文本转成数组并带上语言", async () => {
    const s = useSettingsStore();
    await s.load();
    s.hostsText = "1.1.1.1 a.com\n2.2.2.2 b.com";
    s.tmdbLanguage = "en-US";

    await s.save();
    const payload = mocks.setSettings.mock.calls[0][0];
    expect(payload.hosts).toEqual(["1.1.1.1 a.com", "2.2.2.2 b.com"]);
    expect(payload.tmdbLanguage).toBe("en-US");
    expect(payload.mediaDirs).toEqual(["D:\\Movies"]);
  });

  /** 语言被清成空白时不能把空串发给后端，否则 TMDB 会返回错误。 */
  it("语言为空白时按 zh-CN 保存", async () => {
    const s = useSettingsStore();
    await s.load();
    s.tmdbLanguage = "   ";

    await s.save();
    expect(mocks.setSettings.mock.calls[0][0].tmdbLanguage).toBe("zh-CN");
  });

  it("添加目录去重，移除按下标", () => {
    const s = useSettingsStore();
    s.addDir("D:\\TV");
    s.addDir("D:\\TV");
    s.addDir("  ");
    expect(s.mediaDirs).toEqual(["D:\\TV"]);

    s.addDir("E:\\TV");
    s.removeDir(0);
    expect(s.mediaDirs).toEqual(["E:\\TV"]);
  });

  it("保存失败会抛出并把错误留在状态里", async () => {
    mocks.setSettings.mockRejectedValue(new Error("boom"));
    const s = useSettingsStore();
    await expect(s.save()).rejects.toThrow("boom");
    expect(s.error).toContain("boom");
    expect(s.saving).toBe(false);
  });
});
