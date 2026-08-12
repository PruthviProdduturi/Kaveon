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
  MetricConfig,
  mergeAdvancedOptions,
  DEFAULT_ADVANCED_OPTIONS,
} from "./ChartBuilderContext";

interface ChartHydratorProps {
  chart: any;
  externalFilters?: any[];
  /** Cross-filter extras: applied at query time only, NOT merged into context filter state.
   *  Change in this prop triggers a re-run with these as extra filters. */
  crossFilterExtra?: any[];
  /** Dashboard colour theme — when set, overrides each chart's own colour palette. */
  paletteOverride?: string[] | null;
}

const ChartHydrator: React.FC<ChartHydratorProps> = ({ chart, externalFilters = [], crossFilterExtra = [], paletteOverride = null }) => {
  const {
    setChartId,
    setSelectedDatasetId,
    setName,
    setDescription,
    setChartType,
    setMetricColumn,
    setMetrics,
    setTimeGrain,
    setSortBy,
    setQueryMode,
    groupByColumns,
    setGroupByColumns,
    setScatterAxes,
    setCategoryLabels,
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
  // Tracks whether dataset ID + other metadata has been written for the current
  // chart object. Only resets when the chart itself changes — NOT when
  // datasetColumns reloads. This prevents the column-mapping re-run from
  // overwriting a dataset the user deliberately switched to.
  const hasSetMetadataRef = useRef(false);

  // Keep latest external filters in a ref — avoids re-triggering hydration on
  // every filter change while still applying the current values at run-time.
  const externalFiltersRef = useRef(externalFilters);
  useEffect(() => { externalFiltersRef.current = externalFilters; }, [externalFilters]);
  const crossFilterExtraRef = useRef(crossFilterExtra);
  useEffect(() => { crossFilterExtraRef.current = crossFilterExtra; }, [crossFilterExtra]);

  // Both dashboard (external) filters AND cross-filters are applied as run-time
  // extras — so both are reactive (changing either re-runs the query).
  const buildExtras = (): any[] => [
    ...(externalFiltersRef.current || []).map((f: any) => ({
      column: f.column,
      operator: f.operator || "=",
      value: Array.isArray(f.value) ? f.value.join(", ") : String(f.value ?? ""),
      valueKey: f.valueKey ?? "",
      keyColumn: f.keyColumn ?? null,
    })),
    ...(crossFilterExtraRef.current || []),
  ];

  // Re-run the query whenever dashboard filters OR cross-filters change (after
  // the initial auto-run). Previously only cross-filters re-ran; dashboard filters
  // were baked once at hydration, so applying a filter did nothing.
  const prevExtrasSerialRef = useRef<string>(JSON.stringify([externalFilters ?? [], crossFilterExtra ?? []]));
  useEffect(() => {
    if (!hasAutoRunRef.current) return;
    const serial = JSON.stringify([externalFilters, crossFilterExtra]);
    if (serial === prevExtrasSerialRef.current) return;
    prevExtrasSerialRef.current = serial;
    void runPreviewQuery(true, buildExtras());
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [externalFilters, crossFilterExtra]);

  // Reset metadata flag when a different chart is loaded.
  useEffect(() => {
    hasSetMetadataRef.current = false;
  }, [chart?.id]);

  // Re-hydrate column-mapped fields when datasetColumns become available
  // (they load asynchronously after the dataset ID is set).
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

    // Basic metadata — only set once per chart load so that a user switching
    // datasets doesn't get snapped back when datasetColumns reload triggers
    // a second pass through this effect.
    if (!hasSetMetadataRef.current) {
      setChartId(chart.id ?? null);
      setSelectedDatasetId(chart.dataset_id ?? null);
      setName(chart.name ?? "");
      setDescription(chart.description ?? "");
      // Map generic chart_type names (e.g. from programmatically-created charts)
      // onto concrete template ids the builder actually renders. Without this a
      // chart_type of "bar"/"line" matches no template → no query → the chart
      // shows "Configure your chart to see preview".
      if (chart.chart_type) {
        const CHART_TYPE_ALIASES: Record<string, string> = {
          bar: "bar_vertical",
          column: "bar_vertical",
          horizontal_bar: "bar_horizontal",
          stacked_bar: "stacked_bar_vertical",
          line: "line_multi_series",
          area: "time_series_area",
          number: "big_number",
          kpi: "big_number",
          map: "world_map",
          choropleth: "world_map",
        };
        const resolved = CHART_TYPE_ALIASES[chart.chart_type] || chart.chart_type;
        setChartType(resolved as ChartKind);
      }

      const mergedAdv = chart.viz_config
        ? mergeAdvancedOptions(chart.viz_config.echarts_option || chart.viz_config)
        : { ...DEFAULT_ADVANCED_OPTIONS };
      // Rolling calc lives in advancedOptions at query time, but is often saved
      // on query_config — restore it so cumulative/rolling charts (and KPI trends)
      // re-run correctly instead of silently degrading to raw values.
      if (typeof qc.rolling_calc === "string" && qc.rolling_calc !== "none") {
        (mergedAdv as any).rollingCalc = qc.rolling_calc;
        if (qc.rolling_window) (mergedAdv as any).rollingWindow = qc.rolling_window;
      }
      // Dashboard colour theme overrides the chart's own palette (map scales excepted).
      if (paletteOverride && paletteOverride.length && chart.chart_type !== "world_map") {
        (mergedAdv as any).color = paletteOverride;
      }
      setAdvancedOptions(mergedAdv);

      // These need NO datasetColumns (raw column names / flags) — hydrate them
      // here, ungated, so scatter axes + raw mode are set even before dataset
      // columns finish loading. Otherwise the scatter query falls back to SELECT *
      // and the renderer grabs the wrong column (e.g. context_window, millions) for Y.
      if (qc.query_mode === "raw") setQueryMode("raw");
      if (qc.x_axis || qc.y_axis || qc.size_column || qc.label_column) {
        setScatterAxes({
          x: qc.x_axis || undefined,
          y: qc.y_axis || undefined,
          size: qc.size_column || undefined,
          label: qc.label_column || undefined,
        });
      }
      if (qc.category_labels && typeof qc.category_labels === "object") {
        setCategoryLabels(qc.category_labels);
      }

      hasSetMetadataRef.current = true;
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

    // Metrics — prefer saved metrics array, fall back to legacy single metric
    if (Array.isArray(qc.metrics) && qc.metrics.length > 0) {
      const hydratedMetrics = qc.metrics.map((m: any, idx: number) => {
        const rawCol = m.column || m.col || "";
        const col = normalizeLookup(rawCol) || rawCol;
        return {
          id: `hydrated-${idx}`,
          column: col,
          aggregate: (m.aggregate || m.agg || "SUM") as "SUM" | "COUNT" | "COUNT_DISTINCT" | "AVG" | "MIN" | "MAX",
          label: m.label || m.name || rawCol.split(".").pop() || "value",
        };
      }).filter((m: any) => m.column);
      if (hydratedMetrics.length > 0) {
        setMetrics(hydratedMetrics);
        setMetricColumn(hydratedMetrics[0].column);
      }
    } else {
      // Legacy format: single metric stored as qc.metric = { column, agg }
      let metricCol: string | null = null;
      let metricAgg = "SUM";
      if (qc.metric && typeof qc.metric.column === "string") {
        metricCol = qc.metric.column;
        metricAgg = qc.metric.agg || qc.metric.aggregate || "SUM";
      }
      metricCol = normalizeLookup(metricCol);
      if (metricCol) {
        setMetricColumn(metricCol);
        // Synthesise a metrics[] entry so the config panel shows the metric correctly
        setMetrics([{
          id: "hydrated-0",
          column: metricCol,
          aggregate: metricAgg as "SUM" | "COUNT" | "COUNT_DISTINCT" | "AVG" | "MIN" | "MAX",
          label: metricCol.split(".").pop() || "value",
        }]);
      }
    }

    // Time grain
    if (typeof qc.time_grain === "string" && qc.time_grain !== "none") {
      setTimeGrain(qc.time_grain);
    }

    // Sort by
    if (qc.sort_by && typeof qc.sort_by.column === "string") {
      setSortBy({ column: qc.sort_by.column, direction: qc.sort_by.direction || "asc" });
    }

    // (query_mode, scatter axes, category_labels are hydrated ungated above.)

    // Group-by columns
    if (Array.isArray(qc.groupby) && qc.groupby.length > 0) {
      const mapped = qc.groupby
        .filter((g: unknown) => typeof g === "string")
        .map((g: string) => normalizeLookup(g))
        .filter((g: string | null): g is string => Boolean(g));
      setGroupByColumns(mapped);
    } else if (Array.isArray(qc.columns) && qc.columns.length > 0) {
      // Raw column list (e.g. scatter/bubble: [label, x, y, size]). The backend's
      // raw mode SELECTs the groupby columns in order, so hydrate qc.columns there
      // when there's no explicit groupby. Without this a scatter chart selects
      // nothing and never renders ("Configure your chart to see preview"). Order is
      // preserved so the renderer maps label/x/y/size by column type correctly.
      const rawCols = qc.columns
        .filter((c: unknown) => typeof c === "string")
        .map((c: string) => normalizeLookup(c))
        .filter((c: string | null): c is string => Boolean(c));
      if (rawCols.length > 0) {
        setGroupByColumns(rawCols);
        setQueryMode("raw");
      }
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
      // Dashboard (external) filters are NOT baked here — they're applied as
      // reactive run-time extras (see the effect below), so changing a dashboard
      // filter re-runs the query. Baking them here only applied them once.
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

    const isTable = chartType === "table" || chartType === "pivot_table";
    const isScatterLike = chartType === "scatter" || chartType === "bubble";
    // Scatter/bubble and any raw-column chart render from selected columns, not a
    // metric — don't block their auto-run on metricColumn.
    const hasRawColumns = groupByColumns.length > 0;
    if (!isTable && !isScatterLike && !hasRawColumns && !metricColumn) return;

    const isTimeSeries = [
      "time_series_line",
      "time_series_line_share",
      "time_series_area",
      "time_series_area_share",
    ].includes(chartType);
    if (isTimeSeries && !timeColumn) return;  // Time series needs a time column

    if (!advancedOptions) return;             // Wait for viz config to be applied

    hasAutoRunRef.current = true;
    void runPreviewQuery(false, buildExtras());  // apply any dashboard filters on the first run too
  }, [
    chart?.id,
    chartType,
    selectedDatasetId,
    metricColumn,
    groupByColumns,
    timeColumn,
    sqlPreview.lastSql,
    advancedOptions,
    runPreviewQuery,
  ]);

  return null;
};

export default ChartHydrator;
