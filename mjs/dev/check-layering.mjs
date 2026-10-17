// 分层合规检查：把「谁可以依赖谁」钉死在 CI 里。
//
// 存在的理由：本项目最值钱的结构资产是分层——`core`（领域规则）/
// `provider`（外部数据源）/ `store`（持久化）互不反向依赖，只有 `commands`
// 能同时使用三者。这条规矩靠人记住是不可靠的：`core/scanner.rs` 就曾经
// 悄悄 `use crate::commands::settings` 和 `crate::provider::llm`，
// 把两条边同时破了，而代码照样编译得过。
//
// 因此这里做静态检查，由 verify.bat 每次跑。新增违规会在本地就炸掉，
// 不会等到「分层只剩文档」才被发现。
//
// 用法：node mjs/dev/check-layering.mjs

import { readdirSync, readFileSync, statSync } from 'node:fs';
import { join, relative } from 'node:path';

const ROOT = process.cwd();
const SRC = join(ROOT, 'src-tauri', 'src');

/**
 * 依赖方向白名单：目录 -> 允许它引用的其它模块名。
 *
 * 判定原则：**禁止向上依赖**（`core` 绝不能反向依赖 `commands`），
 * 这是会毁掉分层的那一条；而 `provider` / `share` 复用 `core` 的**数据类型**
 * （如 `ParsedName`）是合理的适配器行为，放行。
 */
const ALLOWED = {
  // 领域规则：只认自己与工具层。这一条是硬红线——曾经被 `core/scanner.rs`
  // 反向依赖 `commands::settings` 破过，所以列为最严格的一档。
  core: ['core', 'util'],
  // 外部数据源：可复用 core 的数据类型（要把解析结果填回 ParsedName）
  provider: ['provider', 'core', 'util'],
  store: ['store', 'util'], // 持久化：只认自己与工具层
  share: ['share', 'core', 'util'], // 互通层：复用 core 的纯类型
  util: ['util'], // 工具层：谁也不认
  commands: ['commands', 'core', 'provider', 'store', 'share', 'util'], // 编排层：全都可用
  boot: ['boot', 'util'],
  // 前端资源提供者：只读磁盘 dist + 持有 tauri 的嵌入资源，谁也不依赖。
  // （`lib.rs` 本身不在白名单里、会被跳过，故新顶层模块必须显式登记，
  // 否则它就成了分层检查的盲区。）
  assets: ['assets'],
};

function walk(dir) {
  const out = [];
  for (const name of readdirSync(dir)) {
    const p = join(dir, name);
    if (statSync(p).isDirectory()) out.push(...walk(p));
    else if (name.endsWith('.rs')) out.push(p);
  }
  return out;
}

/** 这个文件属于哪个顶层模块（tests/ 子目录归回它的模块）。 */
function moduleOf(relPath) {
  const parts = relPath.split(/[\\/]/);
  const top = parts[0];
  // src/core/tests/xxx.rs 仍然算 core
  return ALLOWED[top] ? top : null;
}

function isTestFile(relPath) {
  return relPath.split(/[\\/]/).includes('tests');
}

const violations = [];
for (const file of walk(SRC)) {
  const rel = relative(SRC, file);
  const mod = moduleOf(rel);
  if (!mod || mod === 'commands') continue; // commands 允许引用一切；非模块根目录跳过
  // 测试代码豁免：单测需要直接构造 DB / 调用内部函数，
  // 让它守生产代码的规矩没有意义（架构文档里也是这么约定的）。
  if (isTestFile(rel)) continue;
  const allowed = new Set(ALLOWED[mod]);
  const text = readFileSync(file, 'utf8');
  const lines = text.split(/\r?\n/);
  lines.forEach((line, i) => {
    // 只看 use 声明与带 crate:: 前缀的路径调用，避免误伤注释里的文字
    const trimmed = line.trim();
    if (trimmed.startsWith('//')) return;
    const m = trimmed.match(/crate::([a-z_]+)/g);
    if (!m) return;
    for (const hit of m) {
      const target = hit.slice('crate::'.length);
      if (!allowed.has(target)) {
        violations.push({
          file: rel,
          line: i + 1,
          from: mod,
          to: target,
          text: trimmed,
        });
      }
    }
  });
}

if (violations.length > 0) {
  console.error('\n[分层违规] 以下引用违反依赖方向：\n');
  for (const v of violations) {
    console.error(`  ${v.file}:${v.line}  ${v.from} -> crate::${v.to}`);
    console.error(`      ${v.text}`);
  }
  console.error('\n允许的方向：');
  for (const [k, v] of Object.entries(ALLOWED)) {
    console.error(`  ${k.padEnd(9)} -> ${v.join(', ')}`);
  }
  console.error('');
  process.exit(1);
}

console.log('[分层合规] 通过：core / provider / store 均无非法的跨层引用');
process.exit(0);
