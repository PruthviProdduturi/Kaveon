import React from "react";

import { ReadOnlyField } from "./ChartReadOnly";
import { useChartBuilder } from "./ChartBuilderContext";

const DatasetSelector: React.FC = () => {
  const { datasets, datasetsError, selectedDatasetId, setSelectedDatasetId, readOnly } =
    useChartBuilder();

  // A reader is told which dataset the chart is built on, not invited to
  // point it somewhere else. The field keeps its label and shape so the panel
  // reads the same; only the control it would need write access for is gone.
  if (readOnly) {
    const selected = datasets.find((d) => d.id === selectedDatasetId);
    return (
      <>
        <ReadOnlyField label="Dataset" emptyText="Not available to you">
          {selected?.name ?? ""}
        </ReadOnlyField>
        {datasetsError && (
          <div className="muted" style={{ marginTop: 4, fontSize: 12 }}>
            {datasetsError}
          </div>
        )}
      </>
    );
  }

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
