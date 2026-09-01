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

interface DashboardFilterBarReadOnlyProps {
  /** When provided, the bar reads/writes this filter set instead of the global
   *  dashboard filters — used by the full-screen sandbox so edits stay local. */
  filtersOverride?: DashboardFilter[];
  onUpdateFilter?: (filterId: string, updates: Partial<DashboardFilter>) => void;
}

const DashboardFilterBarReadOnly: React.FC<DashboardFilterBarReadOnlyProps> = ({ filtersOverride, onUpdateFilter }) => {
  const ctx = useDashboard();
  const { filterLogic, layout } = ctx;
  const dashboardFilters = filtersOverride ?? ctx.dashboardFilters;
  const updateDashboardFilter = onUpdateFilter ?? ctx.updateDashboardFilter;

  const [editingFilterId, setEditingFilterId] = useState<string | null>(null);
  const [editValue, setEditValue] = useState<string>('');
  const [editValueKey, setEditValueKey] = useState<string>('');
  const [editValues, setEditValues] = useState<string[]>([]);   // multi-select selection
  const [optSearch, setOptSearch] = useState<string>('');       // dropdown search box
  const [editDateFrom, setEditDateFrom] = useState<string>('');
  const [editDateTo, setEditDateTo] = useState<string>('');
  const [popoverPos, setPopoverPos] = useState<{ left: number; top: number } | null>(null);

  // Per-column options state (loaded on demand)
  const [colOptions, setColOptions] = useState<Record<string, FilterOption[]>>({});
  const [colKeyColumn, setColKeyColumn] = useState<Record<string, string | null>>({});
  const [loadingCol, setLoadingCol] = useState<string | null>(null);

  const popoverRef = useRef<HTMLDivElement>(null);
  const filterBarRef = useRef<HTMLDivElement>(null);

  // column → dataset_id mapping, built once on mount from layout charts
  const [colDatasetMap, setColDatasetMap] = useState<Record<string, number>>({});
  const colDatasetMapRef = useRef(colDatasetMap);
  colDatasetMapRef.current = colDatasetMap;

  // Narrowing signature each column's options were last loaded under, so cached
  // options are refetched when a parent (cascading) selection changes.
  const colNarrowSigRef = useRef<Record<string, string>>({});

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

  useEffect(() => {
    if (!editingFilterId) return;
    const handler = (e: MouseEvent) => {
      if (popoverRef.current?.contains(e.target as Node)) return;
      if (filterBarRef.current?.contains(e.target as Node)) return;
      setEditingFilterId(null);
      setEditValue(''); setEditValueKey(''); setEditValues([]); setOptSearch('');
      setEditDateFrom(''); setEditDateTo('');
    };
    document.addEventListener('mousedown', handler);
    return () => document.removeEventListener('mousedown', handler);
  }, [editingFilterId]);

  /** Resolve dataset_id for a filter: use stored datasetId first, then the map */
  const getDatasetId = (filter: DashboardFilter): number | null => {
    if (filter.datasetId) return filter.datasetId;
    return colDatasetMapRef.current[filter.column] ?? null;
  };

  /** Sibling filters (same dataset, different column, with a value) that narrow
   *  this filter's option list — the cascading-filter inputs. */
  const getNarrowFilters = (target: DashboardFilter) => {
    const tds = getDatasetId(target);
    if (!tds) return [] as { column: string; operator: string; value: string; valueKey?: string; keyColumn?: string }[];
    return dashboardFilters
      .filter((f) =>
        f.id !== target.id &&
        f.enabled &&
        f.filterType !== 'date_range' &&
        String(f.value ?? '').trim() !== '' &&
        getDatasetId(f) === tds
      )
      .map((f) => ({
        column: f.column,
        operator: f.operator,
        value: f.value,
        valueKey: f.valueKey,
        keyColumn: (f as any).keyColumn,
      }));
  };

  /** Stable signature of the narrowing context, so cached options refetch when a
   *  parent selection changes. */
  const narrowSig = (narrow: { column: string; operator: string; value: string }[]) =>
    JSON.stringify(narrow.map((n) => [n.column, n.operator, n.value]).sort());

  /** Fetch distinct values for the given filter's column under the CURRENT
   *  narrowing context, unless the right (context-matched) options are loaded. */
  const loadOptions = async (filter: DashboardFilter) => {
    const col = filter.column;
    const narrow = getNarrowFilters(filter);
    const sig = narrowSig(narrow);
    if (colOptions[col] !== undefined && colNarrowSigRef.current[col] === sig) return;
    if (loadingCol === col) return;

    const cacheKey = `${col}|${sig}`;
    if (cacheKey in optionsCache) {
      const cached = optionsCache[cacheKey];
      colNarrowSigRef.current[col] = sig;
      setColOptions((prev) => ({ ...prev, [col]: cached.options }));
      setColKeyColumn((prev) => ({ ...prev, [col]: cached.keyColumn }));
      return;
    }

    const datasetId = getDatasetId(filter);
    if (!datasetId) return;

    setLoadingCol(col);
    try {
      const dlmParams = new URLSearchParams({
        dataset_id: String(datasetId),
        column: col,
        limit: '200',
      });
      const dlmRes = await msalFetch(`${API_BASE}/api/v1/dlm/filter-values?${dlmParams}`);
      const dlmData = await dlmRes.json();
      if (dlmRes.ok && dlmData.ok && Array.isArray(dlmData.values) && dlmData.values.length > 0) {
        const values: FilterOption[] = dlmData.values
          .map((v: any) => ({ key: String(v.key ?? ''), value: String(v.value ?? '') }))
          .filter((x: FilterOption) => x.value !== '');
        optionsCache[cacheKey] = { options: values, keyColumn: null };
        colNarrowSigRef.current[col] = sig;
        setColOptions((prev) => ({ ...prev, [col]: values }));
        setColKeyColumn((prev) => ({ ...prev, [col]: null }));
        return;
      }

      const params = new URLSearchParams({
        dataset_id: String(datasetId),
        column: col,
        limit: '100',
        source: 'dashboard-filter',
      });
      if (narrow.length) params.set('filters', JSON.stringify(narrow));
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
      optionsCache[cacheKey] = { options: values, keyColumn: keyCol };
      colNarrowSigRef.current[col] = sig;
      setColOptions((prev) => ({ ...prev, [col]: values }));
      setColKeyColumn((prev) => ({ ...prev, [col]: keyCol }));
    } catch {
      optionsCache[cacheKey] = { options: [], keyColumn: null };
      colNarrowSigRef.current[col] = sig;
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

  const handleFilterClick = (filter: DashboardFilter, event?: React.MouseEvent) => {
    if (!filter.enabled) return;
    if (event) {
      const chipEl = event.currentTarget as HTMLElement;
      const barRect = filterBarRef.current?.getBoundingClientRect();
      if (barRect) {
        const chipRect = chipEl.getBoundingClientRect();
        const rawLeft = chipRect.left - barRect.left;
        const maxLeft = Math.max(0, barRect.width - 320);
        setPopoverPos({
          left: Math.max(0, Math.min(rawLeft, maxLeft)),
          top: chipRect.bottom - barRect.top + 4,
        });
      }
    }
    setEditingFilterId(filter.id);
    if (filter.filterType === 'date_range') {
      setEditDateFrom(filter.dateFrom ?? '');
      setEditDateTo(filter.dateTo ?? '');
    } else {
      setEditValue(filter.value);
      setEditValueKey(filter.valueKey ?? filter.value);
      const vals = filter.value ? filter.value.split(',').map((s) => s.trim()).filter(Boolean) : [];
      setEditValues(vals.filter((v) => v !== 'AllUp'));
      setOptSearch('');
      loadOptions(filter);
    }
  };

  const toggleEditValue = (v: string) => {
    setEditValues((prev) => {
      if (prev.includes(v)) return prev.filter((x) => x !== v);
      if (v === 'AllUp') return [v];
      return [...prev.filter((x) => x !== 'AllUp'), v];
    });
  };

  const handleSaveValue = (filterId: string) => {
    const filter = dashboardFilters.find((f) => f.id === filterId);
    if (!filter) return;
    if (filter.filterType === 'date_range') {
      updateDashboardFilter(filterId, { dateFrom: editDateFrom, dateTo: editDateTo, value: `${editDateFrom} – ${editDateTo}`, enabled: true });
    } else {
      const vals = editValues.filter(Boolean);
      if (!vals.length) {
        updateDashboardFilter(filterId, { value: 'AllUp', operator: '=', enabled: true });
      } else {
        updateDashboardFilter(filterId, {
          value: vals.join(', '),
          operator: vals.length > 1 ? 'IN' : '=',
          enabled: true,
        });
      }
    }
    resetEdit();
  };

  const resetEdit = () => {
    setEditingFilterId(null);
    setEditValue(''); setEditValueKey(''); setEditValues([]); setOptSearch('');
    setEditDateFrom(''); setEditDateTo('');
    setPopoverPos(null);
  };

  const handleCancelEdit = () => resetEdit();

  const handleToggleFilter = (filterId: string) => {
    const filter = dashboardFilters.find((f) => f.id === filterId);
    if (filter) updateDashboardFilter(filterId, { enabled: !filter.enabled });
  };

  const clearAll = () => {
    dashboardFilters.forEach((f) => {
      if (f.filterType === 'date_range') {
        updateDashboardFilter(f.id, { value: '', valueKey: '', dateFrom: '', dateTo: '' });
      } else {
        updateDashboardFilter(f.id, { value: 'AllUp', valueKey: '', operator: '=' });
      }
    });
    resetEdit();
  };

  const activeCount = dashboardFilters.filter((f) => f.enabled && String(f.value ?? '').trim()).length;

  const getFilterLabel = (filter: DashboardFilter): string => {
    const name = filter.label?.trim() || filter.column.split('.').pop() || filter.column;
    if (filter.filterType === 'date_range') {
      const from = filter.dateFrom || '…';
      const to = filter.dateTo || '…';
      return `${name}: ${from} – ${to}`;
    }
    const value = filter.value?.trim();
    if (!value) return name;              // "Country" (unset) rather than "…country Equals "
    const parts = value.split(',').map((s) => s.trim()).filter(Boolean);
    if (parts.length > 1) return `${name}: ${parts.length} selected`;
    return `${name}: ${parts[0]}`;
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
    <div className="chart-filter-card" ref={filterBarRef}>
      <div className="chart-filter-body">
        <div className="chart-filter-list" style={{ display: 'flex', flexDirection: 'row', flexWrap: 'wrap', gap: 8, alignItems: 'center', justifyContent: 'flex-start' }}>
          {dashboardFilters.map((filter) => {
            const isEditing = editingFilterId === filter.id;
            const isDisabled = !filter.enabled;

            return (
              <div key={filter.id} className="chart-filter-list-item" style={{ opacity: isDisabled ? 0.5 : 1, width: 'auto', flex: '0 0 auto' }}>
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
                    onClick={(e) => !isEditing && handleFilterClick(filter, e)}
                    disabled={isDisabled}
                    style={{ cursor: isDisabled ? 'default' : 'pointer' }}
                  >
                    <span className="chart-filter-chip-label">{getFilterLabel(filter)}</span>
                    {!isDisabled && <i className="fas fa-chevron-down" style={{ fontSize: 9, opacity: 0.5, marginLeft: 2 }} />}
                  </button>
                </div>

                {isEditing && <div className="chart-filter-chip-active-indicator" />}
              </div>
            );
          })}
          {activeCount > 0 && (
            <button type="button" onClick={clearAll}
              style={{ background: 'none', border: 'none', color: 'var(--text-muted)', cursor: 'pointer', fontSize: 12, display: 'flex', alignItems: 'center', gap: 4, padding: '4px 8px', flexShrink: 0 }}>
              <i className="fas fa-times" style={{ fontSize: 10 }} /> Clear
            </button>
          )}
        </div>
      </div>

      {editingFilter && (
        <div className="chart-filter-popover" ref={popoverRef}
          style={popoverPos ? { left: popoverPos.left, top: popoverPos.top } : undefined}>
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
                <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', marginBottom: 6 }}>
                  <label className="chart-builder-label" style={{ fontSize: '0.75rem', margin: 0 }}>
                    Values {editValues.length > 0 && <span style={{ color: '#2563eb', fontWeight: 600 }}>· {editValues.length}</span>}
                  </label>
                  {editValues.length > 0 && (
                    <button type="button" onClick={() => setEditValues([])}
                      style={{ background: 'none', border: 'none', color: '#64748b', cursor: 'pointer', fontSize: 11 }}>
                      Clear
                    </button>
                  )}
                </div>
                {isEditingLoading ? (
                  <div className="chart-filter-value-field">
                    <div className="chart-filter-spinner" aria-label="Loading values" /> Loading values…
                  </div>
                ) : editingOptions && editingOptions.length > 0 ? (
                  <>
                    <input
                      type="text"
                      className="chart-builder-input"
                      placeholder="Search…"
                      value={optSearch}
                      onChange={(e) => setOptSearch(e.target.value)}
                      autoFocus
                      style={{ marginBottom: 6 }}
                    />
                    <div style={{ maxHeight: 220, overflowY: 'auto', border: '1px solid var(--border)', borderRadius: 6 }}>
                      {editingOptions
                        .filter((opt) => !optSearch || opt.value.toLowerCase().includes(optSearch.toLowerCase()))
                        .slice(0, 300)
                        .map((opt, i) => {
                          const checked = editValues.includes(opt.value);
                          return (
                            <label key={opt.key || `opt-${i}`}
                              style={{ display: 'flex', alignItems: 'center', gap: 8, padding: '6px 10px', cursor: 'pointer',
                                       fontSize: 13, color: 'var(--text-primary)',
                                       background: checked ? 'rgba(37,99,235,0.14)' : 'transparent' }}
                              onMouseOver={(e) => { if (!checked) e.currentTarget.style.background = 'var(--bg-hover)'; }}
                              onMouseOut={(e) => { if (!checked) e.currentTarget.style.background = 'transparent'; }}>
                              <input type="checkbox" checked={checked} onChange={() => toggleEditValue(opt.value)} style={{ accentColor: 'var(--accent)' }} />
                              <span style={{ overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>{opt.value}</span>
                            </label>
                          );
                        })}
                      {editingOptions.filter((opt) => !optSearch || opt.value.toLowerCase().includes(optSearch.toLowerCase())).length === 0 && (
                        <div style={{ padding: 10, color: 'var(--text-muted)', fontSize: 12 }}>No matches</div>
                      )}
                    </div>
                  </>
                ) : (
                  <input
                    id="filter-value-input"
                    type="text"
                    className="chart-builder-input"
                    placeholder="Type a value and press Enter"
                    value={editValues[0] ?? ''}
                    onChange={(e) => setEditValues(e.target.value ? [e.target.value] : [])}
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
