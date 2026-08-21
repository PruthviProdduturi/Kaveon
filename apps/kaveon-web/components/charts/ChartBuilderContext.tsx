
import React, { createContext, useContext, useCallback, useEffect, useMemo, useRef, useState } from "react";

import { API_BASE } from "../../config";
import { msalFetch, msalFetchRetry } from "../../utils/msalFetch";
import { useAuth } from "../../auth/useAuth";
import { useRouter } from "next/navigation";
import { getRegisteredPlugins, getPlugin } from "./chartPluginRegistry";
import { acquireQuerySlot } from "../../utils/querySemaphore";

// ── Client-side query result cache ──────────────────────────────────────────
// Module-level so it survives component unmounts (navigation).
// Dashboard charts hit this before the API — instant repeat views.
const _CLIENT_CACHE = new Map<string, { result: any; ts: number }>();
const CLIENT_CACHE_TTL = 300_000; // 5 min, matches server TTL

function clientCacheKey(database: string, sql: string): string {
  let h = 0;
  const s = `${database}\0${sql}`;
  for (let i = 0; i < s.length; i++) h = ((h << 5) - h + s.charCodeAt(i)) | 0;
  return h.toString(36);
}

function clientCacheGet(key: string): any | null {
  const entry = _CLIENT_CACHE.get(key);
  if (entry && Date.now() - entry.ts < CLIENT_CACHE_TTL) return entry.result;
  if (entry) _CLIENT_CACHE.delete(key);
  return null;
}

function clientCacheSet(key: string, result: any): void {
  if (_CLIENT_CACHE.size > 200) {
    const now = Date.now();
    for (const [k, v] of _CLIENT_CACHE) {
      if (now - v.ts > CLIENT_CACHE_TTL) _CLIENT_CACHE.delete(k);
    }
  }
  _CLIENT_CACHE.set(key, { result, ts: Date.now() });
}

// Default advanced chart options for merging with loaded config
export const DEFAULT_ADVANCED_OPTIONS = {
  title: "",
  titleFont: "sans-serif",
  titleSize: "20",
  xAxis: { show: true, name: "" },
  yAxis: { show: true, name: "" },
  // Vibrant, graceful default palette (indigo→pink→teal→amber…) so charts are
  // colourful out of the box instead of the muted ECharts blue-heavy default.
  color: ["#6366f1", "#ec4899", "#14b8a6", "#f59e0b", "#8b5cf6", "#06b6d4", "#ef4444", "#10b981", "#f97316", "#3b82f6", "#a855f7", "#84cc16"],
  legend: { show: true, left: "top", order: undefined as string[] | undefined },
  tooltip: { show: true, dateFormat: "auto", numberFormat: "none", decimalPlaces: 2, useCommas: true },
  yAxisFormat: "none",
  xAxisFormat: "none",     // Number format for value X axis (horizontal bar / scatter)
  xAxisDateFormat: "auto", // Date format for category X axis
  yAxisDateFormat: "auto", // Date format for category Y axis (horizontal bars)
  labelLayout: undefined as { hideOverlap?: boolean; moveOverlap?: string } | undefined,
  series: [],
  seriesSettings: undefined as { smooth?: boolean; symbol?: string | Function; symbolSize?: number; stack?: string } | undefined,
  showAsPercentage: undefined as boolean | undefined,
  /** Chart-type-specific options (bar labels, pie hole size, KPI format, etc.) */
  chartTypeOptions: undefined as Record<string, any> | undefined,
};

/**
 * Abbreviate large numbers as K / M / B / T so charts and dashboards stay
 * readable (47,072,000 -> "47.1M"). Values below 1,000 are shown as-is (so
 * percentages, temperatures, ratios keep their precision).
 */
export function abbreviateNumber(v: unknown): string {
  const n = Number(v);
  if (!Number.isFinite(n)) return String(v ?? "");
  const abs = Math.abs(n);
  const trim = (s: string) => s.replace(/\.0$/, "");
  if (abs >= 1e12) return trim((n / 1e12).toFixed(1)) + "T";
  if (abs >= 1e9) return trim((n / 1e9).toFixed(1)) + "B";
  if (abs >= 1e6) return trim((n / 1e6).toFixed(1)) + "M";
  if (abs >= 1e4) return trim((n / 1e3).toFixed(1)) + "K"; // only abbreviate 10k+ so ELO/scores/small counts (e.g. 1,400) stay exact
  return n.toLocaleString(undefined, { maximumFractionDigits: 2 });
}

/**
 * Post-process a built ECharts option so every numeric ("value") axis and the
 * tooltip abbreviate large numbers by default — unless the chart already set its
 * own formatter (e.g. a percentage share chart). Applied to the final option so
 * it covers bar/line/area/scatter/etc. without touching each case individually.
 */
export function applyNumberAbbreviation(option: any): any {
  if (!option || typeof option !== "object") return option;
  const fmtAxis = (ax: any) => {
    (Array.isArray(ax) ? ax : [ax]).forEach((a: any) => {
      if (a && a.type === "value") {
        a.axisLabel = { ...(a.axisLabel || {}) };
        if (!a.axisLabel.formatter) a.axisLabel.formatter = (val: number) => abbreviateNumber(val);
      }
    });
  };
  fmtAxis(option.xAxis);
  fmtAxis(option.yAxis);
  if (option.tooltip && !Array.isArray(option.tooltip)) {
    if (!option.tooltip.formatter && !option.tooltip.valueFormatter) {
      option.tooltip.valueFormatter = (val: unknown) => abbreviateNumber(val);
    }
  }
  return option;
}

/**
 * @deprecated No longer needed - dashboards now use ChartBuilderProvider + ChartPreview directly
 */
export function buildEChartsOptionsFromQueryResult(
  chartKind: ChartKind,
  rows: (string | number | null)[][],
  columns: string[],
  advancedOptions: Partial<typeof DEFAULT_ADVANCED_OPTIONS>,
  queryConfig: any
): any | null {
  if (!columns || !rows || columns.length === 0) {
    return null;
  }

  // Prefer advancedOptions.xAxisDateFormat if set, then queryConfig
  const xAxisDateFormat =
    advancedOptions?.xAxisDateFormat ||
    (queryConfig?.date_display_format as DateDisplayFormat | undefined) ||
    "auto";

  const displayFormat: DateDisplayFormat = xAxisDateFormat as DateDisplayFormat;

  // Heuristics: last column is metric, a column containing "date"/"time" is x-axis,
  // remaining dimension columns define series keys.
  const metricIndex = columns.length - 1;
  let timeIndex = columns.findIndex((c) => /date|time|^year$/i.test(c));
  // Only assume a positional time axis for time-series chart kinds. For
  // categorical charts (bar/pie/donut/map) there is no time column — the first
  // non-metric column is the x-axis category, not a series.
  const _isTimeKind = /time_series|_line|_area|line_multi|area_stack/.test(chartKind);
  if (timeIndex === -1 && columns.length >= 2 && _isTimeKind) {
    timeIndex = 1;
  }
  // x-axis column: the time column when present, else the first column.
  const xIndex = timeIndex >= 0 ? timeIndex : 0;

  // Series = every column that is neither the x-axis nor the metric. For a plain
  // (dimension, metric) result this is empty → a single series named after the metric.
  const dimensionIndexes: number[] = [];
  columns.forEach((_, idx) => {
    if (idx !== metricIndex && idx !== xIndex) {
      dimensionIndexes.push(idx);
    }
  });

  const xValues: string[] = [];
  const seriesNamesSet = new Set<string>();
  const dataMap = new Map<string, Map<string, number>>();

  rows.forEach((row) => {
    const xRaw = timeIndex >= 0 ? row[timeIndex] : row[0];
    const x = xRaw == null ? "" : String(xRaw);
    if (!xValues.includes(x)) xValues.push(x);

    const metricRaw = row[metricIndex];
    const metric = metricRaw == null || metricRaw === "" ? 0 : Number(metricRaw);

    const seriesName =
      dimensionIndexes.length === 0
        ? columns[metricIndex] ?? "value"
        : dimensionIndexes.map((i) => row[i] ?? "").join(" - ");

    seriesNamesSet.add(seriesName);

    if (!dataMap.has(seriesName)) {
      dataMap.set(seriesName, new Map<string, number>());
    }
    const innerMap = dataMap.get(seriesName)!;
    const existing = innerMap.get(x) || 0;
    innerMap.set(x, existing + metric);
  });

  const seriesNames = Array.from(seriesNamesSet);

  // Determine legendOrder from advancedOptions
  const savedLegendOrder = advancedOptions?.legend?.order;
  const legendOrder =
    savedLegendOrder && Array.isArray(savedLegendOrder) && savedLegendOrder.length > 0
      ? savedLegendOrder
      : seriesNames;

  // Build helper functions for tooltips
  const formatXAxisLabel = (val: string, format: DateDisplayFormat): string => {
    if (format === "auto") return val;
    const parts = val.split("-");
    if (parts.length < 2) return val;
    const y = parseInt(parts[0], 10);
    const m = parseInt(parts[1], 10);
    const d = parts[2] ? parseInt(parts[2], 10) : 1;
    if (isNaN(y) || isNaN(m)) return val;
    const monthNames = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    const mName = monthNames[m - 1] || m.toString();
    if (format === "month_year") return `${mName} ${y}`;
    if (format === "date") return `${mName} ${d}, ${y}`;
    return val;
  };

  const formatTooltipNumber = (value: number, format: string, decimals: number, useCommas: boolean): string => {
    if (format === "k") return `${(value / 1e3).toFixed(decimals)}K`;
    if (format === "m") return `${(value / 1e6).toFixed(decimals)}M`;
    if (format === "b") return `${(value / 1e9).toFixed(decimals)}B`;
    if (format === "t") return `${(value / 1e12).toFixed(decimals)}T`;
    const fixed = value.toFixed(decimals);
    if (useCommas) {
      const parts = fixed.split(".");
      parts[0] = parts[0].replace(/\B(?=(\d{3})+(?!\d))/g, ",");
      return parts.join(".");
    }
    return fixed;
  };

  // Build line/bar series helper
  const buildLineOrBarSeries = (opts: { type: "line" | "bar"; stacked?: boolean; smooth?: boolean }): any[] => {
    const isTimeSeriesKind =
      chartKind === "time_series_line" ||
      chartKind === "time_series_area" ||
      chartKind === "time_series_line_share" ||
      chartKind === "time_series_area_share";
    const isAreaChart = chartKind.includes("area");
    const isShareChart = chartKind === "time_series_area_share" || chartKind === "time_series_line_share";
    const isStackedBarPercentage =
      (chartKind === "stacked_bar_vertical" || chartKind === "stacked_bar_horizontal") &&
      advancedOptions?.showAsPercentage === true;
    const isPercentageMode = isShareChart || isStackedBarPercentage;

    // For percentage modes, calculate totals for normalization
    const xTotals: Record<string, number> = {};
    if (isPercentageMode) {
      xValues.forEach((x) => {
        let total = 0;
        seriesNames.forEach((name) => {
          total += dataMap.get(name)?.get(x) ?? 0;
        });
        xTotals[x] = total;
      });
    }

    // Check if markers are enabled in saved settings
    let markersEnabled: boolean = false;
    if (advancedOptions) {
      if (advancedOptions.seriesSettings) {
        markersEnabled = !!(advancedOptions.seriesSettings.symbol && advancedOptions.seriesSettings.symbol !== "none");
      } else if (Array.isArray(advancedOptions.series)) {
        markersEnabled = advancedOptions.series.some((s: any) => s.showSymbol || (s.symbol && s.symbol !== "none"));
      }
    }

    // For share charts, check stacking from showAsPercentage
    let stackingOverride: boolean | undefined = undefined;
    let smoothOverride: boolean | undefined = undefined;
    if (isShareChart && advancedOptions) {
      stackingOverride = advancedOptions.showAsPercentage !== false;
      smoothOverride = Array.isArray(advancedOptions.series) && advancedOptions.series.some((s: any) => s.smooth);
    }

    return seriesNames.map((name) => ({
      name,
      type: opts.type,
      stack: (isShareChart ? (stackingOverride ? "total" : undefined) : (opts.stacked ? "total" : undefined)),
      smooth: (isShareChart ? !!smoothOverride : (opts.smooth ?? (opts.type === "line"))),
      symbol: opts.type === "line" && isTimeSeriesKind && markersEnabled ? 'circle' : "none",
      symbolSize: markersEnabled ? 6 : 4,
      showSymbol: opts.type === "line" && isTimeSeriesKind && markersEnabled
        ? function (dataIndex: number) {
            const dataLength = xValues.length;
            if (dataIndex === 0 || dataIndex === dataLength - 1) return true;
            const point1 = Math.round((dataLength - 1) * 1 / 5);
            const point2 = Math.round((dataLength - 1) * 2 / 5);
            const point3 = Math.round((dataLength - 1) * 3 / 5);
            const point4 = Math.round((dataLength - 1) * 4 / 5);
            if (dataIndex === point1 || dataIndex === point2 || dataIndex === point3 || dataIndex === point4) return true;
            return false;
          }
        : false,
      areaStyle: isAreaChart ? {} : undefined,
      label: markersEnabled
        ? {
            show: true,
            formatter: (params: any) => {
              const dataIndex = params.dataIndex;
              const dataLength = xValues.length;
              const point1 = Math.round((dataLength - 1) * 1 / 5);
              const point2 = Math.round((dataLength - 1) * 2 / 5);
              const point3 = Math.round((dataLength - 1) * 3 / 5);
              const point4 = Math.round((dataLength - 1) * 4 / 5);
              const isMarkerPoint = dataIndex === 0 || dataIndex === dataLength - 1 ||
                                    dataIndex === point1 || dataIndex === point2 ||
                                    dataIndex === point3 || dataIndex === point4;
              if (!isMarkerPoint) return "";
              const v = Number(params.value);
              if (Number.isNaN(v)) return "";
              if (isPercentageMode) return `${v.toFixed(1)}%`;
              return v.toLocaleString();
            },
            position: "top",
            fontSize: 10,
            overflow: "truncate",
          }
        : isPercentageMode
        ? {
            show: true,
            formatter: (params: any) => {
              const v = Number(params.value);
              if (Number.isNaN(v)) return "";
              return `${v.toFixed(1)}%`;
            },
            position: chartKind === "time_series_area_share" && opts.type === "line" ? "inside" : "top",
            fontSize: 10,
            overflow: "truncate",
          }
        : undefined,
      labelLayout: markersEnabled || isPercentageMode ? { hideOverlap: true } : undefined,
      data: xValues.map((x, dataIndex) => {
        const raw = dataMap.get(name)?.get(x) ?? 0;
        const value = isPercentageMode
          ? (xTotals[x] > 0 ? (raw / xTotals[x]) * 100 : 0)
          : raw;
        // First/last points get alignment overrides so labels stay within the plot area
        if (dataIndex === 0) return { value, label: { align: 'left' } };
        if (dataIndex === xValues.length - 1) return { value, label: { align: 'right' } };
        return value;
      }),
    }));
  };

  // Build tooltip formatter based on user preferences
  const tooltipDateFormat = advancedOptions?.tooltip?.dateFormat || "auto";
  const tooltipNumberFormat = advancedOptions?.tooltip?.numberFormat || "none";
  const tooltipDecimalPlaces = advancedOptions?.tooltip?.decimalPlaces ?? 2;
  const tooltipUseCommas = advancedOptions?.tooltip?.useCommas ?? true;

  const buildTooltipFormatter = () => {
    return (params: any) => {
      if (!Array.isArray(params)) params = [params];
      const axisValue = params[0]?.axisValue || params[0]?.name;
      let formattedAxisValue = axisValue;
      if (tooltipDateFormat && tooltipDateFormat !== "auto" && axisValue) {
        formattedAxisValue = formatXAxisLabel(String(axisValue), tooltipDateFormat as DateDisplayFormat);
      }
      let result = `<strong>${formattedAxisValue}</strong><br/>`;
      params.forEach((param: any) => {
        const value = Number(param.value);
        let formattedValue = "";
        if (!Number.isNaN(value)) {
          formattedValue = formatTooltipNumber(value, tooltipNumberFormat, tooltipDecimalPlaces, tooltipUseCommas);
        } else {
          formattedValue = String(param.value ?? "");
        }
        result += `${param.marker} ${param.seriesName}: ${formattedValue}<br/>`;
      });
      return result;
    };
  };

  const common = {
    tooltip: {
      trigger: "axis",
      formatter: tooltipDateFormat !== "auto" || tooltipNumberFormat !== "none" || tooltipUseCommas
        ? buildTooltipFormatter()
        : undefined,
    },
    legend: legendOrder.length > 1 ? { data: legendOrder } : undefined,
    grid: {
      left: 10,
      right: 15,
      top: 80,
      bottom: 50,
      containLabel: true,
    },
  } as any;

  const buildCategoryAxis = (boundaryGap: boolean = true) => {
    const axis: any = { type: "category", data: xValues, boundaryGap };
    if (xAxisDateFormat && xAxisDateFormat !== "auto") {
      axis.axisLabel = {
        formatter: (val: string) => formatXAxisLabel(String(val), xAxisDateFormat as DateDisplayFormat),
      };
    }
    return axis;
  };

  switch (chartKind) {
    case "time_series_line":
    case "time_series_line_share":
    case "time_series_area":
    case "time_series_area_share":
    case "line_multi_series":
    case "area_stack":
      const isPercentageArea =
        chartKind === "time_series_area_share" || chartKind === "time_series_line_share";
      return {
        ...common,
        tooltip: isPercentageArea
          ? {
              ...(common.tooltip ?? {}),
              valueFormatter: (value: number | string) => {
                const num = Number(value);
                if (Number.isNaN(num)) return String(value ?? "");
                return `${num.toFixed(1)}%`;
              },
            }
          : common.tooltip,
        xAxis: buildCategoryAxis(false),
        yAxis: isPercentageArea
          ? {
              type: "value",
              min: 0,
              max: 100,
              axisLabel: {
                hideOverlap: true,
                formatter: (val: number | string) => {
                  const num = Number(val);
                  if (Number.isNaN(num)) return String(val ?? "");
                  return `${num.toFixed(1)}%`;
                },
              },
            }
          : { type: "value" },
        series: buildLineOrBarSeries({
          type: "line",
          stacked:
            chartKind === "area_stack" ||
            (isPercentageArea && (advancedOptions?.showAsPercentage !== false)),
          smooth: Array.isArray(advancedOptions?.series) && advancedOptions.series.some((s: any) => s.smooth),
        }),
      };
    case "bar_vertical":
    case "grouped_bar":
    case "stacked_bar_vertical": {
      const pctBar = chartKind === "stacked_bar_vertical" && advancedOptions?.showAsPercentage === true;
      const pctAxis = pctBar ? {
        type: "value",
        max: 100,
        axisLabel: { formatter: (val: number) => `${val}%` },
      } : { type: "value" };
      return {
        ...common,
        tooltip: pctBar
          ? { ...(common.tooltip ?? {}), valueFormatter: (v: number | string) => `${Number(v).toFixed(1)}%` }
          : common.tooltip,
        xAxis: buildCategoryAxis(),
        yAxis: pctAxis,
        series: buildLineOrBarSeries({
          type: "bar",
          stacked: chartKind === "stacked_bar_vertical",
        }),
      };
    }
    case "bar_horizontal":
    case "stacked_bar_horizontal": {
      const pctBarH = chartKind === "stacked_bar_horizontal" && advancedOptions?.showAsPercentage === true;
      return {
        ...common,
        tooltip: pctBarH
          ? { ...(common.tooltip ?? {}), valueFormatter: (v: number | string) => `${Number(v).toFixed(1)}%` }
          : common.tooltip,
        xAxis: pctBarH
          ? { type: "value", max: 100, axisLabel: { formatter: (val: number) => `${val}%` } }
          : { type: "value" },
        yAxis: { type: "category", data: xValues },
        series: buildLineOrBarSeries({
          type: "bar",
          stacked: chartKind === "stacked_bar_horizontal",
        }),
      };
    }
    case "pie":
    case "donut": {
      const data = seriesNames.length > 1 ? seriesNames : xValues;
      const pieData =
        seriesNames.length > 1
          ? seriesNames.map((name) => ({
              name,
              value: xValues.reduce((sum, x) => sum + (dataMap.get(name)?.get(x) ?? 0), 0),
            }))
          : xValues.map((x) => ({ name: x, value: dataMap.get(seriesNames[0])?.get(x) ?? 0 }));

      const pieTooltipFormatter = (params: any) => {
        const value = Number(params.value);
        let formattedValue = "";
        if (!Number.isNaN(value)) {
          formattedValue = formatTooltipNumber(value, tooltipNumberFormat, tooltipDecimalPlaces, tooltipUseCommas);
        } else {
          formattedValue = String(params.value ?? "");
        }
        return `${params.marker} ${params.name}: ${formattedValue} (${params.percent}%)`;
      };

      return {
        tooltip: {
          trigger: "item",
          formatter: pieTooltipFormatter,
        },
        legend: { data, orient: "vertical", left: "left" },
        series: [
          {
            name: seriesNames.length > 1 ? columns[metricIndex] : "Value",
            type: "pie",
            radius: chartKind === "donut" ? ["40%", "70%"] : "50%",
            data: pieData,
            emphasis: {
              itemStyle: {
                shadowBlur: 10,
                shadowOffsetX: 0,
                shadowColor: "rgba(0, 0, 0, 0.5)",
              },
            },
          },
        ],
      };
    }
    case "scatter":
    case "bubble": {
      // Honor the configured axes (x_axis/y_axis) and optional bubble size +
      // point label columns, instead of blindly using columns 0/1 (which breaks
      // when column 0 is a text label like "country").
      const idxOf = (name?: string) => (name ? columns.indexOf(name) : -1);
      let sx = idxOf(queryConfig?.x_axis);
      let sy = idxOf(queryConfig?.y_axis);
      const sizeIndex = idxOf(queryConfig?.size_column);
      const labelIndex = idxOf(queryConfig?.label_column);
      // Fallback: first columns that actually hold numbers (skip label columns).
      if (sx < 0 || sy < 0) {
        const numericCols = columns
          .map((_, i) => i)
          .filter((i) => rows.some((r) => r[i] != null && r[i] !== "" && !Number.isNaN(Number(r[i]))));
        if (sx < 0) sx = numericCols[0] ?? 0;
        if (sy < 0) sy = numericCols.find((i) => i !== sx) ?? 1;
      }

      const sizeVals = sizeIndex >= 0 ? rows.map((r) => Number(r[sizeIndex]) || 0) : [];
      const sMin = sizeVals.length ? Math.min(...sizeVals) : 0;
      const sMax = sizeVals.length ? Math.max(...sizeVals) : 0;
      const baseSize = (advancedOptions as any)?.symbolSize ?? 12;
      const scaleSize = (v: number) =>
        sizeIndex < 0 || sMax === sMin ? baseSize : 8 + ((v - sMin) / (sMax - sMin)) * 42; // 8–50px

      const scatterData = rows.map((r) => {
        const point: any[] = [Number(r[sx]), Number(r[sy])];
        if (sizeIndex >= 0) point.push(Number(r[sizeIndex]) || 0);
        if (labelIndex >= 0) point.push(r[labelIndex]);
        return point;
      });

      return {
        ...common,
        tooltip: {
          trigger: "item",
          formatter: (p: any) => {
            const d = p.data as any[];
            const lbl = labelIndex >= 0 ? `<b>${d[d.length - 1]}</b><br/>` : "";
            let s = `${lbl}${columns[sx]}: ${d[0]}<br/>${columns[sy]}: ${d[1]}`;
            if (sizeIndex >= 0) s += `<br/>${columns[sizeIndex]}: ${d[2]}`;
            return s;
          },
        },
        xAxis: { type: "value", name: columns[sx], nameLocation: "middle", nameGap: 28, scale: true },
        yAxis: { type: "value", name: columns[sy], scale: true },
        series: [
          {
            type: "scatter",
            data: scatterData,
            symbolSize: sizeIndex >= 0 ? (d: any[]) => scaleSize(Number(d[2])) : baseSize,
            itemStyle: { opacity: (advancedOptions as any)?.pointOpacity ?? 0.8 },
            label:
              labelIndex >= 0 && (advancedOptions as any)?.showDataLabels
                ? { show: true, position: "top", fontSize: 10, formatter: (p: any) => { const d = p.data as any[]; return d[d.length - 1]; } }
                : undefined,
          },
        ],
      };
    }
    case "mixed_line_bar": {
      return {
        ...common,
        xAxis: buildCategoryAxis(),
        yAxis: [
          { type: "value" },
          { type: "value", splitLine: { show: false } },
        ],
        series: seriesNames.map((name, idx) => {
          const seriesType = idx === 0 ? "bar" : "line";
          return {
            name,
            type: seriesType,
            yAxisIndex: idx === 0 ? 0 : 1,
            data: xValues.map((x) => dataMap.get(name)?.get(x) ?? 0),
          };
        }),
      };
    }
    case "waterfall": {
      const wfData = xValues.map((x) => {
        const val = dataMap.get(seriesNames[0])?.get(x) ?? 0;
        return val;
      });
      // Build running total for placeholder bars
      let running = 0;
      const placeholders: number[] = [];
      wfData.forEach((v) => {
        placeholders.push(v >= 0 ? running : running + v);
        running += v;
      });
      return {
        ...common,
        xAxis: buildCategoryAxis(),
        yAxis: { type: "value" },
        series: [
          {
            type: "bar",
            stack: "waterfall",
            itemStyle: { color: "transparent", borderColor: "transparent" },
            data: placeholders,
            tooltip: { show: false },
          },
          {
            name: seriesNames[0] || "Value",
            type: "bar",
            stack: "waterfall",
            data: wfData.map((v) => ({
              value: Math.abs(v),
              itemStyle: { color: v >= 0 ? "#22c55e" : "#ef4444" },
            })),
          },
        ],
      };
    }
    case "nightingale_rose": {
      const roseData = seriesNames.length > 1
        ? seriesNames.map(name => ({ name, value: xValues.reduce((s, x) => s + (dataMap.get(name)?.get(x) ?? 0), 0) }))
        : xValues.map(x => ({ name: x, value: dataMap.get(seriesNames[0])?.get(x) ?? 0 }));
      return {
        tooltip: { trigger: "item", formatter: "{b}: {c} ({d}%)" },
        legend: { orient: "vertical", left: "left" },
        series: [{
          type: "pie", roseType: "area",
          radius: ["15%", "72%"], center: ["55%", "50%"],
          itemStyle: { borderRadius: 4, borderColor: "#fff", borderWidth: 2 },
          label: { show: false },
          emphasis: { label: { show: true, fontSize: 12, fontWeight: "bold" } },
          data: roseData,
        }],
      };
    }
    case "histogram": {
      const hvals = rows.map(r => Number(r[metricIndex])).filter(v => !isNaN(v));
      if (!hvals.length) return null;
      const hmin = Math.min(...hvals), hmax = Math.max(...hvals);
      const bins = Math.min(20, Math.ceil(Math.sqrt(hvals.length)));
      const bw = (hmax - hmin) / bins || 1;
      const counts = Array.from({ length: bins }, () => 0);
      hvals.forEach(v => { counts[Math.min(Math.floor((v - hmin) / bw), bins - 1)]++; });
      const labels = Array.from({ length: bins }, (_, i) => {
        const mid = hmin + (i + 0.5) * bw;
        return Math.abs(mid) >= 1e6 ? `${(mid / 1e6).toFixed(1)}M` : Math.abs(mid) >= 1e3 ? `${(mid / 1e3).toFixed(1)}K` : mid.toFixed(1);
      });
      return {
        ...common,
        xAxis: { type: "category", data: labels, name: columns[metricIndex] || "", nameLocation: "center", nameGap: 30 },
        yAxis: { type: "value", name: "Count" },
        series: [{ type: "bar", data: counts, barCategoryGap: "2%", itemStyle: { borderRadius: [3, 3, 0, 0] } }],
      };
    }
    case "sankey": {
      if (columns.length < 3) return null;
      const nodeSet = new Set<string>();
      const sankeyLinks: { source: string; target: string; value: number }[] = [];
      rows.forEach(r => {
        const src = String(r[0] ?? ""), tgt = String(r[1] ?? ""), val = Number(r[2] ?? 0);
        if (src && tgt) { nodeSet.add(src); nodeSet.add(tgt); sankeyLinks.push({ source: src, target: tgt, value: val }); }
      });
      return {
        tooltip: { trigger: "item", formatter: (p: any) => p.dataType === "edge" ? `${p.data.source} → ${p.data.target}: ${p.data.value}` : p.name },
        series: [{ type: "sankey", layout: "none", emphasis: { focus: "adjacency" }, nodeAlign: "left", data: Array.from(nodeSet).map(n => ({ name: n })), links: sankeyLinks }],
      };
    }
    case "calendar_heatmap": {
      if (columns.length < 2) return null;
      const calRows = rows.map(r => [String(r[0] ?? ""), Number(r[metricIndex] ?? 0)]);
      const calVals = calRows.map(d => d[1] as number);
      const years = [...new Set(calRows.map(d => String(d[0]).slice(0, 4)))].sort();
      return {
        tooltip: { formatter: (p: any) => `${p.data[0]}: ${p.data[1]}` },
        visualMap: { min: Math.min(...calVals), max: Math.max(...calVals), calculable: true, orient: "horizontal", left: "center", bottom: 10, inRange: { color: ["#e0f3f8", "#abd9e9", "#74add1", "#4575b4", "#313695"] } },
        calendar: years.map((y, i) => ({ range: y, top: 60 + i * 170, left: 60, right: 20, cellSize: ["auto", 14] })),
        series: years.map((y, i) => ({ type: "heatmap", coordinateSystem: "calendar", calendarIndex: i, data: calRows.filter(d => String(d[0]).startsWith(y)) })),
      };
    }
    case "parallel_coordinates": {
      if (columns.length < 2) return null;
      const hasCatCol = isNaN(Number(rows[0]?.[0]));
      const dimStart = hasCatCol ? 1 : 0;
      const dims = columns.slice(dimStart);
      return {
        parallelAxis: dims.map((d, i) => ({ dim: i, name: d.split(".").pop() || d })),
        parallel: { left: "5%", right: "5%", bottom: "10%", top: "10%" },
        tooltip: { trigger: "item" },
        series: [{ type: "parallel", lineStyle: { width: 1.5, opacity: 0.4 }, data: rows.map(r => dims.map((_, i) => Number(r[dimStart + i] ?? 0))) }],
      };
    }
    case "world_map":
      return { _worldMap: true, _rows: rows, _columns: columns };
    default:
      return {
        ...common,
        xAxis: buildCategoryAxis(),
        yAxis: { type: "value" },
        series: buildLineOrBarSeries({ type: "bar" }),
      };
  }
}

