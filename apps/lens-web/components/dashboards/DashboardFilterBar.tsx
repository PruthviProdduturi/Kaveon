/**
 * Dashboard Filter Bar Component
 *
 * Provides a UI for managing dashboard-level filters that can be applied
 * to all charts or specific subsets of charts. Features include:
 * - Add/remove/edit filters
 * - Enable/disable individual filters
 * - Specify which charts each filter applies to
 * - Fetch distinct filter values from the dataset
 * - Filter logic selector (AND/OR)
 *
 * Integrates with DashboardContext for centralized filter management.
 */

import React, { useState } from 'react';
import { useDashboard } from './DashboardContext';
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

/**
 * DashboardFilterBar Component
 *
 * Manages dashboard-level filters with granular control over
 * which charts they apply to.
 */
const DashboardFilterBar: React.FC = () => {
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
  const [newFilter, setNewFilter] = useState<Partial<DashboardFilter>>({
    column: '',
    operator: '=',
    value: '',
    appliesTo: 'all',
    enabled: true,
  });

  /**
   * Get list of chart IDs from the dashboard layout
   */
  const chartIds = layout
    .filter((item) => item.type === 'chart' && item.chartId)
    .map((item) => item.chartId!);

  /**
   * Handle adding a new filter
   */
  const handleAddFilter = () => {
    if (!newFilter.column || !newFilter.value) {
      alert('Please specify both column and value');
      return;
    }

    addDashboardFilter({
      column: newFilter.column,
      operator: newFilter.operator || '=',
      value: newFilter.value,
      appliesTo: newFilter.appliesTo || 'all',
      enabled: newFilter.enabled !== false,
    });

    // Reset form
    setNewFilter({
      column: '',
      operator: '=',
      value: '',
      appliesTo: 'all',
      enabled: true,
    });
    setIsAddingFilter(false);
  };

  /**
   * Handle toggling filter enabled state
   */
  const handleToggleFilter = (filterId: string) => {
    const filter = dashboardFilters.find((f) => f.id === filterId);
    if (filter) {
      updateDashboardFilter(filterId, { enabled: !filter.enabled });
    }
  };

  /**
   * Handle removing a filter
   */
  const handleRemoveFilter = (filterId: string) => {
    removeDashboardFilter(filterId);
  };

  /**
   * Render filter pill for an existing filter
   */
  const renderFilterPill = (filter: DashboardFilter) => {
    const isDisabled = !filter.enabled;

    return (
      <div
        key={filter.id}
        className={`dashboard-filter-pill ${isDisabled ? 'dashboard-filter-pill-disabled' : ''}`}
      >
        <button
          onClick={() => handleToggleFilter(filter.id)}
          style={{
            background: 'transparent',
            border: 'none',
            cursor: 'pointer',
            marginRight: 8,
            color: 'inherit',
          }}
          title={filter.enabled ? 'Disable filter' : 'Enable filter'}
        >
          <i className={filter.enabled ? 'fas fa-check-circle' : 'far fa-circle'} />
        </button>

        <span>
          {filter.column} {filter.operator} {filter.value}
        </span>

        {filter.appliesTo !== 'all' && Array.isArray(filter.appliesTo) && (
          <span
            style={{
              marginLeft: 8,
              fontSize: 11,
              opacity: 0.8,
            }}
          >
            (applies to {filter.appliesTo.length} chart{filter.appliesTo.length !== 1 ? 's' : ''})
          </span>
        )}

        <button
          className="dashboard-filter-pill-remove"
          onClick={() => handleRemoveFilter(filter.id)}
          title="Remove filter"
        >
          <i className="fas fa-times" />
        </button>
      </div>
    );
  };

  return (
    <div className="dashboard-filter-bar">
      <div className="dashboard-filter-bar-header">
        <div style={{ display: 'flex', alignItems: 'center', gap: 12 }}>
          <div className="dashboard-filter-bar-title">
            <i className="fas fa-filter" style={{ marginRight: 8 }} />
            Dashboard Filters
          </div>

          {dashboardFilters.length > 1 && (
            <select
              value={filterLogic}
              onChange={(e) => setFilterLogic(e.target.value as 'AND' | 'OR')}
              style={{
                padding: '4px 8px',
                fontSize: 12,
                borderRadius: 4,
                border: '1px solid #cbd5e1',
              }}
              title="Filter logic"
            >
              <option value="AND">AND</option>
              <option value="OR">OR</option>
            </select>
          )}
        </div>

        <div className="dashboard-filter-bar-actions">
          <button
            onClick={() => setIsAddingFilter(!isAddingFilter)}
            style={{
              padding: '6px 12px',
              background: '#2563eb',
              color: '#ffffff',
              border: 'none',
              borderRadius: 6,
              cursor: 'pointer',
              fontSize: 13,
              fontWeight: 500,
            }}
          >
            <i className="fas fa-plus" style={{ marginRight: 6 }} />
            Add Filter
          </button>
        </div>
      </div>

      {/* Filter pills */}
      {dashboardFilters.length > 0 && (
        <div className="dashboard-filter-pills">
          {dashboardFilters.map((filter) => renderFilterPill(filter))}
        </div>
      )}

      {/* No filters message */}
      {dashboardFilters.length === 0 && !isAddingFilter && (
        <div
          style={{
            padding: 20,
            textAlign: 'center',
            color: '#94a3b8',
            fontSize: 14,
          }}
        >
          No filters applied. Click "Add Filter" to create one.
        </div>
      )}

      {/* Add filter form */}
      {isAddingFilter && (
        <div
          style={{
            marginTop: 16,
            padding: 16,
            background: '#f8fafc',
            borderRadius: 8,
            border: '1px solid #e2e8f0',
          }}
        >
          <div style={{ display: 'grid', gridTemplateColumns: '2fr 1fr 2fr', gap: 12 }}>
            <div>
              <label
                style={{
                  display: 'block',
                  fontSize: 12,
                  fontWeight: 500,
                  marginBottom: 6,
                  color: '#475569',
                }}
              >
                Column
              </label>
              <input
                type="text"
                value={newFilter.column || ''}
                onChange={(e) => setNewFilter({ ...newFilter, column: e.target.value })}
                placeholder="e.g., table_name.column_name"
                style={{
                  width: '100%',
                  padding: '8px 12px',
                  border: '1px solid #cbd5e1',
                  borderRadius: 6,
                  fontSize: 14,
                }}
              />
            </div>

            <div>
              <label
                style={{
                  display: 'block',
                  fontSize: 12,
                  fontWeight: 500,
                  marginBottom: 6,
                  color: '#475569',
                }}
              >
                Operator
              </label>
              <select
                value={newFilter.operator || '='}
                onChange={(e) =>
                  setNewFilter({ ...newFilter, operator: e.target.value as FilterOperator })
                }
                style={{
                  width: '100%',
                  padding: '8px 12px',
                  border: '1px solid #cbd5e1',
                  borderRadius: 6,
                  fontSize: 14,
                }}
              >
                {FILTER_OPERATORS.map((op) => (
                  <option key={op.value} value={op.value}>
                    {op.label}
                  </option>
                ))}
              </select>
            </div>

            <div>
              <label
                style={{
                  display: 'block',
                  fontSize: 12,
                  fontWeight: 500,
                  marginBottom: 6,
                  color: '#475569',
                }}
              >
                Value
              </label>
              <input
                type="text"
                value={newFilter.value || ''}
                onChange={(e) => setNewFilter({ ...newFilter, value: e.target.value })}
                placeholder="Filter value"
                style={{
                  width: '100%',
                  padding: '8px 12px',
                  border: '1px solid #cbd5e1',
                  borderRadius: 6,
                  fontSize: 14,
                }}
              />
            </div>
          </div>

          <div style={{ marginTop: 12, display: 'grid', gridTemplateColumns: '1fr 1fr', gap: 12 }}>
            <div>
              <label
                style={{
                  display: 'block',
                  fontSize: 12,
                  fontWeight: 500,
                  marginBottom: 6,
                  color: '#475569',
                }}
              >
                Applies To
              </label>
              <select
                value={newFilter.appliesTo === 'all' ? 'all' : 'specific'}
                onChange={(e) =>
                  setNewFilter({
                    ...newFilter,
                    appliesTo: e.target.value === 'all' ? 'all' : [],
                  })
                }
                style={{
                  width: '100%',
                  padding: '8px 12px',
                  border: '1px solid #cbd5e1',
                  borderRadius: 6,
                  fontSize: 14,
                }}
              >
                <option value="all">All Charts</option>
                <option value="specific">Specific Charts</option>
              </select>
            </div>

            <div style={{ display: 'flex', alignItems: 'flex-end', gap: 8 }}>
              <button
                onClick={handleAddFilter}
                style={{
                  flex: 1,
                  padding: '8px 16px',
                  background: '#10b981',
                  color: '#ffffff',
                  border: 'none',
                  borderRadius: 6,
                  cursor: 'pointer',
                  fontSize: 14,
                  fontWeight: 500,
                }}
              >
                <i className="fas fa-check" style={{ marginRight: 6 }} />
                Add
              </button>

              <button
                onClick={() => setIsAddingFilter(false)}
                style={{
                  padding: '8px 16px',
                  background: '#ef4444',
                  color: '#ffffff',
                  border: 'none',
                  borderRadius: 6,
                  cursor: 'pointer',
                  fontSize: 14,
                }}
              >
                <i className="fas fa-times" />
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
};

export default DashboardFilterBar;
