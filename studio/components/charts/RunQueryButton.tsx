import React from "react";
import { useChartBuilder } from "./ChartBuilderContext";

const RunQueryButton: React.FC = () => {
  const { runPreviewQuery, sqlPreview, selectedDatasetId, selectedTemplate, metricColumn, readOnly } =
    useChartBuilder();

  const disabledReason = (() => {
    if (!selectedDatasetId || !selectedTemplate) return "Select a dataset and chart type";
    if (!metricColumn && selectedTemplate.id !== "table" && selectedTemplate.id !== "pivot_table") {
      return "Choose a metric in the config";
    }
    if (sqlPreview.isRunning) return "Running...";
    return null;
  })();

  const handleClick = () => {
    if (disabledReason) return;
    // Force regeneration when Update chart button is clicked
    void runPreviewQuery(true);
  };

  // Read-only is about keeping a change, not about reading the data: a caller
  // who may run this chart's query may run it again. The label says what the
  // press actually does, since nothing about the chart's definition can move.
  if (readOnly) {
    if (!selectedDatasetId || !selectedTemplate) return null;
    return (
      <button
        type="button"
        className="chart-builder-primary-btn"
        onClick={() => { if (!sqlPreview.isRunning) void runPreviewQuery(true); }}
        disabled={sqlPreview.isRunning}
        title="Run this chart's query again and redraw it. The chart's definition is unchanged."
      >
        {sqlPreview.isRunning ? "Refreshing…" : "Refresh data"}
      </button>
    );
  }

  return (
    <button
      type="button"
      className="chart-builder-primary-btn"
      style={{ opacity: disabledReason ? 0.6 : 1 }}
      onClick={handleClick}
      disabled={Boolean(disabledReason)}
    >
      {sqlPreview.isRunning ? "Updating chart..." : "Update chart"}
    </button>
  );
};

export default RunQueryButton;
