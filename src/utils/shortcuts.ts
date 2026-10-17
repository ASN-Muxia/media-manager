/**
 * 快捷键：组合键的**解析、规范化与匹配**（纯函数，不碰 DOM）。
 *
 * 存在的理由：项目里原本只有 `@keyup.enter` 这类「输入框内」的写法，
 * 没有全局快捷键。「全选 / 反选 / 确认所选」是刮削流程里点得最多的三个动作，
 * 每次都要把鼠标移到工具条上很累，而这些动作又因人而异（有人习惯 `Ctrl+A`，
 * 有人嫌它和「全选文本」冲突），所以必须**可自定义**。
 *
 * 组合键写法统一为 `Ctrl+Shift+A`（修饰键在前，主键在后，用 `+` 连接）：
 * - 修饰键只认 `Ctrl` / `Alt` / `Shift` / `Meta`（`Meta` = Windows 键 / Cmd）；
 * - 主键可以是 `A`…`Z`、`0`…`9`、`F1`…`F12`，以及 `Enter` / `Esc` / `Space` /
 *   `Tab` / `Backspace` / `Delete` / 方向键；
 * - 大小写与分隔符（`+` / `-` / 空格）都不敏感，`ctrl+a` 与 `Ctrl+A` 等价。
 */

/** 可绑定的动作。 */
export type ShortcutAction = "selectAll" | "invert" | "confirm" | "clear";

/** 默认绑定：`Ctrl+A` 全选、`Ctrl+Shift+A` 反选、`Ctrl+Enter` 确认、`Esc` 清空。 */
export const DEFAULT_SHORTCUTS: Record<ShortcutAction, string> = {
  selectAll: "Ctrl+A",
  invert: "Ctrl+Shift+A",
  confirm: "Ctrl+Enter",
  clear: "Esc",
};

/** 设置页里显示的名字。 */
export const SHORTCUT_LABELS: Record<ShortcutAction, string> = {
  selectAll: "全选",
  invert: "反选",
  confirm: "确认所选",
  clear: "清空选择",
};

const MODIFIERS = ["ctrl", "alt", "shift", "meta"] as const;

/** `F1`…`F12` 与具名键的规范化写法（小写）。 */
const NAMED_KEYS: Record<string, string> = {
  enter: "enter",
  esc: "esc",
  escape: "esc",
  space: "space",
  " ": "space",
  tab: "tab",
  backspace: "backspace",
  delete: "delete",
  del: "delete",
  up: "up",
  down: "down",
  left: "left",
  right: "right",
  arrowup: "up",
  arrowdown: "down",
  arrowleft: "left",
  arrowright: "right",
};

/**
 * 把一个组合键字符串归一化成 `ctrl+shift+a` 这样的规范形式。
 *
 * 解析不了（空串、只有修饰键、主键非法）返回 `null`——调用方据此判断
 * 「这一项没绑定」。
 */
export function normalizeCombo(raw: string): string | null {
  const parts = String(raw ?? "")
    .split(/[+\-\s]+/)
    .map((p) => p.trim())
    .filter((p) => p.length > 0);
  if (parts.length === 0) return null;

  const mods = new Set<string>();
  let main = "";
  for (const p of parts) {
    const low = p.toLowerCase();
    if ((MODIFIERS as readonly string[]).includes(low)) {
      mods.add(low);
      continue;
    }
    // 主键只取最后一个（写 `Ctrl+A+B` 这种没意义，按最后一个算）
    main = low;
  }
  if (!main) return null;

  const key = NAMED_KEYS[main] ?? main;
  if (key.length > 1 && !/^f([1-9]|1[0-2])$/.test(key) && !Object.values(NAMED_KEYS).includes(key)) {
    return null;
  }

  // 输出顺序固定，便于比较两个组合键是否相同
  const ordered = MODIFIERS.filter((m) => mods.has(m));
  return [...ordered, key].join("+");
}

/** 从一个键盘事件里读出规范化的组合键（`null` = 这个事件不构成组合键）。 */
export function comboFromEvent(e: KeyboardEvent): string | null {
  const key = eventKey(e);
  if (!key) return null;
  const mods: string[] = [];
  if (e.ctrlKey) mods.push("ctrl");
  if (e.altKey) mods.push("alt");
  if (e.shiftKey) mods.push("shift");
  if (e.metaKey) mods.push("meta");
  return [...mods, key].join("+");
}

/** 键盘事件里的主键名 → 规范化写法。 */
function eventKey(e: KeyboardEvent): string | null {
  const k = e.key;
  if (!k) return null;
  const low = k.toLowerCase();
  if (NAMED_KEYS[low]) return NAMED_KEYS[low];
  // 单个可打印字符（含 `a`…`z` / `0`…`9` / 标点）
  if (k.length === 1 && k !== "+" && k !== "-") return low;
  if (/^F([1-9]|1[0-2])$/i.test(k)) return low;
  return null;
}

/** 这个键盘事件是否命中了 `combo`（`combo` 为空串 / `null` 表示未绑定，恒不命中）。 */
export function matchesCombo(e: KeyboardEvent, combo: string | null | undefined): boolean {
  const want = normalizeCombo(combo ?? "");
  if (!want) return false;
  return comboFromEvent(e) === want;
}

/**
 * 把「动作 → 组合键」的表反查成「这次按键命中了哪个动作」。
 *
 * 一个组合键只可能命中一个动作：后写入的动作**不会**偷走先注册的绑定，
 * 重复绑定时取表里靠前的那个（顺序即 `SHORTCUT_ACTIONS`），避免出现
 * 「按一个键触发两件事」这种说不清的行为。
 */
export function actionFor(
  e: KeyboardEvent,
  bindings: Partial<Record<ShortcutAction, string>>,
): ShortcutAction | null {
  const got = comboFromEvent(e);
  if (!got) return null;
  for (const a of SHORTCUT_ACTIONS) {
    const want = normalizeCombo(bindings[a] ?? "");
    if (want && want === got) return a;
  }
  return null;
}

/** 快捷键的展示 / 遍历顺序（也是重复绑定时的优先级顺序）。 */
export const SHORTCUT_ACTIONS: ShortcutAction[] = ["selectAll", "invert", "confirm", "clear"];

/** 展示用：把 `ctrl+shift+a` 变成 `Ctrl+Shift+A`。 */
export function formatCombo(raw: string | null | undefined): string {
  const n = normalizeCombo(raw ?? "");
  if (!n) return "未绑定";
  return n
    .split("+")
    .map((p) => {
      if (p === "ctrl") return "Ctrl";
      if (p === "alt") return "Alt";
      if (p === "shift") return "Shift";
      if (p === "meta") return "Win";
      if (p.length === 1) return p.toUpperCase();
      if (/^f\d{1,2}$/.test(p)) return p.toUpperCase();
      return p.charAt(0).toUpperCase() + p.slice(1);
    })
    .join("+");
}
