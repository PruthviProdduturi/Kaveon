/**
 * ECharts dark mode defaults.
 * Merges theme-aware axis/legend/tooltip colors into any ECharts option.
 * Call this before passing options to ReactECharts.
 */

const DARK_TEXT = "#e2e8f0";
const DARK_MUTED = "#64748b";
const DARK_BORDER = "rgba(255,255,255,0.06)";
const DARK_BG = "#0c0c0f";
const DARK_TOOLTIP_BG = "#1a1a2e";

const LIGHT_TEXT = "#1a1a2e";
const LIGHT_MUTED = "#94a3b8";
const LIGHT_BORDER = "#e2e8f0";
const LIGHT_BG = "#ffffff";
const LIGHT_TOOLTIP_BG = "#ffffff";

export function applyChartTheme(option: any, isDark: boolean): any {
  const text = isDark ? DARK_TEXT : LIGHT_TEXT;
  const muted = isDark ? DARK_MUTED : LIGHT_MUTED;
  const border = isDark ? DARK_BORDER : LIGHT_BORDER;
  const bg = isDark ? DARK_BG : LIGHT_BG;
  const tooltipBg = isDark ? DARK_TOOLTIP_BG : LIGHT_TOOLTIP_BG;

  // Chart-text colors are hardcoded (light-mode slate) in the option builders,
  // so they render dark-on-dark. Remap the known ones to theme tokens so labels,
  // donut-center totals, etc. stay readable in both themes.
  const remap = (c: any): any => {
    const k = typeof c === "string" ? c.toLowerCase() : "";
    if (["#0f172a", "#111827", "#1e293b", "#334155", "#475569"].includes(k)) return text;
    if (["#64748b", "#94a3b8", "#6b7280", "#9ca3af"].includes(k)) return muted;
    if (["#cbd5e1", "#e2e8f0", "#d1d5db"].includes(k)) return border;
    return c;
  };

  const axisDefaults = {
    axisLine: { lineStyle: { color: border } },
    axisLabel: { color: muted },
    splitLine: { lineStyle: { color: border } },
    nameTextStyle: { color: muted },
  };

  const themed: any = {
    ...option,
    backgroundColor: "transparent",
    textStyle: { color: text, ...(option.textStyle || {}) },
    // Title defaults to the theme's text colour (white in dark, dark in light).
    // An explicit colour from the title colour-picker lands in
    // option.title.textStyle.color and overrides this default.
    ...(option.title
      ? {
          title: {
            ...option.title,
            textStyle: { ...(option.title.textStyle || {}), color: remap(option.title.textStyle?.color) ?? text },
          },
        }
      : {}),
    legend: {
      ...(option.legend || {}),
      textStyle: { ...(option.legend?.textStyle || {}), color: remap(option.legend?.textStyle?.color) ?? muted },
    },
    tooltip: {
      ...(option.tooltip || {}),
      backgroundColor: tooltipBg,
      borderColor: border,
      textStyle: { color: text, ...(option.tooltip?.textStyle || {}) },
    },
  };

  // Apply axis defaults to xAxis/yAxis (single or array)
  const applyAxis = (axis: any) => {
    if (!axis) return axis;
    if (Array.isArray(axis)) return axis.map((a: any) => ({ ...axisDefaults, ...a }));
    return { ...axisDefaults, ...axis };
  };

  if (option.xAxis) themed.xAxis = applyAxis(option.xAxis);
  if (option.yAxis) themed.yAxis = applyAxis(option.yAxis);

  // Radar axis
  if (option.radar) {
    themed.radar = {
      ...option.radar,
      axisLine: { lineStyle: { color: border }, ...(option.radar?.axisLine || {}) },
      splitLine: { lineStyle: { color: border }, ...(option.radar?.splitLine || {}) },
      axisName: { color: muted, ...(option.radar?.axisName || {}) },
    };
  }

  // Graphic text (e.g. the donut-centre TOTAL/value) — remap hardcoded dark fills.
  if (Array.isArray(option.graphic)) {
    themed.graphic = option.graphic.map((g: any) =>
      g?.type === "text" && g.style?.fill
        ? { ...g, style: { ...g.style, fill: remap(g.style.fill) } }
        : g
    );
  }

  // Series labels / label lines (pie & donut slice values, bar labels, etc.).
  if (Array.isArray(option.series)) {
    themed.series = option.series.map((s: any) => {
      if (!s || typeof s !== "object") return s;
      const ns: any = { ...s };
      if (s.label?.color) ns.label = { ...s.label, color: remap(s.label.color) };
      if (s.labelLine?.lineStyle?.color) {
        ns.labelLine = { ...s.labelLine, lineStyle: { ...s.labelLine.lineStyle, color: remap(s.labelLine.lineStyle.color) } };
      }
      return ns;
    });
  }

  return themed;
}
