/**
 * Dashboard Column Component (Superset-style)
 *
 * Renders a vertical container whose children are stacked top-to-bottom.
 * No header bar — edit actions appear as a floating overlay on hover.
 * Each child's height is resizable by dragging the handle at its bottom edge.
 */

import React, { useState, useCallback, useRef } from 'react';
import dynamic from 'next/dynamic';
import type { DashboardComponentProps } from '../../../types/dashboard';
import { useDashboard } from '../DashboardContext';
import { useBuilderContext } from '../BuilderContext';

const DashboardItem = dynamic(() => import('../DashboardItem'), { ssr: false });

/** Height of one grid row in pixels — must match rowHeight used elsewhere */
const ROW_HEIGHT = 30;
/** Minimum chart height in grid rows */
const MIN_H = 3;

const DashboardColumnComponent: React.FC<DashboardComponentProps> = ({
  item,
  isEditMode,
}) => {
  const { removeItemFromContainer, removeLayoutItem, updateLayoutItem } = useDashboard();
  const builderCtx = useBuilderContext();
  const children = item.children || [];
  const [isHovered, setIsHovered] = useState(false);

  // ── Delete self ──────────────────────────────────────────────────────────────

  const handleDelete = useCallback(() => {
    if (item.parentId) {
      removeItemFromContainer(item.parentId, item.i);
    } else {
      removeLayoutItem(item.i);
    }
  }, [item.i, item.parentId, removeLayoutItem, removeItemFromContainer]);

  // ── Vertical resize (drag bottom edge of each chart) ─────────────────────────

  const startVerticalResize = useCallback(
    (childId: string, currentH: number, e: React.MouseEvent) => {
      if (!isEditMode) return;
      e.preventDefault();
      e.stopPropagation();

      const startY = e.clientY;
      const startH = currentH;

      const handleMouseMove = (me: MouseEvent) => {
        const deltaH = Math.round((me.clientY - startY) / ROW_HEIGHT);
        const newH = Math.max(MIN_H, startH + deltaH);
        updateLayoutItem(childId, { h: newH });
      };

      const handleMouseUp = () => {
        document.removeEventListener('mousemove', handleMouseMove);
        document.removeEventListener('mouseup', handleMouseUp);
        document.body.style.userSelect = '';
        document.body.style.cursor = '';
      };

      document.body.style.userSelect = 'none';
      document.body.style.cursor = 'row-resize';
      document.addEventListener('mousemove', handleMouseMove);
      document.addEventListener('mouseup', handleMouseUp);
    },
    [isEditMode, updateLayoutItem]
  );

  // ── Render ───────────────────────────────────────────────────────────────────

  return (
    <div
      data-container-id={item.i}
      data-container-type="column"
      className="dashboard-column-superset"
      onMouseEnter={() => setIsHovered(true)}
      onMouseLeave={() => setIsHovered(false)}
      style={{
        display: 'flex',
        flexDirection: 'column',
        height: '100%',
        background: isEditMode ? '#f8fafc' : 'transparent',
        border: isEditMode ? '1px dashed #e2e8f0' : 'none',
        borderRadius: 6,
        position: 'relative',
        boxSizing: 'border-box',
      }}
    >
      {/* ── Floating action overlay (edit mode, on hover) ──────────────────── */}
      {isEditMode && (
        <div
          style={{
            position: 'absolute',
            top: 4,
            right: 4,
            zIndex: 20,
            display: 'flex',
            gap: 2,
            opacity: isHovered ? 1 : 0,
            transition: 'opacity 0.15s ease',
            pointerEvents: isHovered ? 'auto' : 'none',
          }}
        >
          {builderCtx && (
            <button
              onClick={() => builderCtx.openChartPickerForContainer(item.i)}
              title="Add Chart"
              style={{
                background: '#fff',
                border: '1px solid #e2e8f0',
                borderRadius: 4,
                cursor: 'pointer',
                padding: '2px 7px',
                color: '#475569',
                fontSize: 11,
                fontWeight: 500,
                display: 'flex',
                alignItems: 'center',
                gap: 4,
                boxShadow: '0 1px 4px rgba(0,0,0,0.08)',
              }}
              onMouseOver={(e) => {
                e.currentTarget.style.background = '#eff6ff';
                e.currentTarget.style.color = '#2563eb';
                e.currentTarget.style.borderColor = '#bfdbfe';
              }}
              onMouseOut={(e) => {
                e.currentTarget.style.background = '#fff';
                e.currentTarget.style.color = '#475569';
                e.currentTarget.style.borderColor = '#e2e8f0';
              }}
            >
              <i className="fas fa-plus" style={{ fontSize: 10 }} />
              Chart
            </button>
          )}
          <button
            onClick={handleDelete}
            title="Delete Column"
            style={{
              background: '#fff',
              border: '1px solid #e2e8f0',
              borderRadius: 4,
              cursor: 'pointer',
              padding: '2px 7px',
              color: '#ef4444',
              fontSize: 11,
              display: 'flex',
              alignItems: 'center',
              boxShadow: '0 1px 4px rgba(0,0,0,0.08)',
            }}
            onMouseOver={(e) => {
              e.currentTarget.style.background = '#fef2f2';
              e.currentTarget.style.borderColor = '#fecaca';
            }}
            onMouseOut={(e) => {
              e.currentTarget.style.background = '#fff';
              e.currentTarget.style.borderColor = '#e2e8f0';
            }}
          >
            <i className="fas fa-times" style={{ fontSize: 10 }} />
          </button>
        </div>
      )}

      {/* ── Column children ────────────────────────────────────────────────── */}
      <div
        style={{
          display: 'flex',
          flexDirection: 'column',
          padding: isEditMode ? 8 : 0,
          flex: 1,
          minHeight: 0,
        }}
      >
        {/* Empty column CTA */}
        {children.length === 0 && isEditMode && builderCtx && (
          <button
            onClick={() => builderCtx.openChartPickerForContainer(item.i)}
            style={{
              flex: 1,
              minHeight: 80,
              display: 'flex',
              flexDirection: 'column',
              alignItems: 'center',
              justifyContent: 'center',
              gap: 8,
              background: 'transparent',
              border: '2px dashed #e2e8f0',
              borderRadius: 6,
              cursor: 'pointer',
              color: '#94a3b8',
              fontSize: 13,
              fontWeight: 500,
              transition: 'all 0.15s ease',
            }}
            onMouseOver={(e) => {
              e.currentTarget.style.borderColor = '#2563eb';
              e.currentTarget.style.color = '#2563eb';
              e.currentTarget.style.background = '#eff6ff';
            }}
            onMouseOut={(e) => {
              e.currentTarget.style.borderColor = '#e2e8f0';
              e.currentTarget.style.color = '#94a3b8';
              e.currentTarget.style.background = 'transparent';
            }}
          >
            <i className="fas fa-plus-circle" style={{ fontSize: 20 }} />
            Add Chart
          </button>
        )}

        {/* Render children with vertical resize handles */}
        {children.map((child, idx) => (
          <React.Fragment key={child.i}>
            <div
              style={{
                height: (child.h || 8) * ROW_HEIGHT,
                minHeight: MIN_H * ROW_HEIGHT,
                flexShrink: 0,
                position: 'relative',
              }}
            >
              <DashboardItem item={child} isEditMode={isEditMode} />
            </div>

            {/* Vertical resize handle below each child */}
            {isEditMode && (
              <div
                title={`Drag to resize · ${child.h || 8} rows`}
                style={{
                  height: 12,
                  flexShrink: 0,
                  cursor: 'row-resize',
                  display: 'flex',
                  alignItems: 'center',
                  justifyContent: 'center',
                  zIndex: 5,
                }}
                onMouseDown={(e) => startVerticalResize(child.i, child.h || 8, e)}
              >
                <div
                  style={{
                    width: '60%',
                    height: 4,
                    background: isHovered ? '#2563eb' : '#e2e8f0',
                    borderRadius: 2,
                    transition: 'background 0.15s ease',
                  }}
                />
              </div>
            )}
          </React.Fragment>
        ))}

        {/* "+ Add Chart" below existing children */}
        {children.length > 0 && isEditMode && builderCtx && (
          <button
            onClick={() => builderCtx.openChartPickerForContainer(item.i)}
            style={{
              marginTop: 4,
              padding: '6px',
              background: 'transparent',
              border: '2px dashed #e2e8f0',
              borderRadius: 6,
              cursor: 'pointer',
              color: '#94a3b8',
              fontSize: 12,
              fontWeight: 500,
              display: 'flex',
              alignItems: 'center',
              justifyContent: 'center',
              gap: 6,
              transition: 'all 0.15s ease',
              flexShrink: 0,
            }}
            onMouseOver={(e) => {
              e.currentTarget.style.borderColor = '#2563eb';
              e.currentTarget.style.color = '#2563eb';
              e.currentTarget.style.background = '#eff6ff';
            }}
            onMouseOut={(e) => {
              e.currentTarget.style.borderColor = '#e2e8f0';
              e.currentTarget.style.color = '#94a3b8';
              e.currentTarget.style.background = 'transparent';
            }}
          >
            <i className="fas fa-plus" />
            Add Chart
          </button>
        )}
      </div>
    </div>
  );
};

export default DashboardColumnComponent;
