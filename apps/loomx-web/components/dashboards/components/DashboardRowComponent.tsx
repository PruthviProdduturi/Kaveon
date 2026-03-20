/**
 * Dashboard Row Component (Superset-style)
 *
 * Renders a full-width horizontal container whose children (columns or charts)
 * are placed side-by-side with flex-grow widths proportional to their `w` value.
 *
 * Edit-mode features:
 *  - Hover toolbar: "Add Column", "Add Chart", "Delete Row"
 *  - Drag handles between children to resize column widths
 *  - Empty-row placeholder with call-to-action
 */

import React, { useRef, useState, useCallback } from 'react';
import dynamic from 'next/dynamic';
import type { DashboardComponentProps } from '../../../types/dashboard';
import { GRID_COLUMNS } from '../../../types/dashboard';
import { useDashboard } from '../DashboardContext';
import { useBuilderContext } from '../BuilderContext';

// Lazy-load DashboardItem to avoid circular-import issues
const DashboardItem = dynamic(() => import('../DashboardItem'), { ssr: false });

const DashboardRowComponent: React.FC<DashboardComponentProps> = ({
  item,
  isEditMode,
}) => {
  const {
    addItemToContainer,
    removeLayoutItem,
    removeItemFromContainer,
    updateLayoutItem,
  } = useDashboard();

  const builderCtx = useBuilderContext();

  const children = item.children || [];
  const rowRef = useRef<HTMLDivElement>(null);
  const [isHovered, setIsHovered] = useState(false);

  // ── Column resize (snaps to GRID_COLUMNS units) ───────────────────────────

  const startResize = useCallback(
    (leftIdx: number, e: React.MouseEvent) => {
      if (!isEditMode || !rowRef.current) return;
      e.preventDefault();
      e.stopPropagation();

      const rowWidth = rowRef.current.offsetWidth;
      const startX = e.clientX;
      const leftChild = children[leftIdx];
      const rightChild = children[leftIdx + 1];
      if (!leftChild || !rightChild) return;

      const startLeftW = leftChild.w || 1;
      const combinedW = startLeftW + (rightChild.w || 1);
      const pixelsPerUnit = rowWidth / GRID_COLUMNS;

      const handleMouseMove = (me: MouseEvent) => {
        const deltaUnits = Math.round((me.clientX - startX) / pixelsPerUnit);
        const newLeftW = Math.max(1, Math.min(combinedW - 1, startLeftW + deltaUnits));
        const newRightW = combinedW - newLeftW;

        const newChildren = children.map((c, idx) => {
          if (idx === leftIdx) return { ...c, w: newLeftW };
          if (idx === leftIdx + 1) return { ...c, w: newRightW };
          return c;
        });

        updateLayoutItem(item.i, { children: newChildren });
      };

      const handleMouseUp = () => {
        document.removeEventListener('mousemove', handleMouseMove);
        document.removeEventListener('mouseup', handleMouseUp);
        document.body.style.userSelect = '';
        document.body.style.cursor = '';
      };

      document.body.style.userSelect = 'none';
      document.body.style.cursor = 'col-resize';
      document.addEventListener('mousemove', handleMouseMove);
      document.addEventListener('mouseup', handleMouseUp);
    },
    [children, isEditMode, item.i, updateLayoutItem]
  );

  // ── Delete self ──────────────────────────────────────────────────────────────

  const handleDelete = useCallback(() => {
    if (item.parentId) {
      removeItemFromContainer(item.parentId, item.i);
    } else {
      removeLayoutItem(item.i);
    }
  }, [item.i, item.parentId, removeLayoutItem, removeItemFromContainer]);

  // ── Render ───────────────────────────────────────────────────────────────────

  return (
    <div
      ref={rowRef}
      data-container-id={item.i}
      data-container-type="row"
      className="dashboard-row-superset"
      onMouseEnter={() => setIsHovered(true)}
      onMouseLeave={() => setIsHovered(false)}
      style={{
        background: 'transparent',
        border: isEditMode ? '1px solid #e2e8f0' : 'none',
        borderRadius: 8,
        position: 'relative',
      }}
    >
      {/* ── Row toolbar (edit mode, shown on hover) ────────────────────────── */}
      {isEditMode && (
        <div
          style={{
            position: 'absolute',
            top: -14,
            left: 12,
            display: 'flex',
            gap: 0,
            zIndex: 20,
            opacity: isHovered ? 1 : 0,
            transition: 'opacity 0.15s ease',
            pointerEvents: isHovered ? 'auto' : 'none',
          }}
        >
          <div
            style={{
              background: '#fff',
              border: '1px solid #e2e8f0',
              borderRadius: 6,
              padding: '2px 4px',
              display: 'flex',
              gap: 2,
              alignItems: 'center',
              boxShadow: '0 2px 8px rgba(0,0,0,0.1)',
              fontSize: 12,
            }}
          >
            {/* Row label */}
            <span
              style={{
                fontSize: 11,
                fontWeight: 700,
                color: '#94a3b8',
                textTransform: 'uppercase',
                letterSpacing: '0.5px',
                padding: '2px 6px',
              }}
            >
              Row
            </span>

            <div style={{ width: 1, height: 14, background: '#e2e8f0', margin: '0 2px' }} />

            {/* Add Column */}
            <button
              onClick={() => addItemToContainer(item.i, 'column')}
              title="Add Column"
              style={{
                background: 'transparent',
                border: 'none',
                cursor: 'pointer',
                padding: '3px 8px',
                color: '#475569',
                fontSize: 12,
                fontWeight: 500,
                display: 'flex',
                alignItems: 'center',
                gap: 4,
                borderRadius: 4,
                whiteSpace: 'nowrap',
              }}
              onMouseOver={(e) => {
                e.currentTarget.style.background = '#f1f5f9';
                e.currentTarget.style.color = '#1e293b';
              }}
              onMouseOut={(e) => {
                e.currentTarget.style.background = 'transparent';
                e.currentTarget.style.color = '#475569';
              }}
            >
              <i className="fas fa-columns" style={{ fontSize: 11 }} />
              Column
            </button>

            {/* Add Chart */}
            {builderCtx && (
              <button
                onClick={() => builderCtx.openChartPickerForContainer(item.i)}
                title="Add Chart to Row"
                style={{
                  background: 'transparent',
                  border: 'none',
                  cursor: 'pointer',
                  padding: '3px 8px',
                  color: '#475569',
                  fontSize: 12,
                  fontWeight: 500,
                  display: 'flex',
                  alignItems: 'center',
                  gap: 4,
                  borderRadius: 4,
                  whiteSpace: 'nowrap',
                }}
                onMouseOver={(e) => {
                  e.currentTarget.style.background = '#f1f5f9';
                  e.currentTarget.style.color = '#1e293b';
                }}
                onMouseOut={(e) => {
                  e.currentTarget.style.background = 'transparent';
                  e.currentTarget.style.color = '#475569';
                }}
              >
                <i className="fas fa-chart-bar" style={{ fontSize: 11 }} />
                Chart
              </button>
            )}

            <div style={{ width: 1, height: 14, background: '#e2e8f0', margin: '0 2px' }} />

            {/* Delete Row */}
            <button
              onClick={handleDelete}
              title="Delete Row"
              style={{
                background: 'transparent',
                border: 'none',
                cursor: 'pointer',
                padding: '3px 8px',
                color: '#ef4444',
                fontSize: 12,
                borderRadius: 4,
                display: 'flex',
                alignItems: 'center',
              }}
              onMouseOver={(e) => { e.currentTarget.style.background = '#fef2f2'; }}
              onMouseOut={(e) => { e.currentTarget.style.background = 'transparent'; }}
            >
              <i className="fas fa-trash" style={{ fontSize: 11 }} />
            </button>
          </div>
        </div>
      )}

      {/* ── Row content ────────────────────────────────────────────────────── */}
      <div
        style={{
          display: 'flex',
          alignItems: 'stretch',
          minHeight: children.length === 0 && isEditMode ? 80 : undefined,
          padding: isEditMode ? 8 : 0,
          gap: 0,
        }}
      >
        {/* Empty row placeholder */}
        {children.length === 0 && isEditMode && (
          <div
            style={{
              flex: 1,
              display: 'flex',
              alignItems: 'center',
              justifyContent: 'center',
              color: '#94a3b8',
              fontSize: 13,
              border: '2px dashed #e2e8f0',
              borderRadius: 6,
              gap: 8,
              padding: '16px 24px',
            }}
          >
            <i className="fas fa-plus-circle" />
            Add a column or chart using the toolbar above
          </div>
        )}

        {/* Children with resize handles */}
        {children.map((child, idx) => (
          <React.Fragment key={child.i}>
            {/* Child item — width based on 12-column grid */}
            <div
              style={{
                flex: `0 0 ${((child.w || 1) / GRID_COLUMNS) * 100}%`,
                minWidth: 0,
                position: 'relative',
                overflow: 'hidden',
              }}
            >
              <DashboardItem item={child} isEditMode={isEditMode} />
            </div>

            {/* Resize handle between adjacent children */}
            {isEditMode && idx < children.length - 1 && (
              <div
                title={`Drag to resize · ${child.w || 1} / ${GRID_COLUMNS} cols`}
                style={{
                  width: 16,
                  flexShrink: 0,
                  cursor: 'col-resize',
                  display: 'flex',
                  alignItems: 'center',
                  justifyContent: 'center',
                  zIndex: 5,
                  position: 'relative',
                }}
                onMouseDown={(e) => startResize(idx, e)}
              >
                <div
                  style={{
                    width: 4,
                    height: '60%',
                    minHeight: 40,
                    background: isHovered ? '#2563eb' : '#e2e8f0',
                    borderRadius: 2,
                    transition: 'background 0.15s ease',
                  }}
                />
              </div>
            )}
          </React.Fragment>
        ))}
      </div>
    </div>
  );
};

export default DashboardRowComponent;
