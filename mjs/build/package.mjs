#!/usr/bin/env node
/**
 * 源码打包：产出用于压缩分发 / 传输的归档包。
 *
 * 设计要点：
 * 1. **零外部依赖**：ZIP / TAR 容器与 CRC32、deflate 全部用 Node 内置能力实现，
 *    不调用 `tar` / `git` / `zip` 等外部命令。少了进程派生这一层，脚本在受管环境
 *    （沙箱、CI 容器、没有 Git for Windows 的机器）里也能跑，行为完全一致。
 * 2. **优先只打包 git 跟踪的文件**（`git ls-files`）。构建产物、日志、依赖目录
 *    天然不可能混进来，同一 commit 打出的包完全可复现。取不到 git（未安装 / 不是仓库
 *    / 环境禁止派生进程）时自动降级为按排除规则遍历，并在输出里说明降级原因。
 * 3. 落盘后打印条目数、原始体积、打包体积、压缩率与耗时，
 *    便于复盘「压缩打包与传输效率」的改善。
 * 4. **清单里记录 sha256 + 体积**，配合 `--delta-from` 可以只打包跟上一次相比真正
 *    变化过的文件，把「版本升级」的网络成本从整包的几百 KB 压到几 KB。
 *
 * 用法：
 *   node mjs/build/package.mjs                 # release/media-manager-src-<ver>-<短hash>.zip
 *   node mjs/build/package.mjs --format gz     # .tar.gz
 *   node mjs/build/package.mjs --no-git        # 不用 git，按排除规则遍历
 *   node mjs/build/package.mjs --dry           # 只统计，不落盘
 *   node mjs/build/package.mjs --root <dir>    # 打包别的目录（用于同口径对比历史版本）
 *   node mjs/build/package.mjs --tag <name>    # 自定义输出文件名前缀，对比时用
 *   node mjs/build/package.mjs --reproducible  # 归一化时间戳：同一份内容必然产出同一份字节
 *   node mjs/build/package.mjs --delta-from release/xxx.zip.filelist.txt
 *                                            # 增量包：只含新增 / 修改过的文件
 *
 * 增量包怎么工作：`*.filelist.txt` 每行是 `<sha256> <体积> <路径>`。拿它跟当前目录比对，
 * 内容没变的文件直接跳过；被删掉的路径写进 `*.deleted.txt`。接收端先展开增量包、
 * 再按 deleted 清单删路径，结果等价于整包替换，但网络上只走了真正变动的部分。
 */

import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { existsSync, mkdirSync, readFileSync, statSync, writeFileSync } from "node:fs";
import { readdir } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import zlib from "node:zlib";

// 本文件在 <项目根>/mjs/build/ 下，要上溯两级才是项目根（原来放在 scripts/ 时只需一级）。
export const DEFAULT_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..", "..");

/** 打包根目录。默认指向项目根，可用 `--root` 换（同口径对比历史版本时用）。 */
let ROOT = DEFAULT_ROOT;

/**
 * 遍历模式要排除的目录（git 模式用不上，但保底要有）。
 *
 * 除了这些具名目录，任何**以 `.` 开头且不在白名单里**的目录也会被跳过：临时的探针目录、
 * 解压校验目录、IDE 缓存只要以点开头就天然不会混进分发包，不用每次事后来补名单。
 */
const EXCLUDED_DIRS = new Set([
  "node_modules", ".git", "dist", "target", "gen", "release",
  ".vscode", ".idea", ".vite", ".vitest",
]);
/** 点开头的目录里仍然要保留的（CI 配置属于仓库资产，是有意一起分发的）。 */
const DOT_DIR_WHITELIST = new Set([".github", ".gitlab", ".circleci"]);
/** .gitignore 的否定优先级高于前面任何规则，这里先记下来。 */
let gitignoreNegations = [];

