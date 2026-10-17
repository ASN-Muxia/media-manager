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

  /**
   * 老库（升级上来的）没有 sortBy / sortDir 这两个键，
   * 不能把 undefined 传到排序逻辑里——必须回落 name / asc。
   */
  it("排序设置缺失时回落 name / asc", async () => {
    seed({ sortBy: undefined, sortDir: undefined });
    const s = useSettingsStore();
    await s.load();
    expect(s.sortBy).toBe("name");
    expect(s.sortDir).toBe("asc");
  });

  /** 脏值（拼错 / 空串）一律回落，绝不流进比较器。 */
  it("排序设置为脏值时回落默认", async () => {
    seed({ sortBy: "foo", sortDir: "sideways" });
    const s = useSettingsStore();
    await s.load();
    expect(s.sortBy).toBe("name");
    expect(s.sortDir).toBe("asc");
  });

  it("合法排序设置原样读出", async () => {
    seed({ sortBy: "year", sortDir: "desc" });
    const s = useSettingsStore();
    await s.load();
    expect(s.sortBy).toBe("year");
    expect(s.sortDir).toBe("desc");
  });

  /** 落库内容必须带上这两个键，且写之前已归一化（脏值不进库）。 */
  it("保存时带上排序设置，脏值按默认落库", async () => {
    seed();
    const s = useSettingsStore();
    await s.load();
    s.sortBy = "added";
    s.sortDir = "desc";

    await s.save();
    const payload = mocks.setSettings.mock.calls[0][0];
    expect(payload.sortBy).toBe("added");
    expect(payload.sortDir).toBe("desc");

    // 直接把脏值塞进状态（绕过类型）也要在写库时被洗掉：脏值不落库
    s.sortBy = "nonsense" as unknown as typeof s.sortBy;
    await s.save();
    expect(mocks.setSettings.mock.calls[1][0].sortBy).toBe("name");
    expect(mocks.setSettings.mock.calls[1][0].sortDir).toBe("desc");
  });

  it("保存失败会抛出并把错误留在状态里", async () => {
    mocks.setSettings.mockRejectedValue(new Error("boom"));
    const s = useSettingsStore();
    await expect(s.save()).rejects.toThrow("boom");
    expect(s.error).toContain("boom");
    expect(s.saving).toBe(false);
  });
});

