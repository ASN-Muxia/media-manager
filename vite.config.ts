/// <reference types="vitest" />
import { defineConfig } from "vite";
import vue from "@vitejs/plugin-vue";

export default defineConfig({
  plugins: [vue()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    watch: { ignored: ["**/src-tauri/**"] },
  },
  envPrefix: ["VITE_", "TAURI_"],
  // 显式指定缓存目录：不指定的话 vitest 会往系统临时目录写转换缓存，
  // 某些受限环境（杀软 / 沙箱）会直接 EPERM，表现为测试文件随机少跑几个。
  cacheDir: "node_modules/.vite",
  test: {
    // 组件要挂在 DOM 上才能测；后端命令全部靠 vi.mock 顶掉，不碰真实 Tauri。
    environment: "jsdom",
    // src 下是前端用例；scripts 下的 `.spec.mjs` 是 Node 侧工具脚本的用例。
    // 两者归属不同，但共用同一套 runner，省一条命令。
    include: ["src/**/*.spec.ts", "scripts/**/*.spec.mjs"],
    // 用例之间只清调用记录，实现由各用例自己设，避免互相污染。
    clearMocks: true,
    // 关掉「web 模式依赖预打包」：它会把处理过的依赖写到**系统临时目录**，
    // 受限环境（杀软 / 沙箱）下直接 EPERM，表现为测试文件随机少跑几个
    // （同一个文件这次跑、下次不跑）。本项目依赖很少，不走预打包毫无压力。
    deps: { optimizer: { web: { enabled: false } } },
    // 第二道保险：显式把 vitest 自己的缓存目录也挪进项目内，
    // 不让它落到 os.tmpdir()（同上，受限环境会 EPERM）。
    cacheDir: "node_modules/.vitest-cache",
    // 用线程池而不是默认的进程池：vitest 会把转换后的模块写到「临时目录 + 哈希」
    // 这个共享路径上，多进程同时写同一个文件在 Windows 上会 EPERM，
    // 表现为测试文件随机少跑几个。同城单进程内就没有这种争用。
    pool: "threads",
  },
});