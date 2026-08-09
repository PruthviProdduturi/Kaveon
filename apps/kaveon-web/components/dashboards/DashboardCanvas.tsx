/**
 * Dashboard Canvas
 *
 * Flat layouts (charts/text positioned by x/y/w/h) render on a real responsive
 * react-grid-layout (v2) grid: drag any tile to reposition, drag a corner/edge
 * to resize width AND height independently, vertical compaction, and mobile
 * reflow (12→6→2→1 cols by breakpoint).
 *
 * Legacy dashboards built with nested row/column containers fall back to the
 * original vertical stack so they keep rendering.
 */
'use client';

import React from 'react';
// react-grid-layout v2 API (installed 2.2.3). @types are v1, so treat as any.
// eslint-disable-next-line @typescript-eslint/no-explicit-any
import * as RGL from 'react-grid-layout';
import { useDashboard } from './DashboardContext';
import DashboardItem from './DashboardItem';

const ResponsiveGridLayout: any = (RGL as any).ResponsiveGridLayout;
const useContainerWidth: any = (RGL as any).useContainerWidth;

interface DashboardCanvasProps {
  className?: string;
}

const DashboardCanvas: React.FC<DashboardCanvasProps> = ({ className = '' }) => {
  const { layout, setLayout, isEditMode, addLayoutItem } = useDashboard();
  const { width, containerRef } = useContainerWidth();

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

  // ── Flat responsive grid ─────────────────────────────────────────────────────
  const rglLayout = rootItems.map((it) => ({
    i: it.i,
    x: typeof it.x === 'number' ? it.x : 0,
    y: typeof it.y === 'number' ? it.y : 0,
    w: it.w || 6,
    h: it.h || 8,
    minW: it.minW || 2,
    minH: it.minH || 2,
  }));

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
      {width > 0 && (
        <ResponsiveGridLayout
          width={width}
          layouts={{ lg: rglLayout }}
          breakpoints={{ lg: 1200, md: 996, sm: 768, xs: 480, xxs: 0 }}
          cols={{ lg: 12, md: 12, sm: 6, xs: 2, xxs: 1 }}
          rowHeight={30}
          margin={[20, 20]}
          containerPadding={[4, 4]}
          compactType="vertical"
          dragConfig={{ enabled: isEditMode, cancel: 'button, a, input, select, textarea, .chart-actions-overlay, .no-drag' }}
          resizeConfig={{ enabled: isEditMode, handles: ['se', 'e', 's'] }}
          onLayoutChange={handleLayoutChange}
        >
          {rootItems.map((item) => (
            <div key={item.i} style={{ height: '100%', overflow: 'hidden' }}>
              <DashboardItem item={item} isEditMode={isEditMode} />
            </div>
          ))}
        </ResponsiveGridLayout>
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
