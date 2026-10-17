/**
 * 全局快捷键：把 `utils/shortcuts` 的纯函数接到 `document` 的 keydown 上。
 *
 * 刻意**不用** Vue 模板的 `@keydown`：模板只能挂在某个元素上，而快捷键要
 * 在整个窗口生效。这里统一在一个地方监听，由调用方决定「现在该不该响应」。
 *
 * 两个必须处理的坑：
 * 1. **输入框里不能抢键**：用户在搜索框里按 `Ctrl+A` 是想全选文本，不是全选
 *    条目。所以命中 `input / textarea / [contenteditable]` 时直接放行；
 * 2. **弹窗优先**：候选面板打开时，快捷键应该作用于面板而不是背后的列表，
 *    所以提供了 `active` 开关，关掉时整组快捷键不响应。
 */

import { onBeforeUnmount, onMounted, type Ref } from "vue";
import { actionFor, type ShortcutAction } from "../utils/shortcuts";

export interface HotkeyOptions {
  /** 快捷键是否生效（弹窗打开时通常要把页面的那组关掉）。 */
  active?: Ref<boolean>;
  /** 命中后要做的事；返回 `true` 表示已消费（会 `preventDefault`）。 */
  handlers: Partial<Record<ShortcutAction, () => void>>;
  /** 绑定表（`动作 → 组合键字符串`），由调用方从设置里取。 */
  bindings: () => Partial<Record<ShortcutAction, string>>;
}

/** 焦点在可输入元素里时，快捷键一律不响应（避免抢掉 `Ctrl+A` 这类系统键）。 */
function inTextField(target: EventTarget | null): boolean {
  const el = target as HTMLElement | null;
  if (!el || typeof el.tagName !== "string") return false;
  const tag = el.tagName.toLowerCase();
  if (tag === "input" || tag === "textarea" || tag === "select") return true;
  return el.isContentEditable === true;
}

/**
 * 注册一组全局快捷键；组件卸载时自动摘掉监听。
 *
 * `bindings` 写成函数是为了**每次按键都取最新值**——用户在设置页改完快捷键
 * 之后，不用重开界面就能生效。
 */
export function useHotkeys(o: HotkeyOptions): void {
  function onKeydown(e: KeyboardEvent): void {
    if (o.active && !o.active.value) return;
    if (inTextField(e.target)) return;
    const action = actionFor(e, o.bindings());
    if (!action) return;
    const fn = o.handlers[action];
    if (!fn) return;
    // 消费掉：否则 `Ctrl+A` 会顺带把页面文本全选、`Esc` 会有别的默认行为
    e.preventDefault();
    e.stopPropagation();
    fn();
  }

  onMounted(() => {
    document.addEventListener("keydown", onKeydown);
  });
  onBeforeUnmount(() => {
    document.removeEventListener("keydown", onKeydown);
  });
}
