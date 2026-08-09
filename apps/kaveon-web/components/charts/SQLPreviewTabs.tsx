import React, { useEffect, useMemo, useState } from "react";
import { format as formatSql } from "sql-formatter";

import RunQueryButton from "./RunQueryButton";
import { useChartBuilder } from "./ChartBuilderContext";

const SQLPreviewTabs: React.FC = () => {
  const [activeTab, setActiveTab] = useState<"sql" | "data" | "json">("sql");
  const { sqlPreview, datasetColumns } = useChartBuilder();

  const {
    lastSql,
    lastConfigJson,
    dataColumns,
    dataRows,
    isRunning,
    error,
    durationMs,
    fabricDurationMs,
  } = sqlPreview;

  const [liveElapsedMs, setLiveElapsedMs] = useState<number | null>(null);

  const coalesceDiagnostics = useMemo(() => {
    if (!lastSql) {
      return { hasMultiSemanticGroups: false, hasCoalesceInSql: false };
    }

    const groups: Record<string, number> = {};
    for (const col of datasetColumns) {
      if (!col.is_dimension) continue;
      const sem = (col.semantic_type || "").trim().toLowerCase();
      if (!sem || sem === "time") continue;
      groups[sem] = (groups[sem] || 0) + 1;
    }

    const hasMultiSemanticGroups = Object.values(groups).some((count) => count > 1);
    const hasCoalesceInSql = lastSql.toUpperCase().includes("COALESCE(");

    return { hasMultiSemanticGroups, hasCoalesceInSql };
  }, [datasetColumns, lastSql]);

  const formatDurationMs = (ms?: number | null): string => {
    if (ms == null) return "";
    const seconds = ms / 1000;
    return `${seconds.toFixed(2)} s`;
  };

  useEffect(() => {
    if (!isRunning) {
      setLiveElapsedMs(null);
      return undefined;
    }

    const start = performance.now();
    setLiveElapsedMs(0);

    const id = window.setInterval(() => {
      setLiveElapsedMs(performance.now() - start);
    }, 100);

    return () => {
      window.clearInterval(id);
    };
  }, [isRunning]);

  const renderSql = () => {
    if (error) return <div style={{ color: "#f97373" }}>{error}</div>;
    if (isRunning) return <div className="muted">Updating chart...</div>;
    if (!lastSql)
      return <div className="muted">Update chart to see generated SQL.</div>;

    // Format SQL for better readability
    let formattedSql: string;
    try {
      formattedSql = formatSql(lastSql, {
        language: "tsql",
        tabWidth: 2,
        keywordCase: "upper",
        linesBetweenQueries: 2,
      });
    } catch (err) {
      // If formatting fails, fall back to unformatted SQL
      console.warn("[SQLPreviewTabs] SQL formatting failed:", err);
      formattedSql = lastSql;
    }

    return (
      <>
        {coalesceDiagnostics.hasMultiSemanticGroups &&
          !coalesceDiagnostics.hasCoalesceInSql && (
            <div
              style={{
                marginBottom: 8,
                padding: "6px 8px",
                borderRadius: 6,
                background: "#fffbeb",
                border: "1px solid #fbbf24",
                color: "#92400e",
                fontSize: 11,
              }}
            >
              This dataset has semantic dimensions backed by multiple tables, but the
              generated SQL does not contain a COALESCE expression. If you expect
              values to be coalesced across dimensions (e.g. Product names from
              multiple tables), double-check the dataset metadata.
            </div>
          )}
        <pre
          style={{
            background: "var(--bg-surface)",
            borderRadius: 8,
            padding: 8,
            fontSize: 12,
            height: "100%",
            width: "100%",
            boxSizing: "border-box",
            overflow: "auto",
            border: "1px solid var(--border)",
          }}
        >
          {formattedSql}
        </pre>
      </>
    );
  };

  const renderData = () => {
    if (error) return <div style={{ color: "#f97373" }}>{error}</div>;
    if (isRunning) return <div className="muted">Updating chart...</div>;
    if (!dataColumns.length)
      return <div className="muted">No data yet. Update chart to preview rows.</div>;

    return (
      <div
        className="results-table-container"
        style={{ height: "100%", width: "100%", boxSizing: "border-box", overflow: "auto" }}
      >
        <table className="results-table">
          <thead>
            <tr>
              {dataColumns.map((col) => (
                <th key={col}>{col}</th>
              ))}
            </tr>
          </thead>
          <tbody>
            {dataRows.map((row, idx) => (
              // eslint-disable-next-line react/no-array-index-key
              <tr key={idx}>
                {row.map((cell, j) => {
                  const raw = cell === null || cell === undefined ? "" : String(cell);
                  const parsed =
                    raw.trim() !== "" && !Number.isNaN(Number.parseFloat(raw))
                      ? Number.parseFloat(raw)
                      : null;
                  const isNumeric = parsed !== null;

                  return (
                    <td key={j} className={isNumeric ? "numeric-cell" : undefined}>
                      {raw}
                    </td>
                  );
                })}
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    );
  };

  const renderJson = () => {
    if (!lastConfigJson)
      return <div className="muted">Config JSON will appear after updating the chart.</div>;
    return (
      <pre
        style={{
          background: "var(--bg-surface)",
          borderRadius: 8,
          padding: 8,
          fontSize: 12,
          height: "100%",
          width: "100%",
          boxSizing: "border-box",
          overflow: "auto",
          border: "1px solid var(--border)",
        }}
      >
        {JSON.stringify(lastConfigJson, null, 2)}
      </pre>
    );
  };

  return (
    <div className="sql-preview">
      <div className="sql-preview-header">
        <div className="sql-preview-tabs">
          <button
            type="button"
            onClick={() => setActiveTab("sql")}
            className={
              activeTab === "sql" ? "sql-tab sql-tab-active" : "sql-tab sql-tab-muted"
            }
          >
            SQL
          </button>
          <button
            type="button"
            onClick={() => setActiveTab("data")}
            className={
              activeTab === "data" ? "sql-tab sql-tab-active" : "sql-tab sql-tab-muted"
            }
          >
            Data
          </button>
          <button
            type="button"
            onClick={() => setActiveTab("json")}
            className={
              activeTab === "json" ? "sql-tab sql-tab-active" : "sql-tab sql-tab-muted"
            }
          >
            JSON config
          </button>
          {isRunning && liveElapsedMs != null && (
            <span className="sql-preview-runtime">
              Running {formatDurationMs(liveElapsedMs)}
            </span>
          )}
          {!isRunning && durationMs != null && !Number.isNaN(durationMs) && (
            <span className="sql-preview-runtime is-idle">
              <span className="sql-runtime-chip">
                <span className="sql-runtime-dot sql-runtime-dot-total" />
                <span>Last run {formatDurationMs(durationMs)}</span>
              </span>
              {fabricDurationMs != null && !Number.isNaN(fabricDurationMs) && (
                <span className="sql-runtime-chip sql-runtime-fabric-text">
                  <span className="sql-runtime-dot sql-runtime-dot-fabric" />
                  <span>Server {formatDurationMs(fabricDurationMs)}</span>
                </span>
              )}
            </span>
          )}
        </div>
        <RunQueryButton />
      </div>
      <div className="sql-preview-body">
        {activeTab === "sql" && renderSql()}
        {activeTab === "data" && renderData()}
        {activeTab === "json" && renderJson()}
      </div>
    </div>
  );
};

export default SQLPreviewTabs;