export function mergeAdvancedOptions(loaded: Partial<typeof DEFAULT_ADVANCED_OPTIONS>) {
  // Helper to format Y-axis values
  const formatYAxisValue = (val: number, format: string) => {
    if (format === "k") return `${(val / 1e3).toFixed(0)}K`;
    if (format === "m") return `${(val / 1e6).toFixed(0)}M`;
    if (format === "b") return `${(val / 1e9).toFixed(0)}B`;
    if (format === "t") return `${(val / 1e12).toFixed(0)}T`;
    return val.toLocaleString();
  };

  // Reconstruct label formatters and symbol functions for series
  const yAxisFormat = loaded?.yAxisFormat || "none";
  // Check if this is a share chart - need to check loaded chart type if available
  const isShareChart = false; // Will be determined dynamically during option merge

  const reconstructedSeries = (loaded?.series || []).map((s: any) => {
    const result: any = { ...s };

    // Reconstruct label formatter if labels are shown
    if (s.label && s.label.show) {
      result.label = {
        ...s.label,
        position: s.label.position || 'inside',
        formatter: (params: any) => {
          const val = params.value;
          // Note: isShareChart will be re-evaluated in applyAdvancedOptionsToPreview
          return formatYAxisValue(val, yAxisFormat);
        }
      };
    }

    // Reconstruct symbol and showSymbol if markers are enabled (symbol is not "none")
    if (s.symbol && s.symbol !== "none") {
      // Capture the series data length in closure
      const seriesDataLength = s.data?.length || 0;
      result.symbol = 'circle';
      result.showSymbol = function (dataIndex: number) {
        // Show exactly 6 points: first, last, and 4 evenly spaced in between
        const dataLength = seriesDataLength;

        // Always show first and last
        if (dataIndex === 0 || dataIndex === dataLength - 1) {
          return true;
        }

        // Calculate 4 evenly spaced middle points
        // Points at: 1/5, 2/5, 3/5, 4/5 of the way through the data
        const point1 = Math.round((dataLength - 1) * 1 / 5);
        const point2 = Math.round((dataLength - 1) * 2 / 5);
        const point3 = Math.round((dataLength - 1) * 3 / 5);
        const point4 = Math.round((dataLength - 1) * 4 / 5);

        if (dataIndex === point1 || dataIndex === point2 || dataIndex === point3 || dataIndex === point4) {
          return true;
        }

        return false;
      };
    }

    return result;
  });

  // Deep merge for xAxis, yAxis, legend, tooltip
  return {
    ...DEFAULT_ADVANCED_OPTIONS,
    ...loaded,
    xAxis: { ...DEFAULT_ADVANCED_OPTIONS.xAxis, ...(loaded?.xAxis || {}) },
    yAxis: { ...DEFAULT_ADVANCED_OPTIONS.yAxis, ...(loaded?.yAxis || {}) },
    legend: { ...DEFAULT_ADVANCED_OPTIONS.legend, ...(loaded?.legend || {}) },
    tooltip: { ...DEFAULT_ADVANCED_OPTIONS.tooltip, ...(loaded?.tooltip || {}) },
    color: loaded?.color || DEFAULT_ADVANCED_OPTIONS.color,
    labelLayout: loaded?.labelLayout !== undefined ? loaded.labelLayout : DEFAULT_ADVANCED_OPTIONS.labelLayout,
    series: reconstructedSeries,
    // Preserve seriesSettings if present (new format)
    seriesSettings: loaded?.seriesSettings || undefined,
    // Explicitly preserve showAsPercentage for share charts (important: false is a valid value)
    showAsPercentage: loaded?.showAsPercentage !== undefined ? loaded.showAsPercentage : undefined,
    // Preserve chart-type-specific options (bar labels, pie hole size, KPI format, etc.)
    chartTypeOptions: (loaded as any)?.chartTypeOptions || undefined,
  };
}

export type ChartKind =
  | "time_series_line"
  | "time_series_line_share"
  | "time_series_area"
  | "time_series_area_share"
  | "bar_vertical"
  | "bar_horizontal"
  | "stacked_bar_vertical"
  | "stacked_bar_horizontal"
  | "grouped_bar"
  | "line_multi_series"
  | "area_stack"
  | "scatter"
  | "bubble"
  | "heatmap"
  | "radar"
  | "funnel"
  | "gauge"
  | "boxplot"
  | "candlestick"
  | "treemap"
  | "sunburst"
  | "pictorial_bar"
  | "theme_river"
  | "big_number"
  | "big_number_trend"
  | "table"
  | "pivot_table"
  | "pie"
  | "donut"
  | "mixed_line_bar"
  | "waterfall"
  | "nightingale_rose"
  | "sankey"
  | "calendar_heatmap"
  | "histogram"
  | "parallel_coordinates"
  | "world_map";

export type TimeRangePreset =
  | "all_time"
  | "latest_day"
  | "last_day"
  | "last_week"
  | "last_month"
  | "last_quarter"
  | "last_year"
  | "week_to_date"
  | "month_to_date"
  | "quarter_to_date"
  | "year_to_date"
  | "previous_week"
  | "previous_month"
  | "previous_quarter"
  | "previous_year"
  | "last_7_days"
  | "last_30_days"
  | "last_90_days"
  | "last_365_days"
  | "custom"
  | "custom_to_latest";

export type DateDisplayFormat = "auto" | "date" | "weekday" | "month_year";

export interface ChartCategory {
  id: string;
  label: string;
  iconClass: string;
}

export interface ChartTemplate {
  id: ChartKind;
  name: string;
  description: string;
  category: string; // e.g. "Line", "Bar", "Pie" etc. aligned with ECharts examples
  previewKind?: string;
  // Optional PNG thumbnail path (served from Next.js public/)
  thumbnail?: string;
}

export interface DatasetSummary {
  id: number;
  name: string;
  description?: string | null;
  database_name?: string | null;
  schema_name?: string | null;
}

export interface DatasetColumn {
  table_name: string;
  column_name: string;
  data_type: string;
  is_dimension: boolean;
  is_metric: boolean;
  semantic_type?: string | null;
  fact_key?: string | null;
}

export interface MetricConfig {
  id: string;
  column: string;
  aggregate: "SUM" | "COUNT" | "COUNT_DISTINCT" | "AVG" | "MIN" | "MAX";
  label: string;
}

export interface DatasetMetric {
  name: string;
  expression: string;
  metric_type: string;
  format?: string | null;
}

export interface DatasetDetailForChart {
  table_name: string;
  schema_name?: string | null;
  database_name?: string | null;
  date_column?: string | null;
}

export interface ChartFilterOption {
  key: string;   // Unique identifier (e.g., product ID)
  value: string; // Display value (e.g., "Widget A")
}

export interface ChartFilterConfig {
  id: string;
  column: string; // Display column (e.g., "dbo.DimProduct.ProductName")
  columnLabel?: string; // Display name or semantic type (e.g., "Product", "Customer")
  keyColumn?: string; // Key column for filtering (e.g., "dbo.DimProduct.ProductID")
  operator: string;
  value: string; // The display value (e.g., "Widget A")
  valueKey?: string; // Unique identifier/key for the value (e.g., product ID "123")
  options: ChartFilterOption[]; // Changed from string[] to key-value pairs
  isPending?: boolean;
  isLoading: boolean;
  // When true, the filter is still being configured and
  // should not be shown as an active chip yet.
  // Optional grouping key so we can support a secondary
  // group of filters with a different logical operator.
  // When omitted, the filter belongs to the primary group.
  groupKey?: "primary" | "secondary";
}

