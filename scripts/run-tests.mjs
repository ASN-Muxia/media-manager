// 测试入口包装器。
//
// 为什么要包装一层：vitest 会把「处理过的依赖缓存」写到系统临时目录
// （os.tmpdir()，也就是 Windows 的 %TEMP%）。本机 %TEMP% 被安全策略挡住，
// 写入直接 EPERM，症状极具迷惑性——测试文件随机少跑几个，vitest 只报成
// "Unhandled Error"，退出码却是 1，看上去像用例挂了。
//
// 办法：起 vitest 之前把 TEMP / TMP / TMPDIR 指到项目内的
// node_modules/.vitest-tmp，进程内的临时目录查询就落在项目里了
// （vite.config.ts 里的 test.cache.dir 是第二道保险）。
//
// 这里用「派生子进程跑 vitest CLI」而不是 vitest 的 Node API `startVitest`：
// `startVitest` 的第一个参数是 'test' | 'benchmark' | 'typecheck'，不是
// 'run' / 'watch'，传错会静默退化成**监听模式**，CI 里就永远不退出。
import { spawn } from "node:child_process";
import { mkdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const tmp = join(root, "node_modules", ".vitest-tmp");
mkdirSync(tmp, { recursive: true });

const watch = process.argv.slice(2).includes("--watch");

const child = spawn(
  process.execPath,
  [join(root, "node_modules", "vitest", "vitest.mjs"), watch ? "watch" : "run"],
  {
    cwd: root,
    stdio: "inherit",
    env: { ...process.env, TEMP: tmp, TMP: tmp, TMPDIR: tmp },
  },
);

child.on("exit", (code, signal) => {
  if (signal) {
    process.exitCode = 1;
    return;
  }
  process.exitCode = code ?? 1;
});
