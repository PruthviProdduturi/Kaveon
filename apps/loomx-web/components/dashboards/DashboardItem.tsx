/**
 * Dashboard Item Component
 *
 * Wrapper component that renders the appropriate dashboard component type
 * (chart, text, header, tabs, divider) based on the layout item configuration.
 *
 * Provides common functionality:
 * - Drag handle for repositioning (edit mode only)
 * - Action buttons (edit, duplicate, delete)
 * - Selection state visual feedback
 * - Component-specific rendering
 */

import React, { useMemo, useState } from 'react';
import { useDashboard } from './DashboardContext';
import type { DashboardLayoutItem } from '../../types/dashboard';
import { ConfirmModal } from '../../components/ConfirmModal';

// Import component implementations (will be created next)
// For now, we'll use placeholder components
const DashboardChartComponent = React.lazy(() => import('./components/DashboardChartComponent'));
const DashboardTextComponent = React.lazy(() => import('./components/DashboardTextComponent'));
const DashboardHeaderComponent = React.lazy(() => import('./components/DashboardHeaderComponent'));
const DashboardTabsComponent = React.lazy(() => import('./components/DashboardTabsComponent'));
const DashboardDividerComponent = React.lazy(() => import('./components/DashboardDividerComponent'));
const DashboardRowComponent = React.lazy(() => import('./components/DashboardRowComponent'));
const DashboardColumnComponent = React.lazy(() => import('./components/DashboardColumnComponent'));

/**
 * Props for the DashboardItem component
 */
interface DashboardItemProps {
  /** Layout item configuration */
  item: DashboardLayoutItem;
  /** Whether the dashboard is in edit mode (defaults to context if not provided) */
  isEditMode?: boolean;
}

/**
 * DashboardItem Component
 *
 * Renders a single dashboard component with appropriate wrapper,
 * drag handle, and action buttons based on edit mode state.
 */
