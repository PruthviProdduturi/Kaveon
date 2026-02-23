/**
 * Dashboard Filter Bar Component (Read-Only View)
 *
 * Provides a read-only UI for viewing and changing filter values
 * Users can change filter values but cannot add or remove filters
 * Uses exact same styling as ChartBuilder FilterBuilder component
 */

import React, { useState } from 'react';
import { useDashboard } from './DashboardContext';
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

/**
 * DashboardFilterBarReadOnly Component
 *
 * Read-only version for view mode - users can change values but not add/remove filters
 * Styled exactly like chart Pre-filters
 */
const DashboardFilterBarReadOnly: React.FC = () => {
  const {
    dashboardFilters,
    filterLogic,
    updateDashboardFilter,
  } = useDashboard();

  const [editingFilterId, setEditingFilterId] = useState<string | null>(null);
  const [editValue, setEditValue] = useState<string>('');

  /**
   * Handle clicking on a filter to edit its value
   */
  const handleFilterClick = (filter: DashboardFilter) => {
    if (!filter.enabled) return; // Don't allow editing disabled filters

    setEditingFilterId(filter.id);
    setEditValue(filter.value);
  };

  /**
   * Handle saving the edited filter value
   */
  const handleSaveValue = (filterId: string) => {
    const filter = dashboardFilters.find((f) => f.id === filterId);
    if (!filter || !editValue.trim()) return;

    // Compute valueKey based on keyColumn type
    const keyColumn = filter.keyColumn || '';
    const keyColumnName = keyColumn.split('.').pop() || '';

    let valueKey: string;

    // Special case: IDEASBoolFlag dimension has predefined key mappings
    if (keyColumn.includes('IDEASBoolFlag')) {
      const boolMapping: Record<string, string> = {
        'False': '-715022825',
        'True': '888000370',
        'AllUp': '-9999',
        'Unknown': '-1'
      };
      valueKey = boolMapping[editValue] || editValue;
    }
    // For keyColumns ending with "Key": compute MMH3 hash from display value
    else if (keyColumnName.endsWith('Key')) {
      valueKey = mmh3Hash64(editValue);
    }
    // For keyColumns ending with "ID": use the value directly
    else {
      valueKey = editValue;
    }

    console.log(`[DashboardFilterReadOnly] Updating filter: "${editValue}" -> valueKey: ${valueKey}`);

    // Update filter with new value and valueKey
    updateDashboardFilter(filterId, {
      value: editValue,
      valueKey: valueKey
    });

    // Reset state
    setEditingFilterId(null);
    setEditValue('');
  };

  /**
   * Handle canceling the edit
   */
  const handleCancelEdit = () => {
    setEditingFilterId(null);
    setEditValue('');
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
   * Get formatted label for a filter
   */
  const getFilterLabel = (filter: DashboardFilter): string => {
    const operatorLabel = FILTER_OPERATORS.find((op) => op.value === filter.operator)?.label || filter.operator;
    return `${filter.column} ${operatorLabel} ${filter.value}`;
  };

  // If no filters, show empty state
  if (dashboardFilters.length === 0) {
    return (
      <div className="chart-filter-card">
        <div style={{ padding: '12px', textAlign: 'center', color: '#94a3b8', fontSize: 13 }}>
          No filters available
        </div>
      </div>
    );
  }

  return (
    <div className="chart-filter-card">
      {/* Header with filter logic */}
      {dashboardFilters.length > 1 && (
        <div className="chart-filter-body">
          <div className="chart-filter-logic-row">
            <span className="chart-filter-logic-pill" style={{ cursor: 'default' }}>
              {filterLogic}
            </span>
          </div>
        </div>
      )}

      {/* Filter list */}
      <div className="chart-filter-body">
        <div className="chart-filter-list">
          {dashboardFilters.map((filter) => {
            const isEditing = editingFilterId === filter.id;
            const isDisabled = !filter.enabled;

            return (
              <div key={filter.id} className="chart-filter-list-item" style={{ opacity: isDisabled ? 0.5 : 1 }}>
                <div className="chart-filter-list-main">
                  {/* Toggle enabled/disabled */}
                  <button
                    className="chart-filter-chip-remove"
                    onClick={() => handleToggleFilter(filter.id)}
                    title={filter.enabled ? 'Disable filter' : 'Enable filter'}
                    style={{ fontSize: '1rem' }}
                  >
                    <i className={filter.enabled ? 'fas fa-check-circle' : 'far fa-circle'}
                       style={{ color: filter.enabled ? '#10b981' : '#9ca3af' }} />
                  </button>

                  {/* Filter chip */}
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

                {/* Active indicator when editing */}
                {isEditing && <div className="chart-filter-chip-active-indicator" />}
              </div>
            );
          })}
        </div>
      </div>

      {/* Edit popover */}
      {editingFilterId && (
        <div className="chart-filter-popover">
          <div className="chart-filter-popover-header">Edit filter value</div>

          <div style={{ marginTop: '0.5rem' }}>
            <label className="chart-builder-label" htmlFor="filter-value-input" style={{ fontSize: '0.75rem' }}>
              Value
            </label>
            <input
              id="filter-value-input"
              type="text"
              className="chart-builder-input"
              value={editValue}
              onChange={(e) => setEditValue(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === 'Enter') {
                  handleSaveValue(editingFilterId);
                } else if (e.key === 'Escape') {
                  handleCancelEdit();
                }
              }}
              autoFocus
            />
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
              onClick={() => handleSaveValue(editingFilterId)}
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
