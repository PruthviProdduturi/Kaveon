/**
 * Dashboard Tabs Component
 *
 * Renders a tabbed container that can hold multiple layouts.
 * Each tab contains its own set of dashboard components.
 * Useful for organizing related content or providing multiple views.
 */

import React, { useState, useCallback, useEffect, useMemo } from 'react';
import { ConfirmModal } from '../../../components/ConfirmModal';
import dynamic from 'next/dynamic';
import type { DashboardComponentProps, TabConfig, DashboardLayoutItem, ComponentType } from '../../../types/dashboard';
import { useDashboard } from '../DashboardContext';
import { msalFetch } from '../../../utils/msalFetch';
import { API_BASE } from '../../../config';
import { DEFAULT_COMPONENT_DIMENSIONS, MIN_COMPONENT_DIMENSIONS } from '../../../types/dashboard';

// Dynamically import GridLayout
const GridLayout = dynamic(() => import('react-grid-layout'), { ssr: false });
const GridLayoutAny = GridLayout as any;

// Import component types for rendering
const DashboardChartComponent = dynamic(() => import('./DashboardChartComponent'), { ssr: false });
const DashboardTextComponent = dynamic(() => import('./DashboardTextComponent'), { ssr: false });
const DashboardHeaderComponent = dynamic(() => import('./DashboardHeaderComponent'), { ssr: false });
const DashboardDividerComponent = dynamic(() => import('./DashboardDividerComponent'), { ssr: false });

interface Chart {
  id: number;
  name: string;
  chart_type?: string;
}

/**
 * Tab-specific Dashboard Item wrapper
 * Handles remove/duplicate within tab context
 */
