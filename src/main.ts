import { createApp } from "vue";
import { createPinia } from "pinia";
import { invoke } from "@tauri-apps/api/core";
import App from "./App.vue";
import "./assets/main.css";

/**
 * 前端异常上报：写进应用数据目录的 `frontend_error.log`。
 *
 * 存在的理由与「启动自检标记」相同——WebView2 的画面抓不到，窗口标题又只来自
 * tauri.conf.json，所以「窗口在、页面白」这类问题只能靠日志定位。失败不影响使用。
 */
function report(msg: string): void {
  invoke("frontend_error", { message: msg }).catch(() => {
    /* 非 Tauri 环境（例如纯浏览器预览）会失败，忽略即可 */
  });
}

window.addEventListener("error", (e) => {
  report(`error: ${e.message} @ ${e.filename}:${e.lineno}:${e.colno}`);
});
window.addEventListener("unhandledrejection", (e) => {
  report(`unhandledrejection: ${String(e.reason)}`);
});

const tauriReady =
  typeof (window as unknown as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__ !==
  "undefined";
report(
  `boot: href=${window.location.href} readyState=${document.readyState} tauri=${tauriReady}`,
);

try {
  createApp(App).use(createPinia()).mount("#app");
  report("mounted");
} catch (e) {
  report(`mount failed: ${e instanceof Error ? (e.stack ?? e.message) : String(e)}`);
}

// 启动自检：挂载成功后回写标记文件，便于外部（以及设置页）确认
// 「前端确实加载了、命令桥确实通了」。失败不影响使用，静默忽略。
invoke("frontend_ready", {
  url: window.location.href,
  userAgent: navigator.userAgent,
})
  .then(() => report("frontend_ready ok"))
  .catch((e) => report(`frontend_ready failed: ${String(e)}`));
