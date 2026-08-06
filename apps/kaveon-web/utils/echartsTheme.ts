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
    legend: {
      ...(option.legend || {}),
      textStyle: { color: muted, ...(option.legend?.textStyle || {}) },
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

  return themed;
}
