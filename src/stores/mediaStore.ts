import { defineStore } from "pinia";
import { ref } from "vue";
export const useMediaStore = defineStore("media", () => {
  const items = ref<any[]>([]);
  return { items };
});