/**
 * ChartHydrator
 *
 * Single shared component that populates ChartBuilderContext from a pre-fetched
 * chart configuration. Used identically by:
 *   - The chart detail page  (/charts/[id])
 *   - Dashboard chart components (DashboardChartComponent)
 *
 * This guarantees that a chart always renders exactly the same way regardless
 * of where it appears. Any fix to hydration logic automatically applies
 * everywhere — there is no second code path to diverge.
 *
 * This component renders null. It only sets up context state via effects.
 * The parent is responsible for rendering the chart UI (ChartPreview,
 * CreateChartLayout, etc.) as a sibling inside the same ChartBuilderProvider.
 *
 * Props:
 *   chart          — pre-fetched chart object (query_config, viz_config, etc.)
 *   externalFilters — optional filters from the dashboard to merge on top of
 *                     the chart's own saved filters (dashboard use only)
 */

"use client";

import { useEffect, useRef } from "react";
import {
  useChartBuilder,
  ChartKind,
  DateDisplayFormat,
  TimeRangePreset,
  mergeAdvancedOptions,
  DEFAULT_ADVANCED_OPTIONS,
} from "./ChartBuilderContext";

interface ChartHydratorProps {
  chart: any;
  externalFilters?: any[];
}

const ChartHydrator: React.FC<ChartHydratorProps> = ({ chart, externalFilters = [] }) => {
  const {
    setChartId,
    setSelectedDatasetId,
    setName,
    setDescription,
    setChartType,
    setMetricColumn,
    setGroupByColumns,
    setTimeColumn,
    setFilters,
    setFilterLogic,
    setTimeRange,
    setDateDisplayFormat,
    setCustomStartDate,
    setCustomEndDate,
    setRowLimit,
    setAdvancedOptions,
    datasetColumns,
    runPreviewQuery,
    sqlPreview,
    selectedDatasetId,
    chartType,
    metricColumn,
    timeColumn,
    advancedOptions,
    registerInitialSnapshot,
  } = useChartBuilder();

  const hasHydratedRef = useRef(false);
  const hasAutoRunRef = useRef(false);

  // Keep latest external filters in a ref — avoids re-triggering hydration on
  // every filter change while still applying the current values at run-time.
  const externalFiltersRef = useRef(externalFilters);
  useEffect(() => {
    externalFiltersRef.current = externalFilters;
  }, [externalFilters]);

  // Re-hydrate when datasetColumns become available (they load asynchronously
  // after the dataset ID is set, so the first render may have an empty list).
  useEffect(() => {
    if (datasetColumns && datasetColumns.length > 0) {
      hasHydratedRef.current = false;
    }
  }, [datasetColumns]);

  // ── Main hydration effect ───────────────────────────────────────────────────
  useEffect(() => {
    if (!chart) return;
    if (hasHydratedRef.current) return;

    const qc = chart.query_config ?? {};

    // Basic metadata — set immediately, no column mapping needed.
    setChartId(chart.id ?? null);
    setSelectedDatasetId(chart.dataset_id ?? null);
    setName(chart.name ?? "");
    setDescription(chart.description ?? "");
    if (chart.chart_type) setChartType(chart.chart_type as ChartKind);

    // Viz config — prefer echarts_option if nested, fall back to viz_config
    // directly for charts saved before the nested format was introduced.
    if (chart.viz_config) {
      const echartsOption = chart.viz_config.echarts_option || chart.viz_config;
      setAdvancedOptions(mergeAdvancedOptions(echartsOption));
    } else {
      setAdvancedOptions(DEFAULT_ADVANCED_OPTIONS);
    }

    // Column-mapped fields require datasetColumns to be available.
    if (!datasetColumns || datasetColumns.length === 0) return;

    // Build a lookup map: normalised column identifier → canonical "table.column" key.
    const colKeyMap = new Map<string, string>();
    for (const col of datasetColumns) {
      const full = `${col.table_name}.${col.column_name}`;
      const normalize = (raw: string) =>
        raw.replace(/\[|\]/g, "").replace(/\|/g, ".").trim().toLowerCase();
      const fullNorm = normalize(full);
      const shortNorm = normalize(col.column_name);
      if (fullNorm) colKeyMap.set(fullNorm, full);
      if (shortNorm && !colKeyMap.has(shortNorm)) colKeyMap.set(shortNorm, full);
    }

    const normalizeLookup = (raw: string | null | undefined): string | null => {
      if (!raw) return null;
      const norm = raw.replace(/\[|\]/g, "").replace(/\|/g, ".").trim().toLowerCase();
      return colKeyMap.get(norm) ?? raw;
    };

    // Metric column
    let metricCol: string | null = null;
    if (qc.metric && typeof qc.metric.column === "string") {
      metricCol = qc.metric.column;
    } else if (Array.isArray(qc.metrics) && qc.metrics.length > 0) {
      const m0 = qc.metrics[0];
      if (m0 && (typeof m0.column === "string" || typeof m0.col === "string")) {
        metricCol = (m0.column || m0.col) as string;
      }
    }
    metricCol = normalizeLookup(metricCol);
    if (metricCol) setMetricColumn(metricCol);

    // Group-by columns
    if (Array.isArray(qc.groupby)) {
      const mapped = qc.groupby
        .filter((g: unknown) => typeof g === "string")
        .map((g: string) => normalizeLookup(g))
        .filter((g: string | null): g is string => Boolean(g));
      setGroupByColumns(mapped);
    }

    // Time column
    if (typeof qc.time_column === "string") {
      setTimeColumn(normalizeLookup(qc.time_column) ?? qc.time_column);
    }

    // Time range and date display format
    if (typeof qc.time_range === "string") setTimeRange(qc.time_range as TimeRangePreset);
    if (typeof qc.date_display_format === "string")
      setDateDisplayFormat(qc.date_display_format as DateDisplayFormat);

    // Row limit (use saved value so chart renders identically everywhere)
    if (typeof qc.row_limit === "number") setRowLimit(qc.row_limit);

    // Custom date range
    if (qc.custom_start_date) setCustomStartDate(qc.custom_start_date);
    if (qc.custom_end_date) setCustomEndDate(qc.custom_end_date);

    // Filters — chart's own filters first, then external (dashboard-level) filters.
    // External filters always take precedence when the same column is referenced.
    const chartFilters = Array.isArray(qc.filters) ? qc.filters : [];
    const mergedFilters = [
      ...chartFilters.map((f: any, idx: number) => {
        const rawCol = (f.column || f.col) as string;
        const col = normalizeLookup(rawCol) ?? rawCol;
        const rawVal = f.value !== undefined ? f.value : f.val;
        const valueStr = Array.isArray(rawVal)
          ? rawVal.join(", ")
          : rawVal != null ? String(rawVal) : "";
        const rawValueKey = f.valueKey !== undefined ? f.valueKey : f.key;
        return {
          id: f.id ?? `cf-${idx}`,
          column: col,
          columnLabel: f.columnLabel,
          keyColumn: f.keyColumn ?? null,
          operator: f.op ?? "=",
          value: valueStr,
          valueKey: rawValueKey != null ? String(rawValueKey) : "",
          options: [],
          isLoading: false,
          isPending: false,
        };
      }),
      ...externalFiltersRef.current.map((f: any, idx: number) => ({
        id: `ext-${idx}`,
        column: f.column,
        operator: f.operator || "=",
        value: Array.isArray(f.value) ? f.value.join(", ") : String(f.value ?? ""),
        valueKey: f.valueKey ?? "",
        keyColumn: f.keyColumn ?? null,
        options: [],
        isLoading: false,
        isPending: false,
      })),
    ];
    setFilters(mergedFilters);

    if (typeof qc.filter_logic === "string") {
      const logic = qc.filter_logic.toUpperCase();
      if (logic === "AND" || logic === "OR") setFilterLogic(logic);
    }

    registerInitialSnapshot();
    hasHydratedRef.current = true;
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [chart, datasetColumns]);

  // ── Auto-run effect ─────────────────────────────────────────────────────────
  // Identical guards to the chart page — only fires once all required fields
  // are set so the generated SQL is always complete and valid.
  useEffect(() => {
    if (hasAutoRunRef.current) return;
    if (!hasHydratedRef.current) return;
    if (!chart?.id) return;
    if (sqlPreview.lastSql) return;           // Already has a result — skip
    if (!selectedDatasetId || !chartType) return;
    if (!metricColumn) return;                // Chart must have a metric configured

    const isTimeSeries = [
      "time_series_line",
      "time_series_line_share",
      "time_series_area",
      "time_series_area_share",
    ].includes(chartType);
    if (isTimeSeries && !timeColumn) return;  // Time series needs a time column

    if (!advancedOptions) return;             // Wait for viz config to be applied

    hasAutoRunRef.current = true;
    void runPreviewQuery();
  }, [
    chart?.id,
    chartType,
    selectedDatasetId,
    metricColumn,
    timeColumn,
    sqlPreview.lastSql,
    advancedOptions,
    runPreviewQuery,
  ]);

  return null;
};

export default ChartHydrator;
