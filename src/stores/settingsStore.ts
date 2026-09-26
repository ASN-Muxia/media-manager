import { defineStore } from "pinia";
import { ref } from "vue";
import { getSettings, setSettings, type LlmPreset } from "../api/commands";

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

  /* ---------------- 大模型（可选增强层，默认关闭） ---------------- */
  const llmEnabled = ref(false);
  const llmBaseUrl = ref("");
  const llmApiKey = ref("");
  const llmModel = ref("");
  const llmVisionModel = ref("");
  const llmVisionBaseUrl = ref("");

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
      llmEnabled.value = Boolean(s.llmEnabled);
      llmBaseUrl.value = s.llmBaseUrl ?? "";
      llmApiKey.value = s.llmApiKey ?? "";
      llmModel.value = s.llmModel ?? "";
      llmVisionModel.value = s.llmVisionModel ?? "";
      llmVisionBaseUrl.value = s.llmVisionBaseUrl ?? "";
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
        llmEnabled: llmEnabled.value,
        llmBaseUrl: llmBaseUrl.value.trim(),
        llmApiKey: llmApiKey.value.trim(),
        llmModel: llmModel.value.trim(),
        llmVisionModel: llmVisionModel.value.trim(),
        llmVisionBaseUrl: llmVisionBaseUrl.value.trim(),
      });
      loaded.value = true;
    } catch (e) {
      error.value = String(e);
      throw e;
    } finally {
      saving.value = false;
    }
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

  return {
    tmdbKey, mediaDirs, potPlayerPath, hostsText, tmdbLanguage, languageOptions,
    tvdbKey, fanartKey, omdbKey,
    llmEnabled, llmBaseUrl, llmApiKey, llmModel, llmVisionModel, llmVisionBaseUrl,
    loaded, saving, error,
    load, save, addDir, removeDir, hostsLines, applyLlmPreset,
  };
});
