/**
 * Dashboard Canvas
 *
 * Flat layouts (charts/text positioned by x/y/w/h) render on a fixed
 * twelve-column react-grid-layout (v2) grid: drag any tile to reposition, drag
 * a corner/edge to resize width AND height independently, with vertical
 * compaction.
 *
 * Responsiveness is derived here rather than delegated to react-grid-layout's
 * breakpoints — see utils/dashboardGrid. Edit mode always renders the authored
 * grid, because onLayoutChange persists whatever coordinates are on screen and
 * a derived layout must never be written back.
 *
 * Legacy dashboards built with nested row/column containers fall back to the
 * original vertical stack so they keep rendering.
 */
'use client';

import React from 'react';
// react-grid-layout v2 API (installed 2.2.4). @types are v1, so treat as any.
import * as RGL from 'react-grid-layout';
import { useDashboard } from './DashboardContext';
import DashboardItem from './DashboardItem';
import {
  DASHBOARD_GRID_COLS,
  DASHBOARD_GRID_EDGE,
  GridPlacement,
  reflowPlacements,
  tilesPerRow,
} from '../../utils/dashboardGrid';

const GridLayout: any = (RGL as any).GridLayout;
const useContainerWidth: any = (RGL as any).useContainerWidth;

const GRID_CONFIG = {
  cols: DASHBOARD_GRID_COLS,
  rowHeight: 30,
  margin: [20, 20] as [number, number],
  containerPadding: [DASHBOARD_GRID_EDGE, DASHBOARD_GRID_EDGE] as [number, number],
};
const DRAG_CANCEL = 'button, a, input, select, textarea, .chart-actions-overlay, .no-drag';
const RESIZE_HANDLES = ['se', 'e', 's'];

interface DashboardCanvasProps {
  className?: string;
}

const DashboardCanvas: React.FC<DashboardCanvasProps> = ({ className = '' }) => {
  const { layout, setLayout, isEditMode, addLayoutItem } = useDashboard();
  // Measure before the first paint so tiles never land on a stale width.
  const { width, mounted, containerRef } = useContainerWidth({ measureBeforeMount: true });

  const rootItems = layout.filter((item) => !item.parentId);
  const hasContainers = rootItems.some((i) => i.type === 'row' || i.type === 'column');

  // ── Empty state ──────────────────────────────────────────────────────────────
  if (rootItems.length === 0) {
    return (
      <div className={`dashboard-canvas-empty ${className}`}>
        <div className="dashboard-canvas-empty-icon"><i className="fas fa-th-large" /></div>
        <div className="dashboard-canvas-empty-title">
          {isEditMode ? 'Your dashboard is empty' : 'No content available'}
        </div>
        <div className="dashboard-canvas-empty-subtitle">
          {isEditMode ? 'Add a chart to start building your dashboard' : 'This dashboard has no content to display'}
        </div>
        {isEditMode && (
          <button onClick={() => addLayoutItem('chart')} style={addBtnStyle}>
            <i className="fas fa-plus" /> Add Chart
          </button>
        )}
      </div>
    );
  }

  // ── Legacy nested-container dashboards: original vertical stack ───────────────
  if (hasContainers) {
    return (
      <div className={`dashboard-canvas ${className}`} style={{ paddingBottom: 32 }}>
        {rootItems.map((item) => (
          <div key={item.i} style={{ marginBottom: 16 }}>
            <DashboardItem item={item} isEditMode={isEditMode} />
          </div>
        ))}
      </div>
    );
  }

  // ── Flat grid ────────────────────────────────────────────────────────────────
  const authored: GridPlacement[] = rootItems.map((it) => ({
    i: it.i,
    x: typeof it.x === 'number' ? it.x : 0,
    y: typeof it.y === 'number' ? it.y : 0,
    w: it.w || 6,
    h: it.h || 8,
    minW: it.minW || 2,
    minH: it.minH || 2,
  }));

  const perRow = isEditMode ? null : tilesPerRow(width);
  const placements = perRow === null ? authored : reflowPlacements(authored, perRow);

  // Only edit mode persists, and edit mode always renders the authored grid, so
  // the coordinates written back are always the ones the dashboard stores.
  const handleLayoutChange = (current: any[]) => {
    if (!isEditMode || !Array.isArray(current)) return;
    const byId = new Map(current.map((l) => [l.i, l]));
    let changed = false;
    const next = layout.map((it) => {
      const l = byId.get(it.i);
      if (!l) return it;
      if (it.x !== l.x || it.y !== l.y || it.w !== l.w || it.h !== l.h) changed = true;
      return { ...it, x: l.x, y: l.y, w: l.w, h: l.h };
    });
    if (changed) setLayout(next);
  };

  return (
    <div ref={containerRef} className={`dashboard-canvas ${className}`} style={{ paddingBottom: 32 }}>
      {mounted && width > 0 && (
        <GridLayout
          width={width}
          layout={placements}
          gridConfig={GRID_CONFIG}
          dragConfig={{ enabled: isEditMode, cancel: DRAG_CANCEL }}
          resizeConfig={{ enabled: isEditMode, handles: RESIZE_HANDLES }}
          onLayoutChange={handleLayoutChange}
        >
          {rootItems.map((item) => (
            <div key={item.i} style={{ height: '100%', overflow: 'hidden' }}>
              <DashboardItem item={item} isEditMode={isEditMode} />
            </div>
          ))}
        </GridLayout>
      )}

      {isEditMode && (
        <div style={{ display: 'flex', justifyContent: 'center', padding: '12px 0 8px', gap: 10 }}>
          <button onClick={() => addLayoutItem('chart')} style={addBtnStyle}>
            <i className="fas fa-plus" /> Add Chart
          </button>
          <button onClick={() => addLayoutItem('text')} style={{ ...addBtnStyle, background: 'transparent', color: 'var(--text-muted)', border: '2px dashed var(--border)' }}>
            <i className="fas fa-font" /> Add Text
          </button>
        </div>
      )}
    </div>
  );
};

const addBtnStyle: React.CSSProperties = {
  padding: '9px 22px', background: '#2563eb', color: '#fff', border: 'none',
  borderRadius: 8, cursor: 'pointer', fontSize: 14, fontWeight: 600,
  display: 'inline-flex', alignItems: 'center', gap: 8,
};

export default DashboardCanvas;
