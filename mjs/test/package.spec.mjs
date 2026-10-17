// 打包脚本的自测：容器格式必须「写得出去、解得回来」。
//
// 这里刻意**只用 Node 内置能力**做反向解析——不引入 unzip 之类的第三方库。
// 原因是：如果解析器和生成器共用同一套假设，格式写错了两边会一起错，测试照样通过。
// 用手写的解析器来反向验证，等于给 ZIP 规范做了一次独立复核。
import { describe, expect, it } from "vitest";
import { randomBytes } from "node:crypto";
import { existsSync } from "node:fs";
import { basename, join } from "node:path";
import zlib from "node:zlib";
import {
  DEFAULT_ROOT, buildTarGz, buildZip, crc32, dosTime, gitignoreToRegex, isIgnoredByGitignore,
  tarHeader,
} from "../build/package.mjs";

const enc = (s) => Buffer.from(s, "utf8");

// 真正压不动的内容。之前用 `(i * 7919) % 251` 这种"看起来随机"的序列，
// deflate 照样能压到 90%，测不到 STORE 分支——必须用密码学随机字节。
const INCOMPRESSIBLE = randomBytes(2048);

const SAMPLES = [
  { name: "README.md", data: enc("# media-manager\naaa\n".repeat(20)) },
  { name: "src-tauri/icons/icon.ico", data: INCOMPRESSIBLE },
  { name: "空文件.txt", data: Buffer.alloc(0) },
];

const entries = SAMPLES.map((s) => ({ ...s, ms: Date.UTC(2026, 8, 26, 12, 0, 0) }));

/** 反向解析 ZIP：从 EOCD 出发，逐个条目取出数据块。 */
function parseZip(buf) {
  const eocd = buf.length - 22;
  expect(buf.readUInt32LE(eocd)).toBe(0x06054b50);
  const count = buf.readUInt16LE(eocd + 10);
  let p = buf.readUInt32LE(eocd + 16); // 中央目录起始偏移

  const out = [];
  for (let i = 0; i < count; i += 1) {
    expect(buf.readUInt32LE(p)).toBe(0x02014b50);
    const method = buf.readUInt16LE(p + 10);
    const crc = buf.readUInt32LE(p + 16);
    const packedSize = buf.readUInt32LE(p + 20);
    const rawSize = buf.readUInt32LE(p + 24);
    const nameLen = buf.readUInt16LE(p + 28);
    const localAt = buf.readUInt32LE(p + 42);
    const name = buf.toString("utf8", p + 46, p + 46 + nameLen);
    out.push({ name, method, crc, packedSize, rawSize, localAt });
    p += 46 + nameLen;
  }
  return { count, entries: out };
}

function readPayload(buf, e) {
  expect(buf.readUInt32LE(e.localAt)).toBe(0x04034b50);
  const nameLen = buf.readUInt16LE(e.localAt + 26);
  const extraLen = buf.readUInt16LE(e.localAt + 28);
  const start = e.localAt + 30 + nameLen + extraLen;
  const packed = buf.subarray(start, start + e.packedSize);
  return e.method === 8 ? zlib.inflateRawSync(packed) : packed;
}

