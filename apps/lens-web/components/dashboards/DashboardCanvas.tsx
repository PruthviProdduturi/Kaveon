/**
 * Dashboard Canvas Component
 *
 * Renders the dashboard as a vertical stack of rows.
 * Each root-level item is rendered in order with full width.
 * In edit mode an "+ Add Row" button appears at the bottom.
 * In edit mode rows can be reordered by dragging the grip handle.
 */

import React, { useState } from 'react';
import { useDashboard } from './DashboardContext';
import DashboardItem from './DashboardItem';

interface DashboardCanvasProps {
  className?: string;
}

const ROW_UNIT = 32;               // px per grid-height unit
const MIN_ROW_UNITS = 2;

const DashboardCanvas: React.FC<DashboardCanvasProps> = ({ className = '' }) => {
  const { layout, setLayout, isEditMode, addLayoutItem, updateLayoutItem } = useDashboard();

  const [dragOverIndex, setDragOverIndex] = useState<number | null>(null);

  // Only show root-level items (items without a parentId)
  const rootItems = layout.filter((item) => !item.parentId);

  // Text/header/divider grow with content; everything else gets a fixed height
  // derived from its grid `h` so charts fill a consistent, adjustable box.
  const autoHeightTypes = new Set(['text', 'header', 'divider']);

  const startRowResize = (item: any, e: React.MouseEvent) => {
    e.preventDefault();
    e.stopPropagation();
    const startY = e.clientY;
    const startH = item.h || 8;
    const onMove = (me: MouseEvent) => {
      const deltaUnits = Math.round((me.clientY - startY) / ROW_UNIT);
      const newH = Math.max(MIN_ROW_UNITS, startH + deltaUnits);
      if (newH !== (item.h || 8)) updateLayoutItem(item.i, { h: newH });
    };
    const onUp = () => {
      document.removeEventListener('mousemove', onMove);
      document.removeEventListener('mouseup', onUp);
      document.body.style.userSelect = '';
      document.body.style.cursor = '';
    };
    document.body.style.userSelect = 'none';
    document.body.style.cursor = 'ns-resize';
    document.addEventListener('mousemove', onMove);
    document.addEventListener('mouseup', onUp);
  };

  const handleDragStart = (e: React.DragEvent, index: number) => {
    e.dataTransfer.setData('text/plain', String(index));
    e.dataTransfer.effectAllowed = 'move';

    // Small custom drag image so the full row isn't ghosted
    const ghost = document.createElement('div');
    ghost.textContent = 'Moving row…';
    ghost.style.cssText =
      'position:fixed;top:-200px;left:0;background:#1e293b;color:#fff;' +
      'padding:6px 14px;border-radius:6px;font-size:13px;font-weight:600;' +
      'white-space:nowrap;pointer-events:none;';
    document.body.appendChild(ghost);
    e.dataTransfer.setDragImage(ghost, 60, 18);
    requestAnimationFrame(() => document.body.removeChild(ghost));
  };

  const handleDragOver = (e: React.DragEvent, index: number) => {
    e.preventDefault();
    e.dataTransfer.dropEffect = 'move';
    setDragOverIndex(index);
  };

  const handleDrop = (e: React.DragEvent, dropIndex: number) => {
    e.preventDefault();
    const fromIndex = parseInt(e.dataTransfer.getData('text/plain'), 10);
    setDragOverIndex(null);
    if (isNaN(fromIndex) || fromIndex === dropIndex) return;

    const reordered = [...rootItems];
    const [moved] = reordered.splice(fromIndex, 1);
    reordered.splice(dropIndex, 0, moved);

    // Non-root items live inside children arrays — keep them after reordered roots
    const nonRoots = layout.filter((item) => !!item.parentId);
    setLayout([...reordered, ...nonRoots]);
  };

  const handleDragLeave = (e: React.DragEvent) => {
    // Only clear if leaving the canvas entirely (not just moving between children)
    if (!(e.currentTarget as HTMLElement).contains(e.relatedTarget as Node)) {
      setDragOverIndex(null);
    }
  };

  // ── Empty state ──────────────────────────────────────────────────────────────
  if (rootItems.length === 0) {
    return (
      <div className={`dashboard-canvas-empty ${className}`}>
        <div className="dashboard-canvas-empty-icon">
          <i className="fas fa-th-large" />
        </div>
        <div className="dashboard-canvas-empty-title">
          {isEditMode ? 'Your dashboard is empty' : 'No content available'}
        </div>
        <div className="dashboard-canvas-empty-subtitle">
          {isEditMode
            ? 'Add a row to start building your dashboard'
            : 'This dashboard has no content to display'}
        </div>
        {isEditMode && (
          <button
            onClick={() => addLayoutItem('row')}
            style={{
              marginTop: 20,
              padding: '10px 24px',
              background: '#2563eb',
              color: '#fff',
              border: 'none',
              borderRadius: 8,
              cursor: 'pointer',
              fontSize: 14,
              fontWeight: 600,
              display: 'inline-flex',
              alignItems: 'center',
              gap: 8,
            }}
            onMouseOver={(e) => { e.currentTarget.style.background = '#1d4ed8'; }}
            onMouseOut={(e) => { e.currentTarget.style.background = '#2563eb'; }}
          >
            <i className="fas fa-plus" />
            Add Row
          </button>
        )}
      </div>
    );
  }

  // ── Populated layout ─────────────────────────────────────────────────────────
  return (
    <div
      className={`dashboard-canvas ${className}`}
      style={{ paddingBottom: 32 }}
      onDragLeave={handleDragLeave}
    >
      {rootItems.map((item, index) => {
        const isAuto = autoHeightTypes.has(item.type);
        const pxHeight = item.h ? item.h * ROW_UNIT : undefined;
        return (
        <div
          key={item.i}
          onDragOver={(e) => handleDragOver(e, index)}
          onDrop={(e) => handleDrop(e, index)}
          style={{
            marginBottom: 16,
            borderRadius: 8,
            position: 'relative',
            outline:
              dragOverIndex === index
                ? '2px dashed #2563eb'
                : '2px solid transparent',
            transition: 'outline 0.1s ease',
          }}
        >
          {/* Drag handle strip — only in edit mode, draggable instead of the whole row */}
          {isEditMode && (
            <div
              draggable
              onDragStart={(e) => handleDragStart(e, index)}
              title="Drag to reorder row"
              style={{
                display: 'flex',
                alignItems: 'center',
                gap: 6,
                padding: '3px 10px',
                marginBottom: 2,
                cursor: 'grab',
                userSelect: 'none',
                color: '#94a3b8',
                fontSize: 11,
                fontWeight: 600,
                letterSpacing: '0.4px',
              }}
            >
              <i className="fas fa-grip-horizontal" style={{ fontSize: 10 }} />
              <span style={{ textTransform: 'uppercase' }}>Row {index + 1}</span>
            </div>
          )}

          <div style={isAuto
            ? { minHeight: pxHeight }
            : { height: pxHeight ?? 300, minHeight: MIN_ROW_UNITS * ROW_UNIT }}>
            <DashboardItem item={item} isEditMode={isEditMode} />
          </div>

          {/* Row-height resize handle (edit mode, fixed-height rows only) */}
          {isEditMode && !isAuto && (
            <div
              onMouseDown={(e) => startRowResize(item, e)}
              title="Drag to resize row height"
              style={{
                position: 'absolute', left: 0, right: 0, bottom: -8, height: 14,
                display: 'flex', alignItems: 'center', justifyContent: 'center',
                cursor: 'ns-resize', zIndex: 6,
              }}
            >
              <div style={{ width: 44, height: 4, borderRadius: 2, background: '#cbd5e1' }} />
            </div>
          )}
        </div>
        );
      })}

      {/* Add Row button */}
      {isEditMode && (
        <div style={{ display: 'flex', justifyContent: 'center', padding: '4px 0 8px' }}>
          <button
            onClick={() => addLayoutItem('row')}
            style={{
              padding: '8px 20px',
              background: 'transparent',
              color: '#94a3b8',
              border: '2px dashed #cbd5e1',
              borderRadius: 8,
              cursor: 'pointer',
              fontSize: 13,
              fontWeight: 600,
              display: 'inline-flex',
              alignItems: 'center',
              gap: 8,
              transition: 'all 0.15s ease',
            }}
            onMouseOver={(e) => {
              e.currentTarget.style.borderColor = '#2563eb';
              e.currentTarget.style.color = '#2563eb';
              e.currentTarget.style.background = '#eff6ff';
            }}
            onMouseOut={(e) => {
              e.currentTarget.style.borderColor = '#cbd5e1';
              e.currentTarget.style.color = '#94a3b8';
              e.currentTarget.style.background = 'transparent';
            }}
          >
            <i className="fas fa-plus" />
            Add Row
          </button>
        </div>
      )}
    </div>
  );
};

export default DashboardCanvas;