/**
 * 把 .gitignore 的规则读进来。
 *
 * 为什么不直接沿用上面写死的排除表：那份表和 `.gitignore` 是两份独立的清单，
 * 迟早会漂移。实测已经抓到过一次——9 个 `vite.config.ts.timestamp-*.mjs`
 * 被 vite 生成、.gitignore 里明明写着，却照样混进了分发包。
 * 让「什么不该被分发」只有 .gitignore 一个出处，才是能长期站得住的做法。
 *
 * 转换时按 gitignore 语义处理：支持 `*` `?` 与字符类、`/` 结尾表示只匹配目录、
 * 含 `/` 或 `*`/`?` 以外的普通路径按「任意层级同名」处理、`#` 注释、`\` 转义跳过。
 */
export function loadGitignorePatterns() {
  const file = path.join(ROOT, ".gitignore");  if (!existsSync(file)) return [];
  const positives = new Set();
  const negatives = new Set();
  for (const rawLine of readFileSync(file, "utf8").split("\n")) {
    const line = rawLine.trim();
    if (!line || line.startsWith("#")) continue;
    if (line.startsWith("!")) {
      negatives.add(line.slice(1).trim());
      continue;
    }
    positives.add(line.replace(/\\(?=[*?[\]\\])/g, ""));
  }
  gitignoreNegations = [...negatives].map(gitignoreToRegex).filter(Boolean);
  return [...positives].map(gitignoreToRegex).filter(Boolean);
}

/** 单条 gitignore 规则 → 正则。**不含** `/` 的规则在任意层级都生效。 */
export function gitignoreToRegex(pattern) {
  let body = pattern;
  let anchored = false;
  let dirOnly = false;
  if (body.endsWith("/")) { dirOnly = true; body = body.slice(0, -1); }
  if (body.startsWith("/")) { anchored = true; body = body.slice(1); }
  // `*`、`**`、`a/**` 这类「只剩通配符」的写法编译出来等价于匹配一切，
  // 宁可丢掉这条规则，也不能让整棵树被静默排除。
  if (body === "" || /^[*/]+$/.test(body)) return null;
  const hasSlash = body.includes("/");

  let re = "";
  for (let i = 0; i < body.length; i += 1) {
    const ch = body[i];
    if (ch === "*") {
      if (body[i + 1] === "*") {
        // ** 跨目录
        while (body[i + 1] === "*") i += 1;
        re += ".*";
      } else {
        re += "[^/]*";
      }
    } else if (ch === "?") {
      re += "[^/]";
    } else if (ch === "[") {
      const end = body.indexOf("]", i);
      if (end === -1) re += "\\[";
      else { re += body.slice(i, end + 1); i = end; }
    } else {
      re += ch.replace(/[.+^${}()|[\]\\]/g, "\\$&");
    }
  }
  // 不带斜杠 → 作用在任意层级（前面加 ".*/" 前缀效果）；或者像 `*.log` 这种，
  // 直接用「后缀匹配」也是等价的：这里用 `(^|.*/)` 表达任意层级。
  const prefix = anchored ? "^" : hasSlash ? "^(?:.*/)?" : "(?:^|.*/)";
  const suffix = dirOnly ? "(?:/.*)?$" : "$";
  const core = re;
  // core 为空只有 `*` 或 `/` 这类退化写法，转成正则就是「匹配一切」——
  // 宁可丢掉这条规则，也不能让整棵树被静默排除。
  if (core === "") return null;
  try {
    return new RegExp(`${prefix}${core}(?:/.*)?${suffix}`);
  } catch {
    return null;
  }
}

/** `.gitignore` 转出来的规则集，在 main 里按实际 ROOT 加载一次。 */
let gitignorePatterns = [];

/** 相对路径（用 `/` 分隔）是否被 gitignore 命中。 */
export function isIgnoredByGitignore(rel, patterns = gitignorePatterns, negations = gitignoreNegations) {
  return patterns.some((re) => re.test(rel))
    && !negations.some((re) => re.test(rel));
}

/** .gitignore 之外再兜一层：常见构建产物、日志、归档本身。 */
const EXCLUDED_SUFFIXES = [".log", ".zip", ".tar.gz", ".pdb", ".rs.bk", ".tmp", ".manifest.txt"];
const EXCLUDED_BASENAMES = new Set([".DS_Store", "Thumbs.db"]);

const KI = 1024;

