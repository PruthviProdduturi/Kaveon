// Example usage of DataSourceSelector component
// This file shows how to integrate the DataSourceSelector into your forms

import { useState } from "react";
import { DataSourceSelector } from "../../../components/DataSourceSelector";

export function DatasetFormExample() {
  const [dataSourceId, setDataSourceId] = useState<number | null>(null);

  return (
    <div>
      {/* Example 1: Required data source selector */}
      <DataSourceSelector
        value={dataSourceId}
        onChange={setDataSourceId}
        label="Data Source"
        required={true}
      />

      {/* Example 2: Optional data source selector */}
      <DataSourceSelector
        value={dataSourceId}
        onChange={setDataSourceId}
        label="Select a Data Source"
        required={false}
      />

      {/* The selected data source ID will be in the dataSourceId state */}
      {dataSourceId && (
        <div>Selected Data Source ID: {dataSourceId}</div>
      )}
    </div>
  );
}
