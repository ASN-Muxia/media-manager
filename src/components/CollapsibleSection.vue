<script setup lang="ts">
/**
 * 可折叠的小节：一个带头部的容器，点标题就展开 / 收起内容。
 *
 * 用在侧栏的「分类 / 标签」两栏、以及详情页的归类卡片上——两处视觉密度不一样，
 * 所以颜色、字号一律**继承所在面板**，只把这几项交给 CSS 变量调整：
 * `--cs-head-pad`（标题行内边距）、`--cs-body-gap`（内容行间距）、
 * `--cs-title-fs` / `--cs-title-fw` / `--cs-title-color`（标题的字号字重与颜色）。
 *
 * ## 动画为什么用 grid 而不是 max-height
 *
 * `grid-template-rows: 1fr → 0fr` 是纯 CSS 的：不用 JS 去量内容高度，
 * 也不用给 max-height 估一个「够大但别太大」的魔法数字（内容少了动画会迟到，
 * 内容多了又会突然截断）。内容多高都跟手。
 *
 * ## 收起后不能还让键盘 Tab 进去
 *
 * 高度压到 0 只是**看不见**，里面的按钮仍然可被 Tab 聚焦、被读屏念出来。
 * 所以收起时给内容加 `visibility: hidden`，并把 transition 延迟到动画结束，
 * 免得刚点下去内容就瞬间消失。
 */
const props = defineProps<{ title: string; open: boolean }>();
const emit = defineEmits<{ (e: "update:open", v: boolean): void }>();

function toggle(): void {
  emit("update:open", !props.open);
}
</script>

<template>
  <div class="csection">
    <div class="csection-head">
      <button
        type="button"
        class="csection-toggle"
        :aria-expanded="open"
        @click="toggle"
      >
        <span class="csection-chev" :class="{ 'is-collapsed': !open }" aria-hidden="true">▾</span>
        <span class="csection-title" role="heading" aria-level="3">{{ title }}</span>
      </button>
      <!-- 标题行右侧的额外操作（比如分类栏的「新建」）；点它不该触发折叠 -->
      <span v-if="$slots.extra" class="csection-extra"><slot name="extra" /></span>
    </div>

    <div class="csection-body" :class="{ 'is-collapsed': !open }">
      <div class="csection-inner">
        <slot />
      </div>
    </div>
  </div>
</template>

<style scoped>
.csection { display: flex; flex-direction: column; }

.csection-head {
  display: flex; align-items: center; justify-content: space-between; gap: 4px;
  padding: var(--cs-head-pad, 6px 12px 4px);
  color: inherit;
}

.csection-toggle {
  display: flex; align-items: center; gap: 6px;
  flex: 1; min-width: 0;
  padding: 2px 0;
  border: 0; background: none; cursor: pointer;
  font: inherit;
  font-size: var(--cs-title-fs, inherit);
  font-weight: var(--cs-title-fw, inherit);
  color: var(--cs-title-color, inherit);
  text-align: left;
  user-select: none;
  /* 移动端：别让双击缩放 / 长按选中打扰折叠 */
  touch-action: manipulation;
  -webkit-tap-highlight-color: transparent;
}
.csection-toggle:hover .csection-title { color: var(--text); }
.csection-toggle:focus-visible { outline: 1px solid var(--accent); outline-offset: 2px; }
.csection-title { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }

/* ▾ 转 -90° 就是 ▸，比换字符更平滑（字符宽度不一样会抖） */
.csection-chev {
  flex: none; font-size: 10px; line-height: 1; opacity: 0.8;
  transition: transform 0.22s ease;
}
.csection-chev.is-collapsed { transform: rotate(-90deg); }

.csection-extra { flex: none; display: flex; align-items: center; gap: 2px; }

.csection-body {
  display: grid;
  grid-template-rows: 1fr;
  transition: grid-template-rows 0.22s ease;
}
.csection-body.is-collapsed { grid-template-rows: 0fr; }

.csection-inner {
  display: flex; flex-direction: column; gap: var(--cs-body-gap, 2px);
  min-height: 0; overflow: hidden;
  visibility: visible;
  transition: visibility 0s linear 0s;
}
.csection-body.is-collapsed > .csection-inner {
  visibility: hidden;
  transition-delay: 0.22s;
}

/* 触屏：把标题行撑到手指够得着的大小 */
@media (pointer: coarse) {
  .csection-toggle { padding: 8px 0; }
}

@media (prefers-reduced-motion: reduce) {
  .csection-body, .csection-chev, .csection-inner { transition: none; }
}
</style>