describe("打包脚本：ZIP 容器", () => {
  const buf = buildZip(entries);

  it("crc32 对标准测试串给出公认结果", () => {
    expect(crc32(enc(""))).toBe(0);
    expect(crc32(enc("123456789"))).toBe(0xcbf43926);
    expect(crc32(enc("The quick brown fox jumps over the lazy dog"))).toBe(0x414fa339);
  });

  it("DOS 时间戳按 UTC 位域打包，不受运行机器时区影响", () => {
    // 传的是 Date.UTC(...)，若实现用了 getHours()（本地时区），北京时间下会整体偏 8 小时
    const { date, time } = dosTime(Date.UTC(2026, 8, 26, 13, 7, 42));
    expect((date >> 9) + 1980).toBe(2026);
    expect((date >> 5) & 0x0f).toBe(9);
    expect(date & 0x1f).toBe(26);
    expect(time >> 11).toBe(13);
    expect((time >> 5) & 0x3f).toBe(7);
    expect((time & 0x1f) * 2).toBe(42);
  });

  it("1980 年之前的时间戳被夹到 DOS 时间原点，不会溢出", () => {
    const { date } = dosTime(Date.UTC(1970, 0, 1));
    expect(date >> 9).toBe(0);
  });

  it("条目数与文件名能原样解析回来", () => {
    const { count, entries: parsed } = parseZip(buf);
    expect(count).toBe(SAMPLES.length);
    expect(parsed.map((e) => e.name)).toEqual(SAMPLES.map((s) => s.name));
  });

  it("每个条目解压后与原文逐字节一致", () => {
    const { entries: parsed } = parseZip(buf);
    for (const e of parsed) {
      const got = readPayload(buf, e);
      expect(got.length).toBe(e.rawSize);
      expect(crc32(got)).toBe(e.crc);
      expect(got.equals(SAMPLES.find((s) => s.name === e.name).data)).toBe(true);
    }
  });

  it("中文文件名标记了 UTF-8 位，避免跨机器解压乱码", () => {
    const { entries: parsed } = parseZip(buf);
    expect(parsed.some((e) => e.name === "空文件.txt")).toBe(true);
    // 局部头与中央目录的第 11 位（0x0800）都要置上
    for (const e of parsed) {
      expect(buf.readUInt16LE(e.localAt + 6) & 0x0800).toBe(0x0800);
    }
  });

  it("压不动的内容自动退回 STORE，不会越压越大", () => {
    const { entries: parsed } = parseZip(buf);
    const ico = parsed.find((e) => e.name.endsWith("icon.ico"));
    expect(ico.method).toBe(0);
    expect(ico.packedSize).toBe(ico.rawSize);
  });

  it("空文件不额外占 2 个字节的 deflate 头", () => {
    const { entries: parsed } = parseZip(buf);
    const empty = parsed.find((e) => e.name === "空文件.txt");
    expect(empty.rawSize).toBe(0);
    expect(empty.packedSize).toBe(0);
  });

  it("同一份内容重复打包，产出的字节完全一致（可复现）", () => {
    expect(buildZip(entries).equals(buf)).toBe(true);
  });
});

// 打包写死排除名单时，9 个 `vite.config.ts.timestamp-*.mjs` 照样漏进了分发包；
// 改成读 .gitignore 之后这条不会再犯，这里把它作为回归用例钉住。
describe("打包脚本：.gitignore 规则", () => {
  const Ig = (rel, positives, negatives = []) =>
    isIgnoredByGitignore(
      rel,
      positives.map(gitignoreToRegex).filter(Boolean),
      negatives.map(gitignoreToRegex).filter(Boolean),
    );

  it("不带斜杠的规则在任意层级都生效", () => {
    expect(Ig("node_modules", ["node_modules"])).toBe(true);
    expect(Ig("a/b/node_modules/x", ["node_modules"])).toBe(true);
    expect(Ig("node_modules_extra", ["node_modules"])).toBe(false);
  });

  it("带斜杠的规则按相对路径匹配", () => {
    expect(Ig("src-tauri/target/debug/a.exe", ["src-tauri/target"])).toBe(true);
    expect(Ig("other/target/x", ["src-tauri/target"])).toBe(false);
  });

  it("通配符 `*` 不跨目录，`**` 才跨", () => {
    expect(Ig("verify.log", ["*.log"])).toBe(true);
    // `*.log` 不该吃掉别处的同名上级目录内部文件之外的东西
    expect(Ig("src/a.log/inner.txt", ["*.log"])).toBe(true);
    expect(Ig("src-tauri/a/b.rs.bk", ["**/*.rs.bk"])).toBe(true);
  });

  it("vite 的 timestamp 临时文件必须被排除（回归用例）", () => {
    expect(Ig("vite.config.ts.timestamp-1790428801492-7e954c6fa23c4.mjs", ["vite.config.ts.timestamp-*"])).toBe(true);
  });

  it("目录规则（`release/`）连同其下所有文件一起排除", () => {
    expect(Ig("release", ["release/"])).toBe(true);
    expect(Ig("release/after.zip", ["release/"])).toBe(true);
  });

  it("`!` 否定规则能把它重新捞回来", () => {
    expect(Ig(".vscode/settings.json", [".vscode/**"])).toBe(true);
    expect(Ig(".vscode/extensions.json", [".vscode/**"], [".vscode/extensions.json"])).toBe(false);
  });

  it("退化写法（`*`、`/`）不会被当成「匹配一切」把整棵树排除掉", () => {
    // 这类规则编译出来等价于 /(?:)/，宁可丢规则也不能静默全排除
    expect(gitignoreToRegex("*")).toBeNull();
    expect(gitignoreToRegex("/")).toBeNull();
    expect(Ig("src/index.ts", ["*"])).toBe(false);
  });

  it("普通源码文件不会被误伤", () => {
    const rules = ["node_modules", "dist", "src-tauri/target", "*.log", "release/", "*.zip"];
    for (const f of ["src/api/commands.ts", "README.md", "vite.config.ts", "src-tauri/Cargo.toml"]) {
      expect(Ig(f, rules)).toBe(false);
    }
  });
});

