#!/usr/bin/env node
/**
 * 增量包的端到端等价性校验：**base + delta ≡ target**。
 *
 * 为什么需要它：`--delta-from` 只上传「变动过的文件」，正确性取决于三件事都对行——
 * 1. 没变的文件真的没变（sha256 比对）；
 * 2. 变过的文件一个都没漏；
 * 3. 删掉的路径一个都没漏（`*.deleted.txt`）。
 *
 * 任何一条出错，接收方拿到的就是一个「看起来还能编译、其实悄悄错位」的目录，
 * 而且往往要过很久才暴露。所以这里不做「抽样比对」，而是把三方的**全集**
 * 逐字节对齐：重复中不讲运气。
 *
 * 用法：
 *   node mjs/build/verify-delta.mjs ^
 *     --base release/上一版.zip ^
 *     --delta release/新版-delta.zip ^
 *     --expect-list release/新版.zip.filelist.txt ^
 *     --expect-root .
 *
 * 退出码 0 = 完全等价；非 0 = 存在差异（差异会被逐条打印出来）。
 */
import { createHash } from "node:crypto";
import { existsSync, readFileSync } from "node:fs";
import zlib from "node:zlib";

function parseArgs(argv) {
  const out = { base: null, delta: null, expectList: null, expectRoot: null };
  for (let i = 0; i < argv.length; i += 1) {
    switch (argv[i]) {
      case "--base": out.base = argv[++i]; break;
      case "--delta": out.delta = argv[++i]; break;
      case "--expect-list": out.expectList = argv[++i]; break;
      case "--expect-root": out.expectRoot = argv[++i]; break;
      default: break;
    }
  }
  const missing = Object.entries(out).filter(([, v]) => !v).map(([k]) => `--${k}`);
  if (missing.length) {
    console.error(`缺少参数: ${missing.join(", ")}`);
    console.error("用法: node mjs/build/verify-delta.mjs --base <旧版.zip> --delta <增量.zip> --expect-list <新版.filelist.txt> --expect-root <目标目录>");
    process.exit(2);
  }
  return out;
}

const sha256 = (buf) => createHash("sha256").update(buf).digest("hex");

/**
 * 纯手工解 ZIP：从 EOCD 找中央目录，再回到每个条目的局部头取数据。
 * 不用第三方库是为了让「生成器」和「解析器」不共用同一套假设——
 * 两边独立实现，格式写错就会被这里的字节校验抓住。
 */
function readZipMap(file) {
  const buf = readFileSync(file);
  const eocd = buf.length - 22;
  if (buf.readUInt32LE(eocd) !== 0x06054b50) throw new Error(`${file}: 找不到 EOCD，不是有效 ZIP`);
  const count = buf.readUInt16LE(eocd + 10);
  let p = buf.readUInt32LE(eocd + 16);
  const map = new Map();

  for (let i = 0; i < count; i += 1) {
    if (buf.readUInt32LE(p) !== 0x02014b50) throw new Error(`${file}: 第 ${i} 个中央目录项签名错误`);
    const method = buf.readUInt16LE(p + 10);
    const crc = buf.readUInt32LE(p + 16);
    const packedSize = buf.readUInt32LE(p + 20);
    const nameLen = buf.readUInt16LE(p + 28);
    const localAt = buf.readUInt32LE(p + 42);
    const name = buf.toString("utf8", p + 46, p + 46 + nameLen);

    const nameLenL = buf.readUInt16LE(localAt + 26);
    const extraLen = buf.readUInt16LE(localAt + 28);
    const start = localAt + 30 + nameLenL + extraLen;
    const packed = buf.subarray(start, start + packedSize);
    const data = method === 8 ? zlib.inflateRawSync(packed) : packed;

    // CRC32 是 ZIP 自带的完整性校验，先用它挡掉「解出来长度对、内容错」的情况
    let c = 0xffffffff;
    const T = readZipMap.TABLE ?? (readZipMap.TABLE = (() => {
      const t = new Uint32Array(256);
      for (let k = 0; k < 256; k += 1) {
        let v = k;
        for (let b = 0; b < 8; b += 1) v = v & 1 ? 0xedb88320 ^ (v >>> 1) : v >>> 1;
        t[k] = v >>> 0;
      }
      return t;
    })());
    for (const byte of data) c = T[(c ^ byte) & 0xff] ^ (c >>> 8);
    if (((c ^ 0xffffffff) >>> 0) !== crc) throw new Error(`${file}: ${name} 的 CRC32 不匹配，包已损坏`);

    map.set(name, data);
    p += 46 + nameLen;
  }
  return map;
}