const TabDashboardItem: React.FC<{
  item: DashboardLayoutItem;
  isEditMode: boolean;
  onRemove: (itemId: string) => void;
  onDuplicate: (itemId: string) => void;
}> = ({ item, isEditMode, onRemove, onDuplicate }) => {
  const { getEffectiveFilters, selectedItemId, setSelectedItemId } = useDashboard();

  const isSelected = selectedItemId === item.i;
  const [confirmOpen, setConfirmOpen] = useState(false);

  const effectiveFilters = useMemo(() => {
    const result = getEffectiveFilters(item.i);
    return result.effectiveFilters;
  }, [item.i, getEffectiveFilters]);

  const handleRemove = () => setConfirmOpen(true);
  const doRemove = () => { setConfirmOpen(false); onRemove(item.i); };

  const handleDuplicate = () => {
    onDuplicate(item.i);
  };

  const handleClick = () => {
    if (isEditMode) {
      setSelectedItemId(item.i);
    }
  };

  const renderComponent = () => {
    const componentProps = {
      item,
      isEditMode,
      effectiveFilters,
      onConfigChange: () => {}, // Not used in tab items
      onRemove: handleRemove,
    };

    switch (item.type) {
      case 'chart':
        return (
          <React.Suspense fallback={<div>Loading...</div>}>
            <DashboardChartComponent {...componentProps} />
          </React.Suspense>
        );
      case 'text':
        return (
          <React.Suspense fallback={<div>Loading...</div>}>
            <DashboardTextComponent {...componentProps} />
          </React.Suspense>
        );
      case 'header':
        return (
          <React.Suspense fallback={<div>Loading...</div>}>
            <DashboardHeaderComponent {...componentProps} />
          </React.Suspense>
        );
      case 'divider':
        return (
          <React.Suspense fallback={<div>Loading...</div>}>
            <DashboardDividerComponent {...componentProps} />
          </React.Suspense>
        );
      default:
        return <div>Unknown component type: {item.type}</div>;
    }
  };

  const getComponentTitle = () => {
    switch (item.type) {
      case 'chart':
        return item.chartId ? `Chart #${item.chartId}` : 'Chart';
      case 'text':
        return 'Text Block';
      case 'header':
        return item.headerConfig?.content || 'Header';
      case 'divider':
        return 'Divider';
      default:
        return 'Component';
    }
  };

  return (
    <div
      className={`dashboard-component ${isSelected ? 'dashboard-component-selected' : ''}`}
      onClick={handleClick}
    >
      <div className="dashboard-component-header">
        {isEditMode && (
          <div className="dashboard-drag-handle" title="Drag to move">
            <i className="fas fa-grip-vertical" />
          </div>
        )}

        <div className="dashboard-component-title">{getComponentTitle()}</div>

        {isEditMode && (
          <div className="dashboard-component-actions">
            <button
              className="dashboard-component-action-btn"
              onClick={(e) => {
                e.stopPropagation();
                handleDuplicate();
              }}
              title="Duplicate component"
            >
              <i className="fas fa-copy" />
            </button>

            <button
              className="dashboard-component-action-btn"
              onClick={(e) => {
                e.stopPropagation();
                handleRemove();
              }}
              title="Remove component"
            >
              <i className="fas fa-trash" />
            </button>
          </div>
        )}
      </div>

      <div className="dashboard-component-body">{renderComponent()}</div>

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
 * DashboardTabsComponent
 *
 * Multi-tab container component with independent layouts per tab.
 */
const DashboardTabsComponent: React.FC<DashboardComponentProps> = ({
  item,
  isEditMode,
  onConfigChange,
}) => {
  const tabs = item.tabs || [
    { id: 'tab-1', name: 'Tab 1', layout: [] },
  ];

  const [activeTabId, setActiveTabId] = useState(tabs[0]?.id || '');
  const [charts, setCharts] = useState<Chart[]>([]);
  const [showAddMenu, setShowAddMenu] = useState(false);

  // Fetch available charts
  useEffect(() => {
    if (!isEditMode) return;

    msalFetch(`${API_BASE}/api/v1/charts/summary`)
      .then((res) => res.ok ? res.json() : { recent: [] })
      .then((data) => setCharts(Array.isArray(data.recent) ? data.recent : []))
      .catch((err) => console.error('Error fetching charts:', err));
  }, [isEditMode]);

  // Close dropdown when clicking outside
  useEffect(() => {
    if (!showAddMenu) return;

    const handleClickOutside = () => setShowAddMenu(false);
    document.addEventListener('click', handleClickOutside);
    return () => document.removeEventListener('click', handleClickOutside);
  }, [showAddMenu]);

  /**
   * Handle tab switching
   */
  const handleTabClick = (tabId: string, e?: React.MouseEvent) => {
    if (e) {
      e.stopPropagation();
    }
    setActiveTabId(tabId);
  };

  /**
   * Remove an item from the active tab
   */
  const handleRemoveItemFromTab = useCallback(
    (itemId: string) => {
      if (!onConfigChange) return;

      const tab = tabs.find((t) => t.id === activeTabId);
      if (!tab) return;

      const updatedLayout = tab.layout.filter((layoutItem) => layoutItem.i !== itemId);
      const updatedTabs = tabs.map((t) =>
        t.id === activeTabId ? { ...t, layout: updatedLayout } : t
      );

      onConfigChange(item.i, {
        tabs: updatedTabs,
      });
    },
    [tabs, activeTabId, onConfigChange, item.i]
  );

  /**
   * Duplicate an item within the active tab
   */
  const handleDuplicateItemInTab = useCallback(
    (itemId: string) => {
      if (!onConfigChange) return;

      const tab = tabs.find((t) => t.id === activeTabId);
      if (!tab) return;

      const itemToDuplicate = tab.layout.find((layoutItem) => layoutItem.i === itemId);
      if (!itemToDuplicate) return;

      const newItemId = `item-${Date.now()}-${Math.random().toString(36).substr(2, 9)}`;
      const maxY = tab.layout.reduce((max, layoutItem) => Math.max(max, layoutItem.y + layoutItem.h), 0);

      const duplicatedItem: DashboardLayoutItem = {
        ...itemToDuplicate,
        i: newItemId,
        x: 0,
        y: maxY,
      };

      const updatedTabs = tabs.map((t) =>
        t.id === activeTabId
          ? { ...t, layout: [...t.layout, duplicatedItem] }
          : t
      );

      onConfigChange(item.i, {
        tabs: updatedTabs,
      });
    },
    [tabs, activeTabId, onConfigChange, item.i]
  );

  /**
   * Add a new tab
   */
  const handleAddTab = () => {
    if (!onConfigChange) return;

    const newTabId = `tab-${Date.now()}`;
    const newTab: TabConfig = {
      id: newTabId,
      name: `Tab ${tabs.length + 1}`,
      layout: [],
    };

    onConfigChange(item.i, {
      tabs: [...tabs, newTab],
    });

    setActiveTabId(newTabId);
  };

  /**
   * Remove a tab
   */
  const handleRemoveTab = (tabId: string) => {
    if (!onConfigChange || tabs.length === 1) return;

    const updatedTabs = tabs.filter((t) => t.id !== tabId);
    onConfigChange(item.i, {
      tabs: updatedTabs,
    });

    // Switch to first tab if active tab was removed
    if (activeTabId === tabId) {
      setActiveTabId(updatedTabs[0]?.id || '');
    }
  };

  /**
   * Rename a tab
   */
  const handleRenameTab = (tabId: string, newName: string) => {
    if (!onConfigChange) return;

    const updatedTabs = tabs.map((t) =>
      t.id === tabId ? { ...t, name: newName } : t
    );

    onConfigChange(item.i, {
      tabs: updatedTabs,
    });
  };

  /**
   * Add a chart to the active tab
   */
  const handleAddChartToTab = (chart: Chart) => {
    if (!onConfigChange) return;

    const tab = tabs.find((t) => t.id === activeTabId);
    if (!tab) return;

    const itemId = `item-${Date.now()}-${Math.random().toString(36).substr(2, 9)}`;
    const defaultDims = DEFAULT_COMPONENT_DIMENSIONS.chart;
    const minDims = MIN_COMPONENT_DIMENSIONS.chart;

    // Find next available position in tab layout
    const maxY = tab.layout.reduce((max, item) => Math.max(max, item.y + item.h), 0);

    const newLayoutItem: DashboardLayoutItem = {
      i: itemId,
      x: 0,
      y: maxY,
      w: defaultDims.w,
      h: defaultDims.h,
      type: 'chart',
      chartId: chart.id,
      minW: minDims.w,
      minH: minDims.h,
    };

    const updatedTabs = tabs.map((t) =>
      t.id === activeTabId
        ? { ...t, layout: [...t.layout, newLayoutItem] }
        : t
    );

    onConfigChange(item.i, {
      tabs: updatedTabs,
    });

    setShowAddMenu(false);
  };

  /**
   * Add a component to the active tab
   */
  const handleAddComponentToTab = (type: ComponentType) => {
    if (!onConfigChange) return;

    const tab = tabs.find((t) => t.id === activeTabId);
    if (!tab) return;

    const itemId = `item-${Date.now()}-${Math.random().toString(36).substr(2, 9)}`;
    const defaultDims = DEFAULT_COMPONENT_DIMENSIONS[type];
    const minDims = MIN_COMPONENT_DIMENSIONS[type];

    // Find next available position in tab layout
    const maxY = tab.layout.reduce((max, item) => Math.max(max, item.y + item.h), 0);

    const newLayoutItem: DashboardLayoutItem = {
      i: itemId,
      x: 0,
      y: maxY,
      w: defaultDims.w,
      h: defaultDims.h,
      type,
      minW: minDims.w,
      minH: minDims.h,
    };

    // Add type-specific config
    switch (type) {
      case 'text':
        newLayoutItem.textConfig = {
          content: 'Enter your text here...',
          alignment: 'left',
        };
        break;
      case 'header':
        newLayoutItem.headerConfig = {
          content: 'Section Header',
          size: 'large',
          alignment: 'left',
        };
        break;
      case 'divider':
        newLayoutItem.dividerConfig = {
          orientation: 'horizontal',
          thickness: 1,
          color: '#cbd5e1',
          style: 'solid',
        };
        break;
    }

    const updatedTabs = tabs.map((t) =>
      t.id === activeTabId
        ? { ...t, layout: [...t.layout, newLayoutItem] }
        : t
    );

    onConfigChange(item.i, {
      tabs: updatedTabs,
    });

    setShowAddMenu(false);
  };

  /**
   * Handle layout changes within a tab
   */
  const handleTabLayoutChange = useCallback(
    (tabId: string, newLayout: any) => {
      if (!isEditMode || !onConfigChange) return;

      const layoutArray = Array.isArray(newLayout) ? newLayout : Array.from(newLayout);
      const layoutMap = new Map(layoutArray.map((item: any) => [item.i, item]));

      const tab = tabs.find((t) => t.id === tabId);
      if (!tab) return;

      const updatedTabLayout = tab.layout.map((layoutItem) => {
        const gridItem = layoutMap.get(layoutItem.i);
        if (!gridItem) return layoutItem;

        return {
          ...layoutItem,
          x: gridItem.x,
          y: gridItem.y,
          w: gridItem.w,
          h: gridItem.h,
        };
      });

      const updatedTabs = tabs.map((t) =>
        t.id === tabId ? { ...t, layout: updatedTabLayout } : t
      );

      onConfigChange(item.i, {
        tabs: updatedTabs,
      });
    },
    [tabs, isEditMode, onConfigChange, item.i]
  );

  const activeTab = tabs.find((t) => t.id === activeTabId);

  return (
    <div className="dashboard-tabs-component">
      {/* Tab headers */}
      <div className="dashboard-tabs-header">
        {tabs.map((tab) => (
          <div
            key={tab.id}
            className={`dashboard-tab ${
              tab.id === activeTabId ? 'dashboard-tab-active' : ''
            }`}
            onClick={(e) => {
              // Only switch tabs if not clicking on input or close button
              if (
                e.target instanceof HTMLElement &&
                !e.target.closest('input') &&
                !e.target.closest('.tab-close-btn')
              ) {
                handleTabClick(tab.id);
              }
            }}
            style={{ display: 'flex', alignItems: 'center', cursor: 'pointer' }}
          >
            {isEditMode ? (
              <input
                type="text"
                value={tab.name}
                onChange={(e) => handleRenameTab(tab.id, e.target.value)}
                onClick={(e) => {
                  e.stopPropagation();
                  // Switch to this tab when clicking the input
                  if (activeTabId !== tab.id) {
                    handleTabClick(tab.id);
                  }
                }}
                onFocus={() => {
                  // Switch to this tab when focusing the input
                  if (activeTabId !== tab.id) {
                    handleTabClick(tab.id);
                  }
                }}
                style={{
                  background: 'transparent',
                  border: 'none',
                  outline: 'none',
                  color: 'inherit',
                  font: 'inherit',
                  width: 'auto',
                  minWidth: 60,
                  cursor: 'text',
                }}
              />
            ) : (
              <span>{tab.name}</span>
            )}

            {isEditMode && tabs.length > 1 && (
              <button
                className="tab-close-btn"
                onClick={(e) => {
                  e.stopPropagation();
                  handleRemoveTab(tab.id);
                }}
                style={{
                  marginLeft: 8,
                  background: 'transparent',
                  border: 'none',
                  cursor: 'pointer',
                  color: 'inherit',
                  opacity: 0.6,
                  fontSize: 12,
                  padding: '2px 4px',
                }}
                title="Remove tab"
              >
                <i className="fas fa-times" />
              </button>
            )}
          </div>
        ))}

        {isEditMode && (
          <button className="dashboard-tab-add-btn" onClick={handleAddTab}>
            <i className="fas fa-plus" style={{ marginRight: 6 }} />
            Add Tab
          </button>
        )}
      </div>

      {/* Tab content */}
      <div className="dashboard-tabs-content">
        {activeTab ? (
          <div style={{ minHeight: 400, background: '#f8fafc', borderRadius: 8, padding: 16, position: 'relative' }}>
            {/* Add content button (edit mode only) */}
            {isEditMode && (
              <div style={{ position: 'absolute', top: 16, right: 16, zIndex: 10 }}>
                <button
                  onClick={(e) => {
                    e.stopPropagation();
                    setShowAddMenu(!showAddMenu);
                  }}
                  style={{
                    padding: '8px 16px',
                    background: '#2563eb',
                    color: '#ffffff',
                    border: 'none',
                    borderRadius: 6,
                    cursor: 'pointer',
                    fontSize: 13,
                    fontWeight: 600,
                    boxShadow: '0 2px 4px rgba(0, 0, 0, 0.1)',
                  }}
                >
                  <i className="fas fa-plus" style={{ marginRight: 6 }} />
                  Add to Tab
                </button>

                {/* Dropdown menu */}
                {showAddMenu && (
                  <div
                    onClick={(e) => e.stopPropagation()}
                    style={{
                      position: 'absolute',
                      top: '100%',
                      right: 0,
                      marginTop: 8,
                      background: '#ffffff',
                      border: '1px solid #e2e8f0',
                      borderRadius: 8,
                      boxShadow: '0 4px 12px rgba(0, 0, 0, 0.15)',
                      minWidth: 250,
                      maxHeight: 400,
                      overflow: 'auto',
                      zIndex: 20,
                    }}
                  >
                    {/* Charts section */}
                    <div style={{ padding: '12px 16px', borderBottom: '1px solid #e2e8f0' }}>
                      <div style={{ fontSize: 12, fontWeight: 600, color: '#64748b', marginBottom: 8 }}>
                        CHARTS
                      </div>
                      {charts.length > 0 ? (
                        charts.slice(0, 5).map((chart) => (
                          <button
                            key={chart.id}
                            onClick={() => handleAddChartToTab(chart)}
                            style={{
                              display: 'block',
                              width: '100%',
                              padding: '8px 12px',
                              background: 'transparent',
                              border: 'none',
                              borderRadius: 4,
                              textAlign: 'left',
                              cursor: 'pointer',
                              fontSize: 13,
                              color: '#334155',
                              marginBottom: 4,
                            }}
                            onMouseOver={(e) => (e.currentTarget.style.background = '#f1f5f9')}
                            onMouseOut={(e) => (e.currentTarget.style.background = 'transparent')}
                          >
                            <i className="fas fa-chart-bar" style={{ marginRight: 8, width: 16 }} />
                            {chart.name}
                          </button>
                        ))
                      ) : (
                        <div style={{ fontSize: 12, color: '#94a3b8', padding: '8px 12px' }}>
                          No charts available
                        </div>
                      )}
                    </div>

                    {/* Components section */}
                    <div style={{ padding: '12px 16px' }}>
                      <div style={{ fontSize: 12, fontWeight: 600, color: '#64748b', marginBottom: 8 }}>
                        COMPONENTS
                      </div>
                      {[
                        { type: 'header' as ComponentType, icon: 'fa-heading', label: 'Header' },
                        { type: 'text' as ComponentType, icon: 'fa-align-left', label: 'Text Block' },
                        { type: 'divider' as ComponentType, icon: 'fa-minus', label: 'Divider' },
                      ].map((comp) => (
                        <button
                          key={comp.type}
                          onClick={() => handleAddComponentToTab(comp.type)}
                          style={{
                            display: 'block',
                            width: '100%',
                            padding: '8px 12px',
                            background: 'transparent',
                            border: 'none',
                            borderRadius: 4,
                            textAlign: 'left',
                            cursor: 'pointer',
                            fontSize: 13,
                            color: '#334155',
                            marginBottom: 4,
                          }}
                          onMouseOver={(e) => (e.currentTarget.style.background = '#f1f5f9')}
                          onMouseOut={(e) => (e.currentTarget.style.background = 'transparent')}
                        >
                          <i className={`fas ${comp.icon}`} style={{ marginRight: 8, width: 16 }} />
                          {comp.label}
                        </button>
                      ))}
                    </div>
                  </div>
                )}
              </div>
            )}

            {activeTab.layout.length > 0 ? (
              <GridLayoutAny
                className="layout"
                layout={activeTab.layout.map((layoutItem) => ({
                  i: layoutItem.i,
                  x: layoutItem.x,
                  y: layoutItem.y,
                  w: layoutItem.w,
                  h: layoutItem.h,
                  minW: layoutItem.minW,
                  minH: layoutItem.minH,
                  static: !isEditMode,
                }))}
                cols={12}
                rowHeight={30}
                width={1200}
                margin={[16, 16]}
                containerPadding={[0, 0]}
                compactType="vertical"
                isDraggable={isEditMode}
                isResizable={isEditMode}
                onLayoutChange={(newLayout: any) => handleTabLayoutChange(activeTab.id, newLayout)}
                draggableHandle=".dashboard-drag-handle"
                useCSSTransforms={true}
                preventCollision={false}
              >
                {activeTab.layout.map((layoutItem) => (
                  <div key={layoutItem.i}>
                    <TabDashboardItem
                      item={layoutItem}
                      isEditMode={isEditMode}
                      onRemove={handleRemoveItemFromTab}
                      onDuplicate={handleDuplicateItemInTab}
                    />
                  </div>
                ))}
              </GridLayoutAny>
            ) : (
              <div style={{ textAlign: 'center', padding: 40, color: '#94a3b8' }}>
                <i className="fas fa-layer-group" style={{ fontSize: 32, marginBottom: 12 }} />
                <p>This tab is empty</p>
                {isEditMode && (
                  <p style={{ fontSize: 13, marginTop: 8 }}>
                    Click "Add to Tab" button above to add content
                  </p>
                )}
              </div>
            )}
          </div>
        ) : (
          <div style={{ padding: 40, textAlign: 'center', color: '#94a3b8' }}>
            No tab selected
          </div>
        )}
      </div>
    </div>
  );
};

export default DashboardTabsComponent;