describe("settingsStore: 手工排序过的聚合", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    mocks.setSettings.mockResolvedValue(undefined);
  });

  /** 老库里没有这个键（字段是后加的）→ 空名单 = 没有一组是手工排过的。 */
  it("缺失时回落空数组", async () => {
    seed({ manualOrderGroups: undefined });
    const s = useSettingsStore();
    await s.load();
    expect(s.manualOrderGroups).toEqual([]);
  });

  /** 读脏值（不是数组 / 混了非整数 / 非正数）一律回落，绝不把脏 id 放进名单。 */
  it("脏值回落空数组或只留合法 id", async () => {
    seed({ manualOrderGroups: "14135" });
    let s = useSettingsStore();
    await s.load();
    expect(s.manualOrderGroups).toEqual([]);

    // 混了非整数 / 0 / 负数：只留正整数（否则某个分组会永远走不到「跟随顶栏」分支）
    setActivePinia(createPinia());
    seed({ manualOrderGroups: [14135, "x", null, 0, -3, 14150.5, 7] });
    s = useSettingsStore();
    await s.load();
    expect(s.manualOrderGroups).toEqual([14135, 7]);
  });

  /** roundtrip：读出来的名单要能原样写回后端。 */
  it("读出后保存，名单原样写回", async () => {
    seed({ manualOrderGroups: [14135, 14150] });
    const s = useSettingsStore();
    await s.load();
    await s.save();
    expect(mocks.setSettings.mock.calls[0][0].manualOrderGroups).toEqual([14135, 14150]);
  });

  /** markManualOrder 去重 + 立刻写库（这正是「点了 ↑↓ 之后重启还在」的前提）。 */
  it("markManualOrder 去重并写库", async () => {
    seed();
    const s = useSettingsStore();
    await s.load();

    s.markManualOrder(14135);
    s.markManualOrder(14135); // 重复标记不该产生第二份
    s.markManualOrder(14150);
    await Promise.resolve(); // save() 是内部发起的，等它落地
    await Promise.resolve();

    expect(s.manualOrderGroups).toEqual([14135, 14150]);
    expect(mocks.setSettings).toHaveBeenCalled();
    const calls = mocks.setSettings.mock.calls;
    const last = calls[calls.length - 1][0];
    expect(last.manualOrderGroups).toEqual([14135, 14150]);
  });

  /** clearManualOrder 只移出名单并写库，不动其它设置。 */
  it("clearManualOrder 移出名单并写库", async () => {
    seed({ manualOrderGroups: [14135, 14150], tmdbKey: "keep-me" });
    const s = useSettingsStore();
    await s.load();

    s.clearManualOrder(14135);
    await Promise.resolve();
    await Promise.resolve();

    expect(s.manualOrderGroups).toEqual([14150]);
    const calls = mocks.setSettings.mock.calls;
    const last = calls[calls.length - 1][0];
    expect(last.manualOrderGroups).toEqual([14150]);
    expect(last.tmdbKey).toBe("keep-me"); // 没有顺手把别的设置清掉
  });

  /**
   * ⚠️ 核心保护：**未加载完就不写库**。
   *
   * 这时 `manualOrderGroups` 还是模块默认的空数组，一旦 save() 出去，就会拿默认值
   * 把用户库里已有的配置（包括这个名单本身的其它成员）一起覆盖掉。与 App 的
   * `persistSort` 是同一条约定。标记本身仍然生效（内存里改掉即可）。
   */
  it("loaded=false 时不写库（标记仍生效）", async () => {
    seed({ manualOrderGroups: [14135] });
    const s = useSettingsStore();
    expect(s.loaded).toBe(false);

    s.markManualOrder(14150);
    await Promise.resolve();
    expect(mocks.setSettings).not.toHaveBeenCalled();
    expect(s.manualOrderGroups).toContain(14150);

    s.clearManualOrder(14135);
    await Promise.resolve();
    expect(mocks.setSettings).not.toHaveBeenCalled();
    expect(s.manualOrderGroups).toEqual([14150]);
  });
});

describe("settingsStore: 数据源开关", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    mocks.setSettings.mockResolvedValue(undefined);
  });

  /** 老库里没有这一项，行为必须与升级前一致（全部启用）。 */
  it("缺失时按「全部启用」处理，不把 undefined 当空数组之外的东西", async () => {
    seed({ disabledProviders: undefined });
    const s = useSettingsStore();
    await s.load();

    expect(s.disabledProviders).toEqual([]);
    expect(s.isProviderOn("tmdb")).toBe(true);
    expect(s.isProviderOn("douban")).toBe(true);
  });

  it("切换开关只是改名单，不动顺序也不删 Key", async () => {
    seed({ tmdbKey: "keep-me", disabledProviders: ["douban"] });
    const s = useSettingsStore();
    await s.load();

    expect(s.isProviderOn("douban")).toBe(false);
    s.toggleProvider("douban");
    expect(s.isProviderOn("douban")).toBe(true);

    s.toggleProvider("tvdb");
    expect(s.disabledProviders).toEqual(["tvdb"]);
    // Key 一个都没被动过
    expect(s.tmdbKey).toBe("keep-me");
  });

  it("保存时把名单原样带回后端（清洗由后端统一做）", async () => {
    seed();
    const s = useSettingsStore();
    await s.load();
    s.toggleProvider("fanart");
    s.toggleProvider("omdb");

    await s.save();
    expect(mocks.setSettings.mock.calls[0][0].disabledProviders).toEqual(["fanart", "omdb"]);
  });
});

