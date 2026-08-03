/**
 * Dashboard Filter Bar Component (Read-Only View)
 *
 * Provides a read-only UI for viewing and changing filter values.
 * Users can change filter values but cannot add or remove filters.
 * When a filter is opened for editing, real values are fetched from the
 * distinct-filter-values API and shown as a dropdown.
 */

import React, { useState, useEffect, useRef } from 'react';
import { useDashboard } from './DashboardContext';
import { msalFetch } from '../../utils/msalFetch';
import { API_BASE } from '../../config';
import { mmh3Hash64 } from '../../utils/mmh3';
import type { DashboardFilter, FilterOperator } from '../../types/dashboard';

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

interface FilterOption {
  key: string;
  value: string;
}

/** Cache of fetched options per column string, shared across renders */
const optionsCache: Record<string, { options: FilterOption[]; keyColumn: string | null }> = {};

const DashboardFilterBarReadOnly: React.FC = () => {
  const {
    dashboardFilters,
    filterLogic,
    layout,
    updateDashboardFilter,
  } = useDashboard();

  const [editingFilterId, setEditingFilterId] = useState<string | null>(null);
  const [editValue, setEditValue] = useState<string>('');
  const [editValueKey, setEditValueKey] = useState<string>('');
  const [editDateFrom, setEditDateFrom] = useState<string>('');
  const [editDateTo, setEditDateTo] = useState<string>('');

  // Per-column options state (loaded on demand)
  const [colOptions, setColOptions] = useState<Record<string, FilterOption[]>>({});
  const [colKeyColumn, setColKeyColumn] = useState<Record<string, string | null>>({});
  const [loadingCol, setLoadingCol] = useState<string | null>(null);

  // column → dataset_id mapping, built once on mount from layout charts
  const [colDatasetMap, setColDatasetMap] = useState<Record<string, number>>({});
  const colDatasetMapRef = useRef(colDatasetMap);
  colDatasetMapRef.current = colDatasetMap;

  // Build column → dataset_id mapping from the dashboard's charts
  useEffect(() => {
    const chartIds = layout
      .filter((item) => item.type === 'chart' && item.chartId)
      .map((item) => item.chartId!);
    if (!chartIds.length) return;

    (async () => {
      try {
        const chartResps = await Promise.all(
          chartIds.map((id) => msalFetch(`${API_BASE}/api/v1/charts/${id}`))
        );
        const charts: { id: number; dataset_id: number }[] = await Promise.all(
          chartResps.map((r) => r.json())
        );
        const datasetIds = Array.from(new Set(charts.map((c) => c.dataset_id)));
        const colResps = await Promise.all(
          datasetIds.map((id) => msalFetch(`${API_BASE}/api/v1/datasets/${id}/columns`))
        );
        const map: Record<string, number> = {};
        await Promise.all(
          colResps.map(async (resp, idx) => {
            const cols: { table_name: string; column_name: string }[] = await resp.json();
            cols.forEach((col) => {
              const key = `${col.table_name}.${col.column_name}`;
              if (!(key in map)) map[key] = datasetIds[idx];
            });
          })
        );
        setColDatasetMap(map);
      } catch {
        // Non-fatal — fallback to text input
      }
    })();
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [layout.length]);

  /** Resolve dataset_id for a filter: use stored datasetId first, then the map */
  const getDatasetId = (filter: DashboardFilter): number | null => {
    if (filter.datasetId) return filter.datasetId;
    return colDatasetMapRef.current[filter.column] ?? null;
  };

  /** Fetch distinct values for the given filter's column, unless already cached */
  const loadOptions = async (filter: DashboardFilter) => {
    const col = filter.column;
    if (colOptions[col] !== undefined || loadingCol === col) return;
    if (col in optionsCache) {
      const cached = optionsCache[col];
      setColOptions((prev) => ({ ...prev, [col]: cached.options }));
      setColKeyColumn((prev) => ({ ...prev, [col]: cached.keyColumn }));
      return;
    }

    const datasetId = getDatasetId(filter);
    if (!datasetId) return;

    setLoadingCol(col);
    try {
      const params = new URLSearchParams({
        dataset_id: String(datasetId),
        column: col,
        limit: '100',
        source: 'dashboard-filter',
      });
      const res = await msalFetch(`${API_BASE}/api/v1/sql/distinct-filter-values?${params}`);
      const data = await res.json();
      if (!res.ok || !data.success) throw new Error(data.error || 'Failed');

      const values: FilterOption[] = Array.isArray(data.values)
        ? data.values
            .map((v: any) => {
              if (v == null) return null;
              if (typeof v === 'object' && 'key' in v && 'value' in v) {
                return { key: String(v.key ?? ''), value: String(v.value ?? '') };
              }
              const s = String(v);
              return { key: s, value: s };
            })
            .filter((x: FilterOption | null): x is FilterOption => x !== null && x.value !== '')
        : [];

      const keyCol: string | null = data.keyColumn || null;
      optionsCache[col] = { options: values, keyColumn: keyCol };
      setColOptions((prev) => ({ ...prev, [col]: values }));
      setColKeyColumn((prev) => ({ ...prev, [col]: keyCol }));
    } catch {
      optionsCache[col] = { options: [], keyColumn: null };
      setColOptions((prev) => ({ ...prev, [col]: [] }));
    } finally {
      setLoadingCol(null);
    }
  };

  const computeValueKey = (
    selectedValue: string,
    keyColumn: string | null,
    serverKey?: string,
  ): string => {
    const kc = keyColumn || '';
    const kcName = kc.split('.').pop() || '';
    if (kc.includes('IDEASBoolFlag')) {
      const map: Record<string, string> = {
        False: '-715022825', True: '888000370', AllUp: '-9999', Unknown: '-1',
      };
      return map[selectedValue] ?? selectedValue;
    }
    if (serverKey && serverKey !== selectedValue) return serverKey;
    if (kcName.endsWith('Key')) return mmh3Hash64(selectedValue);
    return selectedValue;
  };

  const handleFilterClick = (filter: DashboardFilter) => {
    if (!filter.enabled) return;
    setEditingFilterId(filter.id);
    if (filter.filterType === 'date_range') {
      setEditDateFrom(filter.dateFrom ?? '');
      setEditDateTo(filter.dateTo ?? '');
    } else {
      setEditValue(filter.value);
      setEditValueKey(filter.valueKey ?? filter.value);
      loadOptions(filter);
    }
  };

  const handleSaveValue = (filterId: string) => {
    const filter = dashboardFilters.find((f) => f.id === filterId);
    if (!filter) return;
    if (filter.filterType === 'date_range') {
      updateDashboardFilter(filterId, { dateFrom: editDateFrom, dateTo: editDateTo, value: `${editDateFrom} – ${editDateTo}`, enabled: true });
    } else {
      if (!editValue.trim()) return;
      // Applying a value auto-enables the filter so it takes effect immediately.
      updateDashboardFilter(filterId, { value: editValue, valueKey: editValueKey, enabled: true });
    }
    setEditingFilterId(null);
    setEditValue('');
    setEditValueKey('');
    setEditDateFrom('');
    setEditDateTo('');
  };

  const handleCancelEdit = () => {
    setEditingFilterId(null);
    setEditValue('');
    setEditValueKey('');
    setEditDateFrom('');
    setEditDateTo('');
  };

  const handleToggleFilter = (filterId: string) => {
    const filter = dashboardFilters.find((f) => f.id === filterId);
    if (filter) updateDashboardFilter(filterId, { enabled: !filter.enabled });
  };

  const getFilterLabel = (filter: DashboardFilter): string => {
    const name = filter.label?.trim() || filter.column.split('.').pop() || filter.column;
    if (filter.filterType === 'date_range') {
      const from = filter.dateFrom || '…';
      const to = filter.dateTo || '…';
      return `${name}: ${from} – ${to}`;
    }
    const value = filter.value?.trim();
    if (!value) return name;              // "Country" (unset) rather than "…country Equals "
    const op = FILTER_OPERATORS.find((o) => o.value === filter.operator)?.label ?? filter.operator;
    return `${name} ${op} ${value}`;
  };

  if (dashboardFilters.length === 0) {
    return (
      <div className="chart-filter-card">
        <div style={{ padding: '12px', textAlign: 'center', color: '#94a3b8', fontSize: 13 }}>
          No filters available
        </div>
      </div>
    );
  }

  const editingFilter = dashboardFilters.find((f) => f.id === editingFilterId) ?? null;
  const editingOptions = editingFilter ? (colOptions[editingFilter.column] ?? null) : null;
  const editingKeyCol = editingFilter ? (colKeyColumn[editingFilter.column] ?? null) : null;
  const isEditingLoading = editingFilter && loadingCol === editingFilter.column;

  return (
    <div className="chart-filter-card">
      {dashboardFilters.length > 1 && (
        <div className="chart-filter-body">
          <div className="chart-filter-logic-row">
            <span className="chart-filter-logic-pill" style={{ cursor: 'default' }}>
              {filterLogic}
            </span>
          </div>
        </div>
      )}

      <div className="chart-filter-body">
        <div className="chart-filter-list">
          {dashboardFilters.map((filter) => {
            const isEditing = editingFilterId === filter.id;
            const isDisabled = !filter.enabled;

            return (
              <div key={filter.id} className="chart-filter-list-item" style={{ opacity: isDisabled ? 0.5 : 1 }}>
                <div className="chart-filter-list-main">
                  <button
                    className="chart-filter-chip-remove"
                    onClick={() => handleToggleFilter(filter.id)}
                    title={filter.enabled ? 'Disable filter' : 'Enable filter'}
                    style={{ fontSize: '1rem' }}
                  >
                    <i
                      className={filter.enabled ? 'fas fa-check-circle' : 'far fa-circle'}
                      style={{ color: filter.enabled ? '#10b981' : '#9ca3af' }}
                    />
                  </button>

                  <button
                    className="chart-filter-chip"
                    onClick={() => !isEditing && handleFilterClick(filter)}
                    disabled={isDisabled}
                    style={{ cursor: isDisabled ? 'default' : 'pointer' }}
                  >
                    <span className="chart-filter-chip-label">{getFilterLabel(filter)}</span>
                    {!isDisabled && <span className="chart-filter-chip-arrow">›</span>}
                  </button>
                </div>

                {isEditing && <div className="chart-filter-chip-active-indicator" />}
              </div>
            );
          })}
        </div>
      </div>

      {editingFilter && (
        <div className="chart-filter-popover">
          <div className="chart-filter-popover-header">Edit filter value</div>

          <div style={{ marginTop: '0.5rem' }}>
            {editingFilter.filterType === 'date_range' ? (
              <>
                <label className="chart-builder-label" style={{ fontSize: '0.75rem' }}>From</label>
                <input
                  type="date"
                  className="chart-builder-input"
                  value={editDateFrom}
                  onChange={(e) => setEditDateFrom(e.target.value)}
                  autoFocus
                />
                <label className="chart-builder-label" style={{ fontSize: '0.75rem', marginTop: 6 }}>To</label>
                <input
                  type="date"
                  className="chart-builder-input"
                  value={editDateTo}
                  onChange={(e) => setEditDateTo(e.target.value)}
                />
              </>
            ) : (
              <>
                <label className="chart-builder-label" htmlFor="filter-value-input" style={{ fontSize: '0.75rem' }}>
                  Value
                </label>
                {isEditingLoading ? (
                  <div className="chart-filter-value-field">
                    <select className="chart-builder-select" disabled>
                      <option>{editValue ? `${editValue} (loading…)` : 'Loading values…'}</option>
                    </select>
                    <div className="chart-filter-spinner" aria-label="Loading values" />
                  </div>
                ) : editingOptions && editingOptions.length > 0 ? (
                  <select
                    id="filter-value-input"
                    className="chart-builder-select"
                    value={editValue}
                    onChange={(e) => {
                      const sel = e.target.value;
                      const matched = editingOptions.find((o) => o.value === sel);
                      setEditValue(sel);
                      setEditValueKey(computeValueKey(sel, editingKeyCol, matched?.key));
                    }}
                    autoFocus
                  >
                    <option value="">Select value…</option>
                    {editingOptions.map((opt, i) => (
                      <option key={opt.key || `opt-${i}`} value={opt.value}>
                        {opt.value}
                      </option>
                    ))}
                  </select>
                ) : (
                  <input
                    id="filter-value-input"
                    type="text"
                    className="chart-builder-input"
                    value={editValue}
                    onChange={(e) => {
                      const v = e.target.value;
                      setEditValue(v);
                      setEditValueKey(computeValueKey(v, editingKeyCol));
                    }}
                    onKeyDown={(e) => {
                      if (e.key === 'Enter') handleSaveValue(editingFilter.id);
                      else if (e.key === 'Escape') handleCancelEdit();
                    }}
                    autoFocus
                  />
                )}
              </>
            )}
          </div>

          <div className="chart-filter-popover-actions">
            <button
              type="button"
              className="chart-save-modal-btn chart-save-modal-btn-secondary"
              onClick={handleCancelEdit}
            >
              Cancel
            </button>
            <button
              type="button"
              className="chart-save-modal-btn chart-save-modal-btn-primary"
              onClick={() => handleSaveValue(editingFilter.id)}
            >
              Apply
            </button>
          </div>
        </div>
      )}
    </div>
  );
};

export default DashboardFilterBarReadOnly;
