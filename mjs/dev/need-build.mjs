/**
 * 判断"还需不需要构建"——**只报告，不构建**。
 *
 * 为什么单独一个脚本：`start.bat` 原来每次启动都无条件跑 `vite build` +
 * `cargo build`，而 release 的空跑重链都要几十秒，于是"打开软件"变成了一件
 * 要等半分钟的事。绝大多数启动其实**没有任何源码改动**，这时构建纯属浪费。
 *
 * 为什么不能让它顺手把构建也做了：本机 Node 派生子进程不可靠（EBUSY），
 * 所以这里只用 `process.exitCode` 把结论交给 `start.bat`，由它去调
 * vite / cargo ——各管一段，脚本不碰外部命令。
 *
 * 退出码：
 *   0 = 需要构建（产物缺失，或有输入比产物新，或 --force）
 *   1 = 已是最新，可以跳过
 *
 * 用法：
 *   node mjs/dev/need-build.mjs --frontend [--force]
 *   node mjs/dev/need-build.mjs --rust [--profile release] [--force]
 */

import { existsSync, readdirSync, statSync } from "node:fs";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

// 必须用 fileURLToPath：`URL.pathname` 保留百分号编码，本项目根目录带空格
// （G:\AI agent\...），不解码的话 existsSync 全部落空、判定恒为"需要构建"。
const ROOT = resolve(fileURLToPath(new URL("../..", import.meta.url)));

/** 遍历时一律跳过的目录（产物与依赖，不是"输入"）。 */
const SKIP = new Set(["node_modules", "target", "dist", ".git", ".workbuddy", "release"]);

/** 目录下所有文件的最新 mtime（递归；不存在返回 0）。 */
function newestUnder(dir) {
  let best = 0;
  if (!existsSync(dir)) return best;
  for (const ent of readdirSync(dir, { withFileTypes: true })) {
    if (ent.isDirectory()) {
      if (SKIP.has(ent.name)) continue;
      best = Math.max(best, newestUnder(join(dir, ent.name)));
    } else if (ent.isFile()) {
      const t = statSync(join(dir, ent.name)).mtimeMs;
      if (t > best) best = t;
    }
  }
  return best;
}

/** 若干路径里的最新 mtime（文件或目录都行）。 */
function newestOf(paths) {
  let best = 0;
  for (const p of paths) {
    const abs = join(ROOT, p);
    if (!existsSync(abs)) continue;
    const st = statSync(abs);
    const t = st.isDirectory() ? newestUnder(abs) : st.mtimeMs;
    // 目录本身不存在时 newestUnder 给 0，这里只取已有路径
    if (t > best) best = t;
  }
  return best;
}

function mtime(p) {
  const abs = join(ROOT, p);
  return existsSync(abs) ? statSync(abs).mtimeMs : 0;
}

/** 需要构建就退出 0，否则退出 1。 */
function verdict(need) {
  process.exit(need ? 0 : 1);
}

const args = process.argv.slice(2);
const mode = args.find((a) => a === "--frontend" || a === "--rust");
const profileArg = args.indexOf("--profile");
const profile = profileArg >= 0 ? args[profileArg + 1] : process.env.MM_PROFILE || "release";

if (!mode) {
  // 判不出来就按「需要构建」处理——漏构建的代价是跑旧界面，比多等一次构建严重。
  console.error("用法: node mjs/dev/need-build.mjs --frontend | --rust [--profile release]");
  process.exit(0);
}

// --force：不管新旧，一律要求构建。
if (args.includes("--force")) verdict(true);

if (mode === "--frontend") {
  // 输入 = 前端源码 + 构建配置；产物 = dist/index.html
  const inputs = newestOf([
    "src",
    "index.html",
    "package.json",
    "vite.config.ts",
    "tsconfig.json",
    "tsconfig.node.json",
  ]);
  const out = mtime("dist/index.html");
  verdict(out === 0 || inputs > out);
}

// --rust：输入 = **只有 Rust 侧**（源码 + 清单 + 配置 + capabilities）。
//
// ⚠️ 这里**刻意不再把 `dist` 算进来**（2026-10-16 架构改动）。
//
// 原来 `dist` 是输入：因为前端产物被**编译期嵌进 exe**，前端一变就得重链。
// 而实测「只改前端 → cargo build --release」要 **41.6 秒**（重编 crate + 链接 17MB exe），
// 于是每次改完界面、双击启动都要干等 41 秒，且启动器是隐藏窗口跑的 —— 用户看到的是"卡死"。
//
// 现在前端改成**运行时优先读 `dist` 目录**（见 `src-tauri/src/assets.rs`），
// 嵌入的那份只作为「dist 不存在时」的回退，所以：
//   - 改前端 → 只需 `vite build`（亚秒级），**不需要** cargo build；
//   - 只有动了 Rust 源码 / 清单 / 配置，才真的需要重链。
//
// 代价要说清：不回退重编时，exe 里嵌的那份前端会**变旧**。这在开发机上是无害的
// （运行时读的是磁盘 dist，永远是最新）；只有在「dist 被删掉」的极端情况才会显示旧版，
// 而那种情况本来就需要重新构建（`--frontend` 会因为 dist/index.html 缺失而要求构建）。
const inputs = newestOf([
  "src-tauri/src",
  "src-tauri/Cargo.toml",
  "src-tauri/build.rs",
  "src-tauri/tauri.conf.json",
  "src-tauri/capabilities",
]);
const exe = `src-tauri/target/${profile}/media-manager.exe`;
const out = mtime(exe);
verdict(out === 0 || inputs > out);