describe("settingsStore: 折叠的分组（封面墙展开态记忆）", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    seed();
    mocks.setSettings.mockResolvedValue(undefined);
  });

  /** 老库没有这个键 → 空名单 = 没有任何分组被折叠过 → 全部按默认展开。 */
  it("缺失时回落空数组", async () => {
    seed({ collapsedGroups: undefined });
    const s = useSettingsStore();
    await s.load();
    expect(s.collapsedGroups).toEqual([]);
  });

  /** 脏值：不是数组、混了非整数 / 非正数 —— 只留正整数。 */
  it("脏值只留正整数", async () => {
    seed({ collapsedGroups: "14135" });
    let s = useSettingsStore();
    await s.load();
    expect(s.collapsedGroups).toEqual([]);

    setActivePinia(createPinia());
    seed({ collapsedGroups: [14135, null, -3, 0, 2.5, "x", 14150] });
    s = useSettingsStore();
    await s.load();
    expect(s.collapsedGroups).toEqual([14135, 14150]);
  });

  it("load → save roundtrip：名单原样带回", async () => {
    seed({ collapsedGroups: [14135, 14150] });
    const s = useSettingsStore();
    await s.load();
    expect(s.collapsedGroups).toEqual([14135, 14150]);

    await s.save();
    expect(mocks.setSettings.mock.calls[0][0].collapsedGroups).toEqual([14135, 14150]);
  });

  it("setCollapsedGroups 落库并去重", async () => {
    const s = useSettingsStore();
    await s.load();

    s.setCollapsedGroups([14135, 14135, 14150]);
    await Promise.resolve();
    await Promise.resolve();

    expect(s.collapsedGroups).toEqual([14135, 14150]);
    const calls = mocks.setSettings.mock.calls;
    expect(calls[calls.length - 1][0].collapsedGroups).toEqual([14135, 14150]);
  });

  /**
   * 列表刷新会频繁上报，值没变时**一个请求都不发** —— 白写库既费 IO，
   * 又会把用户的其它修改挤进无意义的保存里。
   */
  it("名单没变化时不写库", async () => {
    seed({ collapsedGroups: [14135] });
    const s = useSettingsStore();
    await s.load();
    mocks.setSettings.mockClear();

    s.setCollapsedGroups([14135]);
    await Promise.resolve();
    expect(mocks.setSettings).not.toHaveBeenCalled();
  });

  it("写入前清洗：非正整数与重复都不落库", async () => {
    const s = useSettingsStore();
    await s.load();
    s.setCollapsedGroups([14135, -1, 0, 2.5, 14135]);
    await Promise.resolve();
    await Promise.resolve();
    expect(mocks.setSettings.mock.calls[mocks.setSettings.mock.calls.length - 1][0].collapsedGroups).toEqual([14135]);
  });

  /**
   * ⚠️ 与 markManualOrder 同一条约定：**未加载完不写库**。
   * 那时 collapsedGroups 还是默认空数组，save() 会把用户库里已存的折叠名单
   * （以及其它所有设置）用默认值覆盖掉。内存里仍要生效，否则用户这次折叠看起来"没反应"。
   */
  it("loaded=false 时不写库（内存仍生效）", async () => {
    seed({ collapsedGroups: [14135] });
    const s = useSettingsStore();
    expect(s.loaded).toBe(false);

    s.setCollapsedGroups([14150]);
    await Promise.resolve();
    expect(mocks.setSettings).not.toHaveBeenCalled();
    expect(s.collapsedGroups).toEqual([14150]);
  });

  it("保存时不顺手改掉别的设置", async () => {
    seed({ collapsedGroups: [14135], tmdbKey: "keep-me" });
    const s = useSettingsStore();
    await s.load();
    s.setCollapsedGroups([14150]);
    await Promise.resolve();
    await Promise.resolve();
    const calls = mocks.setSettings.mock.calls;
    expect(calls[calls.length - 1][0].tmdbKey).toBe("keep-me");
  });
});