function parseArgs(argv) {
  const out = {
    format: "zip", useGit: true, dry: false, outDir: "release",
    root: null, tag: null, reproducible: false, deltaFrom: null,
  };
  for (let i = 0; i < argv.length; i += 1) {
    switch (argv[i]) {
      case "--format": out.format = argv[++i] === "gz" ? "gz" : "zip"; break;
      case "--no-git": out.useGit = false; break;
      case "--dry": out.dry = true; break;
      case "--out": out.outDir = argv[++i]; break;
      case "--root": out.root = path.resolve(argv[++i]); break;
      case "--tag": out.tag = argv[++i]; break;
      case "--reproducible": out.reproducible = true; break;
      case "--delta-from": out.deltaFrom = path.resolve(argv[++i]); break;
      default: break;
    }
  }
  return out;
}

function human(bytes) {
  const units = ["B", "KB", "MB", "GB"];
  let v = bytes;
  let i = 0;
  while (v >= KI && i < units.length - 1) { v /= KI; i += 1; }
  return `${v >= 100 || i === 0 ? v.toFixed(0) : v.toFixed(1)} ${units[i]}`;
}

/* ------------------------------- 文件清单 ------------------------------- */

/** 优先用 git：只包含受版本控制的文件，天然排除一切构建产物。 */
function listViaGit() {
  if (!existsSync(path.join(ROOT, ".git"))) return null;
  try {
    const r = spawnSync("git", ["ls-files"], { cwd: ROOT, encoding: "utf8", maxBuffer: 32 * KI * KI });
    if (r.error || r.status !== 0 || !r.stdout) return null;
    const list = r.stdout.split("\n").map((s) => s.trim()).filter((rel) => rel && existsSync(path.join(ROOT, rel)));
    return list.length ? list : null;
  } catch {
    return null; // 环境禁止派生子进程时走遍历模式
  }
}

/** 降级方案：递归遍历并按排除规则过滤。 */
async function listViaWalk(dir = ROOT, prefix = "") {
  const found = [];
  for (const e of await readdir(dir, { withFileTypes: true })) {
    const rel = prefix ? `${prefix}/${e.name}` : e.name;
    // .gitignore 先过一遍。目录被命中就直接剪枝，省得继续往下扫几万个 target 里的文件
    if (isIgnoredByGitignore(rel)) continue;
    if (e.isDirectory()) {
      if (EXCLUDED_DIRS.has(e.name)) continue;
      if (e.name.startsWith(".") && !DOT_DIR_WHITELIST.has(e.name)) continue;
      found.push(...(await listViaWalk(path.join(dir, e.name), rel)));
    } else if (e.isFile()) {
      if (EXCLUDED_BASENAMES.has(e.name)) continue;
      if (EXCLUDED_SUFFIXES.some((s) => e.name.endsWith(s))) continue;
      found.push(rel);
    }
  }
  return found;
}

/* -------------------------------- CRC32 -------------------------------- */

const CRC_TABLE = (() => {
  const t = new Uint32Array(256);
  for (let i = 0; i < 256; i += 1) {
    let c = i;
    for (let k = 0; k < 8; k += 1) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    t[i] = c >>> 0;
  }
  return t;
})();

