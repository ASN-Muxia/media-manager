import { describe, expect, it } from "vitest";
import { mount } from "@vue/test-utils";
import CollapsibleSection from "./CollapsibleSection.vue";

/**
 * 折叠容器的行为。
 *
 * 重点是三件事：
 * 1. 外面传 `open` 进来，点标题要 emit `update:open`（**组件自己不存状态**，
 *    否则同一份展开状态会在两个地方各存一份，迟早对不上）；
 * 2. `aria-expanded` 要跟着走，读屏才知道现在是开是合；
 * 3. 标题右侧「额外操作」的点击**不能**顺带把栏收起来（分类栏的「新建」就挂在
 *    那个位置，点 ＋ 却把整个栏收掉是最让人恼火的那类 bug）。
 */
function mountAt(open: boolean, slots: Record<string, string> = {}) {
  return mount(CollapsibleSection, {
    props: { title: "分类", open },
    slots: { default: "<p class='probe'>内容</p>", ...slots },
  });
}

describe("CollapsibleSection", () => {
  it("open 为 true 时内容区不带收起标记，且 aria-expanded 为真", () => {
    const w = mountAt(true);
    expect(w.find(".csection-body").classes()).not.toContain("is-collapsed");
    expect(w.find(".csection-toggle").attributes("aria-expanded")).toBe("true");
    expect(w.find(".probe").exists()).toBe(true);
  });

  it("open 为 false 时内容区标记为收起，箭头也跟着转", () => {
    const w = mountAt(false);
    expect(w.find(".csection-body").classes()).toContain("is-collapsed");
    expect(w.find(".csection-chev").classes()).toContain("is-collapsed");
    expect(w.find(".csection-toggle").attributes("aria-expanded")).toBe("false");
  });

  it("点标题发出 update:open，且取反（收起↔展开）", async () => {
    const w = mountAt(true);
    await w.find(".csection-toggle").trigger("click");
    expect(w.emitted("update:open")).toEqual([[false]]);

    const w2 = mountAt(false);
    await w2.find(".csection-toggle").trigger("click");
    expect(w2.emitted("update:open")).toEqual([[true]]);
  });

  it("组件自己不改状态：不 emit 就一直是外面给的那个值", async () => {
    const w = mountAt(true);
    await w.find(".csection-toggle").trigger("click");
    // 父组件没同步 props，DOM 应当仍然是展开的
    expect(w.find(".csection-body").classes()).not.toContain("is-collapsed");
  });

  it("点标题右侧的额外操作不会把栏收起来", async () => {
    const w = mountAt(true, {
      extra: "<button class='plus'>＋</button>",
    });
    await w.find(".plus").trigger("click");
    expect(w.emitted("update:open")).toBeUndefined();
  });

  it("没有额外插槽时不渲染右侧那一格", () => {
    const w = mountAt(true);
    expect(w.find(".csection-extra").exists()).toBe(false);
  });
});
