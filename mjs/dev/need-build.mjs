/**
 * 判断"还需不需要构建"——**只报告，不构建**。
 *
 * 2026-10-17 起改用**内容指纹**（sha256）判定，替代旧版 mtime 对比。
 * 旧版的两个"每次都要构建"的病根一并修掉：
 *   1. `src` 下的 *.spec.ts 等测试文件被算进前端输入——vite 根本不打包它们，
 *      但改一次测试/跑一次测试就把 mtime 刷新，下次启动白白构建一遍；
 *   2. mtime 判据本身脆弱——编辑器格式化、云盘/杀软 touch、git checkout
 *      都会刷新时间戳，"没改代码"也会被误判成"要构建"。
 * 现在对输入文件集合算内容指纹，构建成功后由启动器调 `--commit` 写章：
 * 只要内容没变，时间戳怎么抖都不会触发构建。
 *
 * 为什么单独一个脚本：`启动影视库.bat` 需要的是退出码，而本机 Node 派生子进程
 * 不可靠（EBUSY），所以这里只用 `process.exitCode` 把结论交给调用方，构建由它去跑
 * ——各管一段，脚本不碰外部命令。
 *
 * 退出码：
 *   0 = 需要构建（产物缺失 / 无指纹记录 / 输入内容与指纹不一致 / --force）
 *   1 = 已是最新，可以跳过
 *
 * 用法：
 *   node mjs/dev/need-build.mjs --frontend [--force]
 *   node mjs/dev/need-build.mjs --rust [--profile release] [--force]
 *   node mjs/dev/need-build.mjs --frontend --commit          # 构建成功后写指纹
 *   node mjs/dev/need-build.mjs --rust --profile release --commit
 */