const DashboardItem: React.FC<DashboardItemProps> = ({ item, isEditMode: isEditModeProp }) => {
  const {
    selectedItemId,
    updateLayoutItem,
    removeLayoutItem,
    removeItemFromContainer,
    duplicateLayoutItem,
    getEffectiveFilters,
    isEditMode: isEditModeContext,
    moveItemToRoot,
  } = useDashboard();

  // Use prop if provided, otherwise fall back to context
  const isEditMode = isEditModeProp !== undefined ? isEditModeProp : isEditModeContext;

  // Check if this item is nested inside a container
  const isNested = !!item.parentId;

  const isSelected = selectedItemId === item.i;

  // State for confirm removal dialog
  const [confirmOpen, setConfirmOpen] = useState(false);

  /**
   * Calculate effective filters for this component
   * Merges dashboard filters with component-level filters
   */
  const effectiveFilters = useMemo(() => {
    const result = getEffectiveFilters(item.i);
    return result.effectiveFilters;
  }, [item.i, getEffectiveFilters]);

  /**
   * Handle component configuration changes
   */
  const handleConfigChange = (itemId: string, updates: Partial<DashboardLayoutItem>) => {
    updateLayoutItem(itemId, updates);
  };

  /**
   * Directly remove this item (used by components that have their own confirm dialog).
   */
  const directRemove = (_itemId?: string) => {
    if (item.parentId) {
      removeItemFromContainer(item.parentId, item.i);
    } else {
      removeLayoutItem(item.i);
    }
  };

  /**
   * Handle component removal – opens DashboardItem's own confirm modal (tabs fallback).
   */
  const handleRemove = () => setConfirmOpen(true);

  const doRemove = () => {
    setConfirmOpen(false);
    directRemove();
  };

  /**
   * Handle component duplication
   */
  const handleDuplicate = () => {
    duplicateLayoutItem(item.i);
  };

  /**
   * Handle moving item to root canvas
   */
  const handleMoveToRoot = () => {
    if (item.parentId) {
      moveItemToRoot(item.i);
    }
  };

  /**
   * Render the appropriate component based on type
   */
  const renderComponent = () => {
    const componentProps = {
      item,
      isEditMode,
      effectiveFilters,
      onConfigChange: handleConfigChange,
      onRemove: directRemove,
      onDuplicate: handleDuplicate,
    };

    switch (item.type) {
      case 'chart':
        return (
          <React.Suspense fallback={<ComponentLoading />}>
            <DashboardChartComponent {...componentProps} />
          </React.Suspense>
        );

      case 'text':
        return (
          <React.Suspense fallback={<ComponentLoading />}>
            <DashboardTextComponent {...componentProps} />
          </React.Suspense>
        );

      case 'header':
        return (
          <React.Suspense fallback={<ComponentLoading />}>
            <DashboardHeaderComponent {...componentProps} />
          </React.Suspense>
        );

      case 'tabs':
        return (
          <React.Suspense fallback={<ComponentLoading />}>
            <DashboardTabsComponent {...componentProps} />
          </React.Suspense>
        );

      case 'divider':
        return (
          <React.Suspense fallback={<ComponentLoading />}>
            <DashboardDividerComponent {...componentProps} />
          </React.Suspense>
        );

      case 'row':
        return (
          <React.Suspense fallback={<ComponentLoading />}>
            <DashboardRowComponent {...componentProps} />
          </React.Suspense>
        );

      case 'column':
        return (
          <React.Suspense fallback={<ComponentLoading />}>
            <DashboardColumnComponent {...componentProps} />
          </React.Suspense>
        );

      default:
        return (
          <div className="dashboard-component-error">
            <div className="dashboard-component-error-icon">
              <i className="fas fa-exclamation-triangle" />
            </div>
            <div className="dashboard-component-error-message">
              Unknown component type: {item.type}
            </div>
          </div>
        );
    }
  };

  // Row, column, divider, header and text self-manage their controls — no wrapper.
  if (item.type === 'row' || item.type === 'column' || item.type === 'divider' || item.type === 'header' || item.type === 'text') {
    return (
      <div
        data-item-id={item.i}
        data-item-type={item.type}
        style={{ height: '100%' }}
      >
        {renderComponent()}
      </div>
    );
  }

  // Chart (and similar non-container) components: render with lightweight hover actions.
  if (item.type === 'chart') {
    return (
      <div
        className={`dashboard-component dashboard-component-chart ${isSelected ? 'dashboard-component-selected' : ''}`}
        data-testid={`dashboard-item-${item.i}`}
        data-item-id={item.i}
        data-item-type={item.type}
        data-parent-id={item.parentId || ''}
        style={{ display: 'flex', flexDirection: 'column', height: '100%', position: 'relative' }}
      >
        <div style={{ flex: 1, display: 'flex', flexDirection: 'column', height: '100%' }}>
          {renderComponent()}
        </div>

        <ConfirmModal
          isOpen={confirmOpen}
          title="Remove component"
          message="This component will be permanently removed from the dashboard."
          confirmLabel="Remove"
          onConfirm={doRemove}
          onCancel={() => setConfirmOpen(false)}
        />
      </div>
    );
  }

  // For tab components: render with header but no card border
  return (
    <div
      className={`dashboard-component ${isSelected ? 'dashboard-component-selected' : ''}`}
      data-testid={`dashboard-item-${item.i}`}
      data-item-id={item.i}
      data-item-type={item.type}
      data-is-container={false}
      data-parent-id={item.parentId || ''}
      style={{ position: 'relative', height: '100%' }}
    >
      {/* Header with drag handle and actions */}
      <div className="dashboard-component-header">
        {isEditMode && (
          <div className="dashboard-drag-handle" title="Drag to move">
            <i className="fas fa-grip-vertical" />
          </div>
        )}

        <div className="dashboard-component-title">
          {getComponentTitle(item)}
        </div>

        {isEditMode && (
          <div className="dashboard-component-actions">
            {isNested && (
              <button
                className="dashboard-component-action-btn"
                onClick={handleMoveToRoot}
                title="Move to root canvas"
              >
                <i className="fas fa-arrow-up" />
              </button>
            )}
            <button
              className="dashboard-component-action-btn"
              onClick={handleDuplicate}
              title="Duplicate component"
            >
              <i className="fas fa-copy" />
            </button>

            <button
              className="dashboard-component-action-btn"
              onClick={handleRemove}
              title="Remove component"
            >
              <i className="fas fa-trash" />
            </button>
          </div>
        )}
      </div>

      {/* Component body */}
      <div className="dashboard-component-body">
        {renderComponent()}
      </div>

      <ConfirmModal
        isOpen={confirmOpen}
        title="Remove component"
        message="This component will be permanently removed from the dashboard."
        confirmLabel="Remove"
        onConfirm={doRemove}
        onCancel={() => setConfirmOpen(false)}
      />
    </div>
  );
};

/**
 * Get a display title for the component based on its type and configuration
 */
function getComponentTitle(item: DashboardLayoutItem): string {
  switch (item.type) {
    case 'chart':
      return item.chartId ? `Chart #${item.chartId}` : 'Chart';
    case 'text':
      return 'Text Block';
    case 'header':
      return item.headerConfig?.content || 'Header';
    case 'tabs':
      return 'Tabs';
    case 'divider':
      return 'Divider';
    case 'row':
      return 'Row';
    case 'column':
      return 'Column';
    default:
      return 'Component';
  }
}

/**
 * Loading placeholder for lazy-loaded components
 */
function ComponentLoading() {
  return (
    <div className="dashboard-component-loading">
      <i className="fas fa-spinner fa-spin" /> Loading...
    </div>
  );
}

export default DashboardItem;
