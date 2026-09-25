/// <reference types="vite/client" />

// 让 tsc 认识 .vue 单文件组件（vue-tsc 会给出更精确的类型）
declare module "*.vue" {
  import type { DefineComponent } from "vue";
  const component: DefineComponent<Record<string, unknown>, Record<string, unknown>, unknown>;
  export default component;
}
