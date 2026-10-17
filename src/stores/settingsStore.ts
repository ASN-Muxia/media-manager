import { defineStore } from "pinia";
import { ref } from "vue";
import { getSettings, setSettings, type LlmPreset } from "../api/commands";
import { DEFAULT_SHORTCUTS } from "../utils/shortcuts";
import {
  DEFAULT_SORT_BY, DEFAULT_SORT_DIR, normalizeSortBy, normalizeSortDir,
  type SortBy, type SortDir,
} from "../utils/sort";

/**
 * 设置 store：启动时从 SQLite 读取，修改后调用 `set_settings` 落库。
 */
export const useSettingsStore = defineStore("settings", () => {
  const tmdbKey = ref("");
  const mediaDirs = ref<string[]>([]);
  const potPlayerPath = ref("");
  /** 自定义 hosts 映射，界面上一个文本框一行一条。 */
  const hostsText = ref("");
  /** 元数据语言（TMDB 的 language 参数）。 */
  const tmdbLanguage = ref("zh-CN");

  /* ---------------- 可选元数据 provider ---------------- */
  /** TheTVDB：剧集的季 / 集结构（TMDb 缺集信息时用）。 */
  const tvdbKey = ref("");
  /** Fanart.tv：TMDb 缺图时的补充图源。 */
  const fanartKey = ref("");
  /** OMDb：IMDb 评分补充。 */
  const omdbKey = ref("");
  /** 豆瓣：中文元数据兜底（非官方接口，默认关闭）。 */
  const doubanEnabled = ref(false);
  /** 豆瓣 Cookie（可选）：带上登录态能降低被要求验证码的概率。 */
  const doubanCookie = ref("");
  /** 扫描 / 导入完成后自动刮削（默认关闭）。 */
  const autoScrapeAfterScan = ref(false);

  /* ---------------- 大模型（可选增强层，默认关闭） ---------------- */
  const llmEnabled = ref(false);
  const llmBaseUrl = ref("");
  const llmApiKey = ref("");
  const llmModel = ref("");
  const llmVisionModel = ref("");
  const llmVisionBaseUrl = ref("");
  /**
   * 被关掉的数据源 id（`["douban"]` 这种）。
   *
   * 与「Key 留空」是两件事：留空 = 没配置，这里是「配了但选择不用」。
   * 关掉一个源不会让后面的源提前——权威顺序是写死的。
   */
  const disabledProviders = ref<string[]>([]);

  /* ---------------- 手动匹配的筛选偏好 ---------------- */
  /** 模糊匹配（关掉就只剩「完全一致」与「包含」）。 */
  const matchFuzzy = ref(true);
  /** 年份容差：0 = 必须同一年，1 = 前后一年也算命中。 */
  const matchYearTolerance = ref(1);
  /** 排除词：候选标题里出现任一个就整条剔除（逗号 / 换行分隔）。 */
  const matchBlacklist = ref("");
  /** 一次最多拉多少条候选。 */
  const matchLimit = ref(60);

  /* ---------------- 快捷键（留空 = 不绑定） ---------------- */
  const shortcutSelectAll = ref(DEFAULT_SHORTCUTS.selectAll);
  const shortcutInvert = ref(DEFAULT_SHORTCUTS.invert);
  const shortcutConfirm = ref(DEFAULT_SHORTCUTS.confirm);
  const shortcutClear = ref(DEFAULT_SHORTCUTS.clear);

  /* ---------------- 列表排序（展示层偏好） ---------------- */
  /**
   * 海报墙的排序键：`name` 名称（拼音）/ `year` 上映·首播年份 / `added` 添加时间。
   *
   * 排序**只改内存里的展示顺序**（`utils/sort.ts`），不写媒体行、不碰文件；
   * 这里存的是用户的选择，重启后还在。
   */
  const sortBy = ref<SortBy>(DEFAULT_SORT_BY);
  /** 排序方向：`asc` 正序 / `desc` 倒序。 */
  const sortDir = ref<SortDir>(DEFAULT_SORT_DIR);

  /* ---------------- 已被用户手工排过序的聚合 ---------------- */
  /**
   * 用户**亲手**调过成员次序的聚合（分组）id 列表。
   *
   * 存在的理由（本项目踩过一次）：聚合内的默认次序本该跟随顶栏排序，
   * 但「这一组被手工排过吗」无法用 `media.sort_order` 反推 —— 任何一次
   * `move_nodes_sync`（含在封面墙上拖动聚合）都会把目标同级整组重编号成
   * `0..n-1`，真实库里连没动过的组都是 `0,1,2,…`。「存在 > 0 的值」这个
   * 旧判据于是恒为真，顶栏排序永远不生效。所以必须显式记一个标记。
   *
   * 列表里的分组 → 按库内 `sort_order` 显示；不在列表里 → 跟随顶栏排序偏好。
   */
  const manualOrderGroups = ref<number[]>([]);

  /**
   * 被用户**亲手折叠**过的分组 id（封面墙的展开态记忆）。
   *
   * 封面墙挂在 `App.vue` 的 `v-if/v-else` 链上，点开详情 / 设置页会**卸载**它，
   * 组件内的 `expanded` / `collapsedByUser` 随之销毁。不把折叠态提到这里，
   * 返回主页时组件重建、「新分组自动展开」的 watch 就会把一切重新摊开 ——
   * 用户看到的是「我上次明明收起来了，回来又全开了」。
   *
   * ⚠️ 只记**折叠**，不记展开：展开的分组随刷新不断新增，存它等于存一份只增
   * 不减、越删越脏的名单；折叠是少数派，语义也正是「否决了自动展开」。
   */
  const collapsedGroups = ref<number[]>([]);

  /** 界面上可选的语言（值即 TMDB 的 language 参数）。 */
  const languageOptions = [
    { value: "zh-CN", label: "简体中文" },
    { value: "zh-TW", label: "繁體中文" },
    { value: "en-US", label: "English" },
    { value: "ja-JP", label: "日本語" },
    { value: "ko-KR", label: "한국어" },
  ];

  const loaded = ref(false);
  const saving = ref(false);
  const error = ref<string | null>(null);

  /** 数字型设置：非法值 / 越界都回落到默认，绝不把脏值写进库。 */
  function clampInt(v: unknown, min: number, max: number, fallback: number): number {
    const n = typeof v === "number" ? v : Number(v);
    if (!Number.isFinite(n)) return fallback;
    return Math.min(max, Math.max(min, Math.round(n)));
  }

  /** 文本框里的行 → 设置里的数组（去空行、去首尾空格）。 */
  function hostsLines(): string[] {
    return hostsText.value
      .split(/\r?\n/)
      .map((l) => l.trim())
      .filter((l) => l.length > 0);
  }

  /** 从数据库加载全部设置。 */
  async function load(): Promise<void> {
    error.value = null;
    try {
      const s = await getSettings();
      tmdbKey.value = s.tmdbKey ?? "";
      mediaDirs.value = Array.isArray(s.mediaDirs) ? [...s.mediaDirs] : [];
      potPlayerPath.value = s.potPlayerPath ?? "";
      hostsText.value = Array.isArray(s.hosts) ? s.hosts.join("\n") : "";
      tmdbLanguage.value = s.tmdbLanguage?.trim() || "zh-CN";
      // 后加的可选项在老库里没有值，一律按「未配置」处理，不能崩
      tvdbKey.value = s.tvdbKey ?? "";
      fanartKey.value = s.fanartKey ?? "";
      omdbKey.value = s.omdbKey ?? "";
      doubanEnabled.value = Boolean(s.doubanEnabled);
      doubanCookie.value = s.doubanCookie ?? "";
      autoScrapeAfterScan.value = Boolean(s.autoScrapeAfterScan);
      llmEnabled.value = Boolean(s.llmEnabled);
      llmBaseUrl.value = s.llmBaseUrl ?? "";
      llmApiKey.value = s.llmApiKey ?? "";
      llmModel.value = s.llmModel ?? "";
      llmVisionModel.value = s.llmVisionModel ?? "";
      llmVisionBaseUrl.value = s.llmVisionBaseUrl ?? "";
      // 老库里没有这一项 → 空数组 = 全部启用，行为与升级前一致
      disabledProviders.value = Array.isArray(s.disabledProviders)
        ? [...s.disabledProviders]
        : [];
      // 老库没有这几项 → 回落默认，行为与升级前一致
      matchFuzzy.value = s.matchFuzzy !== false;
      matchYearTolerance.value = clampInt(s.matchYearTolerance, 0, 10, 1);
      matchBlacklist.value = s.matchBlacklist ?? "";
      matchLimit.value = clampInt(s.matchLimit, 1, 200, 60);
      shortcutSelectAll.value = s.shortcutSelectAll?.trim() || DEFAULT_SHORTCUTS.selectAll;
      shortcutInvert.value = s.shortcutInvert?.trim() || DEFAULT_SHORTCUTS.invert;
      shortcutConfirm.value = s.shortcutConfirm?.trim() || DEFAULT_SHORTCUTS.confirm;
      shortcutClear.value = s.shortcutClear?.trim() || DEFAULT_SHORTCUTS.clear;
      // 老库没有这两项 / 值是脏的 → 回落 name / asc（前后端各归一一次，双保险）
      sortBy.value = normalizeSortBy(s.sortBy);
      sortDir.value = normalizeSortDir(s.sortDir);
      // 老库没有这个键（→ undefined）/ 脏值（不是数组、混了非整数）一律回落空数组；
      // 后端读的时候也洗了一遍，这里再洗是第二道保险 —— 脏 id 混进来只会
      // 让某个分组永远走不到「跟随顶栏排序」那条分支。
      manualOrderGroups.value = Array.isArray(s.manualOrderGroups)
        ? s.manualOrderGroups.filter((id) => Number.isInteger(id) && id > 0)
        : [];
      // 同一口径。老库没有这个键（→ undefined）→ 空数组 = 全部默认展开。
      collapsedGroups.value = Array.isArray(s.collapsedGroups)
        ? s.collapsedGroups.filter((id) => Number.isInteger(id) && id > 0)
        : [];
      loaded.value = true;
    } catch (e) {
      error.value = String(e);
    }
  }

  /** 将当前设置写回数据库。 */
  async function save(): Promise<void> {
    saving.value = true;
    error.value = null;
    try {
      await setSettings({
        tmdbKey: tmdbKey.value.trim(),
        mediaDirs: [...mediaDirs.value],
        potPlayerPath: potPlayerPath.value.trim(),
        hosts: hostsLines(),
        tmdbLanguage: tmdbLanguage.value.trim() || "zh-CN",
        tvdbKey: tvdbKey.value.trim(),
        fanartKey: fanartKey.value.trim(),
        omdbKey: omdbKey.value.trim(),
        doubanEnabled: doubanEnabled.value,
        doubanCookie: doubanCookie.value.trim(),
        autoScrapeAfterScan: autoScrapeAfterScan.value,
        llmEnabled: llmEnabled.value,
        llmBaseUrl: llmBaseUrl.value.trim(),
        llmApiKey: llmApiKey.value.trim(),
        llmModel: llmModel.value.trim(),
        llmVisionModel: llmVisionModel.value.trim(),
        llmVisionBaseUrl: llmVisionBaseUrl.value.trim(),
        disabledProviders: [...disabledProviders.value],
        matchFuzzy: matchFuzzy.value,
        matchYearTolerance: clampInt(matchYearTolerance.value, 0, 10, 1),
        matchBlacklist: matchBlacklist.value.trim(),
        matchLimit: clampInt(matchLimit.value, 1, 200, 60),
        shortcutSelectAll: shortcutSelectAll.value.trim(),
        shortcutInvert: shortcutInvert.value.trim(),
        shortcutConfirm: shortcutConfirm.value.trim(),
        shortcutClear: shortcutClear.value.trim(),
        // 写库前再归一一次：脏值不落库（与后端 normalize_sort 是同一套白名单）
        sortBy: normalizeSortBy(sortBy.value),
        sortDir: normalizeSortDir(sortDir.value),
        // 同样在写库前再洗一次：重复 id 与非法值都不落库
        manualOrderGroups: normalizeIdList(manualOrderGroups.value),
    collapsedGroups: normalizeIdList(collapsedGroups.value),
      });
      loaded.value = true;
    } catch (e) {
      error.value = String(e);
      throw e;
    } finally {
      saving.value = false;
    }
  }

  /**
   * 两份 id 名单（手工排序过的聚合 / 被折叠的分组）的统一清洗：
   * 只留正整数、去重（顺序保持插入序）。
   *
   * 两条名单的口径**必须一样**：都是「media 行的 id 集合」，脏值混进去只会让某个
   * 分组永远走不到另一条分支（比如带小数的 id 永远匹配不上任何条目）。
   */
  function normalizeIdList(ids: number[]): number[] {
    return [...new Set(ids.filter((id) => Number.isInteger(id) && id > 0))];
  }

  function addDir(dir: string): void {
    const d = dir.trim();
    if (d && !mediaDirs.value.includes(d)) mediaDirs.value.push(d);
  }

  function removeDir(index: number): void {
    mediaDirs.value.splice(index, 1);
  }

  /** 一键套用大模型接入预设（省得用户手抄地址与模型名）。 */
  function applyLlmPreset(p: LlmPreset): void {
    if (p.baseUrl) llmBaseUrl.value = p.baseUrl;
    if (p.model) llmModel.value = p.model;
    if (p.visionModel) llmVisionModel.value = p.visionModel;
    if (p.visionBaseUrl) llmVisionBaseUrl.value = p.visionBaseUrl;
  }

  /**
   * 把某个聚合记成「已被用户手工排过序」。
   *
   * ⚠️ 调用时机必须是**默认次序已经落库之后**（见 `MediaDetail.ensureManualOrder`）：
   * 反过来先标记，显示会立刻从顶栏序跳回旧的库内序，看起来就是「点了一下顺序乱了」。
   *
   * ⚠️ 未加载完（`loaded === false`）时**不写库**：那时 `manualOrderGroups` 还是默认
   * 空数组，`save()` 会把用户的其它设置一起用默认值覆盖掉。与 `App.persistSort`
   * 是同一条约定。标记本身仍然生效（内存里改掉即可）。
   */
  function markManualOrder(groupId: number): void {
    if (!Number.isInteger(groupId) || groupId <= 0) return;
    if (!manualOrderGroups.value.includes(groupId)) {
      manualOrderGroups.value = [...manualOrderGroups.value, groupId];
    }
    if (loaded.value) void save().catch(() => undefined);
  }

  /**
   * 取消「手工排序」标记，回到「跟随顶栏排序设置显示」。
   *
   * **只清标记**：一个 media 行都不改（`sort_order` 原样留着）。这样用户再手工
   * 排一次时，落库那一步仍然以「当前看得见的顺序」为准，不会凭空跳变。
   */
  function clearManualOrder(groupId: number): void {
    if (!manualOrderGroups.value.includes(groupId)) return;
    manualOrderGroups.value = manualOrderGroups.value.filter((id) => id !== groupId);
    if (loaded.value) void save().catch(() => undefined);
  }

  /**
   * 记下封面墙当前的**折叠**分组集合（整体替换，不是增量）。
   *
   * 为什么是整体替换而不是 `collapse(id)` / `expand(id)` 两个增量操作：封面墙那边的
   * 真相来源是 `expanded` 这个 Set（点一下、一键展开、一键收起都会动它），
   * 让每次变化后把「哪些是折叠的」整体交上来，省掉两套状态互相对齐的麻烦，
   * 也天然处理了「一键展开后折叠名单应清空」这种情况。
   *
   * ⚠️ 未加载完（`loaded === false`）时**不写库**，与 `markManualOrder` 同一条约定：
   * 那时 `collapsedGroups` 还是默认空数组，`save()` 会把用户其它设置一起用默认值覆盖。
   * 内存里照旧生效。
   */
  function setCollapsedGroups(ids: number[]): void {
    const next = normalizeIdList(ids);
    const cur = collapsedGroups.value;
    // 相同就一个请求都不发：列表刷新会频繁触发，白写库既费 IO 又白白压掉撤销栈
    if (next.length === cur.length && next.every((id, i) => id === cur[i])) return;
    collapsedGroups.value = next;
    if (loaded.value) void save().catch(() => undefined);
  }

  /**
   * 切换某个数据源的开关。
   *
   * 只改名单，不改顺序——TMDb 永远最权威，关掉它只会让豆瓣接管，
   * 不会让 TheTVDB 变成主源。
   */
  function toggleProvider(id: string): void {
    const i = disabledProviders.value.indexOf(id);
    if (i >= 0) disabledProviders.value.splice(i, 1);
    else disabledProviders.value.push(id);
  }

  function isProviderOn(id: string): boolean {
    return !disabledProviders.value.includes(id);
  }

  return {
    tmdbKey, mediaDirs, potPlayerPath, hostsText, tmdbLanguage, languageOptions,
    tvdbKey, fanartKey, omdbKey,
    doubanEnabled, doubanCookie, autoScrapeAfterScan,
    llmEnabled, llmBaseUrl, llmApiKey, llmModel, llmVisionModel, llmVisionBaseUrl,
    disabledProviders,
    matchFuzzy, matchYearTolerance, matchBlacklist, matchLimit,
    shortcutSelectAll, shortcutInvert, shortcutConfirm, shortcutClear,
    sortBy, sortDir,
    manualOrderGroups,
    collapsedGroups,
    loaded, saving, error,
    load, save, addDir, removeDir, hostsLines, applyLlmPreset,
    toggleProvider, isProviderOn,
    markManualOrder, clearManualOrder,
    setCollapsedGroups,
  };
});