describe("打包脚本：tar.gz 容器", () => {
  const gz = buildTarGz(entries);

  it("tar 头的校验和自洽", () => {
    const h = tarHeader("a/b.txt", 1234, Date.UTC(2026, 0, 1));
    expect(h.length).toBe(512);
    const withSpaces = Buffer.from(h);
    withSpaces.write("        ", 148, 8, "utf8");
    let sum = 0;
    for (const b of withSpaces) sum += b;
    expect(parseInt(h.toString("utf8", 148, 154).trim(), 8)).toBe(sum);
  });

  it("tar 头里编的文件名与长度能读回原值", () => {
    const h = tarHeader("src-tauri/src/core/scanner.rs", 54321, Date.UTC(2026, 0, 1));
    expect(h.toString("utf8", 0, 29).replace(/\0.*$/, "")).toBe("src-tauri/src/core/scanner.rs");
    expect(parseInt(h.toString("utf8", 124, 135).replace(/\0.*$/, ""), 8)).toBe(54321);
  });

  it("gunzip 后按 512 字节块逐个还原，内容对齐且不多不少", () => {
    const tar = zlib.gunzipSync(gz);
    expect(tar.length % 512).toBe(0);

    const seen = [];
    let p = 0;
    while (p + 512 <= tar.length) {
      const name = tar.toString("utf8", p, p + 100).replace(/\0.*$/, "");
      if (!name) break; // 结尾空块
      const size = parseInt(tar.toString("utf8", p + 124, p + 135).replace(/\0.*$/, ""), 8);
      seen.push({ name, data: tar.subarray(p + 512, p + 512 + size) });
      p += 512 + Math.ceil(size / 512) * 512;
    }

    expect(seen.map((x) => x.name)).toEqual(SAMPLES.map((s) => s.name));
    seen.forEach((x, i) => expect(x.data.equals(SAMPLES[i].data)).toBe(true));
    // 结尾必须是两个空块
    expect(tar.subarray(p, p + 1024).every((b) => b === 0)).toBe(true);
  });
});

// 脚本被搬过家（scripts/ -> mjs/build/）。根路径是靠 import.meta.url 上溯算出来的，
// 目录层级一变就会静默算错：表现不是报错，而是「打包出来的条目数突然只剩几个」——
// 冒烟脚本照样退出 0，只有盯着条目数才能发现。这里把根路径钉死在项目根。
describe("打包脚本：根目录推导", () => {
  it("DEFAULT_ROOT 必须是项目根，不能停在 mjs/ 这类中间目录", () => {
    // 旧路径做不到：脚本原先在 scripts/ 下只上溯一级，搬到 mjs/build/ 后
    // 仍只上溯一级就会停在 <项目根>/mjs，只能装进 5 个 .mjs（约 40 KB）。
    expect(basename(DEFAULT_ROOT)).not.toBe("mjs");
    expect(basename(DEFAULT_ROOT)).not.toBe("build");
  });

  it("DEFAULT_ROOT 下应能找到项目根的标志性文件", () => {
    expect(existsSync(join(DEFAULT_ROOT, "package.json"))).toBe(true);
    expect(existsSync(join(DEFAULT_ROOT, ".gitignore"))).toBe(true);
    expect(existsSync(join(DEFAULT_ROOT, "vite.config.ts"))).toBe(true);
    expect(existsSync(join(DEFAULT_ROOT, "src-tauri", "Cargo.toml"))).toBe(true);
  });
});