export interface SqlPreviewState {
  lastSql: string | null;
  lastConfigJson: any | null;
  dataColumns: string[];
  dataRows: (string | number | null)[][];
  isRunning: boolean;
  error: string | null;
  // Total end-to-end duration as measured in the
  // chart builder (SQL generation + network + Fabric).
  durationMs: number | null;
  // Optional server-side Fabric execution time reported
  // by the /sql/execute endpoint.
  fabricDurationMs?: number | null;
  rowCount: number | null;
  // Saved SQL text from database (loaded when opening existing chart)
  savedSql: string | null;
}

export const CHART_CATEGORIES: ChartCategory[] = [
  { id: "Line",        label: "Line",       iconClass: "fas fa-chart-line" },
  { id: "Bar",         label: "Bar",        iconClass: "fas fa-chart-bar" },
  { id: "Pie",         label: "Pie",        iconClass: "fas fa-chart-pie" },
  { id: "Scatter",     label: "Scatter",    iconClass: "fas fa-braille" },
  { id: "Heatmap",     label: "Heatmap",    iconClass: "fas fa-th-large" },
  { id: "Treemap",     label: "Treemap",    iconClass: "fas fa-th" },
  { id: "Sunburst",    label: "Sunburst",   iconClass: "fas fa-sun" },
  { id: "Funnel",      label: "Funnel",     iconClass: "fas fa-filter" },
  { id: "Radar",       label: "Radar",      iconClass: "fas fa-bullseye" },
  { id: "Gauge",       label: "Gauge",      iconClass: "fas fa-tachometer-alt" },
  { id: "Boxplot",     label: "Boxplot",    iconClass: "fas fa-box" },
  { id: "Candlestick", label: "Candlestick",iconClass: "fas fa-chart-bar" },
  { id: "PictorialBar",label: "Pictorial",  iconClass: "fas fa-images" },
  { id: "ThemeRiver",  label: "Stream",     iconClass: "fas fa-water" },
  { id: "Flow",        label: "Flow",       iconClass: "fas fa-project-diagram" },
  { id: "Map",         label: "Map",        iconClass: "fas fa-globe" },
  { id: "Custom",      label: "Custom",     iconClass: "fas fa-shapes" },
  { id: "Dataset",     label: "Table",      iconClass: "fas fa-table" },
];

export const TEMPLATES: ChartTemplate[] = [
  {
    id: "time_series_line",
    name: "Time-series line",
    description: "Track a metric over time with a simple line.",
    category: "Line",
    previewKind: "line",
    thumbnail: "/chart-thumbnails/time_series_line.png",
  },
  {
    id: "time_series_line_share",
    name: "Time-series line share",
    description: "Line chart where each point shows percentage share.",
    category: "Line",
    previewKind: "line",
  },
  {
    id: "line_multi_series",
    name: "Multi-series line",
    description: "Compare multiple metrics over time on a shared axis.",
    category: "Line",
    previewKind: "line",
  },
  {
    id: "time_series_area",
    name: "Time-series area",
    description: "Visualize volume over time with a filled line.",
    category: "Line",
    previewKind: "area",
    thumbnail: "/chart-thumbnails/time_series_area.png",
  },
  {
    id: "time_series_area_share",
    name: "Time-series area share",
    description: "100% stacked area showing percentage share over time.",
    category: "Line",
    previewKind: "area",
  },
  {
    id: "area_stack",
    name: "Stacked area",
    description: "Multiple stacked areas showing cumulative values over time.",
    category: "Line",
    previewKind: "area",
  },
  {
    id: "bar_vertical",
    name: "Vertical bar",
    description: "Compare categories side by side.",
    category: "Bar",
    previewKind: "bar",
    thumbnail: "/chart-thumbnails/bar_vertical.png",
  },
  {
    id: "bar_horizontal",
    name: "Horizontal bar",
    description: "Use when category labels are long.",
    category: "Bar",
    previewKind: "bar",
    thumbnail: "/chart-thumbnails/bar_horizontal.png",
  },
  {
    id: "grouped_bar",
    name: "Grouped bar",
    description: "Side-by-side bars comparing multiple series per category.",
    category: "Bar",
    previewKind: "bar",
  },
  {
    id: "stacked_bar_vertical",
    name: "Stacked bar",
    description: "Bars stacked to show part-to-whole relationships.",
    category: "Bar",
    previewKind: "bar",
  },
  {
    id: "stacked_bar_horizontal",
    name: "Stacked horizontal bar",
    description: "Horizontal stacked bars comparing composition across categories.",
    category: "Bar",
    previewKind: "bar",
  },
  {
    id: "scatter",
    name: "Scatter",
    description: "Plot points to see correlation.",
    category: "Scatter",
    previewKind: "scatter",
  },
  {
    id: "bubble",
    name: "Bubble",
    description: "Scatter with bubble size as a 3rd metric.",
    category: "Scatter",
    previewKind: "bubble",
  },
  {
    id: "heatmap",
    name: "Heatmap",
    description: "Color intensity over two dimensions.",
    category: "Heatmap",
    previewKind: "heatmap",
  },
  {
    id: "radar",
    name: "Radar",
    description: "Compare metrics across many dimensions.",
    category: "Radar",
    previewKind: "radar",
  },
  {
    id: "funnel",
    name: "Funnel",
    description: "Show step conversion through a pipeline.",
    category: "Funnel",
    previewKind: "funnel",
  },
  {
    id: "gauge",
    name: "Gauge",
    description: "Display a single value vs a target.",
    category: "Gauge",
    previewKind: "gauge",
  },
  {
    id: "boxplot",
    name: "Boxplot",
    description: "Distribution by quartiles and outliers.",
    category: "Boxplot",
    previewKind: "boxplot",
  },
  {
    id: "candlestick",
    name: "Candlestick",
    description: "OHLC financial-style bars.",
    category: "Candlestick",
    previewKind: "candlestick",
  },
  {
    id: "treemap",
    name: "Treemap",
    description: "Nested rectangles sized by value.",
    category: "Treemap",
    previewKind: "treemap",
  },
  {
    id: "sunburst",
    name: "Sunburst",
    description: "Radial hierarchical breakdown.",
    category: "Sunburst",
    previewKind: "sunburst",
  },
  {
    id: "pictorial_bar",
    name: "Pictorial bar",
    description: "Bars drawn with custom symbols.",
    category: "PictorialBar",
    previewKind: "pictorial",
  },
  {
    id: "theme_river",
    name: "Theme river",
    description: "Flowing stacked streams over time.",
    category: "ThemeRiver",
    previewKind: "theme-river",
  },
  {
    id: "big_number",
    name: "Big number",
    description: "Highlight a single key metric.",
    category: "Custom",
    previewKind: "kpi",
  },
  {
    id: "big_number_trend",
    name: "Big number with trend",
    description: "KPI with a small trendline to show direction.",
    category: "Custom",
    previewKind: "kpi-trend",
  },
  {
    id: "table",
    name: "Table",
    description: "See detailed rows in a sortable table.",
    category: "Dataset",
    previewKind: "table",
  },
  {
    id: "pivot_table",
    name: "Pivot table",
    description: "Summarize metrics across rows and columns.",
    category: "Dataset",
  },
  {
    id: "pie",
    name: "Pie chart",
    description: "Show how categories contribute to a whole.",
    category: "Pie",
  },
  {
    id: "donut",
    name: "Donut chart",
    description: "Pie chart with a focus on center KPI.",
    category: "Pie",
  },
  {
    id: "mixed_line_bar",
    name: "Mixed Line + Bar",
    description: "Combine bars and lines on a dual-axis chart.",
    category: "Bar",
    previewKind: "bar",
  },
  {
    id: "waterfall",
    name: "Waterfall",
    description: "Show cumulative effect of sequential positive/negative values.",
    category: "Bar",
    previewKind: "bar",
  },
  {
    id: "nightingale_rose",
    name: "Nightingale rose",
    description: "Pie chart where radius encodes value — great for comparing periodic categories.",
    category: "Pie",
  },
  {
    id: "histogram",
    name: "Histogram",
    description: "Auto-bins a numeric column to show value distribution.",
    category: "Bar",
  },
  {
    id: "sankey",
    name: "Sankey diagram",
    description: "Visualise flow between nodes. Query needs source, target, value columns.",
    category: "Flow",
  },
  {
    id: "calendar_heatmap",
    name: "Calendar heatmap",
    description: "Daily values plotted in a calendar grid. Query needs date + value columns.",
    category: "Heatmap",
  },
  {
    id: "parallel_coordinates",
    name: "Parallel coordinates",
    description: "Compare multi-dimensional numeric data across parallel axes.",
    category: "Custom",
  },
  {
    id: "world_map",
    name: "World map",
    description: "Choropleth world map. Query needs a country name/code column and a value column.",
    category: "Map",
  },
];

export interface ChartBuilderContextValue {
  chartId: number | null;
  setChartId: (id: number | null) => void;
  datasets: DatasetSummary[];
  datasetsError: string | null;
  selectedDatasetId: number | null;
  setSelectedDatasetId: (id: number | null) => void;
  name: string;
  setName: (value: string) => void;
  description: string;
  setDescription: (value: string) => void;
  chartType: ChartKind | null;
  setChartType: (value: ChartKind | null) => void;
  templates: ChartTemplate[];
  categories: ChartCategory[];
  selectedTemplate: ChartTemplate | null;
  previewOptions: any | null;
  setPreviewOptions: (value: any | null) => void;
  advancedOptions: any | null;
  setAdvancedOptions: (value: any | null) => void;
  datasetColumns: DatasetColumn[];
  datasetMetrics: DatasetMetric[];
  datasetDetailError: string | null;
  datasetDetail: DatasetDetailForChart | null;
  metricColumn: string | null;
  setMetricColumn: (value: string | null) => void;
  metrics: MetricConfig[];
  setMetrics: React.Dispatch<React.SetStateAction<MetricConfig[]>>;
  timeGrain: string;
  setTimeGrain: (value: string) => void;
  sortBy: { column: string; direction: "asc" | "desc" } | null;
  setSortBy: (value: { column: string; direction: "asc" | "desc" } | null) => void;
  queryMode: "aggregate" | "raw";
  setQueryMode: (value: "aggregate" | "raw") => void;
  groupByColumns: string[];
  setGroupByColumns: (value: string[]) => void;
  setScatterAxes: (value: { x?: string; y?: string; size?: string; label?: string }) => void;
  setCategoryLabels: (value: Record<string, string> | null) => void;
  timeColumn: string | null;
  setTimeColumn: (value: string | null) => void;
  timeRange: TimeRangePreset;
  setTimeRange: (value: TimeRangePreset) => void;
  customStartDate: string | null;
  setCustomStartDate: (value: string | null) => void;
  customEndDate: string | null;
  setCustomEndDate: (value: string | null) => void;
  dateDisplayFormat: DateDisplayFormat;
  setDateDisplayFormat: (value: DateDisplayFormat) => void;
  rowLimit: number;
  setRowLimit: (value: number) => void;
  filterLogic: "AND" | "OR";
  setFilterLogic: (value: "AND" | "OR") => void;
  filters: ChartFilterConfig[];
  setFilters: React.Dispatch<React.SetStateAction<ChartFilterConfig[]>>;
  sqlPreview: SqlPreviewState;
  runPreviewQuery: (forceRegenerate?: boolean, extraFilters?: any[]) => Promise<void>;
  cancelRunningQuery: () => void;
  runContext?: string;
  isSaving: boolean;
  canSave: boolean;
  saveError: string | null;
  handleSave: () => void;
  /** Save the current chart as a NEW copy; resolves to the new chart id (or null). */
  saveChartAs: (newName: string) => Promise<number | null>;
  registerInitialSnapshot: () => void;
  /** ChartPreview calls this to register a thumbnail-capture fn (returns a JPEG data URI). */
  registerThumbnailCapture: (fn: (() => string | null | Promise<string | null>) | null) => void;
}

const ChartBuilderContext = createContext<ChartBuilderContextValue | undefined>(undefined);

interface ChartBuilderProviderProps {
  children: React.ReactNode;
  runContext?: string;
  initialChartId?: number | string;
  initialDatasetId?: number | string;
  initialTemplate?: string;
}

