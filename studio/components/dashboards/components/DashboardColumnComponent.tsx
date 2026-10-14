/**
 * Dashboard Column Component
 *
 * Renders a vertical container whose children are stacked top-to-bottom.
 * No header bar — edit actions appear as a floating overlay on hover.
 * Each child's height is resizable by dragging the handle at its bottom edge.
 */

import React, { useState, useCallback, useRef } from 'react';
import dynamic from 'next/dynamic';
import type { DashboardComponentProps } from '../../../types/dashboard';
import { useDashboard } from '../DashboardContext';
import { useBuilderContext, moveDragProps, useContainerDropTarget } from '../BuilderContext';

const DashboardItem = dynamic(() => import('../DashboardItem'), { ssr: false });

/** Height of one grid row in pixels — must match rowHeight used elsewhere */
const ROW_HEIGHT = 30;
/** Minimum chart height in grid rows */
const MIN_H = 3;

/** Add-block menu items */
const ADD_ITEMS = [
  { type: 'chart' as const,   icon: 'fas fa-chart-bar',  label: 'Chart' },
  { type: 'header' as const,  icon: 'fas fa-heading',    label: 'Header' },
  { type: 'text' as const,    icon: 'fas fa-align-left', label: 'Text' },
  { type: 'divider' as const, icon: 'fas fa-minus',      label: 'Divider' },
];

