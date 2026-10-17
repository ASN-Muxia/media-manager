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
//
// 用法：
//   node mjs/dev/run-tests.mjs                 全量跑一次
//   node mjs/dev/run-tests.mjs -- sort         只跑路径里含 sort 的用例文件
//   node mjs/dev/run-tests.mjs --watch         监听模式
//
// 除 `--watch` 之外的参数（一般是文件名过滤词）**原样转发**给 vitest——
// 以前它们被静默丢掉，于是「加了过滤参数却仍然跑全量」，看起来像过滤没生效。
import { spawn } from "node:child_process";
import { mkdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..", "..");
const tmp = join(root, "node_modules", ".vitest-tmp");
mkdirSync(tmp, { recursive: true });

const argv = process.argv.slice(2);
const watch = argv.includes("--watch");
/**
 * 转发给 vitest 的额外参数。
 *
 * 剔除两类：本脚本自己的 `--watch` 开关，以及调用方习惯性加的 `--` 分隔符
 * （它是 shell 的惯例，不是 vitest 的参数，留着会让过滤词被当成选项吞掉）。
 * 其余一律原样透传，过滤词 / `--reporter=verbose` 之类都能照常生效。
 */
const passthrough = argv.filter((a) => a !== "--watch" && a !== "--");

const child = spawn(
  process.execPath,
  [join(root, "node_modules", "vitest", "vitest.mjs"), watch ? "watch" : "run", ...passthrough],
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
