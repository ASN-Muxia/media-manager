import { beforeEach, describe, expect, it, vi } from "vitest";

/**
 * `askConfirm` 的兜底行为。
 *
 * 重点不是「能不能弹框」，而是**绝不能把「没确认」当成「已确认」** ——
 * 线上踩过：`window.confirm` 被 dialog 插件覆盖成 async 版本后返回 Promise，
 * 同步写法 `if (!ok) return` 里的 `ok` 恒为 truthy，二次确认被整个绕过，
 * 点一下「删除」就直接删。
 */
const dialog = vi.hoisted(() => ({ confirm: vi.fn() }));
vi.mock("@tauri-apps/plugin-dialog", () => dialog);

import { askConfirm } from "./confirm";

describe("askConfirm", () => {
  beforeEach(() => {
    vi.restoreAllMocks();
    dialog.confirm.mockReset();
  });

  it("用户点确定 → true", async () => {
    dialog.confirm.mockResolvedValue(true);
    expect(await askConfirm("确定删除？")).toBe(true);
  });

  it("用户点取消 → false", async () => {
    dialog.confirm.mockResolvedValue(false);
    expect(await askConfirm("确定删除？")).toBe(false);
  });

  it("原生对话框不可用 → 退回 window.confirm 的布尔结果", async () => {
    dialog.confirm.mockRejectedValue(new Error("dialog.confirm not allowed"));
    vi.spyOn(window, "confirm").mockReturnValue(true);
    expect(await askConfirm("确定删除？")).toBe(true);
  });

  it("退回的 window.confirm 也是坏的（返回 reject 的 Promise）→ 按取消处理", async () => {
    dialog.confirm.mockRejectedValue(new Error("dialog.confirm not allowed"));
    // 这就是线上真实形态：window.confirm 被插件覆盖成 async，且调用必然失败
    vi.spyOn(window, "confirm").mockImplementation(
      () => Promise.reject(new Error("dialog.confirm not allowed")) as never,
    );
    expect(await askConfirm("确定删除？")).toBe(false);
  });

  it("两条路都炸了 → 也按取消处理，绝不放行", async () => {
    dialog.confirm.mockRejectedValue(new Error("boom"));
    vi.spyOn(window, "confirm").mockImplementation(() => {
      throw new Error("boom");
    });
    expect(await askConfirm("确定删除？")).toBe(false);
  });
});