function main() {
  const opts = parseArgs(process.argv.slice(2));
  if (!existsSync(opts.base)) { console.error(`找不到基线包: ${opts.base}`); process.exit(2); }
  if (!existsSync(opts.delta)) { console.error(`找不到增量包: ${opts.delta}`); process.exit(2); }

  const base = readZipMap(opts.base);
  const delta = readZipMap(opts.delta);

  // 1) 叠加：旧的打底，新的覆盖
  const merged = new Map(base);
  for (const [k, v] of delta) merged.set(k, v);

  // 2) 按删除清单摘掉已经不存在的路径
  const deletedFile = `${opts.delta}.deleted.txt`;
  const deleted = existsSync(deletedFile)
    ? readFileSync(deletedFile, "utf8").split("\n").map((s) => s.trim()).filter(Boolean)
    : [];
  const missingInDeleted = [];
  for (const rel of deleted) {
    if (merged.delete(rel)) continue;
    missingInDeleted.push(rel);
  }

  // 3) 与目标清单逐字节比对
  const expect = new Map(
    readFileSync(opts.expectList, "utf8")
      .split("\n")
      .map((l) => l.match(/^([0-9a-f]+) (\d+) (.+)$/))
      .filter(Boolean)
      .map((m) => [m[3], { hash: m[1], size: Number(m[2]) }]),
  );

  const problems = [];
  for (const [rel, meta] of expect) {
    const got = merged.get(rel);
    if (!got) { problems.push(`叠加结果里缺失: ${rel}`); continue; }
    if (got.length !== meta.size) {
      problems.push(`体积不符: ${rel} 期望 ${meta.size}，实得 ${got.length}`);
      continue;
    }
    if (sha256(got) !== meta.hash) problems.push(`内容不一致: ${rel}`);
  }
  for (const rel of merged.keys()) {
    if (!expect.has(rel)) problems.push(`多余残留: ${rel}`);
  }
  for (const rel of missingInDeleted) problems.push(`删除清单里的路径本就不存在: ${rel}`);

  const extraFilesInDeltaNotNeeded = [...delta.keys()].filter((k) => expect.get(k) === undefined);

  console.log(`基线包    : ${base.size} 个条目`);
  console.log(`增量包    : ${delta.size} 个条目`);
  console.log(`删除清单  : ${deleted.length} 个路径，命中 ${deleted.length - missingInDeleted.length} 个`);
  console.log(`叠加结果  : ${merged.size} 个条目`);
  console.log(`目标清单  : ${expect.size} 个条目`);
  if (extraFilesInDeltaNotNeeded.length) {
    console.log(`提示: 增量包里有 ${extraFilesInDeltaNotNeeded.length} 个文件不在目标清单中（多半是仅存在于中间版本的文件）`);
  }
  console.log("");

  if (problems.length === 0) {
    console.log(`逐字节比对 ${expect.size} 个文件：全部一致，0 异常、0 残留。`);
    console.log("结论: 增量包 ≡ 整包替换，可以安全地只传增量。");
    return;
  }
  console.log(`发现 ${problems.length} 处差异：`);
  problems.slice(0, 30).forEach((p) => console.log(`  ✗ ${p}`));
  if (problems.length > 30) console.log(`  …另外 ${problems.length - 30} 处`);
  process.exitCode = 1;
}

main();
