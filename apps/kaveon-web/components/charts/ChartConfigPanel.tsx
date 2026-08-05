import React, { useMemo, useState } from "react";

import ChartTypePicker from "./ChartTypePicker";
import FilterBuilder from "./FilterBuilder";
import { MetricConfig, useChartBuilder, TimeRangePreset } from "./ChartBuilderContext";

// ── Shared sub-components (defined outside to avoid recreation on each render) ──

const SectionLabel = ({ children }: { children: React.ReactNode }) => (
  <div style={{ display: "flex", alignItems: "center", gap: 8, margin: "14px 0 10px" }}>
    <span style={{
      fontSize: 10, fontWeight: 700, color: "#94a3b8",
      letterSpacing: "0.08em", textTransform: "uppercase",
      fontFamily: "Inter, -apple-system, sans-serif", whiteSpace: "nowrap",
    }}>{children}</span>
    <div style={{ flex: 1, height: 1, background: "#f1f5f9" }} />
  </div>
);

const FieldLabel = ({ htmlFor, children, badge }: { htmlFor?: string; children: React.ReactNode; badge?: string }) => (
  <label
    htmlFor={htmlFor}
    style={{
      display: "flex", alignItems: "center", gap: 5,
      fontSize: 12, fontWeight: 500, color: "#475569", marginBottom: 5,
      fontFamily: "Inter, -apple-system, sans-serif",
      textTransform: "none", letterSpacing: "normal",
    }}
  >
    {children}
    {badge && (
      <span style={{
        fontSize: 10, fontWeight: 400, color: "#94a3b8",
        background: "#f1f5f9", borderRadius: 4, padding: "1px 5px",
        letterSpacing: "0.01em",
      }}>{badge}</span>
    )}
  </label>
);

const AGG_OPTIONS = [
  { value: "SUM", label: "SUM" },
  { value: "COUNT", label: "COUNT" },
  { value: "COUNT_DISTINCT", label: "COUNT DISTINCT" },
  { value: "AVG", label: "AVG" },
  { value: "MIN", label: "MIN" },
  { value: "MAX", label: "MAX" },
];

