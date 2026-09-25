import { createApp } from "vue";
import { createPinia } from "pinia";
import { invoke } from "@tauri-apps/api/core";
import App from "./App.vue";
import "./assets/main.css";

createApp(App).use(createPinia()).mount("#app");

// 启动自检：挂载成功后回写标记文件，便于外部（以及设置页）确认
// 「前端确实加载了、命令桥确实通了」。失败不影响使用，静默忽略。
invoke("frontend_ready", {
  url: window.location.href,
  userAgent: navigator.userAgent,
}).catch(() => {
  /* 非 Tauri 环境（例如纯浏览器预览）会失败，忽略即可 */
});
