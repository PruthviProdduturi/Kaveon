import React from "react";
import { useChartBuilder } from "./ChartBuilderContext";

const RunQueryButton: React.FC = () => {
  const { runPreviewQuery, sqlPreview, selectedDatasetId, selectedTemplate, metricColumn } =
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