function crc32(buf) {
  let c = 0xffffffff;
  for (let i = 0; i < buf.length; i += 1) c = CRC_TABLE[(c ^ buf[i]) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
}
export { crc32 };

/* ------------------------------ ZIP 归档写入 ----------------------------- */

/**
 * 毫秒时间戳 → DOS 日期（高 16 位）与时间（低 16 位）。
 *
 * 一律取 **UTC** 字段：ZIP 规范里 DOS 时间是本地时间，那样同一份内容在东京和在柏林
 * 打出来的包字节不同，跨机器的哈希校验、CDN 缓存和二进制差分全都失效。
 * 本脚本打的是分发包而非用户归档，可读性让位于可复现性。
 */
export function dosTime(ms) {
  const d = new Date(ms);
  const year = Math.max(d.getUTCFullYear(), 1980);
  const date = ((year - 1980) << 9) | ((d.getUTCMonth() + 1) << 5) | d.getUTCDate();
  const time = (d.getUTCHours() << 11) | (d.getUTCMinutes() << 5) | (d.getUTCSeconds() >> 1);
  return { date: date & 0xffff, time: time & 0xffff };
}

/**
 * 组装一个 ZIP（file: deflate，method 8；已压不动的就用 store，method 0）。
 *
 * 结构：若干 [local header + 数据] + 中央目录 + EOCD。文件名一律写入 UTF-8，
 * 并置 general-purpose bit 11，交给解压端按 UTF-8 解释，避免中文路径乱码。
 */
export function buildZip(entries) {
  const locals = [];
  const centrals = [];
  let offset = 0;

  for (const { name, data, ms } of entries) {
    const nameBuf = Buffer.from(name, "utf8");
    const crc = crc32(data);
    const { date, time } = dosTime(ms);
    // deflate 后反而变大（已经压过的二进制，如 png/jpg/zip）就原样存；
    // 空文件 deflate 出来是 2 个字节，比存 0 个字节还亏，自然也会落到这里。
    const deflated = zlib.deflateRawSync(data, { level: 9 });
    const useDeflate = deflated.length < data.length;
    const method = useDeflate ? 8 : 0;
    const payload = useDeflate ? deflated : data;

    const lh = Buffer.alloc(30);
    lh.writeUInt32LE(0x04034b50, 0);
    lh.writeUInt16LE(20, 4); // version needed
    lh.writeUInt16LE(0x0800, 6); // UTF-8 文件名
    lh.writeUInt16LE(method, 8);
    lh.writeUInt16LE(time, 10);
    lh.writeUInt16LE(date, 12);
    lh.writeUInt32LE(crc, 14);
    lh.writeUInt32LE(payload.length, 18);
    lh.writeUInt32LE(data.length, 22);
    lh.writeUInt16LE(nameBuf.length, 26);
    lh.writeUInt16LE(0, 28);
    locals.push(lh, nameBuf, payload);

    const ch = Buffer.alloc(46);
    ch.writeUInt32LE(0x02014b50, 0);
    ch.writeUInt16LE(20, 4); // version made by
    ch.writeUInt16LE(20, 6); // version needed
    ch.writeUInt16LE(0x0800, 8);
    ch.writeUInt16LE(method, 10);
    ch.writeUInt16LE(time, 12);
    ch.writeUInt16LE(date, 14);
    ch.writeUInt32LE(crc, 16);
    ch.writeUInt32LE(payload.length, 20);
    ch.writeUInt32LE(data.length, 24);
    ch.writeUInt16LE(nameBuf.length, 28);
    ch.writeUInt32LE(offset, 42);
    centrals.push(ch, nameBuf);

    offset += 30 + nameBuf.length + payload.length;
  }

  const localPart = Buffer.concat(locals);
  const centralPart = Buffer.concat(centrals);
  const eocd = Buffer.alloc(22);
  eocd.writeUInt32LE(0x06054b50, 0);
  eocd.writeUInt16LE(entries.length, 8);
  eocd.writeUInt16LE(entries.length, 10);
  eocd.writeUInt32LE(centralPart.length, 12);
  eocd.writeUInt32LE(localPart.length, 16);
  return Buffer.concat([localPart, centralPart, eocd]);
}

/* --------------------------- tar.gz 归档写入 ---------------------------- */

/** 512 字节一组的 tar 头（ustar 格式），size 用八进制 ASCII。 */
export function tarHeader(name, size, ms) {
  const h = Buffer.alloc(512);
  const writeStr = (s, at, len) => h.write(s, at, Math.min(len, Buffer.byteLength(s)), "utf8");
  writeStr(name, 0, 100);
  writeStr("000644 \0", 100, 8);
  writeStr("000000 \0", 108, 8);
  writeStr("000000 \0", 116, 8);
  writeStr(size.toString(8).padStart(11, "0") + "\0", 124, 12);
  writeStr(Math.floor(ms / 1000).toString(8).padStart(11, "0") + "\0", 136, 12);
  writeStr("        ", 148, 8); // 校验位先填空格
  h.write("0", 156, 1, "utf8"); // 普通文件
  writeStr("ustar\0", 257, 6);
  writeStr("00", 263, 2);

  let sum = 0;
  for (const b of h) sum += b;
  writeStr(sum.toString(8).padStart(6, "0") + "\0 ", 148, 8);
  return h;
}

export function buildTarGz(entries) {
  const parts = [];
  for (const { name, data, ms } of entries) {
    parts.push(tarHeader(name, data.length, ms), data);
    const pad = (512 - (data.length % 512)) % 512;
    if (pad) parts.push(Buffer.alloc(pad));
  }
  parts.push(Buffer.alloc(1024)); // 结尾两个空块
  return zlib.gzipSync(Buffer.concat(parts), { level: 9 });
}

/* -------------------------------- 主流程 -------------------------------- */

function stamp() {
  let version = "0.0.0";
  try {
    version = JSON.parse(readFileSync(path.join(ROOT, "package.json"), "utf8")).version ?? "0.0.0";
  } catch { /* 非标准目录就用默认版本号 */ }
  let sha = "nogit";
  try {
    const r = spawnSync("git", ["rev-parse", "--short", "HEAD"], { cwd: ROOT, encoding: "utf8" });
    if (!r.error && r.status === 0 && r.stdout) sha = r.stdout.trim();
  } catch { /* 无 git 时保持 nogit */ }
  return `${version}-${sha}`;
}

/**
 * 读上一版的清单，返回 路径 → {hash,size} 映射。
 * 兼容两种格式：新格式 `<hash> <size> <path>`，以及早期只有路径的清单。
 */
function readBaselineManifest(file) {
  if (!existsSync(file)) {
    console.error(`找不到基线清单: ${file}`);
    process.exit(1);
  }
  const map = new Map();
  for (const line of readFileSync(file, "utf8").split("\n")) {
    const t = line.trim();
    if (!t) continue;
    const m = t.match(/^([0-9a-f]{8,})\s+(\d+)\s+(.+)$/);
    if (m) map.set(m[3], { hash: m[1], size: Number(m[2]) });
    else map.set(t, { hash: "", size: -1 });
  }
  return map;
}

async function main() {
  const opts = parseArgs(process.argv.slice(2));
  if (opts.root) ROOT = opts.root;
  gitignorePatterns = loadGitignorePatterns();
  const t0 = Date.now();

  let files = opts.useGit && !opts.root ? listViaGit() : null;
  let mode = "walk";
  if (files) {
    mode = "git";
  } else {
    if (opts.useGit) {
      console.log("提示: 取不到 git 文件列表（未安装 git / 非仓库 / 环境不允许派生子进程），降级为目录遍历。");
      console.log("      遍历模式已排除 node_modules、target、dist、gen、release、点开头的目录与各类日志、归档。");
    }
    files = await listViaWalk();
  }
  files.sort();
  if (files.length === 0) {
    console.error("没有找到任何可打包的文件，已中止。");
    process.exit(1);
  }

  // 可复现模式：时间戳统一取 1980-01-01（DOS 时间原点），同一份内容 → 同一份字节，
  // 便于 CDN 缓存命中、二进制差分与哈希校验。
  const FIXED_MS = Date.UTC(1980, 0, 1);

  const baseline = opts.deltaFrom ? readBaselineManifest(opts.deltaFrom) : null;

  let rawBytes = 0;
  let skipped = 0;
  const entries = [];
  // 注意：这里记录的是**当前存在的全部路径**，而不是打包进去的那些。
  // 增量包里没出现的文件有两种完全不同的含义——「没变，不用传」和「已经删了」，
  // 只有拿全集去比对才能区分开。
  const alive = new Set();
  for (const rel of files) {
    const abs = path.join(ROOT, rel);
    alive.add(rel.split(path.sep).join("/"));
    const data = readFileSync(abs);
    const size = data.length;
    const hash = createHash("sha256").update(data).digest("hex");

    // 增量模式：内容与基线完全一致的文件直接跳过
    if (baseline) {
      const prev = baseline.get(rel);
      if (prev && prev.hash === hash && prev.size === size) { skipped += 1; continue; }
    }

    rawBytes += size;
    entries.push({
      name: rel.split(path.sep).join("/"),
      data,
      hash,
      size,
      ms: opts.reproducible ? FIXED_MS : statSync(abs).mtimeMs,
    });
  }

  console.log(`打包模式: ${mode === "git" ? "git ls-files（仅版本控制内的文件）" : "目录遍历（按排除规则）"}`);
  if (opts.reproducible) console.log("可复现模式: 时间戳已归一化，同内容同字节");
  if (opts.deltaFrom) {
    console.log(`增量基线: ${path.basename(opts.deltaFrom)}`);
    console.log(`未变化跳过: ${skipped} 个文件`);
  }
  console.log(`条目数  : ${entries.length}`);
  console.log(`原始体积: ${human(rawBytes)} (${rawBytes} B)`);

  if (opts.dry) {
    console.log("--dry：不落盘。");
    return;
  }
  if (entries.length === 0) {
    console.log("没有变化的文件，不必产出增量包。");
    return;
  }

  const outDir = path.isAbsolute(opts.outDir) ? opts.outDir : path.join(ROOT, opts.outDir);
  mkdirSync(outDir, { recursive: true });
  const ext = opts.format === "gz" ? "tar.gz" : "zip";
  const name = `${opts.tag ?? `media-manager-src-${stamp()}`}.${ext}`;
  const archive = path.join(outDir, name);

  const tPack = Date.now();
  const archiveBuf = opts.format === "gz" ? buildTarGz(entries) : buildZip(entries);
  const packMs = Date.now() - tPack;
  writeFileSync(archive, archiveBuf);

  // 同目录下留一份清单（sha256 + 体积 + 路径），既给接收方校验，也供下次做增量基线
  const filelistName = `${name}.filelist.txt`;
  writeFileSync(
    path.join(outDir, filelistName),
    `${entries.map((e) => `${e.hash} ${e.size} ${e.name}`).join("\n")}\n`,
    "utf8",
  );

  // 增量包要额外告诉接收方「哪些路径已经不存在了」，否则删掉的文件会在对端残留
  let deletedCount = 0;
  if (baseline) {
    const gone = [...baseline.keys()].filter((k) => !alive.has(k)).sort();
    deletedCount = gone.length;
    writeFileSync(path.join(outDir, `${name}.deleted.txt`), `${gone.join("\n")}${gone.length ? "\n" : ""}`, "utf8");
  }

  const lines = [
    `media-manager 源码包 ${name}`,
    `生成时间 : ${new Date().toISOString()}`,
    `打包模式 : ${mode}`,
    `时间戳   : ${opts.reproducible ? "归一化（可复现）" : "保留原文件 mtime"}`,
    baseline ? `增量基线 : ${path.basename(opts.deltaFrom)}，跳过未变化 ${skipped} 个` : null,
    baseline ? `删除清单 : ${deletedCount} 个路径 → ${name}.deleted.txt` : null,
    `条目数   : ${entries.length}`,
    `原始体积 : ${human(rawBytes)} (${rawBytes} B)`,
    `打包体积 : ${human(archiveBuf.length)} (${archiveBuf.length} B)`,
    `压缩率   : ${((1 - archiveBuf.length / rawBytes) * 100).toFixed(1)}% (残余 ${((archiveBuf.length / rawBytes) * 100).toFixed(1)}%)`,
    `压缩耗时 : ${(packMs / 1000).toFixed(3)} s`,
    `总耗时   : ${((Date.now() - t0) / 1000).toFixed(2)} s（含读盘与落盘）`,
  ].filter(Boolean);
  console.log("");
  lines.forEach((l) => console.log(l));
  writeFileSync(
    path.join(outDir, `${name}.manifest.txt`),
    `${lines.join("\n")}\n\n文件清单见同目录 ${filelistName}\n`,
    "utf8",
  );
  console.log(`\n输出: ${path.relative(ROOT, archive)}`);
}

// 只有作为脚本直接运行时才走到 main()；被测试 import 时保持沉默，
// 不会顺手落一个 release 包到磁盘上。
if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main().catch((err) => {
    console.error(err);
    process.exit(1);
  });
}
