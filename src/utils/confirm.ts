/**
 * 统一的二次确认入口（**删除 / 清空这类破坏性操作必须走这里**）。
 *
 * ## 为什么不能再用 `window.confirm`
 *
 * `tauri-plugin-dialog` 会在页面初始化时注入脚本，把 `window.confirm` 覆盖成
 * 调用 `plugin:dialog|confirm`。而这个命令在插件 2.x 里已经并进了 `message`，
 * 默认权限集 `dialog:default`（= `allow-message` / `allow-save` / `allow-open`）
 * 里根本没有它 —— 于是**每一次**调用都会 reject：
 *
 * ```
 * unhandledrejection: dialog.confirm not allowed. Command not found
 * ```
 *
 * ## 更危险的副作用：确认被整个绕过
 *
 * 覆盖后的 `window.confirm` 是 **async**，返回的是 Promise。而老写法是同步的：
 *
 * ```ts
 * const ok = window.confirm("确定删除？");
 * if (!ok) return;          // Promise 恒为 truthy → 这行永远不成立
 * await removeSelected();   // 于是直接删了，连问都不问
 * ```
 *
 * 所以这里的规矩是：**必须 await**，且拿不到明确答复时一律按「取消」处理 ——
 * 宁可让用户删不掉，也绝不能绕过确认直接删。
 */
import { confirm as nativeConfirm } from "@tauri-apps/plugin-dialog";

/**
 * 弹出原生确认框，返回用户是否点了「确定」。
 *
 * 主路径用插件的 `confirm()`：它走的是**已授权**的 `plugin:dialog|message`，
 * 与 `window.confirm` 那个坏掉的路径不是一回事。
 */
export async function askConfirm(message: string, title = "请确认"): Promise<boolean> {
  try {
    return await nativeConfirm(message, { title, kind: "warning" });
  } catch {
    // 原生对话框用不了（权限缺失 / 插件异常）时再退一步走 window.confirm。
    // 它可能已经被覆盖成返回 Promise 的坏版本，所以两种返回形态都要接住；
    // 再失败就按「未确认」处理。
    try {
      const raw = (window.confirm as unknown as (m: string) => unknown)(message);
      if (raw instanceof Promise) {
        return Boolean(await raw.catch(() => false));
      }
      return Boolean(raw);
    } catch {
      return false;
    }
  }
}