const ChartConfigPanel: React.FC = () => {
  const {
    selectedDatasetId,
    datasetColumns,
    metrics,
    setMetrics,
    metricColumn,
    setMetricColumn,
    timeGrain,
    setTimeGrain,
    sortBy,
    setSortBy,
    queryMode,
    setQueryMode,
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
    rowLimit,
    setRowLimit,
    chartType,
    advancedOptions,
    setAdvancedOptions,
  } = useChartBuilder();

  const [isTimeRangeEditorOpen, setIsTimeRangeEditorOpen] = useState(false);
  const [draftTimeRange, setDraftTimeRange] = useState(timeRange);
  const [draftStartDate, setDraftStartDate] = useState(customStartDate || "");
  const [draftEndDate, setDraftEndDate] = useState(customEndDate || "");

  // All numeric/metric columns
  const allColumns = useMemo(() => datasetColumns, [datasetColumns]);

  const metricCandidates = useMemo(
    () => datasetColumns.filter((c) => c.is_metric),
    [datasetColumns],
  );

  // For virtual SQL datasets, all columns are potential metrics
  const effectiveMetricCandidates = metricCandidates.length > 0 ? metricCandidates : allColumns;

  const dimensions = useMemo(() => {
    const seenSemanticDims = new Set<string>();
    return datasetColumns.filter((c) => {
      if (!c.is_dimension) return false;
      const semantic = (c.semantic_type || "").toLowerCase();
      if (!semantic || semantic === "time") return true;
      if (seenSemanticDims.has(semantic)) return false;
      seenSemanticDims.add(semantic);
      return true;
    });
  }, [datasetColumns]);

  const timeCandidates = useMemo(
    () =>
      datasetColumns.filter((c) =>
        ["date", "datetime", "datetime2", "smalldatetime", "timestamp"].some((t) =>
          c.data_type?.toLowerCase().includes(t),
        ),
      ),
    [datasetColumns],
  );

  // For virtual SQL datasets, detect time columns from name
  const effectiveTimeCandidates = timeCandidates.length > 0
    ? timeCandidates
    : datasetColumns.filter((c) => /date|time|created|modified|updated/i.test(c.column_name));

  // All sortable columns (dimension + time + metric results)
  const sortCandidates = useMemo(() => {
    const seen = new Set<string>();
    return [...groupByColumns, timeColumn].filter(Boolean).concat(
      (metrics.length > 0 ? metrics.map((m) => m.label) : metricColumn ? ["value"] : [])
    ).filter((c) => { if (seen.has(c!)) return false; seen.add(c!); return true; }) as string[];
  }, [groupByColumns, timeColumn, metrics, metricColumn]);

  const timeRangeOptions = useMemo(
    () => [
      { value: "all_time", label: "No filter (all time)", group: "Common" },
      { value: "latest_day", label: "Latest available day", group: "Common" },
      { value: "last_day", label: "Last day", group: "Common" },
      { value: "last_week", label: "Last week", group: "Common" },
      { value: "last_month", label: "Last month", group: "Common" },
      { value: "last_quarter", label: "Last quarter", group: "Common" },
      { value: "last_year", label: "Last year", group: "Common" },
      { value: "week_to_date", label: "Week to date", group: "Current" },
      { value: "month_to_date", label: "Month to date", group: "Current" },
      { value: "quarter_to_date", label: "Quarter to date", group: "Current" },
      { value: "year_to_date", label: "Year to date", group: "Current" },
      { value: "previous_week", label: "Previous week", group: "Previous" },
      { value: "previous_month", label: "Previous month", group: "Previous" },
      { value: "previous_quarter", label: "Previous quarter", group: "Previous" },
      { value: "previous_year", label: "Previous year", group: "Previous" },
      { value: "last_7_days", label: "Last 7 days", group: "Fixed" },
      { value: "last_30_days", label: "Last 30 days", group: "Fixed" },
      { value: "last_90_days", label: "Last 90 days", group: "Fixed" },
      { value: "last_365_days", label: "Last 365 days", group: "Fixed" },
      { value: "custom", label: "Custom range...", group: "Custom" },
      { value: "custom_to_latest", label: "Custom to latest...", group: "Custom" },
    ],
    [],
  );

  const currentTimeRangeLabel = useMemo(() => {
    if (timeRange === "custom" && customStartDate && customEndDate) {
      return `${customStartDate} to ${customEndDate}`;
    }
    if (timeRange === "custom_to_latest" && customStartDate) {
      return `${customStartDate} to latest`;
    }
    const match = timeRangeOptions.find((opt) => opt.value === timeRange);
    return match ? match.label : "No filter (all time)";
  }, [timeRange, customStartDate, customEndDate, timeRangeOptions]);

  const handleAddGroupBy = (value: string) => {
    if (!value) return;
    if (groupByColumns.includes(value)) return;
    setGroupByColumns([...groupByColumns, value]);
  };

  const handleRemoveGroupBy = (value: string) => {
    setGroupByColumns(groupByColumns.filter((v) => v !== value));
  };

  const closeTimeRangeEditor = () => setIsTimeRangeEditorOpen(false);

  const applyTimeRangeEditor = () => {
    if (draftTimeRange === "custom" || draftTimeRange === "custom_to_latest") {
      if (!draftStartDate) { alert("Please select a start date"); return; }
      if (draftTimeRange === "custom" && !draftEndDate) { alert("Please select an end date"); return; }
      setTimeRange(draftTimeRange);
      setCustomStartDate(draftStartDate || null);
      setCustomEndDate(draftTimeRange === "custom" ? (draftEndDate || null) : null);
    } else {
      setTimeRange(draftTimeRange);
      setCustomStartDate(null);
      setCustomEndDate(null);
    }
    setIsTimeRangeEditorOpen(false);
  };

  // Metric helpers
  const addMetric = () => {
    const firstCol = effectiveMetricCandidates[0];
    if (!firstCol) return;
    const id = `${firstCol.column_name}.${Date.now()}`;
    const colId = `${firstCol.table_name}.${firstCol.column_name}`;
    setMetrics((prev) => [...prev, { id, column: colId, aggregate: "SUM", label: firstCol.column_name }]);
    // Keep legacy metricColumn in sync for auto-run guard
    if (!metricColumn) setMetricColumn(colId);
  };

  const removeMetric = (id: string) => {
    setMetrics((prev) => {
      const next = prev.filter((m) => m.id !== id);
      if (next.length === 0) setMetricColumn(null);
      else setMetricColumn(next[0].column);
      return next;
    });
  };

  const updateMetric = (id: string, field: keyof MetricConfig, value: string) => {
    setMetrics((prev) =>
      prev.map((m) => {
        if (m.id !== id) return m;
        const updated = { ...m, [field]: value };
        if (field === "column") {
          const parts = value.split(".");
          updated.label = parts[parts.length - 1] || value;
        }
        return updated;
      }),
    );
    if (field === "column") setMetricColumn(value);
  };

  const colLabel = (c: { table_name: string; column_name: string; semantic_type?: string | null }) => {
    if (c.semantic_type && c.semantic_type.toLowerCase() !== "time") return c.semantic_type;
    return c.column_name;
  };
  const colId = (c: { table_name: string; column_name: string }) => `${c.table_name}.${c.column_name}`;

  const isBigNumber = chartType === "big_number" || chartType === "big_number_trend";
  const isTable = chartType === "table" || chartType === "pivot_table";

  if (!selectedDatasetId) {
    return (
      <div>
        <div className="chart-builder-panel-title">Chart configuration</div>
        <div className="muted" style={{ fontSize: 13 }}>
          Select a dataset to configure metric, dimensions, time, and filters.
        </div>
      </div>
    );
  }

  return (
    <div>
      <div className="chart-builder-panel-title">Chart configuration</div>

      <ChartTypePicker />

      {/* ════ QUERY ════════════════════════════════════════════════════════ */}
      {!isBigNumber && <SectionLabel>Query</SectionLabel>}

      {/* ── Query mode ── */}
      {!isBigNumber && (
        <div className="chart-builder-field-group">
          <FieldLabel>Query mode</FieldLabel>
          <div style={{
            display: "flex",
            background: "#f1f5f9",
            borderRadius: 8,
            padding: 3,
            gap: 2,
          }}>
            {(["aggregate", "raw"] as const).map((mode) => (
              <button
                key={mode}
                type="button"
                onClick={() => setQueryMode(mode)}
                style={{
                  flex: 1, padding: "5px 0", fontSize: 12, borderRadius: 6,
                  cursor: "pointer", border: "none",
                  background: queryMode === mode ? "#ffffff" : "transparent",
                  color: queryMode === mode ? "#0078d4" : "#64748b",
                  fontWeight: queryMode === mode ? 600 : 400,
                  boxShadow: queryMode === mode ? "0 1px 3px rgba(0,0,0,0.1)" : "none",
                  transition: "all 0.15s ease",
                  fontFamily: "Inter, -apple-system, sans-serif",
                }}
              >
                {mode === "aggregate" ? "Aggregate" : "Raw records"}
              </button>
            ))}
          </div>
        </div>
      )}

      {/* ── Metrics ── */}
      {queryMode === "aggregate" && (
        <div className="chart-builder-field-group">
          <FieldLabel>{isBigNumber ? "Metric" : "Metrics"}</FieldLabel>

          {metrics.length === 0 && (
            <div style={{ fontSize: 11.5, color: "#94a3b8", marginBottom: 4, fontStyle: "italic" }}>
              No metrics added yet.
            </div>
          )}

          {metrics.map((m) => (
            <div key={m.id} style={{
              display: "flex", gap: 0, marginBottom: 5, alignItems: "center",
              background: "#f8fafc", border: "1.5px solid #e2e8f0",
              borderRadius: 8, overflow: "hidden",
            }}>
              <select
                className="chart-builder-select"
                style={{ width: 100, flexShrink: 0, fontSize: 11.5, border: "none", background: "transparent", borderRadius: 0, boxShadow: "none", padding: "5px 8px", fontWeight: 500, color: "#7c3aed" }}
                value={m.aggregate}
                onChange={(e) => updateMetric(m.id, "aggregate", e.target.value)}
              >
                {AGG_OPTIONS.map((a) => (
                  <option key={a.value} value={a.value}>{a.label}</option>
                ))}
              </select>
              <div style={{ width: 1, alignSelf: "stretch", background: "#e2e8f0", flexShrink: 0 }} />
              <select
                className="chart-builder-select"
                style={{ flex: 1, fontSize: 11.5, border: "none", background: "transparent", borderRadius: 0, boxShadow: "none", padding: "5px 8px" }}
                value={m.column}
                onChange={(e) => updateMetric(m.id, "column", e.target.value)}
              >
                {effectiveMetricCandidates.map((c, i) => (
                  <option key={`${colId(c)}-${i}`} value={colId(c)}>{colLabel(c)}</option>
                ))}
              </select>
              {(!isBigNumber || metrics.length > 1) && (
                <button
                  type="button"
                  onClick={() => removeMetric(m.id)}
                  style={{
                    display: "flex", alignItems: "center", justifyContent: "center",
                    width: 28, alignSelf: "stretch", flexShrink: 0,
                    background: "none", border: "none", borderLeft: "1.5px solid #e2e8f0",
                    cursor: "pointer", color: "#94a3b8", fontSize: 15,
                    transition: "background 0.1s, color 0.1s",
                  }}
                  onMouseEnter={e => { const b = e.currentTarget as HTMLButtonElement; b.style.background = "#fff0f0"; b.style.color = "#dc2626"; }}
                  onMouseLeave={e => { const b = e.currentTarget as HTMLButtonElement; b.style.background = "none"; b.style.color = "#94a3b8"; }}
                  title="Remove metric"
                >×</button>
              )}
            </div>
          ))}

          {(!isBigNumber || metrics.length === 0) && effectiveMetricCandidates.length > 0 && (
            <button
              type="button"
              onClick={addMetric}
              style={{
                display: "inline-flex", alignItems: "center", gap: 5,
                marginTop: 2, padding: "4px 10px", borderRadius: 6,
                border: "1.5px dashed #cbd5e1", background: "transparent",
                color: "#64748b", fontSize: 11.5, cursor: "pointer",
                fontFamily: "Inter, -apple-system, sans-serif",
                transition: "all 0.15s",
              }}
              onMouseEnter={e => { const b = e.currentTarget as HTMLButtonElement; b.style.borderColor = "#0078d4"; b.style.color = "#0078d4"; b.style.background = "#eff6ff"; }}
              onMouseLeave={e => { const b = e.currentTarget as HTMLButtonElement; b.style.borderColor = "#cbd5e1"; b.style.color = "#64748b"; b.style.background = "transparent"; }}
            >
              <i className="fas fa-plus" style={{ fontSize: 9 }} /> Add metric
            </button>
          )}
        </div>
      )}

      {/* ── Dimensions (group by) ── */}
      {queryMode === "aggregate" && !isBigNumber && (
        <div className="chart-builder-field-group">
          <FieldLabel htmlFor="groupby-select">Dimensions</FieldLabel>
          <select
            id="groupby-select"
            className="chart-builder-select"
            value=""
            onChange={(e) => { if (e.target.value) handleAddGroupBy(e.target.value); }}
          >
            <option value="">— add a dimension —</option>
            {dimensions.map((c, i) => (
              <option key={`${colId(c)}-${i}`} value={colId(c)}>{colLabel(c)}</option>
            ))}
          </select>
          {groupByColumns.length > 0 && (
            <div style={{ marginTop: 6, display: "flex", flexWrap: "wrap", gap: 5 }}>
              {groupByColumns.map((g) => {
                const col = datasetColumns.find((c) => colId(c) === g);
                const lbl = col ? (col.semantic_type && col.semantic_type.toLowerCase() !== "time" ? col.semantic_type : col.column_name) : g;
                return (
                  <span key={g} style={{
                    fontSize: 11.5, padding: "3px 8px 3px 10px", borderRadius: 6,
                    background: "#f0f4ff", color: "#3b5bdb",
                    border: "1.5px solid #c5d0fa",
                    display: "inline-flex", alignItems: "center", gap: 4,
                    fontFamily: "Inter, -apple-system, sans-serif", fontWeight: 500,
                  }}>
                    {lbl}
                    <button
                      type="button"
                      onClick={() => handleRemoveGroupBy(g)}
                      style={{
                        display: "flex", alignItems: "center", justifyContent: "center",
                        width: 14, height: 14, borderRadius: "50%",
                        border: "none", background: "transparent", cursor: "pointer",
                        fontSize: 13, color: "#748ffc", lineHeight: 1, padding: 0,
                        transition: "background 0.1s, color 0.1s",
                      }}
                      onMouseEnter={e => { const b = e.currentTarget as HTMLButtonElement; b.style.background = "#dde4ff"; b.style.color = "#3b5bdb"; }}
                      onMouseLeave={e => { const b = e.currentTarget as HTMLButtonElement; b.style.background = "transparent"; b.style.color = "#748ffc"; }}
                    >×</button>
                  </span>
                );
              })}
            </div>
          )}
        </div>
      )}

      {/* ════ TIME ═════════════════════════════════════════════════════════ */}
      <SectionLabel>Time</SectionLabel>

      {/* ── Time column ── */}
      <div className="chart-builder-field-group">
        <FieldLabel htmlFor="time-column-select" badge={queryMode === "aggregate" ? "optional" : undefined}>
          Time column
        </FieldLabel>
        <select
          id="time-column-select"
          className="chart-builder-select"
          value={timeColumn ?? ""}
          onChange={(e) => setTimeColumn(e.target.value || null)}
        >
          <option value="">No time column</option>
          {effectiveTimeCandidates.map((c, i) => (
            <option key={`${colId(c)}-${i}`} value={colId(c)}>{colLabel(c)}</option>
          ))}
        </select>
      </div>

      {/* ── Time grain ── */}
      {timeColumn && queryMode === "aggregate" && (
        <div className="chart-builder-field-group">
          <FieldLabel>Time grain</FieldLabel>
          <select
            className="chart-builder-select"
            value={timeGrain}
            onChange={(e) => setTimeGrain(e.target.value)}
          >
            <option value="none">No grouping (raw)</option>
            <option value="day">Day</option>
            <option value="week">Week (Mon)</option>
            <option value="month">Month</option>
            <option value="quarter">Quarter</option>
            <option value="year">Year</option>
          </select>
        </div>
      )}

      {/* Rolling calculation */}
      {timeColumn && (
        <div className="chart-builder-field-group">
          <FieldLabel>Rolling calculation</FieldLabel>
          <select
            className="chart-builder-select"
            value={advancedOptions?.rollingCalc || "none"}
            onChange={(e) => setAdvancedOptions((prev: any) => ({ ...(prev || {}), rollingCalc: e.target.value }))}
          >
            <option value="none">None (raw values)</option>
            <option value="rolling_avg">Rolling average</option>
            <option value="cumulative_sum">Cumulative sum</option>
          </select>
        </div>
      )}
      {timeColumn && (advancedOptions?.rollingCalc === "rolling_avg") && (
        <div className="chart-builder-field-group">
          <FieldLabel>Window (periods)</FieldLabel>
          <select
            className="chart-builder-select"
            value={advancedOptions?.rollingWindow || 3}
            onChange={(e) => setAdvancedOptions((prev: any) => ({ ...(prev || {}), rollingWindow: Number(e.target.value) }))}
          >
            {[2,3,4,5,6,7,8,9,10,12].map(n => <option key={n} value={n}>{n}</option>)}
          </select>
        </div>
      )}

      {/* ── Time range ── */}
      {timeColumn && (
        <div className="chart-builder-field-group chart-timerange-field">
          <FieldLabel htmlFor="time-range-select">Time range</FieldLabel>
          <div
            id="time-range-select"
            className="chart-builder-input chart-builder-input-clickable"
            role="button"
            tabIndex={0}
            onClick={() => {
              if (isTimeRangeEditorOpen) { closeTimeRangeEditor(); }
              else { setDraftTimeRange(timeRange); setDraftStartDate(customStartDate || ""); setDraftEndDate(customEndDate || ""); setIsTimeRangeEditorOpen(true); }
            }}
            onKeyDown={(e) => {
              if (e.key === "Enter" || e.key === " ") {
                e.preventDefault();
                if (isTimeRangeEditorOpen) closeTimeRangeEditor();
                else { setDraftTimeRange(timeRange); setDraftStartDate(customStartDate || ""); setDraftEndDate(customEndDate || ""); setIsTimeRangeEditorOpen(true); }
              }
            }}
          >
            <span style={{ fontSize: 12.5, color: "#1e293b", fontFamily: "Inter, -apple-system, sans-serif" }}>
              {currentTimeRangeLabel}
            </span>
            <svg viewBox="0 0 16 16" width="14" height="14" style={{ flexShrink: 0, color: "#94a3b8" }}>
              <polyline points="3,5 8,11 13,5" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round" />
            </svg>
          </div>

          {isTimeRangeEditorOpen && (
            <div className="chart-timerange-popover">
              <div className="chart-timerange-popover-header">Edit time range</div>
              <div className="chart-builder-field-group" style={{ marginBottom: 8 }}>
                <FieldLabel htmlFor="time-range-modal-select">Range type</FieldLabel>
                <select id="time-range-modal-select" className="chart-builder-select" value={draftTimeRange} onChange={(e) => setDraftTimeRange(e.target.value as typeof draftTimeRange)}>
                  <optgroup label="Common">{timeRangeOptions.filter((o) => o.group === "Common").map((o) => <option key={o.value} value={o.value}>{o.label}</option>)}</optgroup>
                  <optgroup label="Current Period">{timeRangeOptions.filter((o) => o.group === "Current").map((o) => <option key={o.value} value={o.value}>{o.label}</option>)}</optgroup>
                  <optgroup label="Previous Period">{timeRangeOptions.filter((o) => o.group === "Previous").map((o) => <option key={o.value} value={o.value}>{o.label}</option>)}</optgroup>
                  <optgroup label="Fixed Duration">{timeRangeOptions.filter((o) => o.group === "Fixed").map((o) => <option key={o.value} value={o.value}>{o.label}</option>)}</optgroup>
                  <optgroup label="Custom">{timeRangeOptions.filter((o) => o.group === "Custom").map((o) => <option key={o.value} value={o.value}>{o.label}</option>)}</optgroup>
                </select>
              </div>
              {(draftTimeRange === "custom" || draftTimeRange === "custom_to_latest") && (
                <>
                  <div className="chart-builder-field-group" style={{ marginBottom: 8, marginTop: 10 }}>
                    <FieldLabel htmlFor="custom-start-date">Start date</FieldLabel>
                    <input id="custom-start-date" type="date" className="chart-builder-select" value={draftStartDate} onChange={(e) => setDraftStartDate(e.target.value)} />
                  </div>
                  {draftTimeRange === "custom" && (
                    <div className="chart-builder-field-group" style={{ marginBottom: 8 }}>
                      <FieldLabel htmlFor="custom-end-date">End date</FieldLabel>
                      <input id="custom-end-date" type="date" className="chart-builder-select" value={draftEndDate} onChange={(e) => setDraftEndDate(e.target.value)} />
                    </div>
                  )}
                </>
              )}
              <div className="chart-timerange-popover-actions">
                <button type="button" className="chart-save-modal-btn chart-save-modal-btn-secondary" onClick={closeTimeRangeEditor}>Cancel</button>
                <button type="button" className="chart-save-modal-btn chart-save-modal-btn-primary" onClick={applyTimeRangeEditor}>Apply</button>
              </div>
            </div>
          )}
        </div>
      )}

      {/* ════ OPTIONS ══════════════════════════════════════════════════════ */}
      <SectionLabel>Options</SectionLabel>

      {/* ── Sort by ── */}
      {queryMode === "aggregate" && !isBigNumber && (groupByColumns.length > 0 || timeColumn) && (
        <div className="chart-builder-field-group">
          <FieldLabel>Sort by</FieldLabel>
          <div style={{ display: "flex", gap: 6 }}>
            <select
              className="chart-builder-select"
              style={{ flex: 1 }}
              value={sortBy?.column ?? ""}
              onChange={(e) => {
                const col = e.target.value;
                setSortBy(col ? { column: col, direction: sortBy?.direction ?? "desc" } : null);
              }}
            >
              <option value="">Default order</option>
              {groupByColumns.map((g) => {
                const col = datasetColumns.find((c) => colId(c) === g);
                const lbl = col ? colLabel(col) : g;
                return <option key={g} value={g}>{lbl}</option>;
              })}
              {timeColumn && <option value={timeColumn}>Time column</option>}
              {metrics.map((m) => <option key={m.id} value={m.label}>{m.label}</option>)}
              {metrics.length === 0 && metricColumn && <option value="value">Metric value</option>}
            </select>
            {sortBy && (
              <select
                className="chart-builder-select"
                style={{ width: 76, flexShrink: 0 }}
                value={sortBy.direction}
                onChange={(e) => setSortBy({ ...sortBy, direction: e.target.value as "asc" | "desc" })}
              >
                <option value="desc">Desc</option>
                <option value="asc">Asc</option>
              </select>
            )}
          </div>
        </div>
      )}

      {/* ── Row limit ── */}
      <div className="chart-builder-field-group">
        <FieldLabel htmlFor="row-limit-select">Row limit</FieldLabel>
        <select
          id="row-limit-select"
          className="chart-builder-select"
          value={rowLimit}
          onChange={(e) => setRowLimit(Number(e.target.value))}
        >
          {[50, 100, 200, 500, 1000, 5000, 0].map((v) => (
            <option key={v} value={v}>{v === 0 ? "No limit" : v.toLocaleString()}</option>
          ))}
        </select>
      </div>

      {/* ════ FILTERS ══════════════════════════════════════════════════════ */}
      <SectionLabel>Filters</SectionLabel>
      <div className="chart-builder-field-group" style={{ marginTop: -4 }}>
        <FilterBuilder />
      </div>
    </div>
  );
};

export default ChartConfigPanel;
