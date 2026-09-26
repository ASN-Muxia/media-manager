/**
 * 媒体类型的展示常量与格式化。
 *
 * 以前「类型 → 中文名」「类型 → 图标」「评分格式化」在 `PosterCard`、`MediaDetail`、
 * `Sidebar` 里各写了一遍，同一套文案散落四处。改动时漏一处就会出现
 * 「列表显示剧集、详情页显示电影」这种不一致，所以收成一份。
 *
 * 侧栏的 `VIEW_*` 键名与媒体 `type` 取值同名（`movie` / `tv` / `unknown`），
 * 因此两边可以共用同一张表；只有 `all` / `collections` 这类纯导航键才属于侧栏自己。
 */

/** 媒体类型的中文名。判定不出来（`unknown`）必须显示「待确认」，不能冒充电影。 */
export const MEDIA_TYPE_LABEL: Record<string, string> = {
  movie: "电影",
  tv: "剧集",
  unknown: "待确认",
};

/** 类型对应的图标（用作占位图与侧栏图标）。 */
export const MEDIA_TYPE_ICON: Record<string, string> = {
  movie: "🎬",
  tv: "📺",
  unknown: "❓",
};

/** 类型换行成一个标签。未知取值回落「电影」，与后端默认判定保持一致。 */
export function typeLabel(type?: string | null): string {
  return (type ? MEDIA_TYPE_LABEL[type] : undefined) ?? "电影";
}

/** 类型对应的图标，取值不认识时回落电影图标。 */
export function typeIcon(type?: string | null): string {
  return (type ? MEDIA_TYPE_ICON[type] : undefined) ?? "🎬";
}

/** 评分格式化：0 分与空值都不该显示成 `0.0`。 */
export function formatScore(v?: number | null): string | null {
  return v ? v.toFixed(1) : null;
}
