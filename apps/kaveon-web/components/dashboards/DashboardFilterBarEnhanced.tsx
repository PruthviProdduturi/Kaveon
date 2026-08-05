/**
 * Dashboard Filter Bar Component (Enhanced with Dropdowns)
 *
 * Edit mode filter management for dashboards
 * Users can select columns to add as filters (auto-defaults to "AllUp")
 * Uses exact same styling as ChartBuilder FilterBuilder component
 */

import React, { useState, useEffect, useMemo } from 'react';
import { useDashboard } from './DashboardContext';
import { msalFetch } from '../../utils/msalFetch';
import { API_BASE } from '../../config';
import { mmh3Hash64 } from '../../utils/mmh3';
import type { DashboardFilter, FilterOperator } from '../../types/dashboard';

/**
 * Available filter operators
 */
const FILTER_OPERATORS: { value: FilterOperator; label: string }[] = [
  { value: '=', label: 'Equals' },
  { value: '!=', label: 'Not Equals' },
  { value: '>', label: 'Greater Than' },
  { value: '<', label: 'Less Than' },
  { value: '>=', label: 'Greater or Equal' },
  { value: '<=', label: 'Less or Equal' },
  { value: 'IN', label: 'In List' },
  { value: 'NOT IN', label: 'Not In List' },
  { value: 'LIKE', label: 'Contains' },
  { value: 'NOT LIKE', label: 'Not Contains' },
];

interface ChartInfo {
  id: number;
  dataset_id: number;
}

interface DatasetColumn {
  table_name: string;
  column_name: string;
  semantic_type?: string;
  is_dimension: boolean;
}

interface DatasetInfo {
  id: number;
  database_name: string;
  columns: DatasetColumn[];
}

interface FilterColumnInfo {
  id: string;
  label: string;
  datasetIds: number[];
  isDateColumn: boolean;
}

/**
 * DashboardFilterBarEnhanced Component
 */
