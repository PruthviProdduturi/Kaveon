
import React, { createContext, useContext, useEffect, useMemo, useRef, useState } from "react";

import { API_BASE } from "../../config";
import { msalFetch } from "../../utils/msalFetch";
import { useAuth } from "../../auth/useAuth";
import { useRouter } from "next/navigation";

// Default advanced chart options for merging with loaded config
export const DEFAULT_ADVANCED_OPTIONS = {
  title: "",
  titleFont: "sans-serif",
  titleSize: "20",
  xAxis: { show: true, name: "" },
  yAxis: { show: true, name: "" },
  color: ["#5470C6", "#91CC75", "#EE6666", "#FAC858", "#73C0DE", "#3BA272", "#FC8452", "#9A60B4", "#EA7CCC"],
  legend: { show: true, left: "top", order: undefined as string[] | undefined },
  tooltip: { show: true, dateFormat: "auto", numberFormat: "none", decimalPlaces: 2, useCommas: true },
  yAxisFormat: "none",
  xAxisDateFormat: "auto", // Added to support advancedOptions?.xAxisDateFormat
  labelLayout: undefined as { hideOverlap?: boolean; moveOverlap?: string } | undefined,
  series: [],
  seriesSettings: undefined as { smooth?: boolean; symbol?: string | Function; symbolSize?: number; stack?: string } | undefined,
  showAsPercentage: undefined as boolean | undefined,
};

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
  let timeIndex = columns.findIndex((c) => /date|time/i.test(c));
  if (timeIndex === -1 && columns.length >= 2) {
    timeIndex = 1;
  }

  const dimensionIndexes: number[] = [];
  columns.forEach((_, idx) => {
    if (idx !== metricIndex && idx !== timeIndex) {
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

    // For share charts, calculate totals for percentage conversion
    const xTotals: Record<string, number> = {};
    if (isShareChart) {
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
              if (isShareChart) return `${v.toFixed(1)}%`;
              return v.toLocaleString();
            },
            position: "top",
            fontSize: 10,
            overflow: "truncate",
          }
        : isShareChart
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
      labelLayout: markersEnabled || isShareChart ? { hideOverlap: true } : undefined,
      data: xValues.map((x, dataIndex) => {
        const raw = dataMap.get(name)?.get(x) ?? 0;
        const value = isShareChart
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

  const buildCategoryAxis = () => {
    const axis: any = { type: "category", data: xValues };
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
        xAxis: buildCategoryAxis(),
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
    case "stacked_bar_vertical":
      return {
        ...common,
        xAxis: buildCategoryAxis(),
        yAxis: { type: "value" },
        series: buildLineOrBarSeries({
          type: "bar",
          stacked: chartKind === "stacked_bar_vertical",
        }),
      };
    case "bar_horizontal":
    case "stacked_bar_horizontal":
      return {
        ...common,
        xAxis: { type: "value" },
        yAxis: { type: "category", data: xValues },
        series: buildLineOrBarSeries({
          type: "bar",
          stacked: chartKind === "stacked_bar_horizontal",
        }),
      };
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
      const xIndex = 0;
      const yIndex = 1;
      const scatterData = rows.map((r) => [r[xIndex], r[yIndex]]);
      return {
        ...common,
        xAxis: { type: "value", name: columns[xIndex] },
        yAxis: { type: "value", name: columns[yIndex] },
        series: [
          {
            type: "scatter",
            data: scatterData,
            symbolSize: 10,
          },
        ],
      };
    }
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
  | "donut";

export type TimeRangePreset =
  | "all_time"
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
  { id: "Line", label: "Line", iconClass: "fas fa-chart-line" },
  { id: "Bar", label: "Bar", iconClass: "fas fa-chart-bar" },
  { id: "Pie", label: "Pie", iconClass: "fas fa-chart-pie" },
  { id: "Heatmap", label: "Heatmap", iconClass: "fas fa-th-large" },
  { id: "Treemap", label: "Treemap", iconClass: "fas fa-th" },
  { id: "Sunburst", label: "Sunburst", iconClass: "fas fa-sun" },
  { id: "Funnel", label: "Funnel", iconClass: "fas fa-filter" },
  { id: "Custom", label: "Custom", iconClass: "fas fa-shapes" },
  { id: "Dataset", label: "Dataset", iconClass: "fas fa-table" },
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
    id: "bar_vertical",
    name: "Vertical bar",
    description: "Compare categories with a vertical bar chart.",
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
    id: "stacked_bar_vertical",
    name: "Stacked bar (vertical)",
    description: "Compare parts of a whole across categories.",
    category: "Bar",
    previewKind: "stacked-bar",
  },
  {
    id: "stacked_bar_horizontal",
    name: "Stacked bar (horizontal)",
    description: "Stacked bars with long category labels.",
    category: "Bar",
    previewKind: "stacked-bar",
  },
  {
    id: "grouped_bar",
    name: "Grouped bar",
    description: "Compare multiple series side by side.",
    category: "Bar",
    previewKind: "grouped-bar",
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
  groupByColumns: string[];
  setGroupByColumns: (value: string[]) => void;
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
  runPreviewQuery: (forceRegenerate?: boolean) => Promise<void>;
  isSaving: boolean;
  canSave: boolean;
  saveError: string | null;
  handleSave: () => void;
  registerInitialSnapshot: () => void;
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
  const [groupByColumns, setGroupByColumns] = useState<string[]>([]);
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
    if (initialTemplate && TEMPLATES.some((t) => t.id === initialTemplate)) {
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

  const selectedTemplate = useMemo(
    () => (chartType ? TEMPLATES.find((t) => t.id === chartType) ?? null : null),
    [chartType],
  );

  const categories = useMemo(
    () => CHART_CATEGORIES.filter((cat) => TEMPLATES.some((t) => t.category === cat.id)),
    [],
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

  const buildQueryConfigForPreview = () => {
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

          console.log('[ChartBuilder] Building filter payload:', {
            column: payload.column,
            keyColumn: payload.keyColumn,
            value: payload.value,
            valueKey: payload.valueKey
          });

          return payload;
        });

    const filtersPayload = buildFiltersPayload(primaryFilters);
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

    return {
      dataset_id: selectedDatasetId,
      template: selectedTemplate.id,
      datasource,
      metric: metricColumn
        ? {
            column: metricColumn,
            agg: "SUM",
          }
        : null,
      groupby: groupByColumns,
      time_column: timeColumn,
      filters: filtersPayload,
      filter_logic: filterLogic,
      filter_groups: filterGroups,
      time_range: timeRange,
      custom_start_date: customStartDate,
      custom_end_date: customEndDate,
      date_display_format: dateDisplayFormat,
      row_limit: rowLimit,
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
    // Prefer advancedOptions.xAxisDateFormat if set, then previewOptions, then config
    const xAxisDateFormat =
      (advancedOptions && advancedOptions.xAxisDateFormat) ||
      (previewOptions && previewOptions.xAxisDateFormat) ||
      (config?.date_display_format as DateDisplayFormat | undefined) ||
      "auto";

    const displayFormat: DateDisplayFormat = xAxisDateFormat;

    // Heuristics: last column is metric, a column containing "date"/"time" is x-axis,
    // remaining dimension columns define series keys.
    const metricIndex = columns.length - 1;
    let timeIndex = columns.findIndex((c) => /date|time/i.test(c));
    if (timeIndex === -1 && columns.length >= 2) {
      timeIndex = 1;
    }

    const dimensionIndexes: number[] = [];
    columns.forEach((_, idx) => {
      if (idx !== metricIndex && idx !== timeIndex) {
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
          : dimensionIndexes.map((idx) => String(row[idx] ?? "")).join(" · ");

      seriesNamesSet.add(seriesName);
      if (!dataMap.has(seriesName)) dataMap.set(seriesName, new Map());
      dataMap.get(seriesName)!.set(x, metric);
    });

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
      const xTotals: Record<string, number> = {};
      if (isShareChart) {
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

      return seriesNames.map((name) => ({
        name,
        type: opts.type,
        stack: (isShareChart ? (stackingOverride ? "total" : undefined) : (opts.stacked ? "total" : undefined)),
        smooth: (isShareChart ? !!smoothOverride : (opts.smooth ?? (opts.type === "line"))),
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
        // Area charts (filled line): all *_area variants and area_stack share this feature
        areaStyle: isAreaChart ? {} : undefined,
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
                // For share charts, show percentage
                if (isShareChart) {
                  return `${v.toFixed(1)}%`;
                }
                return v.toLocaleString();
              },
              position: "top",
              fontSize: 10,
              overflow: "truncate",
            }
          : isShareChart
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
        labelLayout: markersEnabled || isShareChart ? { hideOverlap: true } : undefined,
        data: xValues.map((x, dataIndex) => {
          const raw = dataMap.get(name)?.get(x) ?? 0;
          const value = isShareChart
            ? (xTotals[x] > 0 ? (raw / xTotals[x]) * 100 : 0)
            : raw;
          if (dataIndex === 0) return { value, label: { align: 'left' } };
          if (dataIndex === xValues.length - 1) return { value, label: { align: 'right' } };
          return value;
        }),
      }));
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
    const buildCategoryAxis = () => {
      // Use the actual UI format string for the formatter
      const axis: any = { type: "category", data: xValues };
      if (xAxisDateFormat && xAxisDateFormat !== "auto") {
        axis.axisLabel = {
          formatter: (val: string) => formatXAxisLabel(String(val), xAxisDateFormat),
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
          xAxis: buildCategoryAxis(),
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
      case "stacked_bar_vertical":
        return {
          ...common,
          xAxis: buildCategoryAxis(),
          yAxis: { type: "value" },
          series: buildLineOrBarSeries({
            type: "bar",
            stacked: chartKind === "stacked_bar_vertical",
          }),
        };
      case "bar_horizontal":
      case "stacked_bar_horizontal":
        return {
          ...common,
          xAxis: { type: "value" },
          yAxis: { type: "category", data: xValues },
          series: buildLineOrBarSeries({
            type: "bar",
            stacked: chartKind === "stacked_bar_horizontal",
          }),
        };
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

        return {
          tooltip: {
            trigger: "item",
            formatter: tooltipNumberFormat !== "none" || tooltipUseCommas ? pieTooltipFormatter : undefined,
          },
          legend: { orient: "vertical", left: "left" },
          grid: {
            left: 10,
            right: 10,
            top: 10,
            bottom: 10,
            containLabel: true,
          },
          series: [
            {
              name: "Share",
              type: "pie",
              radius: chartKind === "donut" ? ["50%", "75%"] : "70%",
              center: ["55%", "50%"],
              avoidLabelOverlap: false,
              itemStyle: { borderRadius: 6, borderColor: "#ffffff", borderWidth: 2 },
              label: { show: false, position: "center" },
              emphasis: {
                label: { show: true, fontSize: 14, fontWeight: "bold" },
              },
              labelLine: { show: false },
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
      default:
        return null;
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

  const runPreviewQuery = async (forceRegenerate: boolean = false) => {
    if (!selectedDatasetId || !selectedTemplate) {
      return;
    }

    // Prevent concurrent runs — if a query is already in flight, skip
    if (isQueryRunningRef.current) {
      console.log('[ChartBuilder] Skipping runPreviewQuery — already running');
      return;
    }

    const config = buildQueryConfigForPreview();
    if (!config) return;

    isQueryRunningRef.current = true;
    setSqlPreview((prev) => ({ ...prev, isRunning: true, error: null }));

    const start = performance.now();

    try {
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

      const executeRes = await msalFetch(`${API_BASE}/api/v1/sql/execute`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        // Let the backend use its default cap (50k rows) so
        // charts can aggregate over the full result set.
        body: JSON.stringify({
          sql_text: sqlText,
          database: datasetDetail?.database_name,
          source: executeSource,
          tables_used: tablesUsed.length > 0 ? JSON.stringify(tablesUsed) : null,
          // Provide full context for rich query history entries.
          chart_id:     chartId              ?? undefined,
          chart_type:   selectedTemplate?.id ?? undefined,
          dataset_id:   selectedDatasetId    ?? undefined,
          dashboard_id: dashboardIdFromContext,
        }),
      });

      if (!executeRes.ok) {
        const text = await executeRes.text();
        throw new Error(`Failed to execute SQL: ${executeRes.status} ${text}`);
      }

      const executeJson = await executeRes.json();

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
            // If yAxis is an array, update all; if object, update directly
            const setFormatter = (yAxisObj: any) => {
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
            if (Array.isArray(option.yAxis)) {
              option.yAxis.forEach(setFormatter);
            } else {
              setFormatter(option.yAxis);
            }
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
          // Update the xAxis formatter to use the new date format
          if (option.xAxis && advancedOptions.xAxisDateFormat !== "auto") {
            const updateAxisFormatter = (axis: any) => {
              axis.axisLabel = {
                ...(axis.axisLabel || {}),
                formatter: (val: string) => formatXAxisLabel(String(val), advancedOptions.xAxisDateFormat),
              };
            };
            if (Array.isArray(option.xAxis)) {
              option.xAxis.forEach(updateAxisFormatter);
            } else {
              updateAxisFormatter(option.xAxis);
            }
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
      // Query history is written server-side by /api/v1/sql/execute with full
      // context (source, chart_id, dataset_id, tables_used). No secondary
      // record-query call needed.
    } catch (e: unknown) {
      const message = e instanceof Error ? e.message : "Unknown error";
      isQueryRunningRef.current = false;
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

      console.log('[ChartBuilder] Saving chart with payload:', {
        name: payload.name,
        has_sql_text: Boolean(payload.sql_text),
        sql_text_length: payload.sql_text?.length || 0,
        filters: config?.filters,
        query_config_keys: Object.keys(config || {}),
      });

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

      // Don't redirect - keep the user on the edit page with their chart preview intact
    } catch (e: unknown) {
      const message = e instanceof Error ? e.message : "Unknown error";
      setSaveError(message);
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
    templates: TEMPLATES,
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
    groupByColumns,
    setGroupByColumns,
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
    isSaving,
    canSave,
    saveError,
    handleSave,
    registerInitialSnapshot,
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
