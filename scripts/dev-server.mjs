// 开发服务器启动器（不依赖 cmd 的 PATH）。
//
// 背景：本机 cmd 里 PATH 查找失效，`npm` / `node` 都不是可直接执行的命令，
// 于是 `tauri.conf.json` 里的 `beforeDevCommand: "npm run dev"` 起不来，
// 连带 `npm run tauri dev` 整条链路失败。
//
// 处理办法：
//   1. `beforeDevCommand` 置空 —— Tauri dev 模式只负责连 devUrl，不再拉起 npm；
//   2. 前端开发服务器由本脚本直接用 Vite 的 Node API 起，
//      用「已经跑起来的那个 node」自己启动，不再二次查找可执行文件。
//
// 用法：
//   node scripts/dev-server.mjs
import { createServer } from "vite";

const PORT = Number(process.env.TAURI_DEV_PORT || 1420);

const server = await createServer({
  configFile: new URL("../vite.config.ts", import.meta.url).pathname,
  server: { port: PORT, strictPort: true },
});

await server.listen();
server.printUrls();

// 让父进程（Tauri CLI / 终端）能干净地收尾。
for (const signal of ["SIGINT", "SIGTERM"]) {
  process.on(signal, async () => {
    await server.close();
    process.exit(0);
  });
}