export const ChartBuilderProvider: React.FC<ChartBuilderProviderProps> = ({
  children,
  runContext,
  initialChartId,
  initialDatasetId,
  initialTemplate,
}) => {
  const { isAuthenticated, account } = useAuth();

  const [chartId, setChartId] = useState<number | null>(null);
  const [datasets, setDatasets] = useState<DatasetSummary[]>([]);
  const [datasetsError, setDatasetsError] = useState<string | null>(null);
  const [selectedDatasetId, setSelectedDatasetId] = useState<number | null>(null);
  const [datasetColumns, setDatasetColumns] = useState<DatasetColumn[]>([]);
  const [datasetMetrics, setDatasetMetrics] = useState<DatasetMetric[]>([]);
  const [datasetDetailError, setDatasetDetailError] = useState<string | null>(null);
  const [datasetDetail, setDatasetDetail] = useState<DatasetDetailForChart | null>(null);
  const [name, setName] = useState<string>("");
  const [description, setDescription] = useState<string>("");
  const [chartType, setChartType] = useState<ChartKind | null>(null);
  const [metricColumn, setMetricColumn] = useState<string | null>(null);
  const [metrics, setMetrics] = useState<MetricConfig[]>([]);
  const [timeGrain, setTimeGrain] = useState<string>("none");
  const [sortBy, setSortBy] = useState<{ column: string; direction: "asc" | "desc" } | null>(null);
  const [queryMode, setQueryMode] = useState<"aggregate" | "raw">("aggregate");
  const [groupByColumns, setGroupByColumns] = useState<string[]>([]);
  // Scatter/bubble axis columns (by name) — carried through so the renderer maps
  // the intended columns instead of guessing by position.
  const [scatterAxes, setScatterAxes] = useState<{ x?: string; y?: string; size?: string; label?: string }>({});
  // Friendly display labels for category values (e.g. {"true":"Open Source","false":"Proprietary"}).
  const [categoryLabels, setCategoryLabels] = useState<Record<string, string> | null>(null);
  const [timeColumn, setTimeColumn] = useState<string | null>(null);
  const [timeRange, setTimeRange] = useState<TimeRangePreset>("last_30_days");
  const [customStartDate, setCustomStartDate] = useState<string | null>(null);
  const [customEndDate, setCustomEndDate] = useState<string | null>(null);
  const [dateDisplayFormat, setDateDisplayFormat] = useState<DateDisplayFormat>("auto");
  // Row limit is used only for the Data tab preview;
  // charts themselves should use all matching rows.
  const [rowLimit, setRowLimit] = useState<number>(500);
  const [filterLogic, setFilterLogic] = useState<"AND" | "OR">("AND");
  const [filters, setFilters] = useState<ChartFilterConfig[]>([]);
  const [isSaving, setIsSaving] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);
  // User's advanced config (from Advanced Editor)
  const [advancedOptions, setAdvancedOptions] = useState<any | null>(null);
  // Preview config (auto-generated for chart preview)
  const [previewOptions, setPreviewOptions] = useState<any | null>(null);
  const [hasChanges, setHasChanges] = useState(false);
  const isQueryRunningRef = useRef(false);
  const cancelQueryRef = useRef(false);
  const activeJobIdRef = useRef<string | null>(null);
  // ChartPreview registers a fn that snapshots the rendered chart to a JPEG data
  // URI, so save() can persist a real thumbnail (like dashboards do).
  const thumbnailCaptureRef = useRef<null | (() => string | null | Promise<string | null>)>(null);
  const initialSnapshotRef = useRef<{
    config: any | null;
    name: string;
    description: string;
  } | null>(null);
  const [sqlPreview, setSqlPreview] = useState<SqlPreviewState>({
    lastSql: null,
    lastConfigJson: null,
    dataColumns: [],
    dataRows: [],
    isRunning: false,
    error: null,
    durationMs: null,
    fabricDurationMs: null,
    rowCount: null,
    savedSql: null,
  });

  useEffect(() => {
    if (!isAuthenticated) return;

    const loadDatasets = async () => {
      try {
        const res = await msalFetch(`${API_BASE}/api/v1/datasets/summary`);
        if (!res.ok) {
          throw new Error(`Failed to load datasets: ${res.status}`);
        }
        const data = await res.json();
        // The API returns { count, recent: [...] }
        setDatasets((data.recent || []).map((d: any) => ({
          id: d.id,
          name: d.dataset_name,
          description: d.description || null
        })));
      } catch (e: unknown) {
        const message = e instanceof Error ? e.message : "Unknown error";
        setDatasetsError(message);
      }
    };

    void loadDatasets();
  }, [isAuthenticated]);

  useEffect(() => {
    if (!isAuthenticated || !selectedDatasetId) {
      setDatasetColumns([]);
      setDatasetMetrics([]);
      setDatasetDetailError(null);
      setDatasetDetail(null);
      setMetricColumn(null);
      setMetrics([]);
      setTimeGrain("none");
      setSortBy(null);
      setQueryMode("aggregate");
      setGroupByColumns([]);
      setTimeColumn(null);
      setFilterLogic("AND");
      setFilters([]);
      return;
    }

    const loadDatasetDetail = async () => {
      try {
        const res = await msalFetch(`${API_BASE}/api/v1/datasets/${selectedDatasetId}`);
        if (!res.ok) {
          throw new Error(`Failed to load dataset detail: ${res.status}`);
        }
        const detail = await res.json();
        setDatasetColumns(detail.columns ?? []);
        setDatasetMetrics(detail.metrics ?? []);
        setDatasetDetail({
          table_name: detail.table_name,
          schema_name: detail.schema_name ?? null,
          database_name: detail.database_name ?? null,
          date_column: detail.date_column ?? null,
        });
        setDatasetDetailError(null);
      } catch (e: unknown) {
        const message = e instanceof Error ? e.message : "Unknown error";
        setDatasetDetailError(message);
        setDatasetColumns([]);
        setDatasetMetrics([]);
        setDatasetDetail(null);
      }
    };

    void loadDatasetDetail();
  }, [isAuthenticated, selectedDatasetId]);

  // Initialize from props on mount
  useEffect(() => {
    if (initialTemplate && [...TEMPLATES, ...getRegisteredPlugins()].some((t) => t.id === initialTemplate)) {
      setChartType(initialTemplate as ChartKind);
    }

    if (initialDatasetId) {
      const parsed = typeof initialDatasetId === 'string' ? Number(initialDatasetId) : initialDatasetId;
      if (!Number.isNaN(parsed)) {
        setSelectedDatasetId(parsed);
      }
    }

    // Note: Chart loading for editing is now handled by ChartBuilderHydrator component in pages
    // This keeps the context simpler and allows pages to control the loading logic
  }, [initialTemplate, initialDatasetId, initialChartId]);

  // Merge built-in TEMPLATES with registered plugins so plugin chart types
  // appear in the picker and are handled throughout the builder.
  const allTemplates = useMemo(() => {
    const plugins = getRegisteredPlugins();
    if (plugins.length === 0) return TEMPLATES;
    const pluginIds = new Set(plugins.map((p) => p.id));
    return [...TEMPLATES.filter((t) => !pluginIds.has(t.id)), ...plugins];
  }, []);

  const selectedTemplate = useMemo(
    () => (chartType ? allTemplates.find((t) => t.id === chartType) ?? null : null),
    [chartType, allTemplates],
  );

  const categories = useMemo(
    () => CHART_CATEGORIES.filter((cat) => allTemplates.some((t) => t.category === cat.id)),
    [allTemplates],
  );

  // Save button is always enabled (removed hasChanges check)
  const canSave = Boolean(selectedTemplate && selectedDatasetId && !isSaving);

  // Enhanced to support all UI date format options and robustly handle datetime strings
  // IMPORTANT: Parse dates without timezone conversion to keep the exact date from Fabric
  const formatXAxisLabel = (value: string, displayFormat: string): string => {
    if (!value || displayFormat === "auto") return value;

    // Remove time portion if present (e.g., '2025-07-01 00:00:00' -> '2025-07-01')
    const datePart = value.split(" ")[0];

    // Parse date manually to avoid timezone conversion issues
    // When using new Date('2025-07-01'), JS interprets as UTC and converts to local time,
    // which can shift the date by one day. We parse manually to keep the exact date.
    const parts = datePart.split("-");
    if (parts.length !== 3) return value;

    const year = parseInt(parts[0], 10);
    const month = parseInt(parts[1], 10);
    const day = parseInt(parts[2], 10);

    if (isNaN(year) || isNaN(month) || isNaN(day)) return value;

    const monthNames = [
      "Jan", "Feb", "Mar", "Apr", "May", "Jun",
      "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"
    ];

    switch (displayFormat) {
      case "YYYY-MM-DD":
        return `${year}-${String(month).padStart(2, "0")}-${String(day).padStart(2, "0")}`;
      case "MM/DD/YYYY":
        return `${String(month).padStart(2, "0")}/${String(day).padStart(2, "0")}/${year}`;
      case "MMM YYYY":
        return `${monthNames[month - 1]} ${year}`;
      case "MMM D, YYYY":
        return `${monthNames[month - 1]} ${day}, ${year}`;
      case "YYYY":
        return `${year}`;
      case "date":
        return `${year}-${String(month).padStart(2, "0")}-${String(day).padStart(2, "0")}`;
      case "weekday": {
        // For weekday, we need a Date object but use local timezone to avoid shifts
        const date = new Date(year, month - 1, day);
        const weekdays = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
        return weekdays[date.getDay()] ?? value;
      }
      case "month_year":
        return `${monthNames[month - 1]} ${year}`;
      default:
        return value;
    }
  };

  // Format numbers for tooltips based on user preferences
  const formatTooltipNumber = (value: number, format: string, decimalPlaces: number, useCommas: boolean = true): string => {
    let result: string;

    if (format === "k") {
      result = (value / 1e3).toFixed(decimalPlaces);
      return useCommas ? Number(result).toLocaleString(undefined, { minimumFractionDigits: decimalPlaces, maximumFractionDigits: decimalPlaces }) + "K" : result + "K";
    }
    if (format === "m") {
      result = (value / 1e6).toFixed(decimalPlaces);
      return useCommas ? Number(result).toLocaleString(undefined, { minimumFractionDigits: decimalPlaces, maximumFractionDigits: decimalPlaces }) + "M" : result + "M";
    }
    if (format === "b") {
      result = (value / 1e9).toFixed(decimalPlaces);
      return useCommas ? Number(result).toLocaleString(undefined, { minimumFractionDigits: decimalPlaces, maximumFractionDigits: decimalPlaces }) + "B" : result + "B";
    }
    if (format === "t") {
      result = (value / 1e12).toFixed(decimalPlaces);
      return useCommas ? Number(result).toLocaleString(undefined, { minimumFractionDigits: decimalPlaces, maximumFractionDigits: decimalPlaces }) + "T" : result + "T";
    }
    // Default: show with decimal places and optional comma separators
    if (useCommas) {
      return value.toLocaleString(undefined, { minimumFractionDigits: decimalPlaces, maximumFractionDigits: decimalPlaces });
    }
    return value.toFixed(decimalPlaces);
  };

  const buildQueryConfigForPreview = (extraFilters: any[] = []) => {
    if (!selectedTemplate || !selectedDatasetId) {
      return null;
    }

    const datasourceParts = [
      datasetDetail?.database_name,
      datasetDetail?.schema_name,
      datasetDetail?.table_name,
    ].filter(Boolean) as string[];
    const datasource = datasourceParts.join(".") || datasetDetail?.table_name || null;

    const primaryFilters = filters.filter((f) => !f.groupKey || f.groupKey === "primary");
    const secondaryFilters = filters.filter((f) => f.groupKey === "secondary");

    const buildFiltersPayload = (source: typeof filters) =>
      source
        .filter((f) => f.column && f.operator && (f.value !== "" || f.valueKey !== ""))
        .map((f) => {
          const op = (f.operator || "=").toUpperCase();

          // Prepare values for IN operator
          let displayValue = f.value;
          let keyValue = f.valueKey;

          if (op === "IN") {
            // For IN operator, split comma-separated values
            displayValue = f.value;
            keyValue = f.valueKey || f.value;
          }

          const payload = {
            column: f.column, // Display column (e.g., ProductName)
            keyColumn: f.keyColumn, // Key column for filtering (e.g., ProductPrimaryKey)
            columnLabel: f.columnLabel,
            op,
            value: displayValue, // Display value (e.g., "M365 Copilot All Up")
            valueKey: keyValue, // Key to use in SQL (e.g., "4999588089921664978")
          };

          return payload;
        });

    const extraFiltersPayload = extraFilters
      .filter((f: any) => f.column && f.value !== '' && f.value !== null && f.value !== undefined
              && f.value !== 'AllUp' && (f.filterType || 'value') !== 'date_range')
      .map((f: any) => ({
        column: f.column,
        keyColumn: f.keyColumn ?? null,
        columnLabel: f.columnLabel ?? null,
        op: (f.operator || "=").toUpperCase(),
        value: String(f.value ?? ""),
        valueKey: f.valueKey ?? "",
      }));

    const filtersPayload = [...buildFiltersPayload(primaryFilters), ...extraFiltersPayload];
    const secondaryFiltersPayload = buildFiltersPayload(secondaryFilters);
    const filterGroups = secondaryFiltersPayload.length
      ? [
          {
            logic: filterLogic,
            filters: filtersPayload,
          },
          {
            logic: filterLogic === "AND" ? "OR" : "AND",
            filters: secondaryFiltersPayload,
          },
        ]
      : null;

    // Build metrics array — prefer the metrics[] array, fall back to legacy metricColumn
    const metricsPayload = metrics.length > 0
      ? metrics.map((m) => ({ column: m.column, aggregate: m.aggregate, label: m.label }))
      : metricColumn
        ? [{ column: metricColumn, aggregate: "SUM", label: "value" }]
        : [];

    return {
      dataset_id: selectedDatasetId,
      template: selectedTemplate.id,
      datasource,
      metrics: metricsPayload,
      // legacy single-metric kept for backward compat with saved charts
      metric: metricColumn && metrics.length === 0 ? { column: metricColumn, agg: "SUM" } : null,
      groupby: groupByColumns,
      // Scatter/bubble axis columns (by name) — the renderer maps these instead
      // of guessing by position.
      x_axis: scatterAxes.x || null,
      y_axis: scatterAxes.y || null,
      size_column: scatterAxes.size || null,
      label_column: scatterAxes.label || null,
      category_labels: categoryLabels || null,
      time_column: timeColumn,
      time_grain: timeGrain !== "none" ? timeGrain : null,
      sort_by: sortBy,
      query_mode: queryMode,
      filters: filtersPayload,
      filter_logic: filterLogic,
      filter_groups: filterGroups,
      time_range: timeRange,
      custom_start_date: customStartDate,
      custom_end_date: customEndDate,
      date_display_format: dateDisplayFormat,
      row_limit: rowLimit,
      rolling_calc: advancedOptions?.rollingCalc || "none",
      rolling_window: advancedOptions?.rollingWindow || 3,
    };
  };

  const registerInitialSnapshot = () => {
    if (initialSnapshotRef.current) return;
    const config = buildQueryConfigForPreview();
    initialSnapshotRef.current = {
      config,
      name: name.trim(),
      description: description.trim(),
    };
    setHasChanges(false);
  };

  /**
   * Apply chart-type-specific options stored in advancedOptions.chartTypeOptions
   * to the ECharts option object just before rendering.
   */
  const applyChartTypeOptions = (option: any, chartKind: string, ctOpts: any): any => {
    if (!ctOpts || !option) return option;

    const isBar = ["bar_vertical", "bar_horizontal", "stacked_bar_vertical", "stacked_bar_horizontal", "grouped_bar"].includes(chartKind);
    const isPieOrDonut = chartKind === "pie" || chartKind === "donut";
    const isScatterOrBubble = chartKind === "scatter" || chartKind === "bubble";

    if (isBar && Array.isArray(option.series)) {
      option = {
        ...option,
        series: option.series.map((s: any) => {
          const updated: any = { ...s };
          if (ctOpts.barBorderRadius !== undefined) {
            updated.itemStyle = { ...s.itemStyle, borderRadius: Number(ctOpts.barBorderRadius) };
          }
          if (ctOpts.showDataLabels !== undefined) {
            const labelNumFmt = ctOpts.labelNumberFormat || "none";
            const fmtLabelVal = (v: number) => {
              if (labelNumFmt === "k") return `${(v / 1e3).toFixed(1)}K`;
              if (labelNumFmt === "m") return `${(v / 1e6).toFixed(1)}M`;
              if (labelNumFmt === "b") return `${(v / 1e9).toFixed(1)}B`;
              if (labelNumFmt === "t") return `${(v / 1e12).toFixed(1)}T`;
              return abbreviateNumber(v); // default: auto K/M/B/T
            };
            updated.label = {
              ...s.label,
              show: ctOpts.showDataLabels,
              position: ctOpts.labelPosition || (chartKind === "bar_horizontal" ? "right" : "top"),
              fontSize: 11,
              color: "#334155",
              formatter: ctOpts.showDataLabels
                ? (params: any) => fmtLabelVal(Number(params.value))
                : undefined,
            };
          }
          return updated;
        }),
      };
    }

    if (isPieOrDonut && Array.isArray(option.series)) {
      option = {
        ...option,
        series: option.series.map((s: any) => {
          const updated: any = { ...s };
          if (ctOpts.showPieLabels !== undefined) {
            const labelContent = ctOpts.labelContent || "name_pct";
            const formatter =
              labelContent === "percentage" ? "{d}%" :
              labelContent === "value"      ? "{c}" :
              labelContent === "name_value" ? "{b}: {c}" :
                                              "{b}: {d}%";
            updated.label = { show: ctOpts.showPieLabels, formatter };
            updated.labelLine = { show: ctOpts.showPieLabels };
          }
          if (chartKind === "donut" && ctOpts.donutHoleSize !== undefined) {
            updated.radius = [`${ctOpts.donutHoleSize}%`, "75%"];
          }
          if (ctOpts.roseType !== undefined) {
            updated.roseType = ctOpts.roseType ? "area" : undefined;
          }
          return updated;
        }),
      };
    }

    if (isScatterOrBubble && Array.isArray(option.series)) {
      option = {
        ...option,
        series: option.series.map((s: any) => {
          const updated: any = { ...s };
          // Don't clobber a bubble-size function (data-driven point size) with a
          // fixed manual size — only apply the manual size to plain scatters.
          if (ctOpts.symbolSize !== undefined && typeof s.symbolSize !== "function") {
            updated.symbolSize = Number(ctOpts.symbolSize);
          }
          if (ctOpts.pointOpacity !== undefined) {
            updated.itemStyle = { ...s.itemStyle, opacity: Number(ctOpts.pointOpacity) };
          }
          return updated;
        }),
      };
    }

    if (chartKind === "funnel" && Array.isArray(option.series)) {
      option = {
        ...option,
        series: option.series.map((s: any) => ({
          ...s,
          sort: ctOpts.funnelSort || s.sort || "descending",
          label: ctOpts.showFunnelLabels !== undefined ? { ...s.label, show: ctOpts.showFunnelLabels, position: ctOpts.funnelLabelPos || "inside" } : s.label,
        })),
      };
    }

    if (chartKind === "gauge" && Array.isArray(option.series)) {
      option = {
        ...option,
        series: option.series.map((s: any) => ({
          ...s,
          min: ctOpts.gaugeMin !== undefined ? Number(ctOpts.gaugeMin) : s.min,
          max: ctOpts.gaugeMax !== undefined ? Number(ctOpts.gaugeMax) : s.max,
        })),
      };
    }

    if (chartKind === "heatmap" && Array.isArray(option.series)) {
      option = {
        ...option,
        series: option.series.map((s: any) => ({
          ...s,
          label: ctOpts.showCellValues !== undefined ? { show: ctOpts.showCellValues } : s.label,
        })),
      };
    }

    if (chartKind === "radar") {
      if (option.radar && ctOpts.radarShape) {
        option = { ...option, radar: { ...option.radar, shape: ctOpts.radarShape } };
      }
      if (ctOpts.radarFill !== undefined && Array.isArray(option.series)) {
        option = {
          ...option,
          series: option.series.map((s: any) => ({
            ...s,
            areaStyle: ctOpts.radarFill ? { opacity: 0.4 } : undefined,
          })),
        };
      }
    }

    // Store kpiOptions for BigNumberKpiCard in ChartPreview
    if ((chartKind === "big_number" || chartKind === "big_number_trend") && ctOpts) {
      option = { ...option, kpiOptions: ctOpts };
    }

    if (chartKind === "mixed_line_bar") {
      const labelNumFmt = ctOpts.labelNumberFormat || "none";
      const seriesTypeMap = ctOpts.seriesTypes || {};
      const seriesAxisMap = ctOpts.seriesAxis || {};
      const fmtVal = (v: number) => {
        if (labelNumFmt === "k") return `${(v / 1e3).toFixed(1)}K`;
        if (labelNumFmt === "m") return `${(v / 1e6).toFixed(1)}M`;
        if (labelNumFmt === "b") return `${(v / 1e9).toFixed(1)}B`;
        if (labelNumFmt === "t") return `${(v / 1e12).toFixed(1)}T`;
        return v.toLocaleString();
      };
      if (option && Array.isArray(option.series)) {
        option.series = option.series.filter((s: any) => s.tooltip?.show !== false).map((s: any, idx: number) => {
          const st = seriesTypeMap[s.name] || (idx === 0 ? "bar" : "line");
          const yi = seriesAxisMap[s.name] ?? (idx === 0 ? 0 : 1);
          return {
            ...s,
            type: st,
            yAxisIndex: yi,
            smooth: st === "line",
            label: {
              ...s.label,
              show: ctOpts.showDataLabels ?? false,
              formatter: ctOpts.showDataLabels ? (params: any) => fmtVal(Number(params.value)) : undefined,
            },
          };
        });
        if (Array.isArray(option.yAxis) && option.yAxis.length >= 2) {
          option.yAxis[0] = { ...option.yAxis[0], name: ctOpts.leftAxisName || "" };
          option.yAxis[1] = { ...option.yAxis[1], name: ctOpts.rightAxisName || "" };
        }
      }
    }

    return option;
  };

  const buildEchartsOptionFromPreview = (
    chartKind: ChartKind,
    executeJson: { columns?: string[]; rows?: unknown[][] },
    config: ReturnType<typeof buildQueryConfigForPreview>,
  ): any | null => {
    if (!executeJson?.columns || !executeJson.rows || executeJson.columns.length === 0) {
      return null;
    }

    const columns = executeJson.columns;
    const rows = executeJson.rows as (string | number | null)[][];
    const xAxisDateFormat =
      (advancedOptions && advancedOptions.xAxisDateFormat) ||
      (previewOptions && previewOptions.xAxisDateFormat) ||
      (config?.date_display_format as DateDisplayFormat | undefined) ||
      "auto";
    const yAxisDateFormat: string =
      (advancedOptions && (advancedOptions as any).yAxisDateFormat) ||
      (previewOptions && previewOptions.yAxisDateFormat) ||
      "auto";

    const displayFormat: DateDisplayFormat = xAxisDateFormat;

    // Last column is the metric. A column named date/time is the x-axis for
    // time-series charts; for categorical charts (bar/pie/donut/map) the FIRST
    // column is the x-axis category — NOT a series. Series = any remaining column.
    const metricIndex = columns.length - 1;
    let timeIndex = columns.findIndex((c) => /date|time|^year$/i.test(c));
    const _isTimeKind = /time_series|_line|_area|line_multi|area_stack/.test(chartKind);
    if (timeIndex === -1 && columns.length >= 2 && _isTimeKind) {
      timeIndex = 1;
    }
    const xIndex = timeIndex >= 0 ? timeIndex : 0;

    const dimensionIndexes: number[] = [];
    columns.forEach((_, idx) => {
      if (idx !== metricIndex && idx !== xIndex) {
        dimensionIndexes.push(idx);
      }
    });

    // Multi-metric charts (e.g. an energy-mix stacked bar with solar/wind/hydro/…)
    // have several metric columns. The backend appends metrics last, so the final
    // `metricCount` columns are the metrics — turn each into its own series.
    const metricCount = Array.isArray(config?.metrics) && config.metrics.length > 0
      ? config.metrics.length : 1;
    const isMultiMetric = metricCount > 1 && columns.length > metricCount;
    const metricLabels: string[] = isMultiMetric
      ? (config!.metrics as any[]).map((m, i) => m.label || m.column || `metric ${i + 1}`)
      : [];

    const xValues: string[] = [];
    const seriesNamesSet = new Set<string>();
    const dataMap = new Map<string, Map<string, number>>();

    if (isMultiMetric) {
      const metricStart = columns.length - metricCount;
      rows.forEach((row) => {
        const x = row[0] == null ? "" : String(row[0]);
        if (!xValues.includes(x)) xValues.push(x);
        for (let j = 0; j < metricCount; j++) {
          const sName = metricLabels[j];
          const raw = row[metricStart + j];
          const val = raw == null || raw === "" ? 0 : Number(raw);
          seriesNamesSet.add(sName);
          if (!dataMap.has(sName)) dataMap.set(sName, new Map());
          dataMap.get(sName)!.set(x, val);
        }
      });
    } else {
      rows.forEach((row) => {
        const xRaw = row[xIndex];
        const x = xRaw == null ? "" : String(xRaw);
        if (!xValues.includes(x)) xValues.push(x);

        const metricRaw = row[metricIndex];
        const metric = metricRaw == null || metricRaw === "" ? 0 : Number(metricRaw);

        const seriesName =
          dimensionIndexes.length === 0
            ? columns[metricIndex] ?? "value"
            : dimensionIndexes.map((idx) => String(row[idx] ?? "")).join(" · ");

        seriesNamesSet.add(seriesName);
        if (!dataMap.has(seriesName)) dataMap.set(seriesName, new Map());
        dataMap.get(seriesName)!.set(x, metric);
      });
    }

    let seriesNames = Array.from(seriesNamesSet);
    // Apply legend order if present in advancedOptions, but always validate
    let legendOrder = seriesNames;
    if (advancedOptions?.legend?.order && Array.isArray(advancedOptions.legend.order)) {
      const order = advancedOptions.legend.order;
      // Only keep valid series names, and append any missing ones
      const validOrder = order.filter((n: string) => seriesNames.includes(n));
      const missing = seriesNames.filter((n: string) => !validOrder.includes(n));
      if (validOrder.length > 0) {
        legendOrder = [...validOrder, ...missing];
      }
    }
    seriesNames = legendOrder;

    const buildLineOrBarSeries = (opts: { type: "line" | "bar"; stacked?: boolean; smooth?: boolean }) => {
      // Time-series charts: line and area variants share the same template
      const isTimeSeriesKind =
        chartKind === "time_series_line" ||
        chartKind === "time_series_area" ||
        chartKind === "time_series_line_share" ||
        chartKind === "time_series_area_share";

      // Check if this is an area chart (filled line)
      const isAreaChart = chartKind.includes("area");

      // Share variants: line_share and area_share both show percentages and share the same template
      const isShareChart =
        chartKind === "time_series_area_share" || chartKind === "time_series_line_share";
      const isStackedBarPercentage =
        (chartKind === "stacked_bar_vertical" || chartKind === "stacked_bar_horizontal") &&
        advancedOptions?.showAsPercentage === true;
      const isPercentageMode = isShareChart || isStackedBarPercentage;
      const xTotals: Record<string, number> = {};
      if (isPercentageMode) {
        xValues.forEach((x) => {
          let total = 0;
          seriesNames.forEach((name) => {
            const v = dataMap.get(name)?.get(x) ?? 0;
            total += v;
          });
          xTotals[x] = total;
        });
      }

      // For share charts, use showAsPercentage for stacking (new format)
      let stackingOverride: boolean | undefined = undefined;
      let smoothOverride: boolean | undefined = undefined;
      if (isShareChart && advancedOptions) {
        // Use showAsPercentage for stacking control (default true if not set)
        stackingOverride = advancedOptions.showAsPercentage !== false;
        // If any series has smooth, enable smooth
        smoothOverride = Array.isArray(advancedOptions.series) && advancedOptions.series.some((s: any) => s.smooth);
      }

      // Check if markers are enabled in saved settings
      let markersEnabled: boolean = false;
      if (advancedOptions) {
        if (advancedOptions.seriesSettings) {
          // New format: check global settings
          markersEnabled = advancedOptions.seriesSettings.symbol && advancedOptions.seriesSettings.symbol !== "none";
        } else if (Array.isArray(advancedOptions.series)) {
          // Old format: check if any series has markers
          markersEnabled = advancedOptions.series.some((s: any) => s.showSymbol || (s.symbol && s.symbol !== "none"));
        }
      }

      const _palette: string[] = (advancedOptions?.color && advancedOptions.color.length)
        ? advancedOptions.color
        : DEFAULT_ADVANCED_OPTIONS.color;
      // Single-series categorical bars get per-bar colours (colourful, PBI-like);
      // multi-series keep one colour per series so the legend stays meaningful.
      const _colorByData = opts.type === "bar" && seriesNames.length === 1 && !isTimeSeriesKind;

      return seriesNames.map((name, sIdx) => {
        const _c = _palette[sIdx % _palette.length];
        // Vertical gradient fill for area charts: series colour → transparent.
        const _areaGradient = {
          color: {
            type: "linear", x: 0, y: 0, x2: 0, y2: 1,
            colorStops: [
              { offset: 0, color: _c + "66" },
              { offset: 1, color: _c + "05" },
            ],
          },
        };
        return {
        name,
        type: opts.type,
        colorBy: _colorByData ? "data" : "series",
        stack: (isShareChart ? (stackingOverride ? "total" : undefined) : (opts.stacked ? "total" : undefined)),
        smooth: (isShareChart ? !!smoothOverride : (opts.smooth ?? (opts.type === "line"))),
        // High-end line styling: thicker, rounded stroke + soft gradient area fill
        lineStyle: opts.type === "line" ? { width: 3, cap: "round", join: "round" } : undefined,
        itemStyle: opts.type === "bar" ? { borderRadius: 4 } : undefined,
        // Only show markers for line charts when enabled
        // Show exactly 6 points: first, last, and 4 evenly spaced in between
        symbol: opts.type === "line" && isTimeSeriesKind && markersEnabled ? 'circle' : "none",
        symbolSize: markersEnabled ? 6 : 4,
        showSymbol: opts.type === "line" && isTimeSeriesKind && markersEnabled
          ? function (dataIndex: number) {
              const dataLength = xValues.length;

              // Always show first and last
              if (dataIndex === 0 || dataIndex === dataLength - 1) {
                return true;
              }

              // Calculate 4 evenly spaced middle points
              const point1 = Math.round((dataLength - 1) * 1 / 5);
              const point2 = Math.round((dataLength - 1) * 2 / 5);
              const point3 = Math.round((dataLength - 1) * 3 / 5);
              const point4 = Math.round((dataLength - 1) * 4 / 5);

              if (dataIndex === point1 || dataIndex === point2 || dataIndex === point3 || dataIndex === point4) {
                return true;
              }

              return false;
            }
          : false,
        // Area charts (filled line): soft vertical gradient from the series colour
        areaStyle: isAreaChart ? _areaGradient : undefined,
        // Show labels on marker points only
        label: markersEnabled
          ? {
              show: true,
              formatter: (params: any) => {
                const dataIndex = params.dataIndex;
                const dataLength = xValues.length;

                // Only show labels on the 6 marker points
                const point1 = Math.round((dataLength - 1) * 1 / 5);
                const point2 = Math.round((dataLength - 1) * 2 / 5);
                const point3 = Math.round((dataLength - 1) * 3 / 5);
                const point4 = Math.round((dataLength - 1) * 4 / 5);

                const isMarkerPoint = dataIndex === 0 || dataIndex === dataLength - 1 ||
                                      dataIndex === point1 || dataIndex === point2 ||
                                      dataIndex === point3 || dataIndex === point4;

                if (!isMarkerPoint) return "";

                const v = Number(params.value);
                if (Number.isNaN(v)) return "";
                if (isPercentageMode) {
                  return `${v.toFixed(1)}%`;
                }
                return v.toLocaleString();
              },
              position: "top",
              fontSize: 10,
              overflow: "truncate",
            }
          : isPercentageMode
          ? {
              show: true,
              formatter: (params: any) => {
                const v = Number(params.value);
                if (Number.isNaN(v)) return "";
                return `${v.toFixed(1)}%`;
              },
              position:
                chartKind === "time_series_area_share" && opts.type === "line"
                  ? "inside"
                  : "top",
              fontSize: 10,
              overflow: "truncate",
            }
          : undefined,
        labelLayout: markersEnabled || isPercentageMode ? { hideOverlap: true } : undefined,
        data: xValues.map((x, dataIndex) => {
          const raw = dataMap.get(name)?.get(x) ?? 0;
          const value = isPercentageMode
            ? (xTotals[x] > 0 ? (raw / xTotals[x]) * 100 : 0)
            : raw;
          if (dataIndex === 0) return { value, label: { align: 'left' } };
          if (dataIndex === xValues.length - 1) return { value, label: { align: 'right' } };
          return value;
        }),
      };
      });
    };

    // Build custom tooltip formatter based on user preferences
    const tooltipDateFormat = advancedOptions?.tooltip?.dateFormat || "auto";
    const tooltipNumberFormat = advancedOptions?.tooltip?.numberFormat || "none";
    const tooltipDecimalPlaces = advancedOptions?.tooltip?.decimalPlaces ?? 2;
    const tooltipUseCommas = advancedOptions?.tooltip?.useCommas ?? true;

    const buildTooltipFormatter = () => {
      return (params: any) => {
        if (!Array.isArray(params)) params = [params];

        // Get the axis value (date/x-axis value)
        const axisValue = params[0]?.axisValue || params[0]?.name;
        let formattedAxisValue = axisValue;

        // Format date if dateFormat is specified
        if (tooltipDateFormat && tooltipDateFormat !== "auto" && axisValue) {
          formattedAxisValue = formatXAxisLabel(String(axisValue), tooltipDateFormat);
        }

        // Build the tooltip content
        let result = `<strong>${formattedAxisValue}</strong><br/>`;

        params.forEach((param: any) => {
          const value = Number(param.value);
          let formattedValue = "";

          if (!Number.isNaN(value)) {
            formattedValue = formatTooltipNumber(value, tooltipNumberFormat, tooltipDecimalPlaces, tooltipUseCommas);
          } else {
            formattedValue = String(param.value ?? "");
          }

          result += `${param.marker} ${param.seriesName}: ${formattedValue}<br/>`;
        });

        return result;
      };
    };

    const common = {
      tooltip: {
        trigger: "axis",
        formatter: tooltipDateFormat !== "auto" || tooltipNumberFormat !== "none" || tooltipUseCommas
          ? buildTooltipFormatter()
          : undefined,
      },
      legend: legendOrder.length > 1 ? { data: legendOrder } : undefined,
      grid: {
        left: 50,
        right: 30,
        top: 60,
        bottom: 50,
        containLabel: true,
      },
    } as any;

    // Map UI dropdown values to internal displayFormat
    const mapDateFormat = (val: string | undefined): DateDisplayFormat => {
      switch (val) {
        case "YYYY-MM-DD":
        case "MM/DD/YYYY":
        case "MMM D, YYYY":
          return "date";
        case "MMM YYYY":
        case "YYYY":
          return "month_year";
        default:
          return (val as DateDisplayFormat) || "auto";
      }
    };
    const buildCategoryAxis = (boundaryGap: boolean = true) => {
      // Use the actual UI format string for the formatter
      const axis: any = { type: "category", data: xValues, boundaryGap };
      if (xAxisDateFormat && xAxisDateFormat !== "auto") {
        axis.axisLabel = {
          formatter: (val: string) => formatXAxisLabel(String(val), xAxisDateFormat),
        };
      }
      // Optional friendly category labels (e.g. is_open_source false/true →
      // Proprietary/Open Source), applied to display only.
      const catLabels = (config as any)?.category_labels;
      if (catLabels && typeof catLabels === "object") {
        axis.axisLabel = {
          ...(axis.axisLabel || {}),
          formatter: (val: string) => catLabels[String(val)] ?? val,
        };
      }
      return axis;
    };

    // Value axis with a smart non-zero baseline: when values are large AND
    // clustered away from 0 (e.g. Arena ELO ~1200–1400), a 0-baseline makes every
    // bar look identical. scale:true lets ECharts pick a sensible min so the
    // differences read. Small values (min < 100, e.g. counts) keep a 0 baseline so
    // they aren't misleadingly truncated.
    const valueAxis = (extra: any = {}) => {
      let vMin = Infinity, vMax = -Infinity;
      dataMap.forEach((m) => m.forEach((v) => { if (v < vMin) vMin = v; if (v > vMax) vMax = v; }));
      const clustered = Number.isFinite(vMin) && vMin > 100 && vMax > 0 && vMin / vMax > 0.5;
      return { type: "value", ...(clustered ? { scale: true } : {}), ...extra };
    };

    switch (chartKind) {
      case "time_series_line":
      case "time_series_line_share":
      case "time_series_area":
      case "time_series_area_share":
      case "line_multi_series":
      case "area_stack":
        const isPercentageArea =
          chartKind === "time_series_area_share" || chartKind === "time_series_line_share";
        return {
          ...common,
          tooltip: isPercentageArea
            ? {
                ...(common.tooltip ?? {}),
                valueFormatter: (value: number | string) => {
                  const num = Number(value);
                  if (Number.isNaN(num)) return String(value ?? "");
                  return `${num.toFixed(1)}%`;
                },
              }
            : common.tooltip,
          xAxis: buildCategoryAxis(false),
          yAxis: isPercentageArea
            ? {
                type: "value",
                axisLabel: {
                  hideOverlap: true,
                  formatter: (val: number | string) => {
                    const num = Number(val);
                    if (Number.isNaN(num)) return String(val ?? "");
                    return `${num.toFixed(1)}%`;
                  },
                },
              }
            : { type: "value" },
          series: buildLineOrBarSeries({
            type: "line",
            stacked:
              chartKind === "area_stack" ||
              // For share charts, honor the showAsPercentage checkbox (default true if not set)
              (isPercentageArea && (advancedOptions?.showAsPercentage !== false)),
            smooth: Array.isArray(advancedOptions?.series) && advancedOptions.series.some((s: any) => s.smooth),
          }),
        };
      case "bar_vertical":
      case "grouped_bar":
      case "stacked_bar_vertical": {
        const pctBar = chartKind === "stacked_bar_vertical" && advancedOptions?.showAsPercentage === true;
        const pctAxis = pctBar ? {
          type: "value",
          max: 100,
          axisLabel: { formatter: (val: number) => `${val}%` },
        } : (chartKind === "stacked_bar_vertical" ? { type: "value" } : valueAxis());
        return {
          ...common,
          tooltip: pctBar
            ? { ...(common.tooltip ?? {}), valueFormatter: (v: number | string) => `${Number(v).toFixed(1)}%` }
            : common.tooltip,
          xAxis: buildCategoryAxis(),
          yAxis: pctAxis,
          series: buildLineOrBarSeries({
            type: "bar",
            stacked: chartKind === "stacked_bar_vertical",
          }),
        };
      }
      case "bar_horizontal":
      case "stacked_bar_horizontal": {
        const pctBarH = chartKind === "stacked_bar_horizontal" && advancedOptions?.showAsPercentage === true;
        const hBarYAxis: any = { type: "category", data: xValues };
        if (yAxisDateFormat && yAxisDateFormat !== "auto") {
          hBarYAxis.axisLabel = {
            formatter: (val: string) => formatXAxisLabel(String(val), yAxisDateFormat as DateDisplayFormat),
          };
        }
        return {
          ...common,
          tooltip: pctBarH
            ? { ...(common.tooltip ?? {}), valueFormatter: (v: number | string) => `${Number(v).toFixed(1)}%` }
            : common.tooltip,
          xAxis: pctBarH
            ? { type: "value", max: 100, axisLabel: { formatter: (val: number) => `${val}%` } }
            : (chartKind === "stacked_bar_horizontal" ? { type: "value" } : valueAxis()),
          yAxis: hBarYAxis,
          series: buildLineOrBarSeries({
            type: "bar",
            stacked: chartKind === "stacked_bar_horizontal",
          }),
        };
      }
      case "pie":
      case "donut": {
        const data = seriesNames.length > 1 ? seriesNames : xValues;
        const pieData =
          seriesNames.length > 1
            ? seriesNames.map((name) => ({
                name,
                value: xValues.reduce((sum, x) => sum + (dataMap.get(name)?.get(x) ?? 0), 0),
              }))
            : xValues.map((x) => ({ name: x, value: dataMap.get(seriesNames[0])?.get(x) ?? 0 }));

        // Custom formatter for pie/donut tooltips
        const pieTooltipFormatter = (params: any) => {
          const value = Number(params.value);
          let formattedValue = "";

          if (!Number.isNaN(value)) {
            formattedValue = formatTooltipNumber(value, tooltipNumberFormat, tooltipDecimalPlaces, tooltipUseCommas);
          } else {
            formattedValue = String(params.value ?? "");
          }

          return `${params.marker} ${params.name}: ${formattedValue} (${params.percent}%)`;
        };

        const pieTotal = pieData.reduce((s: number, d: any) => s + (Number(d.value) || 0), 0);
        const pieTotalFmt = Math.abs(pieTotal) >= 1e9 ? `${(pieTotal / 1e9).toFixed(1)}B`
          : Math.abs(pieTotal) >= 1e6 ? `${(pieTotal / 1e6).toFixed(1)}M`
          : Math.abs(pieTotal) >= 1e3 ? `${(pieTotal / 1e3).toFixed(1)}K`
          : pieTotal.toLocaleString(undefined, { maximumFractionDigits: 2 });
        return {
          // Explicitly clear cartesian components — pie/donut has no axes, and
          // without this an ECharts option merge can leave a prior chart's
          // xAxis/yAxis/grid drawing empty axis lines behind the pie.
          xAxis: [],
          yAxis: [],
          grid: undefined,
          tooltip: {
            trigger: "item",
            appendToBody: true,
            formatter: tooltipNumberFormat !== "none" || tooltipUseCommas ? pieTooltipFormatter : undefined,
          },
          // Horizontal scrolling legend across the top (PBI/Superset style); the
          // donut sits below it so labels never collide with the legend.
          legend: { type: "scroll", orient: "horizontal", top: 6, left: "center", icon: "circle",
                    itemWidth: 10, itemHeight: 10, itemGap: 14, textStyle: { fontSize: 12, color: "#475569" } },
          // Donut centre total (the whole selling point of a donut vs pie)
          ...(chartKind === "donut" ? {
            graphic: [{
              type: "text", left: "center", top: "50%",
              style: { text: "TOTAL", textAlign: "center", fill: "#94a3b8", fontSize: 10, fontWeight: 600 },
            }, {
              type: "text", left: "center", top: "55%",
              style: { text: pieTotalFmt, textAlign: "center", fill: "#0f172a", fontSize: 22, fontWeight: 700, fontFamily: "Inter, sans-serif" },
            }],
          } : {}),
          series: [
            {
              name: "Share",
              type: "pie",
              radius: chartKind === "donut" ? ["46%", "70%"] : ["0%", "68%"],
              center: ["50%", "57%"],
              avoidLabelOverlap: true,
              minAngle: 3,
              padAngle: chartKind === "donut" ? 2 : 0,
              itemStyle: { borderRadius: 6, borderColor: "#ffffff", borderWidth: 2 },
              label: {
                show: true,
                position: "outside",
                formatter: "{d}%",
                fontSize: 11,
                fontWeight: 600,
                color: "#475569",
              },
              labelLine: { show: true, length: 12, length2: 12, smooth: true, lineStyle: { color: "#cbd5e1" } },
              // Donut centre total
              ...(chartKind === "donut" ? {
                emphasis: {
                  scale: true, scaleSize: 6,
                  label: { show: true, fontSize: 15, fontWeight: "bold" },
                  itemStyle: { shadowBlur: 18, shadowColor: "rgba(0,0,0,0.20)" },
                },
              } : {
                emphasis: {
                  scale: true, scaleSize: 8,
                  itemStyle: { shadowBlur: 18, shadowColor: "rgba(0,0,0,0.20)" },
                },
              }),
              data: pieData,
            },
          ],
        };
      }
      case "big_number":
      case "big_number_trend": {
        const latestRow = rows[rows.length - 1];
        const metricRaw = latestRow?.[metricIndex];
        const value = metricRaw == null || metricRaw === "" ? 0 : Number(metricRaw);
        return {
          title: {
            text: value.toLocaleString(),
            left: "center",
            top: "40%",
            textStyle: {
              fontSize: 32,
              fontWeight: "600",
              color: "#0f172a",
            },
          },
        };
      }
      case "scatter":
      case "bubble": {
        // Prefer the configured axes by COLUMN NAME (x_axis/y_axis/size/label) —
        // robust even when the query returns extra columns (e.g. a huge params or
        // context-window column that would otherwise get plotted on Y and blow the
        // scale to the millions). Fall back to column-type detection only when the
        // config didn't specify them.
        const isNumericCol = (i: number) =>
          rows.some((r) => r[i] != null && r[i] !== "" && !Number.isNaN(Number(r[i])));
        const byName = (name: any): number =>
          typeof name === "string" && name ? columns.indexOf(name) : -1;
        let xIdx = byName(config?.x_axis);
        let yIdx = byName(config?.y_axis);
        let labelIdx = byName(config?.label_column);
        const sizeIdx = byName(config?.size_column); // bubble-size ONLY when configured
        if (xIdx < 0 || yIdx < 0 || labelIdx < 0) {
          const autoLabel = columns.findIndex((_, i) => !isNumericCol(i));
          const numericIdx = columns.map((_, i) => i).filter((i) => i !== autoLabel && isNumericCol(i));
          if (xIdx < 0) xIdx = numericIdx[0] ?? 0;
          if (yIdx < 0) yIdx = numericIdx[1] ?? Math.min(1, columns.length - 1);
          if (labelIdx < 0) labelIdx = autoLabel;
        }

        const sizeVals = sizeIdx >= 0 ? rows.map((r) => Number(r[sizeIdx]) || 0) : [];
        const sMin = sizeVals.length ? Math.min(...sizeVals) : 0;
        const sMax = sizeVals.length ? Math.max(...sizeVals) : 0;
        const scaleSize = (v: number) =>
          sizeIdx < 0 || sMax === sMin ? 14 : 8 + ((v - sMin) / (sMax - sMin)) * 42; // 8–50px

        const scatterData = rows.map((r) => {
          const pt: any[] = [Number(r[xIdx]), Number(r[yIdx])];
          if (sizeIdx >= 0) pt.push(Number(r[sizeIdx]) || 0);
          if (labelIdx >= 0) pt.push(r[labelIdx]);
          return pt;
        });

        return {
          ...common,
          tooltip: {
            trigger: "item",
            appendToBody: true,
            formatter: (p: any) => {
              const d = p.data as any[];
              const lbl = labelIdx >= 0 ? `<b>${d[d.length - 1]}</b><br/>` : "";
              let s = `${lbl}${columns[xIdx]}: ${d[0]}<br/>${columns[yIdx]}: ${d[1]}`;
              if (sizeIdx >= 0) s += `<br/>${columns[sizeIdx]}: ${d[2]}`;
              return s;
            },
          },
          xAxis: { type: "value", name: columns[xIdx], nameLocation: "center", nameGap: 30, scale: true },
          yAxis: { type: "value", name: columns[yIdx], scale: true },
          series: [{
            type: "scatter",
            data: scatterData,
            symbolSize: sizeIdx >= 0 ? (d: any[]) => scaleSize(Number(d[2])) : 14,
            itemStyle: { opacity: 0.8 },
            label:
              labelIdx >= 0 && (advancedOptions as any)?.showDataLabels
                ? { show: true, position: "top", fontSize: 10, formatter: (p: any) => { const d = p.data as any[]; return d[d.length - 1]; } }
                : undefined,
          }],
        };
      }
      case "heatmap": {
        // col[0]=x-category, col[1]=y-category, col[2]=value
        const hxCats = [...new Set(rows.map((r) => String(r[0] ?? "")))];
        const hyCats = [...new Set(rows.map((r) => String(r[1] ?? "")))];
        const heatData = rows.map((r) => [
          hxCats.indexOf(String(r[0] ?? "")),
          hyCats.indexOf(String(r[1] ?? "")),
          Number(r[2] ?? 0),
        ]);
        const heatVals = heatData.map((d) => d[2] as number);
        const heatMin = Math.min(...heatVals);
        const heatMax = Math.max(...heatVals);
        return {
          tooltip: { trigger: "item", formatter: (p: any) => `${hxCats[p.data[0]]}, ${hyCats[p.data[1]]}: ${p.data[2]}` },
          grid: { ...common.grid, bottom: "15%" },
          xAxis: { type: "category", data: hxCats, splitArea: { show: true } },
          yAxis: { type: "category", data: hyCats, splitArea: { show: true } },
          visualMap: {
            min: heatMin, max: heatMax, calculable: true,
            orient: "horizontal", left: "center", bottom: "2%",
            inRange: { color: ["#e0f3f8", "#abd9e9", "#74add1", "#4575b4", "#313695"] },
          },
          series: [{
            name: columns[2] || "value",
            type: "heatmap",
            data: heatData,
            label: { show: false },
            emphasis: { itemStyle: { shadowBlur: 10, shadowColor: "rgba(0,0,0,0.5)" } },
          }],
        };
      }
      case "radar": {
        if (columns.length > 2) {
          // Multi-column: col[0]=series name, rest=indicator values
          const indicators = columns.slice(1).map((c, i) => ({
            name: c,
            max: Math.max(...rows.map((r) => Number(r[i + 1] ?? 0))) * 1.25 || 100,
          }));
          const radarData = rows.map((r) => ({
            name: String(r[0] ?? ""),
            value: columns.slice(1).map((_, i) => Number(r[i + 1] ?? 0)),
          }));
          return {
            tooltip: { trigger: "item" },
            legend: { data: radarData.map((d) => d.name) },
            radar: { indicator: indicators },
            series: [{ type: "radar", data: radarData }],
          };
        }
        // 2 columns: col[0]=indicator name, col[1]=value → single radar shape
        const indicators2 = rows.map((r) => ({
          name: String(r[0] ?? ""),
          max: Math.max(...rows.map((row) => Number(row[1] ?? 0))) * 1.25 || 100,
        }));
        const vals2 = rows.map((r) => Number(r[1] ?? 0));
        return {
          tooltip: { trigger: "item" },
          radar: { indicator: indicators2 },
          series: [{ type: "radar", data: [{ name: columns[1] || "value", value: vals2 }] }],
        };
      }
      case "funnel": {
        const funnelData = xValues
          .map((x) => ({
            name: x,
            value: seriesNames.reduce((sum, n) => sum + (dataMap.get(n)?.get(x) ?? 0), 0),
          }))
          .sort((a, b) => b.value - a.value);
        return {
          tooltip: { trigger: "item", formatter: "{b} : {c} ({d}%)" },
          legend: { data: funnelData.map((d) => d.name) },
          series: [{
            name: columns[metricIndex] || "value",
            type: "funnel",
            left: "10%", width: "80%", top: 60, bottom: 60,
            min: 0, max: funnelData[0]?.value || 100,
            minSize: "0%", maxSize: "100%",
            sort: "descending", gap: 2,
            label: { show: true, position: "inside" },
            labelLine: { length: 10, lineStyle: { width: 1, type: "solid" } },
            itemStyle: { borderColor: "#fff", borderWidth: 1 },
            emphasis: { label: { fontSize: 20 } },
            data: funnelData,
          }],
        };
      }
      case "gauge": {
        const gaugeRow = rows[rows.length - 1];
        const gaugeVal = Number(gaugeRow?.[metricIndex] ?? 0);
        const allMetricVals = rows.map((r) => Number(r[metricIndex] ?? 0)).filter((v) => !isNaN(v));
        const gaugeMax = allMetricVals.length > 0 ? Math.ceil(Math.max(...allMetricVals) * 1.25) : 100;
        return {
          tooltip: { formatter: "{a} <br/>{b} : {c}" },
          series: [{
            name: columns[metricIndex] || "value",
            type: "gauge",
            min: 0, max: gaugeMax,
            progress: { show: true, width: 18 },
            axisLine: { lineStyle: { width: 18 } },
            axisTick: { show: false },
            splitLine: { length: 15, lineStyle: { width: 2, color: "#999" } },
            axisLabel: { distance: 25, color: "#999", fontSize: 12 },
            anchor: { show: true, showAbove: true, size: 25, itemStyle: { borderWidth: 10 } },
            detail: { valueAnimation: true, fontSize: 28, offsetCenter: [0, "70%"] },
            data: [{ value: gaugeVal, name: columns[metricIndex] || "value" }],
          }],
        };
      }
      case "boxplot": {
        if (columns.length >= 6) {
          // Pre-aggregated: col[0]=category, col[1]=min, col[2]=Q1, col[3]=median, col[4]=Q3, col[5]=max
          return {
            ...common,
            xAxis: { type: "category", data: rows.map((r) => String(r[0] ?? "")) },
            yAxis: { type: "value" },
            series: [{ type: "boxplot", data: rows.map((r) => [r[1], r[2], r[3], r[4], r[5]].map(Number)) }],
          };
        }
        // Compute box stats grouped by dimension
        const bpGroups = new Map<string, number[]>();
        rows.forEach((r) => {
          const cat = String(r[0] ?? "");
          const val = Number(r[metricIndex] ?? 0);
          if (!bpGroups.has(cat)) bpGroups.set(cat, []);
          bpGroups.get(cat)!.push(val);
        });
        const bpCats: string[] = [];
        const bpData: number[][] = [];
        bpGroups.forEach((vals, cat) => {
          vals.sort((a, b) => a - b);
          const n = vals.length;
          bpCats.push(cat);
          bpData.push([
            vals[0],
            vals[Math.floor(n * 0.25)],
            vals[Math.floor(n * 0.5)],
            vals[Math.floor(n * 0.75)],
            vals[n - 1],
          ]);
        });
        return {
          ...common,
          xAxis: { type: "category", data: bpCats },
          yAxis: { type: "value" },
          series: [{ type: "boxplot", data: bpData }],
        };
      }
      case "candlestick": {
        // col[0]=date, col[1]=open, col[2]=close, col[3]=low, col[4]=high
        const csDate = rows.map((r) => String(r[0] ?? ""));
        const csData = rows.map((r) => [Number(r[1]), Number(r[2]), Number(r[3]), Number(r[4])]);
        return {
          tooltip: { trigger: "axis", axisPointer: { type: "cross" } },
          grid: common.grid,
          xAxis: { type: "category", data: csDate, scale: true, boundaryGap: false, axisLine: { onZero: false }, splitLine: { show: false }, min: "dataMin", max: "dataMax" },
          yAxis: { scale: true, splitArea: { show: true } },
          dataZoom: [
            { type: "inside", start: 0, end: 100 },
            { show: true, type: "slider", bottom: 10, start: 0, end: 100 },
          ],
          series: [{
            name: columns[0] || "Price",
            type: "candlestick",
            data: csData,
            itemStyle: { color: "#ef232a", color0: "#14b143", borderColor: "#ef232a", borderColor0: "#14b143" },
          }],
        };
      }
      case "treemap": {
        const tmData = xValues
          .map((x) => ({
            name: x,
            value: seriesNames.reduce((sum, n) => sum + (dataMap.get(n)?.get(x) ?? 0), 0),
          }))
          .filter((d) => d.value > 0);
        return {
          tooltip: { formatter: (info: any) => `${info.name}: ${Number(info.value).toLocaleString()}` },
          series: [{
            type: "treemap",
            data: tmData,
            leafDepth: 1,
            label: { show: true, formatter: "{b}" },
            upperLabel: { show: true, height: 30 },
            emphasis: { label: { fontSize: 14 } },
            breadcrumb: { show: false },
          }],
        };
      }
      case "sunburst": {
        if (columns.length >= 3) {
          // Hierarchical: col[0]=parent, col[1]=name, col[2]=value
          const sbParentMap = new Map<string, { name: string; value: number }[]>();
          rows.forEach((r) => {
            const parent = String(r[0] ?? "");
            if (!sbParentMap.has(parent)) sbParentMap.set(parent, []);
            sbParentMap.get(parent)!.push({ name: String(r[1] ?? ""), value: Number(r[2] ?? 0) });
          });
          const allChildNames = new Set([...sbParentMap.values()].flat().map((c) => c.name));
          const roots = [...sbParentMap.keys()].filter((k) => !allChildNames.has(k));
          const buildSbTree = (key: string): any[] =>
            (sbParentMap.get(key) || []).map((c) =>
              sbParentMap.has(c.name)
                ? { name: c.name, children: buildSbTree(c.name) }
                : c
            );
          const sbData =
            roots.length > 0
              ? roots.map((r) => ({ name: r, children: buildSbTree(r) }))
              : [...sbParentMap.entries()].map(([k, v]) => ({ name: k, children: v }));
          return {
            tooltip: { trigger: "item" },
            series: [{ type: "sunburst", data: sbData, radius: [0, "90%"], label: { rotate: "radial" }, emphasis: { focus: "ancestor" } }],
          };
        }
        // Flat sunburst: col[0]=name, col[1]=value
        const sbFlat = xValues.map((x) => ({
          name: x,
          value: seriesNames.reduce((sum, n) => sum + (dataMap.get(n)?.get(x) ?? 0), 0),
        }));
        return {
          tooltip: { trigger: "item" },
          series: [{ type: "sunburst", data: sbFlat, radius: [0, "90%"], label: { rotate: "radial" } }],
        };
      }
      case "pictorial_bar": {
        const pbData = xValues.map((x) =>
          seriesNames.reduce((sum, n) => sum + (dataMap.get(n)?.get(x) ?? 0), 0)
        );
        return {
          ...common,
          xAxis: buildCategoryAxis(),
          yAxis: { type: "value" },
          series: [{
            type: "pictorialBar",
            symbol: "roundRect",
            symbolRepeat: true,
            symbolSize: [20, 12],
            symbolMargin: 2,
            data: pbData,
          }],
        };
      }
      case "theme_river": {
        // col[0]=time, col[1]=value, col[2]=category  — or fall back to dimension/metric layout
        const trTime = (r: (string | number | null)[]) => String(r[0] ?? "");
        const trVal = (r: (string | number | null)[]) =>
          Number(columns.length >= 3 ? r[1] : r[metricIndex]) ?? 0;
        const trCat = (r: (string | number | null)[]) =>
          String(columns.length >= 3 ? r[2] : (dimensionIndexes.length > 0 ? r[dimensionIndexes[0]] : r[0])) ?? "";
        const trData: [string, number, string][] = rows.map((r) => [trTime(r), trVal(r), trCat(r)]);
        const trLegend = [...new Set(trData.map((d) => d[2]))];
        return {
          tooltip: { trigger: "axis", axisPointer: { type: "line" } },
          legend: { data: trLegend, top: 0 },
          singleAxis: {
            top: 50, bottom: 50, type: "time",
            axisTick: {}, axisLabel: {},
            axisPointer: { animation: true, label: { show: true } },
            splitLine: { show: true, lineStyle: { type: "dashed", opacity: 0.2 } },
          },
          series: [{ type: "themeRiver", emphasis: { focus: "series" }, data: trData }],
        };
      }
      case "table":
      case "pivot_table": {
        // Sentinel — ChartPreview detects _tableData and renders an HTML table instead of ECharts
        return { _tableData: true, _columns: columns, _rows: rows, _chartType: chartKind };
      }
      case "mixed_line_bar": {
        const seriesTypeMap: Record<string, "bar" | "line"> = advancedOptions?.chartTypeOptions?.seriesTypes || {};
        const seriesAxisMap: Record<string, number> = advancedOptions?.chartTypeOptions?.seriesAxis || {};

        const mixedSeries = seriesNames.map((name, idx) => {
          const seriesType = seriesTypeMap[name] || (idx === 0 ? "bar" : "line");
          const yAxisIndex = seriesAxisMap[name] ?? (idx === 0 ? 0 : 1);
          const ctOpts = advancedOptions?.chartTypeOptions || {};
          const labelNumFmt = ctOpts.labelNumberFormat || "none";
          const fmtVal = (v: number) => {
            if (labelNumFmt === "k") return `${(v / 1e3).toFixed(1)}K`;
            if (labelNumFmt === "m") return `${(v / 1e6).toFixed(1)}M`;
            if (labelNumFmt === "b") return `${(v / 1e9).toFixed(1)}B`;
            if (labelNumFmt === "t") return `${(v / 1e12).toFixed(1)}T`;
            return v.toLocaleString();
          };
          return {
            name,
            type: seriesType,
            yAxisIndex,
            data: xValues.map((x) => dataMap.get(name)?.get(x) ?? 0),
            smooth: seriesType === "line",
            label: {
              show: ctOpts.showDataLabels ?? false,
              position: "top",
              formatter: ctOpts.showDataLabels ? (params: any) => fmtVal(Number(params.value)) : undefined,
            },
          };
        });

        return {
          ...common,
          xAxis: buildCategoryAxis(),
          yAxis: [
            { type: "value", alignTicks: true },
            { type: "value", alignTicks: true, splitLine: { show: false } },
          ],
          series: mixedSeries,
        };
      }
      case "waterfall": {
        const wfRaw = xValues.map((x) => dataMap.get(seriesNames[0])?.get(x) ?? 0);
        let wfRunning = 0;
        const wfPlaceholders: number[] = [];
        const wfBars: any[] = [];
        wfRaw.forEach((v) => {
          wfPlaceholders.push(v >= 0 ? wfRunning : wfRunning + v);
          wfBars.push({ value: Math.abs(v), itemStyle: { color: v >= 0 ? "#22c55e" : "#ef4444" } });
          wfRunning += v;
        });
        return {
          ...common,
          xAxis: buildCategoryAxis(),
          yAxis: { type: "value" },
          series: [
            {
              type: "bar",
              stack: "waterfall",
              itemStyle: { color: "transparent", borderColor: "transparent" },
              data: wfPlaceholders,
              tooltip: { show: false },
            },
            {
              name: seriesNames[0] || "Value",
              type: "bar",
              stack: "waterfall",
              data: wfBars,
              label: { show: false },
            },
          ],
        };
      }
      case "nightingale_rose": {
        const roseData = seriesNames.length > 1
          ? seriesNames.map(name => ({ name, value: xValues.reduce((s, x) => s + (dataMap.get(name)?.get(x) ?? 0), 0) }))
          : xValues.map(x => ({ name: x, value: dataMap.get(seriesNames[0])?.get(x) ?? 0 }));
        return {
          tooltip: { trigger: "item", formatter: "{b}: {c} ({d}%)" },
          legend: { orient: "vertical", left: "left" },
          series: [{
            type: "pie", roseType: "area",
            radius: ["15%", "72%"], center: ["55%", "50%"],
            itemStyle: { borderRadius: 4, borderColor: "#fff", borderWidth: 2 },
            label: { show: false },
            emphasis: { label: { show: true, fontSize: 13, fontWeight: "bold" } },
            data: roseData,
          }],
        };
      }
      case "histogram": {
        const hvals = rows.map(r => Number(r[metricIndex])).filter(v => !isNaN(v));
        if (!hvals.length) return null;
        const hmin = Math.min(...hvals), hmax = Math.max(...hvals);
        const binCount = Math.min(20, Math.ceil(Math.sqrt(hvals.length)));
        const bw = (hmax - hmin) / binCount || 1;
        const counts = Array.from({ length: binCount }, () => 0);
        hvals.forEach(v => { counts[Math.min(Math.floor((v - hmin) / bw), binCount - 1)]++; });
        const histLabels = Array.from({ length: binCount }, (_, i) => {
          const mid = hmin + (i + 0.5) * bw;
          return Math.abs(mid) >= 1e6 ? `${(mid / 1e6).toFixed(1)}M` : Math.abs(mid) >= 1e3 ? `${(mid / 1e3).toFixed(1)}K` : mid.toFixed(1);
        });
        return {
          ...common,
          tooltip: {
            trigger: "axis",
            formatter: (params: any[]) => {
              const b0 = hmin + params[0].dataIndex * bw;
              const b1 = b0 + bw;
              return `${b0.toFixed(2)} – ${b1.toFixed(2)}: <b>${params[0].value}</b>`;
            },
          },
          xAxis: { type: "category", data: histLabels, name: columns[metricIndex] || "", nameLocation: "center", nameGap: 30 },
          yAxis: { type: "value", name: "Count" },
          series: [{ type: "bar", data: counts, barCategoryGap: "2%", itemStyle: { borderRadius: [3, 3, 0, 0] } }],
        };
      }
      case "sankey": {
        if (columns.length < 3) return null;
        const snkSet = new Set<string>();
        const snkLinks: { source: string; target: string; value: number }[] = [];
        rows.forEach(r => {
          const src = String(r[0] ?? ""), tgt = String(r[1] ?? ""), val = Number(r[2] ?? 0);
          if (src && tgt) { snkSet.add(src); snkSet.add(tgt); snkLinks.push({ source: src, target: tgt, value: val }); }
        });
        return {
          tooltip: {
            trigger: "item",
            formatter: (p: any) => p.dataType === "edge"
              ? `${p.data.source} → ${p.data.target}: <b>${p.data.value.toLocaleString()}</b>`
              : `<b>${p.name}</b>`,
          },
          series: [{
            type: "sankey",
            layout: "none",
            emphasis: { focus: "adjacency" },
            nodeAlign: "left",
            nodeGap: 12,
            nodeWidth: 18,
            label: { fontSize: 11 },
            data: Array.from(snkSet).map(n => ({ name: n })),
            links: snkLinks,
          }],
        };
      }
      case "calendar_heatmap": {
        if (columns.length < 2) return null;
        const calRows = rows.map(r => [String(r[0] ?? ""), Number(r[metricIndex] ?? 0)]);
        const calVals = calRows.map(d => d[1] as number);
        const calYears = [...new Set(calRows.map(d => String(d[0]).slice(0, 4)))].sort();
        if (!calYears.length) return null;
        return {
          tooltip: { formatter: (p: any) => `${p.data[0]}: <b>${Number(p.data[1]).toLocaleString()}</b>` },
          visualMap: {
            min: Math.min(...calVals), max: Math.max(...calVals),
            calculable: true, orient: "horizontal", left: "center", bottom: 10,
            inRange: { color: ["#e0f3f8", "#74add1", "#4575b4", "#313695"] },
          },
          calendar: calYears.map((y, i) => ({
            range: y, top: 60 + i * 170, left: 70, right: 20, cellSize: ["auto", 15],
            dayLabel: { firstDay: 1 }, monthLabel: { nameMap: "en" },
          })),
          series: calYears.map((y, i) => ({
            type: "heatmap", coordinateSystem: "calendar", calendarIndex: i,
            data: calRows.filter(d => String(d[0]).startsWith(y)),
          })),
        };
      }
      case "parallel_coordinates": {
        if (columns.length < 2) return null;
        const hasCat = isNaN(Number(rows[0]?.[0]));
        const dimStart = hasCat ? 1 : 0;
        const pcDims = columns.slice(dimStart);
        return {
          parallelAxis: pcDims.map((d, i) => ({ dim: i, name: d.split(".").pop() || d })),
          parallel: { left: "5%", right: "5%", bottom: "15%", top: "15%" },
          tooltip: { trigger: "item" },
          series: [{
            type: "parallel",
            lineStyle: { width: 1.5, opacity: 0.45 },
            data: rows.map(r => pcDims.map((_, i) => Number(r[dimStart + i] ?? 0))),
          }],
        };
      }
      case "world_map":
        return { _worldMap: true, _rows: rows, _columns: columns };
      default: {
        // Try registered plugins
        const plugin = getPlugin(chartKind);
        if (plugin?.buildOptions) {
          return plugin.buildOptions({ rows, columns, advancedOptions, config });
        }
        return null;
      }
    }
  };

  useEffect(() => {
    const currentConfig = buildQueryConfigForPreview();
    const snapshot = initialSnapshotRef.current;

    if (!snapshot) {
      // New chart (no saved baseline yet): consider the builder
      // "dirty" only after the user has provided some
      // meaningful input beyond the initial empty state.
      const hasAnyInput =
        Boolean(name.trim()) ||
        Boolean(description.trim()) ||
        Boolean(metricColumn) ||
        groupByColumns.length > 0 ||
        Boolean(timeColumn) ||
        filters.some((f) => f.column && f.operator && f.value !== "");
      setHasChanges(hasAnyInput);
      return;
    }

    const sameConfig =
      JSON.stringify(currentConfig ?? null) === JSON.stringify(snapshot.config ?? null);
    const sameName = name.trim() === snapshot.name;
    const sameDescription = description.trim() === snapshot.description;

    setHasChanges(!(sameConfig && sameName && sameDescription));
  }, [
    name,
    description,
    metricColumn,
    metrics,
    timeGrain,
    sortBy,
    queryMode,
    groupByColumns,
    timeColumn,
    timeRange,
    customStartDate,
    customEndDate,
    dateDisplayFormat,
    rowLimit,
    filters,
    filterLogic,
    selectedDatasetId,
    selectedTemplate,
  ]);

  // Update X axis formatter when xAxisDateFormat changes
  useEffect(() => {
    if (previewOptions && previewOptions.xAxis && previewOptions.xAxisDateFormat) {
      const dateFormat = previewOptions.xAxisDateFormat;
      if (dateFormat !== "auto") {
        setPreviewOptions((prev: any) => {
          if (!prev || !prev.xAxis) return prev;
          const updateAxisFormatter = (axis: any) => ({
            ...axis,
            axisLabel: {
              ...(axis.axisLabel || {}),
              formatter: (val: string) => formatXAxisLabel(String(val), dateFormat),
            },
          });
          return {
            ...prev,
            xAxis: Array.isArray(prev.xAxis)
              ? prev.xAxis.map(updateAxisFormatter)
              : updateAxisFormatter(prev.xAxis),
          };
        });
      }
    }
  }, [previewOptions?.xAxisDateFormat]);

  // Update Y axis formatter when yAxisDateFormat changes (horizontal bar charts)
  useEffect(() => {
    const dateFormat = previewOptions?.yAxisDateFormat;
    if (!previewOptions || !previewOptions.yAxis || !dateFormat) return;
    const fmt = dateFormat === "auto" ? null : (val: string) => formatXAxisLabel(String(val), dateFormat as DateDisplayFormat);
    setPreviewOptions((prev: any) => {
      if (!prev || !prev.yAxis) return prev;
      const updateAxisFormatter = (axis: any) => {
        if (axis.type !== "category") return axis;
        return {
          ...axis,
          axisLabel: fmt
            ? { ...(axis.axisLabel || {}), formatter: fmt }
            : { ...(axis.axisLabel || {}), formatter: undefined },
        };
      };
      return {
        ...prev,
        yAxis: Array.isArray(prev.yAxis)
          ? prev.yAxis.map(updateAxisFormatter)
          : updateAxisFormatter(prev.yAxis),
      };
    });
  }, [previewOptions?.yAxisDateFormat]);

  const runPreviewQuery = async (forceRegenerate: boolean = false, extraFilters: any[] = []) => {
    if (!selectedDatasetId || !selectedTemplate) {
      return;
    }

    // Prevent concurrent runs — if a query is already in flight, skip
    if (isQueryRunningRef.current) {
      return;
    }

    const config = buildQueryConfigForPreview(extraFilters);
    if (!config) return;

    isQueryRunningRef.current = true;
    setSqlPreview((prev) => ({ ...prev, isRunning: true, error: null }));

    // Acquire a slot from the global semaphore — limits concurrent dashboard queries
    const releaseSlot = await acquireQuerySlot();
    const start = performance.now();

    try {
      // ── Context-first path for dashboard charts ────────────────────────
      // When running inside a dashboard, try to serve the chart from the
      // DLM's precomputed context before generating SQL. Single-metric
      // breakdowns (the vast majority of dashboard charts) are answered
      // instantly from dlm_answers with zero database trip.
      const isDashboardCtx = (runContext || "").startsWith("dashboard");
      const isSingleMetric = (config.metrics?.length || 0) === 1;
      if (isDashboardCtx && !forceRegenerate && isSingleMetric) {
        const primaryMetric = config.metrics?.[0];
        const groupBy = config.groupby?.[0] || null;
        if (primaryMetric?.column && primaryMetric?.aggregate) {
          // Map dashboard filter format to the serve-chart filter format
          const serveFilters = (extraFilters || [])
            .filter((f: any) => f.column && f.value && f.value !== "AllUp"
                    && (f.filterType || "value") !== "date_range")
            .map((f: any) => ({
              column: f.column,
              operator: f.operator || "=",
              value: String(f.value),
            }));
          try {
            const serveRes = await msalFetch(`${API_BASE}/api/v1/dlm/serve-chart`, {
              method: "POST",
              headers: { "Content-Type": "application/json" },
              body: JSON.stringify({
                dataset_id: selectedDatasetId,
                metric_column: primaryMetric.column,
                aggregation: primaryMetric.aggregate,
                group_by: groupBy,
                filters: serveFilters,
              }),
            });
            if (serveRes.ok) {
              const serveJson = await serveRes.json();
              if (serveJson.served) {
                // Context hit — build the chart option from the served data
                const executeJson = {
                  columns: serveJson.columns || [],
                  rows: serveJson.rows || [],
                  from_context: true,
                };
                const totalDurationMs = performance.now() - start;
                const rows = executeJson.rows;
                let option = buildEchartsOptionFromPreview(selectedTemplate.id, executeJson, config) || {};
                option = applyNumberAbbreviation(option);
                setPreviewOptions(option);

                // Populate client-side cache so repeat views are instant
                const filterSig = serveFilters.map((f: any) => `${f.column}=${f.value}`).sort().join("|");
                const contextCacheKey = `ctx:${selectedDatasetId}:${primaryMetric.column}:${primaryMetric.aggregate}:${groupBy || ""}:${filterSig}`;
                clientCacheSet(contextCacheKey, executeJson);

                setSqlPreview({
                  lastSql: "(served from DLM context)",
                  lastConfigJson: config,
                  dataColumns: executeJson.columns,
                  dataRows: rows,
                  isRunning: false,
                  error: null,
                  durationMs: totalDurationMs,
                  fabricDurationMs: null,
                  rowCount: rows.length,
                  savedSql: sqlPreview.savedSql,
                });
                isQueryRunningRef.current = false;
                releaseSlot();
                return;
              }
            }
          } catch {
            // Context serve failed — fall through to SQL path silently
          }
        }
      }

      let sqlText: string;
      let tablesUsed: string[] = [];

      // Use saved SQL if available and not forcing regeneration
      if (!forceRegenerate && sqlPreview.savedSql) {
        sqlText = sqlPreview.savedSql;
      } else {
        // Generate new SQL
        const generateRes = await msalFetch(`${API_BASE}/api/v1/sql/generate`, {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({
            dataset_id: selectedDatasetId,
            chart_type: selectedTemplate.id,
            config,
          }),
        });

        if (!generateRes.ok) {
          const text = await generateRes.text();
          throw new Error(`Failed to generate SQL: ${generateRes.status} ${text}`);
        }

        const generateJson = await generateRes.json();
        sqlText = generateJson.sql_text;
        tablesUsed = generateJson.tables_used || [];
      }

      // Derive source and optional dashboard ID from runContext.
      // runContext is either:
      //   undefined / "chart-builder"    → chart builder page
      //   "dashboard"                    → dashboard (ID unknown)
      //   "dashboard:123"                → dashboard with ID 123
      const isDashboard = (runContext || "").startsWith("dashboard");
      const executeSource = isDashboard ? "dashboard-chart" : "chart-builder";
      const dashboardIdFromContext = isDashboard
        ? (runContext || "").split(":")[1] || undefined
        : undefined;

      const executeBody = {
        sql_text: sqlText,
        database: datasetDetail?.database_name,
        source: executeSource,
        tables_used: tablesUsed.length > 0 ? JSON.stringify(tablesUsed) : null,
        chart_id:     chartId              ?? undefined,
        chart_type:   selectedTemplate?.id ?? undefined,
        dataset_id:   selectedDatasetId    ?? undefined,
        dashboard_id: dashboardIdFromContext,
        // Cache results for dashboard charts — repeated renders of the same
        // chart (filter changes, refresh) benefit from sub-ms cache hits.
        use_cache:  isDashboard,
        cache_ttl:  300,
      };

      let executeJson: any;

      // Client-side cache: skip the HTTP round-trip entirely on repeat views.
      const cck = isDashboard && datasetDetail?.database_name
        ? clientCacheKey(datasetDetail.database_name, sqlText)
        : null;
      const clientCached = cck ? clientCacheGet(cck) : null;

      if (clientCached) {
        executeJson = clientCached;
      } else {
        const executeRes = await msalFetchRetry(`${API_BASE}/api/v1/sql/execute`, {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify(executeBody),
        });
        if (!executeRes.ok) {
          const text = await executeRes.text();
          throw new Error(`Query execution failed: ${executeRes.status} ${text}`);
        }
        executeJson = await executeRes.json();
        if (cck) clientCacheSet(cck, executeJson);
      }

      const totalDurationMs = performance.now() - start;
      const fabricDurationMs =
        typeof executeJson.duration_ms === "number" && executeJson.duration_ms >= 0
          ? executeJson.duration_ms
          : null;
      const rows = executeJson.rows ?? [];
      const truncatedRows = Array.isArray(rows) ? rows : [];

      // Build a lightweight ECharts option for live preview from the
      // preview result set so users see an immediate chart.
      let option = buildEchartsOptionFromPreview(selectedTemplate.id, executeJson, config) || {};

      // ── Shared high-end theme: crisp typography + faint dashed gridlines,
      //    so cartesian charts stop looking like raw ECharts demos. Applied
      //    before advancedOptions so user overrides still win. ──────────────
      const INTER = "Inter, -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif";
      option.textStyle = { fontFamily: INTER, color: "#334155", ...(option.textStyle || {}) };
      const themeAxis = (ax: any) => {
        if (!ax || typeof ax !== "object") return ax;
        const isValue = ax.type === "value";
        return {
          ...ax,
          axisLine: { show: !isValue, lineStyle: { color: "#e2e8f0" }, ...(ax.axisLine || {}) },
          axisTick: { show: false, ...(ax.axisTick || {}) },
          axisLabel: { color: "#64748b", fontSize: 11, fontFamily: INTER, ...(ax.axisLabel || {}) },
          splitLine: { show: isValue, lineStyle: { color: "#f1f5f9", type: "dashed" }, ...(ax.splitLine || {}) },
        };
      };
      if (option.xAxis) option.xAxis = Array.isArray(option.xAxis) ? option.xAxis.map(themeAxis) : themeAxis(option.xAxis);
      if (option.yAxis) option.yAxis = Array.isArray(option.yAxis) ? option.yAxis.map(themeAxis) : themeAxis(option.yAxis);
      if (option.legend && !Array.isArray(option.legend)) {
        option.legend = { textStyle: { color: "#475569", fontFamily: INTER, fontSize: 12 }, ...option.legend };
      }

      // Merge in advancedOptions for title, font, size, legend, color, axis titles, yAxisFormat, stacking, smooth, marker, and date settings
      if (advancedOptions) {
        // X Axis Date Format is now handled in buildCategoryAxis for live and persisted preview
        // Merge title
        let advTitle = advancedOptions.title;
        let advTitleText = (typeof advTitle === "object" && advTitle !== null && "text" in advTitle)
          ? advTitle.text
          : typeof advTitle === "string"
            ? advTitle
            : "";
        option.title = {
          ...(option.title || {}),
          text: advTitleText,
          left: "center",
          top: 10,
          textStyle: {
            fontSize: Number(advancedOptions.titleSize) || 20,
            fontWeight: "bold",
            fontFamily: advancedOptions.titleFont || "sans-serif",
            ...(advancedOptions.titleColor ? { color: advancedOptions.titleColor } : {}),
            ...(option.title && option.title.textStyle ? option.title.textStyle : {})
          },
        };
        // Merge legend position and order
        if (advancedOptions.legend) {
          const { order, ...legendProps } = advancedOptions.legend;
          option.legend = {
            ...(option.legend || {}),
            ...legendProps,
            // Use order array as legend data if present, otherwise keep existing data
            data: order && Array.isArray(order) && order.length > 0 ? order : option.legend?.data,
          };
        }
        // Merge color palette
        if (advancedOptions.color) {
          option.color = advancedOptions.color;
        }
        // Merge xAxis and yAxis titles, preserving axisLabel formatter and hideOverlap
        if (advancedOptions.xAxis) {
          if (option.xAxis) {
            if (Array.isArray(option.xAxis)) {
              option.xAxis = option.xAxis.map((x: any, i: number) => ({
                ...x,
                ...advancedOptions.xAxis,
                // Preserve the axisLabel formatter for date formatting and ensure hideOverlap
                axisLabel: {
                  ...(advancedOptions.xAxis.axisLabel || {}),
                  ...(x.axisLabel || {}),
                  hideOverlap: true,
                },
              }));
            } else {
              option.xAxis = {
                ...option.xAxis,
                ...advancedOptions.xAxis,
                // Preserve the axisLabel formatter for date formatting and ensure hideOverlap
                axisLabel: {
                  ...(advancedOptions.xAxis.axisLabel || {}),
                  ...(option.xAxis.axisLabel || {}),
                  hideOverlap: true,
                },
              };
            }
          } else {
            option.xAxis = {
              ...advancedOptions.xAxis,
              axisLabel: {
                ...(advancedOptions.xAxis.axisLabel || {}),
                hideOverlap: true,
              },
            };
          }
        }
        if (advancedOptions.yAxis) {
          if (option.yAxis) {
            if (Array.isArray(option.yAxis)) {
              option.yAxis = option.yAxis.map((y: any, i: number) => ({
                ...y,
                ...advancedOptions.yAxis,
                axisLabel: {
                  ...(advancedOptions.yAxis.axisLabel || {}),
                  ...(y.axisLabel || {}),
                  hideOverlap: true,
                },
              }));
            } else {
              option.yAxis = {
                ...option.yAxis,
                ...advancedOptions.yAxis,
                axisLabel: {
                  ...(advancedOptions.yAxis.axisLabel || {}),
                  ...(option.yAxis.axisLabel || {}),
                  hideOverlap: true,
                },
              };
            }
          } else {
            option.yAxis = {
              ...advancedOptions.yAxis,
              axisLabel: {
                ...(advancedOptions.yAxis.axisLabel || {}),
                hideOverlap: true,
              },
            };
          }
        }
        // Merge yAxisFormat and formatter
        if (advancedOptions.yAxisFormat) {
          option.yAxisFormat = advancedOptions.yAxisFormat;
          if (option.yAxis && typeof option.yAxis === "object") {
            const setYFormatter = (yAxisObj: any) => {
              yAxisObj.axisLabel = {
                ...(yAxisObj.axisLabel || {}),
                hideOverlap: true,
                formatter: (val: any) => {
                  if (advancedOptions.yAxisFormat === "k") return `${(val / 1e3).toFixed(0)}K`;
                  if (advancedOptions.yAxisFormat === "m") return `${(val / 1e6).toFixed(0)}M`;
                  if (advancedOptions.yAxisFormat === "b") return `${(val / 1e9).toFixed(0)}B`;
                  if (advancedOptions.yAxisFormat === "t") return `${(val / 1e12).toFixed(0)}T`;
                  return val.toLocaleString();
                },
              };
            };
            if (Array.isArray(option.yAxis)) option.yAxis.forEach(setYFormatter);
            else setYFormatter(option.yAxis);
          }
        }
        // Merge xAxisFormat — for horizontal bar charts where X is the value axis
        if (advancedOptions.xAxisFormat && advancedOptions.xAxisFormat !== "none") {
          option.xAxisFormat = advancedOptions.xAxisFormat;
          if (option.xAxis && typeof option.xAxis === "object") {
            const setXFormatter = (xAxisObj: any) => {
              xAxisObj.axisLabel = {
                ...(xAxisObj.axisLabel || {}),
                hideOverlap: true,
                formatter: (val: any) => {
                  if (advancedOptions.xAxisFormat === "k") return `${(val / 1e3).toFixed(0)}K`;
                  if (advancedOptions.xAxisFormat === "m") return `${(val / 1e6).toFixed(0)}M`;
                  if (advancedOptions.xAxisFormat === "b") return `${(val / 1e9).toFixed(0)}B`;
                  if (advancedOptions.xAxisFormat === "t") return `${(val / 1e12).toFixed(0)}T`;
                  return val.toLocaleString();
                },
              };
            };
            if (Array.isArray(option.xAxis)) option.xAxis.forEach(setXFormatter);
            else setXFormatter(option.xAxis);
          }
        }
        // Merge stacking, smooth, and marker settings for each series
        // Match by name instead of index to handle reordering correctly
        if (Array.isArray(advancedOptions.series) && Array.isArray(option.series)) {
          // Helper function to format Y-axis values
          const formatYAxisValue = (val: number, format: string) => {
            if (format === "k") return `${(val / 1e3).toFixed(0)}K`;
            if (format === "m") return `${(val / 1e6).toFixed(0)}M`;
            if (format === "b") return `${(val / 1e9).toFixed(0)}B`;
            if (format === "t") return `${(val / 1e12).toFixed(0)}T`;
            return val.toLocaleString();
          };

          const currentYAxisFormat = advancedOptions.yAxisFormat || "none";

          option.series = option.series.map((s: any) => {
            const adv = advancedOptions.series.find((advSeries: any) => advSeries.name === s.name) || {};

            // Reconstruct label formatter if labels are shown
            let labelConfig = adv.label !== undefined ? adv.label : s.label;
            if (labelConfig && labelConfig.show) {
              // Check if this is a share chart
              const isShareChart = selectedTemplate?.id?.endsWith("_share") || false;

              labelConfig = {
                ...labelConfig,
                position: labelConfig.position || 'inside',
                formatter: (params: any) => {
                  const val = params.value;
                  // For share charts, always show as percentage
                  if (isShareChart) {
                    return `${val.toFixed(1)}%`;
                  }
                  return formatYAxisValue(val, currentYAxisFormat);
                }
              };
            }

            // Reconstruct symbol and showSymbol if markers are enabled
            let symbolConfig = adv.symbol !== undefined ? adv.symbol : s.symbol;
            let showSymbolConfig: any = false;

            if (symbolConfig && symbolConfig !== "none") {
              // Show exactly 6 points: first, last, and 4 evenly spaced in between
              // Capture the series data length in closure
              const seriesDataLength = s.data?.length || 0;
              symbolConfig = 'circle';
              showSymbolConfig = function (dataIndex: number) {
                const dataLength = seriesDataLength;

                // Always show first and last
                if (dataIndex === 0 || dataIndex === dataLength - 1) {
                  return true;
                }

                // Calculate 4 evenly spaced middle points
                const point1 = Math.round((dataLength - 1) * 1 / 5);
                const point2 = Math.round((dataLength - 1) * 2 / 5);
                const point3 = Math.round((dataLength - 1) * 3 / 5);
                const point4 = Math.round((dataLength - 1) * 4 / 5);

                if (dataIndex === point1 || dataIndex === point2 || dataIndex === point3 || dataIndex === point4) {
                  return true;
                }

                return false;
              };
            }

            return {
              ...s,
              stack: adv.stack !== undefined ? adv.stack : s.stack,
              smooth: adv.smooth !== undefined ? adv.smooth : s.smooth,
              symbol: symbolConfig,
              symbolSize: adv.symbolSize !== undefined ? adv.symbolSize : s.symbolSize,
              showSymbol: showSymbolConfig || (symbolConfig && symbolConfig !== 'none'),
              label: labelConfig,
            };
          });
        }
        // Merge tooltip settings and preserve formatter
        if (advancedOptions.tooltip) {
          const tooltipDateFmt = advancedOptions.tooltip.dateFormat || "auto";
          const tooltipNumFmt = advancedOptions.tooltip.numberFormat || "none";
          const tooltipDecimals = advancedOptions.tooltip.decimalPlaces ?? 2;
          const tooltipUseCommas = advancedOptions.tooltip.useCommas ?? true;

          // Check if this is a share chart
          const isShareChart = selectedTemplate?.id?.endsWith("_share") || false;

          // Apply custom formatter if user has specified custom formatting or wants commas
          if (tooltipDateFmt !== "auto" || tooltipNumFmt !== "none" || tooltipUseCommas || isShareChart) {
            const customTooltipFormatter = (params: any) => {
              if (!Array.isArray(params)) params = [params];

              // For pie/donut charts
              if (params[0]?.componentSubType === "pie" || params[0]?.seriesType === "pie") {
                const value = Number(params[0].value);
                let formattedValue = "";

                if (!Number.isNaN(value)) {
                  formattedValue = formatTooltipNumber(value, tooltipNumFmt, tooltipDecimals, tooltipUseCommas);
                } else {
                  formattedValue = String(params[0].value ?? "");
                }

                return `${params[0].marker} ${params[0].name}: ${formattedValue} (${params[0].percent}%)`;
              }

              // For line/bar/area charts
              const axisValue = params[0]?.axisValue || params[0]?.name;
              let formattedAxisValue = axisValue;

              if (tooltipDateFmt && tooltipDateFmt !== "auto" && axisValue) {
                formattedAxisValue = formatXAxisLabel(String(axisValue), tooltipDateFmt);
              }

              let result = `<strong>${formattedAxisValue}</strong><br/>`;

              params.forEach((param: any) => {
                const value = Number(param.value);
                let formattedValue = "";

                if (!Number.isNaN(value)) {
                  // For share charts, always show as percentage
                  if (isShareChart) {
                    formattedValue = `${value.toFixed(1)}%`;
                  } else {
                    formattedValue = formatTooltipNumber(value, tooltipNumFmt, tooltipDecimals, tooltipUseCommas);
                  }
                } else {
                  formattedValue = String(param.value ?? "");
                }

                result += `${param.marker} ${param.seriesName}: ${formattedValue}<br/>`;
              });

              return result;
            };

            option.tooltip = {
              ...(option.tooltip || {}),
              ...advancedOptions.tooltip,
              formatter: customTooltipFormatter,
            };
          } else {
            option.tooltip = {
              ...(option.tooltip || {}),
              ...advancedOptions.tooltip,
            };
          }
        }
        // Merge labelLayout for native ECharts label overlap handling
        if (advancedOptions.labelLayout) {
          option.labelLayout = advancedOptions.labelLayout;
        }
        // Apply chart-type-specific options (from "Chart Options" section in Customize tab)
        if (advancedOptions.chartTypeOptions) {
          option = applyChartTypeOptions(option, selectedTemplate?.id ?? "", advancedOptions.chartTypeOptions);
        }
        // Merge date settings (timeColumn, timeRange, dateDisplayFormat)
        if (advancedOptions.timeColumn) {
          option.timeColumn = advancedOptions.timeColumn;
        }
        if (advancedOptions.timeRange) {
          option.timeRange = advancedOptions.timeRange;
        }
        if (advancedOptions.dateDisplayFormat) {
          option.dateDisplayFormat = advancedOptions.dateDisplayFormat;
        }
        // Merge xAxisDateFormat for persistence and preview
        if (advancedOptions.xAxisDateFormat) {
          option.xAxisDateFormat = advancedOptions.xAxisDateFormat;
          if (option.xAxis && advancedOptions.xAxisDateFormat !== "auto") {
            const updateXFormatter = (axis: any) => {
              axis.axisLabel = {
                ...(axis.axisLabel || {}),
                formatter: (val: string) => formatXAxisLabel(String(val), advancedOptions.xAxisDateFormat),
              };
            };
            if (Array.isArray(option.xAxis)) option.xAxis.forEach(updateXFormatter);
            else updateXFormatter(option.xAxis);
          }
        }
        // Merge yAxisDateFormat — for horizontal bars where Y is the category axis
        if ((advancedOptions as any).yAxisDateFormat && (advancedOptions as any).yAxisDateFormat !== "auto") {
          option.yAxisDateFormat = (advancedOptions as any).yAxisDateFormat;
          if (option.yAxis) {
            const updateYDateFormatter = (axis: any) => {
              if (axis.type === "category") {
                axis.axisLabel = {
                  ...(axis.axisLabel || {}),
                  formatter: (val: string) => formatXAxisLabel(String(val), (advancedOptions as any).yAxisDateFormat),
                };
              }
            };
            if (Array.isArray(option.yAxis)) option.yAxis.forEach(updateYDateFormatter);
            else updateYDateFormatter(option.yAxis);
          }
        }
        // Update advancedOptions.series to match the current series order and names
        // This ensures that when users change settings (stacking, smooth, markers),
        // the properties stay with the correct series even after reordering
        if (Array.isArray(option.series)) {
          setAdvancedOptions((prev: any) => ({
            ...(prev || {}),
            series: option.series.map((s: any) => ({
              name: s.name,
              // Only save stack for non-share charts (share charts use showAsPercentage instead)
              stack: (selectedTemplate?.id?.endsWith('_share') ? undefined : s.stack),
              smooth: s.smooth,
              symbol: s.symbol,
            })),
          }));
        }
      }
      // Abbreviate large numbers on value axes + tooltips by default (K/M/B/T).
      option = applyNumberAbbreviation(option);
      setPreviewOptions(option);

      setSqlPreview({
        lastSql: sqlText,
        lastConfigJson: config,
        dataColumns: executeJson.columns ?? [],
        // Only show the first N rows in the Data tab, but keep
        // rowCount based on the full result set.
        dataRows: truncatedRows,
        isRunning: false,
        error: null,
        // Total end-to-end time from the chart builder's
        // perspective, including network and SQL generation.
        durationMs: totalDurationMs,
        fabricDurationMs,
        rowCount: Array.isArray(rows) ? rows.length : 0,
        savedSql: sqlPreview.savedSql,
      });
      isQueryRunningRef.current = false;
      releaseSlot();
      // Query history is written server-side by /api/v1/sql/execute with full
      // context (source, chart_id, dataset_id, tables_used). No secondary
      // record-query call needed.
    } catch (e: unknown) {
      const message = e instanceof Error ? e.message : "Unknown error";
      isQueryRunningRef.current = false;
      releaseSlot();
      setSqlPreview((prev) => ({ ...prev, isRunning: false, error: message }));
    }
  };

  const handleSave = async () => {
    if (!canSave || !selectedTemplate || !selectedDatasetId) return;

    setIsSaving(true);
    setSaveError(null);

    try {
      const config = buildQueryConfigForPreview();

      // Prepare advanced options for serialization
      // Convert function-based properties to serializable values
      const serializableOptions = {
        ...advancedOptions,
        legend: {
          ...(advancedOptions.legend || {}),
          order: (advancedOptions.legend && advancedOptions.legend.order) || undefined,
        },
        // Convert symbol functions to simple string for serialization
        series: (advancedOptions.series || []).map((s: any) => ({
          ...s,
          symbol: typeof s.symbol === 'function' ? 'circle' : s.symbol,
          labelLayout: undefined, // Remove per-series labelLayout as it's at top level
        })),
        // Preserve labelLayout for serialization
        labelLayout: advancedOptions.labelLayout,
      };

      const payload = {
        name: name.trim(),
        description: description.trim() || null,
        chart_type: selectedTemplate.id,
        dataset_id: selectedDatasetId,
        query_config: config,
        viz_config: {
          echarts_option: serializableOptions,
        },
        sql_text: sqlPreview.lastSql || null,
      };

      // If the chart name has changed from the original, always create a new chart (POST)
      let isUpdate = Boolean(chartId);
      if (isUpdate && initialSnapshotRef.current && name.trim() !== initialSnapshotRef.current.name) {
        isUpdate = false;
      }
      const url = isUpdate
        ? `${API_BASE}/api/v1/charts/${chartId}`
        : `${API_BASE}/api/v1/charts`;
      const method = isUpdate ? "PATCH" : "POST";

      const res = await msalFetch(url, {
        method,
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(payload),
      });

      if (!res.ok) {
        throw new Error(
          `Failed to ${isUpdate ? "update" : "create"} chart: ${res.status}`,
        );
      }

      const saved = await res.json();
      const targetId = saved.id ?? chartId;

      // Update the initial snapshot to reflect the saved state
      if (saved) {
        initialSnapshotRef.current = {
          name: name.trim(),
          description: description.trim(),
          config: config,
        };
        setHasChanges(false);

        // Update savedSql to the current SQL so subsequent loads use this
        if (sqlPreview.lastSql) {
          setSqlPreview((prev) => ({
            ...prev,
            savedSql: sqlPreview.lastSql,
          }));
        }
      }

      // If this was a new chart (no chartId before), update the chartId
      if (!chartId && targetId) {
        setChartId(targetId);
        // Update the URL without navigation to reflect the chart ID
        window.history.replaceState(null, "", `/charts/${targetId}/edit`);
      }

      // Best-effort: snapshot the rendered chart and persist it as a thumbnail,
      // so the workspace shows a real preview instead of a generic glyph.
      if (targetId) {
        try {
          const thumb = await thumbnailCaptureRef.current?.();
          if (thumb) {
            void msalFetch(`${API_BASE}/api/v1/charts/${targetId}`, {
              method: "PATCH",
              headers: { "Content-Type": "application/json" },
              body: JSON.stringify({ thumbnail: thumb }),
            }).catch(() => {});
          }
        } catch { /* thumbnail is best-effort */ }
      }

      // Don't redirect - keep the user on the edit page with their chart preview intact
    } catch (e: unknown) {
      const message = e instanceof Error ? e.message : "Unknown error";
      setSaveError(message);
    } finally {
      setIsSaving(false);
    }
  };

  // Save As — always create a NEW chart from the current config, capture a
  // thumbnail for it, and return its id so the caller can open the copy.
  const saveChartAs = async (newName: string): Promise<number | null> => {
    if (!selectedTemplate || !selectedDatasetId) return null;
    setIsSaving(true);
    setSaveError(null);
    try {
      const config = buildQueryConfigForPreview();
      const serializableOptions = {
        ...advancedOptions,
        legend: { ...(advancedOptions.legend || {}), order: (advancedOptions.legend && advancedOptions.legend.order) || undefined },
        series: (advancedOptions.series || []).map((s: any) => ({ ...s, symbol: typeof s.symbol === 'function' ? 'circle' : s.symbol, labelLayout: undefined })),
        labelLayout: advancedOptions.labelLayout,
      };
      const payload = {
        name: newName.trim(),
        description: description.trim() || null,
        chart_type: selectedTemplate.id,
        dataset_id: selectedDatasetId,
        query_config: config,
        viz_config: { echarts_option: serializableOptions },
        sql_text: sqlPreview.lastSql || null,
      };
      const res = await msalFetch(`${API_BASE}/api/v1/charts`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(payload),
      });
      if (!res.ok) throw new Error(`Failed to create copy: ${res.status}`);
      const saved = await res.json();
      const newId = saved?.id ?? null;
      if (newId) {
        try {
          const thumb = await thumbnailCaptureRef.current?.();
          if (thumb) {
            void msalFetch(`${API_BASE}/api/v1/charts/${newId}`, {
              method: "PATCH",
              headers: { "Content-Type": "application/json" },
              body: JSON.stringify({ thumbnail: thumb }),
            }).catch(() => {});
          }
        } catch { /* thumbnail is best-effort */ }
      }
      return newId;
    } catch (e: unknown) {
      setSaveError(e instanceof Error ? e.message : "Failed to save a copy");
      return null;
    } finally {
      setIsSaving(false);
    }
  };

  const value: ChartBuilderContextValue = {
    chartId,
    setChartId,
    datasets,
    datasetsError,
    selectedDatasetId,
    setSelectedDatasetId,
    name,
    setName,
    description,
    setDescription,
    chartType,
    setChartType,
    templates: allTemplates,
    categories,
    selectedTemplate,
    previewOptions,
    setPreviewOptions,
    advancedOptions,
    setAdvancedOptions,
    datasetColumns,
    datasetMetrics,
    datasetDetailError,
    datasetDetail,
    metricColumn,
    setMetricColumn,
    metrics,
    setMetrics,
    timeGrain,
    setTimeGrain,
    sortBy,
    setSortBy,
    queryMode,
    setQueryMode,
    groupByColumns,
    setGroupByColumns,
    setScatterAxes,
    setCategoryLabels,
    timeColumn,
    setTimeColumn,
    timeRange,
    setTimeRange,
    customStartDate,
    setCustomStartDate,
    customEndDate,
    setCustomEndDate,
    dateDisplayFormat,
    setDateDisplayFormat,
    rowLimit,
    setRowLimit,
    filterLogic,
    setFilterLogic,
    filters,
    setFilters,
    sqlPreview,
    runPreviewQuery,
    cancelRunningQuery: () => { cancelQueryRef.current = true; },
    runContext,
    isSaving,
    canSave,
    saveError,
    handleSave,
    saveChartAs,
    registerInitialSnapshot,
    registerThumbnailCapture: useCallback((fn: (() => string | null | Promise<string | null>) | null) => { thumbnailCaptureRef.current = fn; }, []),
  };

  return <ChartBuilderContext.Provider value={value}>{children}</ChartBuilderContext.Provider>;
};

export const useChartBuilder = (): ChartBuilderContextValue => {
  const ctx = useContext(ChartBuilderContext);
  if (!ctx) {
    throw new Error("useChartBuilder must be used within a ChartBuilderProvider");
  }
  return ctx;
};