const DashboardFilterBarEnhanced: React.FC = () => {
  const {
    dashboardFilters,
    filterLogic,
    layout,
    addDashboardFilter,
    updateDashboardFilter,
    removeDashboardFilter,
    setFilterLogic,
  } = useDashboard();

  const [isAddingFilter, setIsAddingFilter] = useState(false);
  const [loading, setLoading] = useState(true);
  const [filterColumns, setFilterColumns] = useState<FilterColumnInfo[]>([]);
  const [datasetsInfo, setDatasetsInfo] = useState<Map<number, DatasetInfo>>(new Map());

  /**
   * Get list of chart IDs from the dashboard layout
   */
  const chartIds = useMemo(() => {
    return layout
      .filter((item) => item.type === 'chart' && item.chartId)
      .map((item) => item.chartId!);
  }, [layout]);

  /**
   * Fetch all chart and dataset info when component mounts
   */
  useEffect(() => {
    if (chartIds.length === 0) {
      setLoading(false);
      return;
    }

    const fetchChartDatasets = async () => {
      try {
        setLoading(true);

        // Fetch all charts in parallel
        const chartResponses = await Promise.all(
          chartIds.map((id) => msalFetch(`${API_BASE}/api/v1/charts/${id}`))
        );

        const charts: ChartInfo[] = await Promise.all(
          chartResponses.map((res) => res.json())
        );

        // Extract unique dataset IDs
        const datasetIds = Array.from(new Set(charts.map((c) => c.dataset_id)));

        // Fetch all datasets in parallel
        const datasetResponses = await Promise.all(
          datasetIds.map((id) => msalFetch(`${API_BASE}/api/v1/datasets/${id}/columns`))
        );

        const datasets: DatasetInfo[] = await Promise.all(
          datasetResponses.map(async (res, idx) => {
            const columns = await res.json();
            return {
              id: datasetIds[idx],
              database_name: '',
              columns,
            };
          })
        );

        // Build dataset info map
        const dsMap = new Map<number, DatasetInfo>();
        datasets.forEach((ds) => dsMap.set(ds.id, ds));
        setDatasetsInfo(dsMap);

        // Build merged column list
        const columnMap = new Map<string, FilterColumnInfo>();

        const isDateSemantic = (col: DatasetColumn) => {
          const st = (col.semantic_type || '').toLowerCase();
          return st === 'time' || st.includes('date') || st.includes('time');
        };

        datasets.forEach((ds) => {
          ds.columns
            .filter((col) => col.is_dimension || isDateSemantic(col))
            .forEach((col) => {
              const columnKey = `${col.table_name}.${col.column_name}`;
              const label = col.semantic_type || col.column_name;
              const isDate = isDateSemantic(col);

              if (columnMap.has(columnKey)) {
                const existing = columnMap.get(columnKey)!;
                if (!existing.datasetIds.includes(ds.id)) {
                  existing.datasetIds.push(ds.id);
                }
              } else {
                columnMap.set(columnKey, {
                  id: columnKey,
                  label,
                  datasetIds: [ds.id],
                  isDateColumn: isDate,
                });
              }
            });
        });

        const columns = Array.from(columnMap.values()).sort((a, b) =>
          a.label.localeCompare(b.label)
        );

        setFilterColumns(columns);
        setLoading(false);
      } catch (err) {
        console.error('[DashboardFilter] Error fetching chart datasets:', err);
        setLoading(false);
      }
    };

    fetchChartDatasets();
  }, [chartIds]);

  /**
   * Handle column selection - auto-defaults to "AllUp"
   */
  const handleAddFilter = async (columnId: string) => {
    if (!columnId) return;

    // Get column metadata
    const columnInfo = filterColumns.find((col) => col.id === columnId);
    if (!columnInfo) return;

    // Find a dataset that has this column to get keyColumn
    let keyColumn = '';
    for (const dsId of columnInfo.datasetIds) {
      const ds = datasetsInfo.get(dsId);
      if (ds) {
        const col = ds.columns.find(
          (c) => `${c.table_name}.${c.column_name}` === columnId
        );
        if (col) {
          // Use the table_name.column_name with "Key" suffix for the key column
          // This is a simplified approach - in production you'd query the dataset schema
          keyColumn = `${col.table_name}.${col.column_name.replace(/Name$/, 'Key')}`;
          break;
        }
      }
    }

    // Date columns get a date_range filter; dimension columns get AllUp value filter
    if (columnInfo.isDateColumn) {
      addDashboardFilter({
        column: columnId,
        operator: '=',
        value: '',
        enabled: true,
        appliesTo: 'all',
        options: [],
        isLoading: false,
        isPending: false,
        datasetId: columnInfo.datasetIds[0],
        filterType: 'date_range',
        dateFrom: '',
        dateTo: '',
      });
      setIsAddingFilter(false);
      return;
    }

    // Default value to "AllUp"
    const defaultValue = 'AllUp';

    // Compute valueKey for "AllUp" based on keyColumn type
    let valueKey: string;
    if (keyColumn.includes('IDEASBoolFlag')) {
      valueKey = '-9999'; // AllUp for BoolFlag
    } else if (keyColumn.split('.').pop()?.endsWith('Key')) {
      valueKey = mmh3Hash64(defaultValue);
    } else {
      valueKey = defaultValue;
    }


    // Add the filter
    addDashboardFilter({
      column: columnId,
      operator: '=',
      value: defaultValue,
      valueKey: valueKey,
      keyColumn: keyColumn,
      enabled: true,
      appliesTo: 'all',
      options: [],
      isLoading: false,
      isPending: false,
      datasetId: columnInfo.datasetIds[0],
    });

    setIsAddingFilter(false);
  };

  /**
   * Get formatted label for a filter
   */
  const getFilterLabel = (filter: DashboardFilter): string => {
    if (filter.filterType === 'date_range') {
      const from = filter.dateFrom || '…';
      const to = filter.dateTo || '…';
      return `${filter.column} between ${from} and ${to}`;
    }
    const operatorLabel = FILTER_OPERATORS.find((op) => op.value === filter.operator)?.label || filter.operator;
    return `${filter.column} ${operatorLabel} ${filter.value}`;
  };

  if (loading) {
    return (
      <div className="chart-filter-card">
        <div style={{ padding: '12px', textAlign: 'center', color: '#64748b', fontSize: 13 }}>
          <i className="fas fa-spinner fa-spin" style={{ marginRight: 8 }} />
          Loading filter options...
        </div>
      </div>
    );
  }

  return (
    <div className="chart-filter-card">
      {/* Header with Add Filter button and Logic toggle */}
      <div className="chart-filter-header">
        <button
          type="button"
          className="chart-filter-add-btn chart-filter-add-btn-primary-left"
          onClick={() => setIsAddingFilter(!isAddingFilter)}
        >
          {isAddingFilter ? '− Cancel' : '+ Add filter'}
        </button>

        {dashboardFilters.length > 1 && (
          <div className="chart-filter-header-actions">
            <button
              type="button"
              className="chart-filter-add-btn chart-filter-add-btn-secondary chart-filter-add-btn-right"
              onClick={() => setFilterLogic(filterLogic === 'AND' ? 'OR' : 'AND')}
            >
              {filterLogic}
            </button>
          </div>
        )}
      </div>

      {/* Filter list */}
      {dashboardFilters.length > 0 && (
        <div className="chart-filter-body">
          {dashboardFilters.length > 1 && (
            <div className="chart-filter-logic-row">
              <span className="chart-filter-logic-pill" style={{ cursor: 'default' }}>
                {filterLogic}
              </span>
            </div>
          )}

          <div className="chart-filter-list">
            {dashboardFilters.map((filter) => (
              <div key={filter.id} className="chart-filter-list-item">
                <div className="chart-filter-list-main">
                  {/* Remove button */}
                  <button
                    className="chart-filter-chip-remove"
                    onClick={() => removeDashboardFilter(filter.id)}
                    title="Remove filter"
                  >
                    ×
                  </button>

                  {/* Filter chip */}
                  <button
                    className="chart-filter-chip"
                    style={{ cursor: 'default' }}
                  >
                    <span className="chart-filter-chip-label">{getFilterLabel(filter)}</span>
                  </button>
                </div>
              </div>
            ))}
          </div>
        </div>
      )}

      {/* Add filter dropdown */}
      {isAddingFilter && (
        <div className="chart-filter-body" style={{ marginTop: 8 }}>
          <label className="chart-builder-label" htmlFor="add-filter-select" style={{ fontSize: '0.75rem' }}>
            Select column (will default to "AllUp")
          </label>
          <select
            id="add-filter-select"
            className="chart-builder-select"
            value=""
            onChange={(e) => handleAddFilter(e.target.value)}
            autoFocus
          >
            <option value="">Select a column...</option>
            {filterColumns.map((col) => (
              <option key={col.id} value={col.id}>
                {col.isDateColumn ? '📅 ' : ''}{col.label} ({col.datasetIds.length} dataset{col.datasetIds.length !== 1 ? 's' : ''})
              </option>
            ))}
          </select>
        </div>
      )}

      {/* Empty state */}
      {dashboardFilters.length === 0 && !isAddingFilter && (
        <div style={{ padding: '12px', textAlign: 'center', color: '#94a3b8', fontSize: 13 }}>
          No filters added. Click "+ Add filter" to get started.
        </div>
      )}
    </div>
  );
};

export default DashboardFilterBarEnhanced;
