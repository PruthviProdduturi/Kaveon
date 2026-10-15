/**
 * Dashboard Row Component
 *
 * Renders a full-width horizontal container whose children (columns or charts)
 * are placed side-by-side with flex-grow widths proportional to their `w` value.
 *
 * Edit-mode features:
 *  - Hover toolbar: "Add Column", "Add Chart", "Delete Row"
 *  - Drag handles between children to resize column widths
 *  - Empty-row placeholder with call-to-action
 *  - Accepts a drop: a palette entry, or an existing tile dragged in from the
 *    canvas or out of another container, AT A POSITION — the indicator between
 *    two children is where the drop lands (D-37)
 *  - Each child carries a grip, which is the only way a child can be dragged
 *    out — the row itself is a grid tile whose whole body is a drag surface, so
 *    a child cannot share that gesture
 */

import React, { useRef, useState, useCallback } from 'react';
import dynamic from 'next/dynamic';
import type { DashboardComponentProps } from '../../../types/dashboard';
import { GRID_COLUMNS } from '../../../types/dashboard';
import { useDashboard } from '../DashboardContext';
import {
  useBuilderContext,
  moveDragProps,
  useContainerDropTarget,
  CHILD_SLOT_ATTR,
} from '../BuilderContext';

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
  // The element whose direct `[data-child-slot]` children are the drop slots.
  const contentRef = useRef<HTMLDivElement>(null);
  const [isHovered, setIsHovered] = useState(false);
  const { isDropTarget, dropSlot, dropProps } = useContainerDropTarget(
    item.i,
    'row',
    !!isEditMode && !!builderCtx,
    { axis: 'x', contentRef }
  );

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
      className="dashboard-row"
      onMouseEnter={() => setIsHovered(true)}
      onMouseLeave={() => setIsHovered(false)}
      {...dropProps}
      style={{
        // Only shown when the model will actually take the drop — the hook
        // asks `canMoveItemToContainer` first, so a row that cannot hold what
        // is being dragged (another row) stays exactly as it was.
        background: isDropTarget ? 'rgba(var(--accent-rgb), 0.08)' : 'transparent',
        border: isDropTarget
          ? '2px dashed var(--accent)'
          : isEditMode ? '1px solid var(--border)' : 'none',
        borderRadius: 8,
        position: 'relative',
        // A row is a grid tile now (D-31), so it fills the height the author
        // gave that tile instead of collapsing to its content. Without this a
        // chart placed directly in a row has no height to inherit.
        height: '100%',
        boxSizing: 'border-box',
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
              background: 'var(--bg-elevated)',
              border: '1px solid var(--border)',
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
                color: 'var(--text-faint)',
                textTransform: 'uppercase',
                letterSpacing: '0.5px',
                padding: '2px 6px',
              }}
            >
              Row
            </span>

            <div style={{ width: 1, height: 14, background: 'var(--border)', margin: '0 2px' }} />

            {/* Add Column */}
            <button
              onClick={() => addItemToContainer(item.i, 'column')}
              title="Add Column"
              style={{
                background: 'transparent',
                border: 'none',
                cursor: 'pointer',
                padding: '3px 8px',
                color: 'var(--text-secondary)',
                fontSize: 12,
                fontWeight: 500,
                display: 'flex',
                alignItems: 'center',
                gap: 4,
                borderRadius: 4,
                whiteSpace: 'nowrap',
              }}
              onMouseOver={(e) => {
                e.currentTarget.style.background = 'var(--bg-hover)';
                e.currentTarget.style.color = 'var(--text-primary)';
              }}
              onMouseOut={(e) => {
                e.currentTarget.style.background = 'transparent';
                e.currentTarget.style.color = 'var(--text-secondary)';
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
                  color: 'var(--text-secondary)',
                  fontSize: 12,
                  fontWeight: 500,
                  display: 'flex',
                  alignItems: 'center',
                  gap: 4,
                  borderRadius: 4,
                  whiteSpace: 'nowrap',
                }}
                onMouseOver={(e) => {
                  e.currentTarget.style.background = 'var(--bg-hover)';
                  e.currentTarget.style.color = 'var(--text-primary)';
                }}
                onMouseOut={(e) => {
                  e.currentTarget.style.background = 'transparent';
                  e.currentTarget.style.color = 'var(--text-secondary)';
                }}
              >
                <i className="fas fa-chart-bar" style={{ fontSize: 11 }} />
                Chart
              </button>
            )}

            <div style={{ width: 1, height: 14, background: 'var(--border)', margin: '0 2px' }} />

            {/* Delete Row */}
            <button
              onClick={handleDelete}
              title="Delete Row"
              style={{
                background: 'transparent',
                border: 'none',
                cursor: 'pointer',
                padding: '3px 8px',
                color: 'var(--danger, #ef4444)',
                fontSize: 12,
                borderRadius: 4,
                display: 'flex',
                alignItems: 'center',
              }}
              onMouseOver={(e) => { e.currentTarget.style.background = 'rgba(239, 68, 68, 0.10)'; }}
              onMouseOut={(e) => { e.currentTarget.style.background = 'transparent'; }}
            >
              <i className="fas fa-trash" style={{ fontSize: 11 }} />
            </button>
          </div>
        </div>
      )}

      {/* ── Row content ────────────────────────────────────────────────────── */}
      <div
        ref={contentRef}
        style={{
          display: 'flex',
          alignItems: 'stretch',
          // `minHeight` rather than `height`, so a row holding a column taller
          // than the row's own tile grows to its content instead of letting it
          // spill over the tile below. The canvas then grows the tile to match
          // — see the fit-to-content effect in DashboardCanvas (D-34).
          // Measured: with `height: 100%` the content box stayed at the tile's
          // 380px and the nested column's 480px of children overflowed it; with
          // `min-height` the content box reports 548px and the tile follows.
          // The empty-row floor is the tile's own minimum (row minH 2 = 80px),
          // so one rule covers both states.
          minHeight: '100%',
          padding: isEditMode ? 8 : 0,
          gap: 0,
          boxSizing: 'border-box',
          position: 'relative',
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
              color: 'var(--text-muted)',
              fontSize: 13,
              border: '2px dashed var(--border)',
              borderRadius: 6,
              gap: 8,
              padding: '16px 24px',
            }}
          >
            <i className="fas fa-plus-circle" />
            Add a column or chart using the toolbar above, or drag a tile in from the canvas
          </div>
        )}

        {/*
          Where the drop will land. Drawn on the boundary the resolver picked,
          so the indicator and the insertion index are the same number — and
          absolutely positioned rather than inserted between the children,
          because an element that takes up space would move the very rects the
          index is measured from.
        */}
        {isDropTarget && dropSlot && children.length > 0 && (
          <div
            aria-hidden
            style={{
              position: 'absolute',
              left: dropSlot.offset - 2,
              top: 4,
              bottom: 4,
              width: 4,
              borderRadius: 2,
              background: 'var(--accent)',
              boxShadow: '0 0 0 3px rgba(var(--accent-rgb), 0.25)',
              pointerEvents: 'none',
              zIndex: 25,
            }}
          />
        )}

        {/* Children with resize handles */}
        {children.map((child, idx) => (
          <React.Fragment key={child.i}>
            {/* Child item — width based on 12-column grid */}
            <div
              // One insertion slot. The drop resolver measures these rects to
              // decide which boundary the pointer is nearest.
              {...{ [CHILD_SLOT_ATTR]: '' }}
              style={{
                flex: `0 0 ${((child.w || 1) / GRID_COLUMNS) * 100}%`,
                minWidth: 0,
                position: 'relative',
                overflow: 'hidden',
              }}
            >
              {isEditMode && builderCtx && (
                <div
                  {...moveDragProps(builderCtx.pendingDrag, { ...child, parentId: item.i })}
                  title="Drag out to the canvas, or into another container"
                  style={{
                    display: 'flex',
                    alignItems: 'center',
                    justifyContent: 'center',
                    gap: 5,
                    height: 14,
                    cursor: 'grab',
                    userSelect: 'none',
                    color: 'var(--text-muted)',
                    fontSize: 9,
                  }}
                >
                  <i className="fas fa-grip-horizontal" />
                </div>
              )}
              <DashboardItem item={child} isEditMode={isEditMode} />
            </div>

            {/* Resize handle between adjacent children */}
            {isEditMode && idx < children.length - 1 && (
              <div
                title={`Drag to resize · ${child.w || 1} / ${GRID_COLUMNS} cols`}
                // The row is itself a draggable grid tile, and this bar is its
                // own gesture. Declared in DashboardCanvas's DRAG_CANCEL so a
                // press here never also starts moving the tile.
                data-no-tile-drag="true"
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
                    background: isHovered ? 'var(--accent)' : 'var(--border)',
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
