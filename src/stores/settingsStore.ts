import { defineStore } from "pinia";
import { ref } from "vue";
import { getSettings, setSettings } from "../api/commands";

/**
 * 设置 store：启动时从 SQLite 读取，修改后调用 `set_settings` 落库。
 */
export const useSettingsStore = defineStore("settings", () => {
  const tmdbKey = ref("");
  const mediaDirs = ref<string[]>([]);
  const potPlayerPath = ref("");

  const loaded = ref(false);
  const saving = ref(false);
  const error = ref<string | null>(null);

  /** 从数据库加载全部设置。 */
  async function load(): Promise<void> {
    error.value = null;
    try {
      const s = await getSettings();
      tmdbKey.value = s.tmdbKey ?? "";
      mediaDirs.value = Array.isArray(s.mediaDirs) ? [...s.mediaDirs] : [];
      potPlayerPath.value = s.potPlayerPath ?? "";
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

  return {
    tmdbKey, mediaDirs, potPlayerPath,
    loaded, saving, error,
    load, save, addDir, removeDir,
  };
});
