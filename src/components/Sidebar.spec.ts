import { beforeEach, describe, expect, it, vi } from "vitest";
import { mount } from "@vue/test-utils";
import { createPinia, setActivePinia } from "pinia";
import Sidebar from "./Sidebar.vue";
import CollapsibleSection from "./CollapsibleSection.vue";
import { useCategoryStore } from "../stores/categoryStore";
import type { Category, Tag } from "../api/commands";

// Sidebar 的删除按钮要用确认框，这里挡掉，免得单测依赖真实插件
vi.mock("@tauri-apps/plugin-dialog", () => ({ confirm: vi.fn(async () => false) }));

function catItem(id: number, name: string): Category {
  return { id, name, kind: "manual", origin: "manual", sortOrder: id, seriesCount: 1 };
}
function tagItem(id: number, name: string): Tag {
  return { id, name, usageCount: 2 };
}

/**
 * 侧栏「分类」「标签」两栏的折叠。
 *
 * 最要紧的是**两栏各管各的**：收起分类不该动标签，反之亦然。
 * 另外默认都得是展开的——侧栏是主导航，藏起来等于让人找不到入口。
 */
describe("Sidebar 的分类 / 标签折叠", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    const cat = useCategoryStore();
    cat.categories = [catItem(1, "国产剧"), catItem(2, "纪录片")];
    cat.tags = [tagItem(1, "4K"), tagItem(2, "治愈")];
  });

  function mounted() {
    return mount(Sidebar, { props: { modelValue: "all" } });
  }

  it("默认两栏都展开", () => {
    const w = mounted();
    const sections = w.findAllComponents(CollapsibleSection);
    expect(sections).toHaveLength(2);
    const states = sections.map((s) => s.props("open"));
    expect(states).toEqual([true, true]);
    for (const s of sections) {
      expect(s.find(".csection-body").classes()).not.toContain("is-collapsed");
    }
  });

  it("收起分类不会影响标签", async () => {
    const w = mounted();
    const [catSec, tagSec] = w.findAllComponents(CollapsibleSection);

    await catSec.find(".csection-toggle").trigger("click");

    expect(catSec.props("open")).toBe(false);
    expect(catSec.find(".csection-body").classes()).toContain("is-collapsed");
    // 标签那一栏必须纹丝不动
    expect(tagSec.props("open")).toBe(true);
    expect(tagSec.find(".csection-body").classes()).not.toContain("is-collapsed");
  });

  it("收起标签不会影响分类", async () => {
    const w = mounted();
    const [catSec, tagSec] = w.findAllComponents(CollapsibleSection);

    await tagSec.find(".csection-toggle").trigger("click");

    expect(tagSec.props("open")).toBe(false);
    expect(catSec.props("open")).toBe(true);
    expect(catSec.find(".csection-body").classes()).not.toContain("is-collapsed");
  });

  it("两栏都收起后再各自展开，互不清空", async () => {
    const w = mounted();
    const [catSec, tagSec] = w.findAllComponents(CollapsibleSection);

    await catSec.find(".csection-toggle").trigger("click");
    await tagSec.find(".csection-toggle").trigger("click");
    expect([catSec.props("open"), tagSec.props("open")]).toEqual([false, false]);

    // 只把分类点回来，标签应当还收着
    await catSec.find(".csection-toggle").trigger("click");
    expect([catSec.props("open"), tagSec.props("open")]).toEqual([true, false]);
  });

  it("分类栏的「新建」按钮不会把栏收起来", async () => {
    const w = mounted();
    const [catSec] = w.findAllComponents(CollapsibleSection);

    await catSec.find(".plus").trigger("click");

    expect(catSec.props("open")).toBe(true);
    // 新建输入框倒是应该出现了
    expect(catSec.find(".mini-input").exists()).toBe(true);
  });

  it("收起后标题栏仍然在（只是内容区被压平）", async () => {
    const w = mounted();
    const [catSec] = w.findAllComponents(CollapsibleSection);

    await catSec.find(".csection-toggle").trigger("click");

    expect(catSec.find(".csection-head").exists()).toBe(true);
    expect(catSec.find(".csection-title").text()).toBe("分类");
  });
});
