import React, { useMemo, useState } from "react";

import ChartTypePicker from "./ChartTypePicker";
import FilterBuilder from "./FilterBuilder";
import { useChartBuilder, TimeRangePreset } from "./ChartBuilderContext";

const ChartConfigPanel: React.FC = () => {
  const {
    selectedDatasetId,
    datasetColumns,
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
  } = useChartBuilder();

  const [isTimeRangeEditorOpen, setIsTimeRangeEditorOpen] = useState(false);
  const [draftTimeRange, setDraftTimeRange] = useState(timeRange);
  const [draftStartDate, setDraftStartDate] = useState(customStartDate || "");
  const [draftEndDate, setDraftEndDate] = useState(customEndDate || "");

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

  const metricCandidates = useMemo(
    () => datasetColumns.filter((c) => c.is_metric),
    [datasetColumns],
  );

  const timeCandidates = useMemo(
    () =>
      datasetColumns.filter((c) =>
        ["date", "datetime", "datetime2", "smalldatetime", "timestamp"].some((t) =>
          c.data_type?.toLowerCase().includes(t),
        ),
      ),
    [datasetColumns],
  );

  const timeRangeOptions = useMemo(
    () => [
      { value: "all_time", label: "No filter (all time)", group: "Common" },
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

  const dateFormatOptions = useMemo(
    () => [
      { value: "auto", label: "Auto (raw values)" },
      { value: "date", label: "Date (YYYY-MM-DD)" },
      { value: "weekday", label: "Weekday name (Mon)" },
      { value: "month_year", label: "Month + year (Jan 2025)" },
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

  const closeTimeRangeEditor = () => {
    setIsTimeRangeEditorOpen(false);
  };

  const applyTimeRangeEditor = () => {
    if (draftTimeRange === "custom" || draftTimeRange === "custom_to_latest") {
      // Validate custom date ranges
      if (!draftStartDate) {
        alert("Please select a start date");
        return;
      }
      if (draftTimeRange === "custom" && !draftEndDate) {
        alert("Please select an end date for custom range");
        return;
      }
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

  if (!selectedDatasetId) {
    return (
      <div>
        <div className="chart-builder-panel-title">Chart configuration</div>
        <div className="muted" style={{ fontSize: 13 }}>
          Select a dataset on the left to configure metric, dimensions, time, and pre-filters.
        </div>
      </div>
    );
  }

  return (
    <div>
      <div className="chart-builder-panel-title">Chart configuration</div>

      <ChartTypePicker />

      <div className="chart-builder-field-group">
        <label className="chart-builder-label" htmlFor="metric-select">
          Metric (Y axis)
        </label>
        <select
          id="metric-select"
          className="chart-builder-select"
          value={metricColumn ?? ""}
          onChange={(e) => setMetricColumn(e.target.value || null)}
        >
          <option value="">Select a metric column...</option>
          {metricCandidates.map((c, index) => {
            const id = `${c.table_name}.${c.column_name}`;
            // Use semantic_type as key if available (it's unique), otherwise use id + index
            const uniqueKey = c.semantic_type || `${id}-${index}`;
            const label =
              c.semantic_type && c.semantic_type.toLowerCase() !== "time"
                ? c.semantic_type
                : c.column_name;
            return (
              <option key={uniqueKey} value={id}>
                {label}
              </option>
            );
          })}
        </select>
      </div>

      <div className="chart-builder-field-group">
        <label className="chart-builder-label" htmlFor="time-column-select">
          Time column (optional)
        </label>
        <select
          id="time-column-select"
          className="chart-builder-select"
          value={timeColumn ?? ""}
          onChange={(e) => setTimeColumn(e.target.value || null)}
        >
          <option value="">Select a time column...</option>
          {timeCandidates.map((c, index) => {
            const id = `${c.table_name}.${c.column_name}`;
            // Use semantic_type as key if available (it's unique), otherwise use id + index
            const uniqueKey = c.semantic_type || `${id}-${index}`;
            const label =
              c.semantic_type && c.semantic_type.toLowerCase() !== "time"
                ? c.semantic_type
                : c.column_name;
            return (
              <option key={uniqueKey} value={id}>
                {label}
              </option>
            );
          })}
        </select>
      </div>

      <div className="chart-builder-field-group chart-timerange-field">
        <label className="chart-builder-label" htmlFor="time-range-select">
          Time range
        </label>
        <div
          id="time-range-select"
          className="chart-builder-input chart-builder-input-clickable"
          role="button"
          tabIndex={0}
          onClick={() => {
            if (isTimeRangeEditorOpen) {
              closeTimeRangeEditor();
            } else {
              setDraftTimeRange(timeRange);
              setDraftStartDate(customStartDate || "");
              setDraftEndDate(customEndDate || "");
              setIsTimeRangeEditorOpen(true);
            }
          }}
          onKeyDown={(e) => {
            if (e.key === "Enter" || e.key === " ") {
              e.preventDefault();
              if (isTimeRangeEditorOpen) {
                closeTimeRangeEditor();
              } else {
                setDraftTimeRange(timeRange);
                setDraftStartDate(customStartDate || "");
                setDraftEndDate(customEndDate || "");
                setIsTimeRangeEditorOpen(true);
              }
            }
          }}
        >
          <span>{currentTimeRangeLabel}</span>
          <span className="chart-builder-input-clickable-icon" aria-hidden="true" />
        </div>

        {isTimeRangeEditorOpen && (
          <div className="chart-timerange-popover">
            <div className="chart-timerange-popover-header">Edit time range</div>

            <div className="chart-builder-field-group" style={{ marginBottom: 8 }}>
              <label
                className="chart-builder-label"
                htmlFor="time-range-modal-select"
                style={{ fontSize: 11 }}
              >
                Range type
              </label>
              <select
                id="time-range-modal-select"
                className="chart-builder-select"
                value={draftTimeRange}
                onChange={(e) =>
                  setDraftTimeRange(e.target.value as typeof draftTimeRange)
                }
              >
                <optgroup label="Common">
                  {timeRangeOptions
                    .filter((opt) => opt.group === "Common")
                    .map((opt) => (
                      <option key={opt.value} value={opt.value}>
                        {opt.label}
                      </option>
                    ))}
                </optgroup>
                <optgroup label="Current Period">
                  {timeRangeOptions
                    .filter((opt) => opt.group === "Current")
                    .map((opt) => (
                      <option key={opt.value} value={opt.value}>
                        {opt.label}
                      </option>
                    ))}
                </optgroup>
                <optgroup label="Previous Period">
                  {timeRangeOptions
                    .filter((opt) => opt.group === "Previous")
                    .map((opt) => (
                      <option key={opt.value} value={opt.value}>
                        {opt.label}
                      </option>
                    ))}
                </optgroup>
                <optgroup label="Fixed Duration">
                  {timeRangeOptions
                    .filter((opt) => opt.group === "Fixed")
                    .map((opt) => (
                      <option key={opt.value} value={opt.value}>
                        {opt.label}
                      </option>
                    ))}
                </optgroup>
                <optgroup label="Custom">
                  {timeRangeOptions
                    .filter((opt) => opt.group === "Custom")
                    .map((opt) => (
                      <option key={opt.value} value={opt.value}>
                        {opt.label}
                      </option>
                    ))}
                </optgroup>
              </select>
            </div>

            {(draftTimeRange === "custom" || draftTimeRange === "custom_to_latest") && (
              <>
                <div className="chart-builder-field-group" style={{ marginBottom: 8, marginTop: 12 }}>
                  <label
                    className="chart-builder-label"
                    htmlFor="custom-start-date"
                    style={{ fontSize: 11 }}
                  >
                    Start Date
                  </label>
                  <input
                    id="custom-start-date"
                    type="date"
                    className="chart-builder-select"
                    value={draftStartDate}
                    onChange={(e) => setDraftStartDate(e.target.value)}
                    style={{ padding: "6px 8px" }}
                  />
                </div>

                {draftTimeRange === "custom" && (
                  <div className="chart-builder-field-group" style={{ marginBottom: 8 }}>
                    <label
                      className="chart-builder-label"
                      htmlFor="custom-end-date"
                      style={{ fontSize: 11 }}
                    >
                      End Date
                    </label>
                    <input
                      id="custom-end-date"
                      type="date"
                      className="chart-builder-select"
                      value={draftEndDate}
                      onChange={(e) => setDraftEndDate(e.target.value)}
                      style={{ padding: "6px 8px" }}
                    />
                  </div>
                )}

                {draftTimeRange === "custom_to_latest" && (
                  <div
                    style={{
                      marginBottom: 8,
                      padding: "8px",
                      backgroundColor: "#f0f9ff",
                      border: "1px solid #bfdbfe",
                      borderRadius: "4px",
                      fontSize: 11,
                      color: "#1e40af",
                    }}
                  >
                    <i className="fas fa-info-circle" style={{ marginRight: "6px" }}></i>
                    End date will be the latest date in your dataset
                  </div>
                )}
              </>
            )}

            {!(draftTimeRange === "custom" || draftTimeRange === "custom_to_latest") && (
              <div className="muted" style={{ fontSize: 11, marginBottom: 8 }}>
                Actual time range will be calculated from this preset.
              </div>
            )}

            <div className="chart-timerange-popover-actions">
              <button
                type="button"
                className="chart-save-modal-btn chart-save-modal-btn-secondary"
                onClick={closeTimeRangeEditor}
              >
                Cancel
              </button>
              <button
                type="button"
                className="chart-save-modal-btn chart-save-modal-btn-primary"
                onClick={applyTimeRangeEditor}
              >
                Apply
              </button>
            </div>
          </div>
        )}
      </div>

      {/* Date display format moved to Customize tab */}

      <div className="chart-builder-field-group">
        <label className="chart-builder-label" htmlFor="groupby-select">
          Dimensions
        </label>
        <select
          id="groupby-select"
          className="chart-builder-select"
          value=""
          onChange={(e) => {
            const value = e.target.value;
            if (!value) return;
            handleAddGroupBy(value);
          }}
        >
          <option value="">Add a dimension...</option>
          {dimensions.map((c, index) => {
            const id = `${c.table_name}.${c.column_name}`;
            // Use semantic_type as key if available (it's unique), otherwise use id + index
            const uniqueKey = c.semantic_type || `${id}-${index}`;
            const label =
              c.semantic_type && c.semantic_type.toLowerCase() !== "time"
                ? c.semantic_type
                : c.column_name;
            return (
              <option key={uniqueKey} value={id}>
                {label}
              </option>
            );
          })}
        </select>
        {groupByColumns.length > 0 && (
          <div style={{ marginTop: 6, display: "flex", flexWrap: "wrap", gap: 6 }}>
            {groupByColumns.map((g) => {
              const col = datasetColumns.find(
                (c) => `${c.table_name}.${c.column_name}` === g,
              );
              const label = col
                ? col.semantic_type && col.semantic_type.toLowerCase() !== "time"
                  ? col.semantic_type
                  : col.column_name
                : g;
              return (
                <span
                  key={g}
                  style={{
                    fontSize: 12,
                    padding: "2px 6px",
                    borderRadius: 999,
                    background: "#e5edff",
                    color: "#1d4ed8",
                    display: "inline-flex",
                    alignItems: "center",
                    gap: 4,
                  }}
                >
                  {label}
                  <button
                    type="button"
                    onClick={() => handleRemoveGroupBy(g)}
                    style={{
                      border: "none",
                      background: "transparent",
                      cursor: "pointer",
                      fontSize: 11,
                      color: "#1d4ed8",
                    }}
                  >
                    ×
                  </button>
                </span>
              );
            })}
          </div>
        )}
      </div>

      <div className="chart-builder-field-group">
        <label className="chart-builder-label">Pre-filters</label>
        <FilterBuilder />
      </div>
    </div>
  );
};

export default ChartConfigPanel;
