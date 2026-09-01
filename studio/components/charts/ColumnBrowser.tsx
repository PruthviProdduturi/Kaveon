import React, { useMemo, useState } from "react";

import { useChartBuilder } from "./ChartBuilderContext";

const ColumnBrowser: React.FC = () => {
  const {
    selectedDatasetId,
    datasetColumns,
    datasetMetrics,
    datasetDetailError,
  } = useChartBuilder();

  const [search, setSearch] = useState("");
  const [isMetricsOpen, setIsMetricsOpen] = useState(true);
  const [isColumnsOpen, setIsColumnsOpen] = useState(true);

  const grouped = useMemo(() => {
    const timeLike = datasetColumns.filter((c) =>
      ["date", "datetime", "datetime2", "smalldatetime", "timestamp"].some((t) =>
        c.data_type?.toLowerCase().includes(t),
      ),
    );

    const metricsFromColumns = datasetColumns.filter((c) => c.is_metric);

    const seenSemanticDims = new Set<string>();
    const dimensions = datasetColumns.filter((c) => {
      if (!c.is_dimension) return false;
      const semantic = (c.semantic_type || "").toLowerCase();
      if (!semantic || semantic === "time") return true;
      if (seenSemanticDims.has(semantic)) return false;
      seenSemanticDims.add(semantic);
      return true;
    });

    const other = datasetColumns.filter(
      (c) => !c.is_dimension && !c.is_metric && !timeLike.includes(c),
    );

    return {
      dimensions,
      metricsFromColumns,
      timeLike,
      other,
    };
  }, [datasetColumns]);

  const allMetrics = useMemo(
    () =>
      datasetMetrics.map((m) => ({
        key: m.name,
        label: m.name,
        meta: "Metric",
        kind: "metric" as const,
      })),
    [datasetMetrics],
  );

  const allColumns = useMemo(
    () => {
      const byKey = new Map<string, (typeof datasetColumns)[number]>();

      [...grouped.dimensions, ...grouped.timeLike].forEach((c) => {
        const key = `${c.table_name}.${c.column_name}`;
        if (!byKey.has(key)) {
          byKey.set(key, c);
        }
      });

      return Array.from(byKey.values()).map((c) => {
        const semantic = (c.semantic_type || "").toLowerCase();
        const dataType = (c.data_type || "").toLowerCase();
        const isTimeLike =
          semantic === "time" ||
          ["date", "datetime", "datetime2", "smalldatetime", "timestamp"].some((t) =>
            dataType.includes(t),
          );

        return {
          key: `${c.table_name}.${c.column_name}`,
          label:
            c.semantic_type && semantic !== "time" ? c.semantic_type : c.column_name,
          meta: isTimeLike ? "Time" : c.is_dimension ? "Dimension" : "Column",
          kind: isTimeLike ? ("time" as const) : ("dimension" as const),
        };
      });
    },
    [datasetColumns, grouped.dimensions, grouped.timeLike],
  );

  const loweredSearch = search.trim().toLowerCase();

  const filteredMetrics = useMemo(
    () =>
      loweredSearch
        ? allMetrics.filter(
            (m) =>
              m.label.toLowerCase().includes(loweredSearch) ||
              m.meta.toLowerCase().includes(loweredSearch),
          )
        : allMetrics,
    [allMetrics, loweredSearch],
  );

  const filteredColumns = useMemo(
    () =>
      loweredSearch
        ? allColumns.filter(
            (c) =>
              c.label.toLowerCase().includes(loweredSearch) ||
              c.meta.toLowerCase().includes(loweredSearch),
          )
        : allColumns,
    [allColumns, loweredSearch],
  );

  if (!selectedDatasetId) {
    return (
      <div className="chart-source-empty">
        Select a dataset to browse its metrics and columns.
      </div>
    );
  }

  return (
    <>
      {datasetDetailError && (
        <p className="text-sm text-red-600 mb-2">{datasetDetailError}</p>
      )}

      <div className="chart-source-search">
        <input
          type="text"
          className="chart-source-search-input"
          placeholder="Search metrics & columns"
          value={search}
          onChange={(e) => setSearch(e.target.value)}
        />
      </div>

      <div className="chart-source-section">
        <button
          type="button"
          className="chart-source-section-header"
          onClick={() => setIsMetricsOpen((prev) => !prev)}
        >
          <div className="chart-source-section-header-left">
            <span
              className={
                isMetricsOpen
                  ? "chart-source-section-arrow chart-source-section-arrow-open"
                  : "chart-source-section-arrow"
              }
            >
              ▾
            </span>
            <span className="chart-source-section-title">Metrics</span>
          </div>
          <span className="chart-source-count">
            Showing {filteredMetrics.length} of {allMetrics.length}
          </span>
        </button>
        {isMetricsOpen && (
          <div className="chart-source-list">
            {allMetrics.length === 0 ? (
              <div className="chart-source-empty">No metrics defined for this dataset.</div>
            ) : filteredMetrics.length === 0 ? (
              <div className="chart-source-empty">No metrics match your search.</div>
            ) : (
              filteredMetrics.map((m) => (
                <div key={m.key} className="chart-source-item">
                  <div
                    className={
                      m.kind === "metric"
                        ? "chart-source-item-icon chart-source-item-icon-metric"
                        : "chart-source-item-icon chart-source-item-icon-metric-column"
                    }
                  >
                    fx
                  </div>
                  <div className="chart-source-item-main">
                    <span className="chart-source-item-label">{m.label}</span>
                    <span className="chart-source-item-meta">{m.meta}</span>
                  </div>
                  <div className="chart-source-item-handle">⋮⋮</div>
                </div>
              ))
            )}
          </div>
        )}
      </div>

      <div className="chart-source-section">
        <button
          type="button"
          className="chart-source-section-header"
          onClick={() => setIsColumnsOpen((prev) => !prev)}
        >
          <div className="chart-source-section-header-left">
            <span
              className={
                isColumnsOpen
                  ? "chart-source-section-arrow chart-source-section-arrow-open"
                  : "chart-source-section-arrow"
              }
            >
              ▾
            </span>
            <span className="chart-source-section-title">Columns</span>
          </div>
          <span className="chart-source-count">
            Showing {filteredColumns.length} of {allColumns.length}
          </span>
        </button>
        {isColumnsOpen && (
          <div className="chart-source-list">
            {allColumns.length === 0 ? (
              <div className="chart-source-empty">No columns detected for this dataset.</div>
            ) : filteredColumns.length === 0 ? (
              <div className="chart-source-empty">No columns match your search.</div>
            ) : (
              filteredColumns.map((c) => (
                <div key={c.key} className="chart-source-item">
                  <div
                    className={
                      c.kind === "time"
                        ? "chart-source-item-icon chart-source-item-icon-time"
                        : "chart-source-item-icon chart-source-item-icon-dimension"
                    }
                  >
                    {c.kind === "time" ? "⏱" : "abc"}
                  </div>
                  <div className="chart-source-item-main">
                    <span className="chart-source-item-label">{c.label}</span>
                    <span className="chart-source-item-meta">{c.meta}</span>
                  </div>
                  <div className="chart-source-item-handle">⋮⋮</div>
                </div>
              ))
            )}
          </div>
        )}
      </div>
    </>
  );
};

export default ColumnBrowser;
