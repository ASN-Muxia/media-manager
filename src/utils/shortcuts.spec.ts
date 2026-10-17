import { describe, expect, it } from "vitest";
import {
  DEFAULT_SHORTCUTS,
  actionFor,
  comboFromEvent,
  formatCombo,
  matchesCombo,
  normalizeCombo,
} from "./shortcuts";

/** jsdom 里没有真正的 `KeyboardEvent` 构造细节差异，这里统一手造。 */
function ev(key: string, mods: Partial<Record<"ctrlKey" | "altKey" | "shiftKey" | "metaKey", boolean>> = {}) {
  return {
    key,
    ctrlKey: Boolean(mods.ctrlKey),
    altKey: Boolean(mods.altKey),
    shiftKey: Boolean(mods.shiftKey),
    metaKey: Boolean(mods.metaKey),
  } as KeyboardEvent;
}

describe("normalizeCombo", () => {
  it("大小写与分隔符不敏感，输出固定为 修饰键+主键", () => {
    expect(normalizeCombo("Ctrl+A")).toBe("ctrl+a");
    expect(normalizeCombo("ctrl+a")).toBe("ctrl+a");
    expect(normalizeCombo("Ctrl Shift A")).toBe("ctrl+shift+a");
    expect(normalizeCombo("Shift+Ctrl+a")).toBe("ctrl+shift+a");
  });

  it("修饰键顺序固定，A 与 Shift+A 是两回事", () => {
    expect(normalizeCombo("Ctrl+Shift+A")).not.toBe(normalizeCombo("Ctrl+A"));
  });

  it("具名键统一写法", () => {
    expect(normalizeCombo("Esc")).toBe("esc");
    expect(normalizeCombo("escape")).toBe("esc");
    expect(normalizeCombo("Ctrl+Enter")).toBe("ctrl+enter");
    expect(normalizeCombo("ctrl + space")).toBe("ctrl+space");
    expect(normalizeCombo("F5")).toBe("f5");
  });

  it("空串 / 只有修饰键 / 非法主键 → 未绑定", () => {
    expect(normalizeCombo("")).toBeNull();
    expect(normalizeCombo("   ")).toBeNull();
    expect(normalizeCombo("Ctrl")).toBeNull();
    expect(normalizeCombo("Ctrl+")).toBeNull();
  });
});

describe("comboFromEvent", () => {
  it("从键盘事件读出规范组合键", () => {
    expect(comboFromEvent(ev("a", { ctrlKey: true }))).toBe("ctrl+a");
    expect(comboFromEvent(ev("A", { ctrlKey: true, shiftKey: true }))).toBe("ctrl+shift+a");
    expect(comboFromEvent(ev("Escape"))).toBe("esc");
    expect(comboFromEvent(ev("Enter", { ctrlKey: true }))).toBe("ctrl+enter");
  });

  it("`+` 与 `-` 不当主键（它们是分隔符）", () => {
    expect(comboFromEvent(ev("+"))).toBeNull();
    expect(comboFromEvent(ev("-"))).toBeNull();
  });
});

describe("matchesCombo", () => {
  it("未绑定的组合键恒不命中（避免把每个按键都当成快捷键）", () => {
    expect(matchesCombo(ev("a", { ctrlKey: true }), "")).toBe(false);
    expect(matchesCombo(ev("a", { ctrlKey: true }), null)).toBe(false);
    expect(matchesCombo(ev("a", { ctrlKey: true }), undefined)).toBe(false);
  });

  it("修饰键必须完全对上：多按一个 Shift 就不算", () => {
    expect(matchesCombo(ev("a", { ctrlKey: true }), "Ctrl+A")).toBe(true);
    expect(matchesCombo(ev("a", { ctrlKey: true, shiftKey: true }), "Ctrl+A")).toBe(false);
    expect(matchesCombo(ev("a", { ctrlKey: true, shiftKey: true }), "Ctrl+Shift+A")).toBe(true);
  });

  it("默认绑定自洽：默认表里的键都能被对应事件命中", () => {
    expect(matchesCombo(ev("a", { ctrlKey: true }), DEFAULT_SHORTCUTS.selectAll)).toBe(true);
    expect(
      matchesCombo(ev("a", { ctrlKey: true, shiftKey: true }), DEFAULT_SHORTCUTS.invert),
    ).toBe(true);
    expect(matchesCombo(ev("Enter", { ctrlKey: true }), DEFAULT_SHORTCUTS.confirm)).toBe(true);
    expect(matchesCombo(ev("Escape"), DEFAULT_SHORTCUTS.clear)).toBe(true);
  });
});

describe("actionFor", () => {
  it("命中对应动作", () => {
    const b = { ...DEFAULT_SHORTCUTS };
    expect(actionFor(ev("a", { ctrlKey: true }), b)).toBe("selectAll");
    expect(actionFor(ev("a", { ctrlKey: true, shiftKey: true }), b)).toBe("invert");
    expect(actionFor(ev("Enter", { ctrlKey: true }), b)).toBe("confirm");
    expect(actionFor(ev("Escape"), b)).toBe("clear");
  });

  it("没绑定的按键不触发任何动作", () => {
    expect(actionFor(ev("b"), { ...DEFAULT_SHORTCUTS })).toBeNull();
    expect(actionFor(ev("a", { ctrlKey: true }), { selectAll: "" })).toBeNull();
  });

  /** 一个键只能触发一件事：重复绑定时按表里顺序取第一个，不能触发两次。 */
  it("重复绑定只认一个动作", () => {
    const b = { selectAll: "Ctrl+A", invert: "Ctrl+A" };
    expect(actionFor(ev("a", { ctrlKey: true }), b)).toBe("selectAll");
  });
});

describe("formatCombo", () => {
  it("展示成人类可读的形式", () => {
    expect(formatCombo("ctrl+shift+a")).toBe("Ctrl+Shift+A");
    expect(formatCombo("ctrl+enter")).toBe("Ctrl+Enter");
    expect(formatCombo("esc")).toBe("Esc");
    expect(formatCombo("f5")).toBe("F5");
  });

  it("空值显示「未绑定」而不是空白", () => {
    expect(formatCombo("")).toBe("未绑定");
    expect(formatCombo(null)).toBe("未绑定");
  });
});