import { createHash } from "node:crypto";
import {
  existsSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";

// 必须用 fileURLToPath：`URL.pathname` 保留百分号编码，本项目根目录带空格
// （G:\AI agent\...），不解码的话 existsSync 全部落空、判定恒为"需要构建"。
const ROOT = resolve(fileURLToPath(new URL("../..", import.meta.url)));

/** 指纹落盘目录（git 忽略；机器本地状态，不进仓库也不进构建输入）。 */
const STAMP_DIR = join(ROOT, "mjs", "dev", ".build-stamp");

/** 遍历时一律跳过的目录（产物、依赖、归档与非源码）。 */
const SKIP_DIRS = new Set([
  "node_modules",
  "target",
  "dist",
  ".git",
  ".workbuddy",
  "release",
  "logs",
  "tools",
  ".build-stamp",
]);

/** 测试文件：不是 vite 构建输入，改了不该触发重建。 */
const TEST_RE = /\.(spec|test)\.[cm]?[jt]sx?$/;

function walk(dir, acc, { skipTests = false } = {}) {
  if (!existsSync(dir)) return acc;
  for (const ent of readdirSync(dir, { withFileTypes: true })) {
    if (ent.isDirectory()) {
      if (SKIP_DIRS.has(ent.name)) continue;
      walk(join(dir, ent.name), acc, { skipTests });
    } else if (ent.isFile()) {
      if (skipTests && TEST_RE.test(ent.name)) continue;
      acc.push(join(dir, ent.name));
    }
  }
  return acc;
}

/** 输入集合 → { 相对路径: sha256 }（路径统一正斜杠，跨机器可比）。 */
function fingerprint(files) {
  const map = {};
  for (const abs of files.sort()) {
    map[relative(ROOT, abs).replaceAll("\\", "/")] = createHash("sha256")
      .update(readFileSync(abs))
      .digest("hex");
  }
  return map;
}

function inputsOf(mode) {
  if (mode === "frontend") {
    // vite 真正打包的东西：前端源码（不含测试）+ 入口与构建配置。
    const files = walk(join(ROOT, "src"), [], { skipTests: true });
    for (const f of [
      "index.html",
      "package.json",
      "vite.config.ts",
      "tsconfig.json",
      "tsconfig.node.json",
    ]) {
      const abs = join(ROOT, f);
      if (existsSync(abs)) files.push(abs);
    }
    return files;
  }
  // rust：只有 Rust 侧（源码 + 清单 + 配置 + capabilities）。
  //
  // ⚠️ 这里**刻意不把 `dist` 算进来**（2026-10-16 架构改动）：前端产物现在是
  // **运行时**从 dist 目录读的（见 src-tauri/src/assets.rs），改前端只需
  // `vite build`（亚秒级），不需要重链 exe（实测重链 41.6 秒，正是"启动卡半分钟"的根因）。
  const files = [];
  for (const d of ["src-tauri/src", "src-tauri/capabilities"]) {
    walk(join(ROOT, d), files);
  }
  for (const f of [
    "src-tauri/Cargo.toml",
    "src-tauri/Cargo.lock",
    "src-tauri/build.rs",
    "src-tauri/tauri.conf.json",
  ]) {
    const abs = join(ROOT, f);
    if (existsSync(abs)) files.push(abs);
  }
  return files;
}

function outputOf(mode, profile) {
  return mode === "frontend"
    ? join(ROOT, "dist", "index.html")
    : join(ROOT, "src-tauri", "target", profile, "media-manager.exe");
}

const stampPathOf = (mode, profile) =>
  join(STAMP_DIR, mode === "frontend" ? "frontend.json" : `rust-${profile}.json`);

function readPrevFiles(p) {
  try {
    return JSON.parse(readFileSync(p, "utf8")).files ?? null;
  } catch {
    return null;
  }
}

function reportDiff(prev, cur) {
  const changed = [];
  const added = [];
  const removed = [];
  for (const [k, v] of Object.entries(cur)) {
    if (!(k in prev)) added.push(k);
    else if (prev[k] !== v) changed.push(k);
  }
  for (const k of Object.keys(prev)) if (!(k in cur)) removed.push(k);
  const parts = [];
  const fmt = (label, list) => {
    if (!list.length) return;
    const head = list.slice(0, 5).join(", ");
    parts.push(`${label} ${list.length} 个${list.length > 5 ? `（${head} …）` : `（${head}）`}`);
  };
  fmt("内容变化", changed);
  fmt("新增", added);
  fmt("删除", removed);
  return parts.join("；");
}

// ---------------------------------------------------------------- 主流程
const args = process.argv.slice(2);
const mode = args.includes("--frontend")
  ? "frontend"
  : args.includes("--rust")
    ? "rust"
    : null;
const profileArg = args.indexOf("--profile");
const profile = profileArg >= 0 ? args[profileArg + 1] : "release";
const commit = args.includes("--commit");
const force = args.includes("--force");

if (!mode) {
  // 判不出来就按「需要构建」处理——漏构建的代价是跑旧界面，比多等一次构建严重。
  console.error("用法: node mjs/dev/need-build.mjs --frontend | --rust [--profile release] [--force] [--commit]");
  process.exit(0);
}

const cur = fingerprint(inputsOf(mode));
const stampPath = stampPathOf(mode, profile);

if (commit) {
  mkdirSync(STAMP_DIR, { recursive: true });
  writeFileSync(
    stampPath,
    JSON.stringify({
      mode,
      profile: mode === "rust" ? profile : undefined,
      writtenAt: new Date().toISOString(),
      files: cur,
    }),
    "utf8",
  );
  console.log(`[need-build] 已写入内容指纹（${Object.keys(cur).length} 个输入）`);
  process.exit(0);
}

const out = outputOf(mode, profile);

if (force) {
  console.log("[need-build] --force：需要构建");
  process.exit(0);
}
if (!existsSync(out)) {
  console.log(`[need-build] 产物缺失（${relative(ROOT, out)}）：需要构建`);
  process.exit(0);
}
const prev = readPrevFiles(stampPath);
if (!prev) {
  // 一次性引导：没有指纹记录时，先用旧的「mtime 对比」兜一次底。
  // 若产物确实比所有输入都新，就直接写章并跳过——避免升级判定规则反而
  // 让使用者白白多跑一次完整构建。mtime 判不出来时才真的要构建。
  const outStat = statSync(out);
  const newestInput = Math.max(
    ...inputsOf(mode).map((p) => {
      try {
        return statSync(p).mtimeMs;
      } catch {
        return 0;
      }
    }, []),
  );
  if (outStat.mtimeMs >= newestInput) {
    mkdirSync(STAMP_DIR, { recursive: true });
    writeFileSync(
      stampPath,
      JSON.stringify({
        mode,
        profile: mode === "rust" ? profile : undefined,
        writtenAt: new Date().toISOString(),
        bootstrapped: true,
        files: cur,
      }),
      "utf8",
    );
    console.log("[need-build] 无指纹记录，但产物比全部输入新：已建立基线，跳过构建");
    process.exit(1);
  }
  console.log("[need-build] 无指纹记录且产物非最新：需要构建（本次起建立内容指纹基线）");
  process.exit(0);
}
const diff = reportDiff(prev, cur);
if (diff) {
  console.log(`[need-build] 输入内容已变化（${diff}）：需要构建`);
  process.exit(0);
}
console.log("[need-build] 输入内容与产物指纹一致：跳过构建");
process.exit(1);
