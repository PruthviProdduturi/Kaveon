import React from "react";

import { useChartBuilder } from "./ChartBuilderContext";

const DatasetSelector: React.FC = () => {
  const { datasets, datasetsError, selectedDatasetId, setSelectedDatasetId } =
    useChartBuilder();

  return (
    <div className="chart-builder-field-group">
      <label className="chart-builder-label" htmlFor="dataset-select">
        Dataset
      </label>
      <select
        id="dataset-select"
        className="chart-builder-select"
        value={selectedDatasetId ?? ""}
        onChange={(e) => {
          const value = e.target.value;
          setSelectedDatasetId(value ? Number(value) : null);
        }}
      >
        <option value="">Select a dataset...</option>
        {datasets.map((d) => (
          <option key={d.id} value={d.id}>
            {d.name}
          </option>
        ))}
      </select>

      {datasetsError && (
        <div className="muted" style={{ marginTop: 4, fontSize: 12 }}>
          {datasetsError}
        </div>
      )}
    </div>
  );
};

export default DatasetSelector;