const DashboardColumnComponent: React.FC<DashboardComponentProps> = ({
  item,
  isEditMode,
}) => {
  const { removeItemFromContainer, removeLayoutItem, updateLayoutItem, addItemToContainer } = useDashboard();
  const builderCtx = useBuilderContext();
  const children = item.children || [];
  const [isHovered, setIsHovered] = useState(false);
  const [menuOpen, setMenuOpen] = useState(false);
  const menuRef = useRef<HTMLDivElement>(null);
  const [dragOverIndex, setDragOverIndex] = useState<number | null>(null);
  const { isDropTarget, dropProps } = useContainerDropTarget(
    item.i,
    'column',
    !!isEditMode && !!builderCtx
  );

  // ── Child drag-to-reorder ─────────────────────────────────────────────────────

  /**
   * The grip's drag means two things, and which one it is depends only on where
   * it is released: dropped on a sibling slot in this column it reorders,
   * dropped on the canvas or another container it moves the child out. The
   * index below drives the reorder; `moveDragProps` arms the payload the other
   * drop targets read. One gesture, so the user does not have to know.
   */
  const handleChildDragStart = (e: React.DragEvent, index: number) => {
    e.dataTransfer.setData('text/plain', String(index));
    e.dataTransfer.effectAllowed = 'move';
    const ghost = document.createElement('div');
    ghost.textContent = 'Moving item…';
    ghost.style.cssText =
      'position:fixed;top:-200px;left:0;background:#1e293b;color:#fff;' +
      'padding:5px 12px;border-radius:6px;font-size:12px;font-weight:600;pointer-events:none;';
    document.body.appendChild(ghost);
    e.dataTransfer.setDragImage(ghost, 50, 14);
    requestAnimationFrame(() => document.body.removeChild(ghost));
  };

  /**
   * Is the drag in flight one of THIS column's own children?
   *
   * The reorder below reads an index out of `dataTransfer`, and an index is
   * only meaningful within the column that issued it. Without this check,
   * dragging a child of one column over another column's slot reordered the
   * wrong column's children using a stranger's index.
   */
  const isOwnChildDrag = () => {
    const payload = builderCtx?.pendingDrag.current;
    return !!payload && payload.kind === 'move' && payload.fromParentId === item.i;
  };

  const handleChildDragOver = (e: React.DragEvent, index: number) => {
    if (!isOwnChildDrag()) return;
    e.preventDefault();
    e.stopPropagation();
    e.dataTransfer.dropEffect = 'move';
    setDragOverIndex(index);
  };

  const handleChildDrop = (e: React.DragEvent, dropIndex: number) => {
    setDragOverIndex(null);
    if (!isOwnChildDrag()) return;
    e.preventDefault();
    e.stopPropagation();
    const fromIndex = parseInt(e.dataTransfer.getData('text/plain'), 10);
    if (builderCtx) builderCtx.pendingDrag.current = null;
    if (isNaN(fromIndex) || fromIndex === dropIndex) return;
    const reordered = [...children];
    const [moved] = reordered.splice(fromIndex, 1);
    reordered.splice(dropIndex, 0, moved);
    updateLayoutItem(item.i, { children: reordered });
  };

  // ── Delete self ──────────────────────────────────────────────────────────────

  const handleDelete = useCallback(() => {
    if (item.parentId) {
      removeItemFromContainer(item.parentId, item.i);
    } else {
      removeLayoutItem(item.i);
    }
  }, [item.i, item.parentId, removeLayoutItem, removeItemFromContainer]);

  // ── Add item to this column ──────────────────────────────────────────────────

  const handleAdd = useCallback((type: typeof ADD_ITEMS[number]['type']) => {
    setMenuOpen(false);
    if (type === 'chart') {
      builderCtx?.openChartPickerForContainer(item.i);
      return;
    }
    const configs: Record<string, any> = {
      header:  { headerConfig:  { content: 'Section Header', size: 'h2', alignment: 'left' } },
      text:    { textConfig:    { content: 'Click to edit text', alignment: 'left', fontSize: 14 } },
      divider: { dividerConfig: { orientation: 'horizontal', thickness: 1, color: '#e2e8f0', style: 'solid' } },
    };
    addItemToContainer(item.i, type, configs[type] || {});
  }, [item.i, addItemToContainer, builderCtx]);

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
      className="dashboard-column"
      onMouseEnter={() => setIsHovered(true)}
      onMouseLeave={() => { setIsHovered(false); setMenuOpen(false); }}
      {...dropProps}
      style={{
        display: 'flex',
        flexDirection: 'column',
        height: '100%',
        // Only shown when the model will take the drop. A column cannot hold a
        // row or another column, so dragging one over it changes nothing.
        background: isDropTarget
          ? 'rgba(var(--accent-rgb), 0.08)'
          : isEditMode ? 'var(--bg-primary)' : 'transparent',
        border: isDropTarget
          ? '2px dashed var(--accent)'
          : isEditMode ? '1px dashed var(--border)' : 'none',
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
          {/* Add block dropdown */}
          <div ref={menuRef} style={{ position: 'relative' }}>
            <button
              onClick={() => setMenuOpen((v) => !v)}
              title="Add block"
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
              Add
              <i className="fas fa-chevron-down" style={{ fontSize: 8, marginLeft: 1 }} />
            </button>

            {menuOpen && (
              <div
                style={{
                  position: 'absolute',
                  top: '100%',
                  right: 0,
                  marginTop: 4,
                  background: '#fff',
                  border: '1px solid #e2e8f0',
                  borderRadius: 6,
                  boxShadow: '0 4px 16px rgba(0,0,0,0.12)',
                  overflow: 'hidden',
                  minWidth: 130,
                  zIndex: 100,
                }}
              >
                {ADD_ITEMS.map((opt) => (
                  (opt.type === 'chart' && !builderCtx) ? null : (
                    <button
                      key={opt.type}
                      onClick={() => handleAdd(opt.type)}
                      style={{
                        width: '100%',
                        display: 'flex',
                        alignItems: 'center',
                        gap: 8,
                        padding: '7px 12px',
                        background: 'transparent',
                        border: 'none',
                        cursor: 'pointer',
                        fontSize: 12,
                        color: '#374151',
                        textAlign: 'left',
                        whiteSpace: 'nowrap',
                      }}
                      onMouseOver={(e) => { e.currentTarget.style.background = '#f1f5f9'; }}
                      onMouseOut={(e) => { e.currentTarget.style.background = 'transparent'; }}
                    >
                      <i className={opt.icon} style={{ fontSize: 11, width: 14, color: '#64748b' }} />
                      {opt.label}
                    </button>
                  )
                ))}
              </div>
            )}
          </div>

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
        {children.length === 0 && isEditMode && (
          <div style={{ position: 'relative', flex: 1, minHeight: 80 }}>
            <div
              style={{
                height: '100%',
                minHeight: 80,
                display: 'flex',
                flexDirection: 'column',
                alignItems: 'center',
                justifyContent: 'center',
                gap: 12,
                background: 'transparent',
                border: '2px dashed #e2e8f0',
                borderRadius: 6,
                color: '#94a3b8',
                fontSize: 12,
                padding: '16px 12px',
              }}
            >
              <span style={{ fontWeight: 500 }}>Add a block</span>
              <div style={{ display: 'flex', gap: 6, flexWrap: 'wrap', justifyContent: 'center' }}>
                {ADD_ITEMS.map((opt) =>
                  opt.type === 'chart' && !builderCtx ? null : (
                    <button
                      key={opt.type}
                      onClick={() => handleAdd(opt.type)}
                      title={`Add ${opt.label}`}
                      style={{
                        display: 'flex',
                        alignItems: 'center',
                        gap: 5,
                        padding: '4px 10px',
                        background: '#fff',
                        border: '1px solid #e2e8f0',
                        borderRadius: 4,
                        cursor: 'pointer',
                        fontSize: 11,
                        color: '#475569',
                        fontWeight: 500,
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
                      <i className={opt.icon} style={{ fontSize: 10 }} />
                      {opt.label}
                    </button>
                  )
                )}
              </div>
            </div>
          </div>
        )}

        {/* Render children with drag-to-reorder + resize handles */}
        {children.map((child, idx) => {
          const isDivider = child.type === 'divider';
          const childHeight = isDivider
            ? Math.max(1, child.h || 1) * ROW_HEIGHT
            : Math.max(MIN_H * ROW_HEIGHT, (child.h || 8) * ROW_HEIGHT);
          const isDropTarget = dragOverIndex === idx;

          return (
            <React.Fragment key={child.i}>
              <div
                onDragOver={(e) => handleChildDragOver(e, idx)}
                onDrop={(e) => handleChildDrop(e, idx)}
                onDragLeave={() => setDragOverIndex(null)}
                style={{
                  flexShrink: 0,
                  marginBottom: 8,
                  outline: isDropTarget ? '2px dashed #2563eb' : '2px solid transparent',
                  borderRadius: 5,
                  transition: 'outline 0.1s',
                }}
              >
                {/* Drag handle — only in edit mode */}
                {isEditMode && builderCtx && (
                  <div
                    {...moveDragProps(
                      builderCtx.pendingDrag,
                      { ...child, parentId: item.i },
                      { onDragStart: (e) => handleChildDragStart(e, idx) }
                    )}
                    title="Drag to reorder, or out to the canvas"
                    style={{
                      display: 'flex',
                      alignItems: 'center',
                      gap: 5,
                      padding: '2px 6px',
                      cursor: 'grab',
                      userSelect: 'none',
                      color: '#cbd5e1',
                      fontSize: 10,
                      fontWeight: 600,
                    }}
                  >
                    <i className="fas fa-grip-horizontal" style={{ fontSize: 9 }} />
                  </div>
                )}

                <div style={{ height: childHeight, position: 'relative' }}>
                  <DashboardItem item={child} isEditMode={isEditMode} />
                </div>

                {/* Vertical resize handle — hidden for dividers */}
                {isEditMode && !isDivider && (
                  <div
                    title={`Drag to resize · ${child.h || 8} rows`}
                    // The column is itself a draggable grid tile, and this bar
                    // is its own gesture. Declared in DashboardCanvas's
                    // DRAG_CANCEL so a press here never also moves the tile.
                    data-no-tile-drag="true"
                    style={{
                      height: 12,
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
              </div>
            </React.Fragment>
          );
        })}

        {/* "+ Add block" below existing children */}
        {children.length > 0 && isEditMode && (
          <div style={{ position: 'relative', marginTop: 4 }}>
            <button
              onClick={() => setMenuOpen((v) => !v)}
              style={{
                width: '100%',
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
              Add block
            </button>

            {menuOpen && (
              <div
                style={{
                  position: 'absolute',
                  bottom: '100%',
                  left: 0,
                  marginBottom: 4,
                  background: '#fff',
                  border: '1px solid #e2e8f0',
                  borderRadius: 6,
                  boxShadow: '0 4px 16px rgba(0,0,0,0.12)',
                  overflow: 'hidden',
                  minWidth: 130,
                  zIndex: 100,
                }}
              >
                {ADD_ITEMS.map((opt) =>
                  opt.type === 'chart' && !builderCtx ? null : (
                    <button
                      key={opt.type}
                      onClick={() => handleAdd(opt.type)}
                      style={{
                        width: '100%',
                        display: 'flex',
                        alignItems: 'center',
                        gap: 8,
                        padding: '7px 12px',
                        background: 'transparent',
                        border: 'none',
                        cursor: 'pointer',
                        fontSize: 12,
                        color: '#374151',
                        textAlign: 'left',
                        whiteSpace: 'nowrap',
                      }}
                      onMouseOver={(e) => { e.currentTarget.style.background = '#f1f5f9'; }}
                      onMouseOut={(e) => { e.currentTarget.style.background = 'transparent'; }}
                    >
                      <i className={opt.icon} style={{ fontSize: 11, width: 14, color: '#64748b' }} />
                      {opt.label}
                    </button>
                  )
                )}
              </div>
            )}
          </div>
        )}
      </div>
    </div>
  );
};

export default DashboardColumnComponent;
