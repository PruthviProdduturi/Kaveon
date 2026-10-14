/**
 * Dashboard Canvas
 *
 * Every root item — charts, text, headers, dividers AND the row/column/tabs
 * containers — is positioned by x/y/w/h on one fixed twelve-column
 * react-grid-layout (v2) grid: drag any tile to reposition, drag a corner/edge
 * to resize width AND height independently, with vertical compaction.
 *
 * Responsiveness is derived here rather than delegated to react-grid-layout's
 * breakpoints — see utils/dashboardGrid. Edit mode always renders the authored
 * grid, because onLayoutChange persists whatever coordinates are on screen and
 * a derived layout must never be written back.
 *
 * Every state renders inside ONE `.dashboard-canvas` element, and that element
 * carries react-grid-layout's measuring ref. It has to: `useContainerWidth`
 * measures and installs its ResizeObserver in a single effect whose only
 * dependency is its own callback, so the node held by the ref on the canvas's
 * first mount is the only node it will ever see. Returning a different root for
 * the empty state meant the ref was still null when that effect ran, `mounted`
 * stayed false for the life of the component, and the `mounted && width > 0`
 * guard below then withheld the grid *forever* — so on a brand-new dashboard
 * the first chart added never appeared, and nothing could be dragged, moved or
 * resized. See docs/engineering/OPEN-DEFECTS.md D-2.
 *
 * Containers are tiles, not an escape from the grid (D-31). Until now a single
 * root `row` or `column` switched the whole canvas to a plain vertical stack
 * with no grid at all, so one click of Row in the Items palette silently killed
 * drag and resize for every tile on that dashboard — which is why, across eight
 * live dashboards and 73 items, not one row, column or tab had ever been used.
 * Row and column already carry grid dimensions (`DEFAULT_COMPONENT_DIMENSIONS`
 * gives a row 12x4, a column 3x8, and a row `maxW: 12`); nothing but this
 * component's own branch kept them off the grid.
 *
 * A tile drags by its whole body. The grab strip along its top edge is an
 * affordance, not the only way in: `dragConfig.handle` was briefly declared and
 * that took whole-tile dragging away, which is the behaviour the builder is
 * meant to have. What keeps the gesture out of the chart's way instead is
 * DRAG_THRESHOLD plus DRAG_CANCEL.
 */
'use client';

import React from 'react';
// react-grid-layout v2 API (installed 2.2.4). @types are v1, so treat as any.
import * as RGL from 'react-grid-layout';
import { useDashboard } from './DashboardContext';
import { useBuilderContext } from './BuilderContext';
import DashboardItem from './DashboardItem';
import styles from './DashboardCanvas.module.css';
import {
  DEFAULT_COMPONENT_DIMENSIONS,
  MIN_COMPONENT_DIMENSIONS,
  MAX_COMPONENT_DIMENSIONS,
} from '../../types/dashboard';
import type { ComponentType, DashboardLayoutItem } from '../../types/dashboard';
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
/**
 * Nothing in this list may begin a tile drag.
 *
 * The first group is the controls — a press on a button, link or field is that
 * control's, never the tile's. The rest are the drag surfaces that belong to
 * something *inside* a tile, and they matter because a container tile is full
 * of them:
 *
 *  - `[draggable="true"]` is the column's child reorder grip, which runs an
 *    HTML5 drag of its own. Both gestures would otherwise start at once.
 *  - `[data-no-tile-drag]` is worn by the row's column-width bar and the
 *    column's child-height bar. Both also stopPropagation, so this is belt and
 *    braces — but it states the intent where a reader will look for it.
 *  - `.dashboard-tab-drag-handle` and `[data-nested-grid-item]` are the tabs
 *    component's inner grid. That grid is the one place a declared handle is
 *    unavoidable: two grids cannot both treat the same press as a whole-tile
 *    drag, so the inner one takes the handle and the outer one stands off its
 *    items entirely.
 */
const DRAG_CANCEL = [
  'button',
  'a',
  'input',
  'select',
  'textarea',
  '[draggable="true"]',
  '[data-no-tile-drag]',
  '.dashboard-tab-drag-handle',
  '[data-nested-grid-item]',
].join(', ');
/**
 * Pixels the pointer must travel before a press becomes a drag (the library
 * default is 3). A dashboard tile is almost entirely chart, and the whole tile
 * is the drag surface, so this is what separates "I clicked a bar to
 * cross-filter" from "I moved the tile": below the threshold the gesture is
 * never reported to the grid at all, and the click lands on the chart. Measured
 * in a browser against the installed library — a 4px press-and-release on a
 * chart canvas produces no drag-start and no layout change.
 */
const DRAG_THRESHOLD = 8;
const RESIZE_HANDLES = ['se', 'e', 's'];

interface DashboardCanvasProps {
  className?: string;
}

/** Grid dimensions for a type, falling back to a chart-sized tile. */
const dimsFor = (type: ComponentType) =>
  DEFAULT_COMPONENT_DIMENSIONS[type] || DEFAULT_COMPONENT_DIMENSIONS.chart;

const DashboardCanvas: React.FC<DashboardCanvasProps> = ({ className = '' }) => {
  const { layout, setLayout, adoptLayout, isEditMode, addLayoutItem } = useDashboard();
  // Present inside the builder, null in view-only mode. The builder owns the
  // chart picker and the palette drag; the canvas only asks for them.
  const builder = useBuilderContext();
  // Measure before the first paint so tiles never land on a stale width.
  const { width, mounted, containerRef } = useContainerWidth({ measureBeforeMount: true });
  // react-grid-layout reports its own normalisation of the authored layout
  // (vertical compaction, defaulted widths) as soon as it mounts. That is not
  // an edit, so it must not light the Unsaved badge; only coordinates that
  // follow a drag or a resize are the user's.
  const userMovedTilesRef = React.useRef(false);

  const rootItems = layout.filter((item) => !item.parentId);
  const isEmpty = rootItems.length === 0;

  // ── Placements ───────────────────────────────────────────────────────────────
  // Authored coordinates, with per-type defaults for anything a legacy record
  // left unset. A row defaults to the full twelve columns rather than six,
  // because a row that is half the canvas is not a row.
  const authored: GridPlacement[] = rootItems.map((it) => {
    const def = dimsFor(it.type);
    const min = MIN_COMPONENT_DIMENSIONS[it.type] || MIN_COMPONENT_DIMENSIONS.chart;
    const max = MAX_COMPONENT_DIMENSIONS[it.type] || {};
    return {
      i: it.i,
      x: typeof it.x === 'number' ? it.x : 0,
      y: typeof it.y === 'number' ? it.y : 0,
      w: it.w || def.w,
      h: it.h || def.h,
      minW: it.minW || min.w,
      minH: it.minH || min.h,
      // maxW/maxH only exist for some types; the grid's minMaxSize constraint
      // reads them off the layout item, and a row's maxW: 12 is what keeps it
      // full width when it is resized.
      ...(it.maxW ?? max.w ? { maxW: it.maxW ?? max.w } : {}),
      ...(it.maxH ?? max.h ? { maxH: it.maxH ?? max.h } : {}),
    } as GridPlacement;
  });

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
    if (!changed) return;
    if (userMovedTilesRef.current) setLayout(next);
    else adoptLayout(next);
  };

  const markUserMove = () => { userMovedTilesRef.current = true; };

  /**
   * "Add Chart" picks a chart first.
   *
   * It used to call `addLayoutItem('chart')` with no config, which puts a chart
   * item on the canvas with no `chartId`. DashboardChartComponent then renders
   * `item.chartId!` — undefined — and the tile reports "Failed to load chart"
   * the moment it appears. The builder already holds the picker state for
   * exactly this (`chartPickerTarget`), so the button opens it and the chosen
   * chart arrives with its id. Outside the builder there is no picker, and no
   * button either, so the direct add stays as the fallback.
   */
  const handleAddChart = () => {
    if (builder) builder.openChartPickerForContainer(null);
    else addLayoutItem('chart');
  };

  // ── Drop from the Items palette ──────────────────────────────────────────────
  /**
   * The grid sizes the drop placeholder while the pointer is still moving, and
   * `dataTransfer.getData` is unreadable until the drop itself, so the payload
   * travels through the builder (a ref, not state — it changes on every
   * dragover and must not re-render the canvas) and the MIME type travels on
   * the event for anything that only needs to recognise the drag.
   *
   * Returning `false` rejects the drag: that happens for any drag the palette
   * did not start, which includes the HTML5 drag the column's own reorder grip
   * runs inside a tile. Without the rejection that gesture would leave a
   * phantom placeholder on the grid.
   */
  const handleDropDragOver = () => {
    const payload = builder?.pendingPaletteDrag.current;
    if (!isEditMode || !payload) return false;
    return dimsFor(payload.type);
  };

  const handleDrop = (_current: any[], dropped: any) => {
    const payload = builder?.pendingPaletteDrag.current || null;
    if (builder) builder.pendingPaletteDrag.current = null;
    if (!isEditMode || !payload || !dropped) return;
    const def = dimsFor(payload.type);
    const min = MIN_COMPONENT_DIMENSIONS[payload.type] || MIN_COMPONENT_DIMENSIONS.chart;
    const max = MAX_COMPONENT_DIMENSIONS[payload.type] || {};
    markUserMove();
    // The grid reports the cell the pointer released over; everything else
    // about the item comes from its type, the same as a click-to-add.
    addLayoutItem(payload.type, {
      ...(payload.config as Partial<DashboardLayoutItem>),
      x: Math.max(0, Math.min(DASHBOARD_GRID_COLS - def.w, dropped.x ?? 0)),
      y: Math.max(0, dropped.y ?? 0),
      w: def.w,
      h: def.h,
      minW: min.w,
      minH: min.h,
      maxW: max.w,
      maxH: max.h,
    });
  };

  /**
   * The empty canvas has no grid to drop onto — an autosized grid with no
   * children is twenty pixels tall — so the host element takes the drop and
   * the item lands at the origin. Once there is one tile the grid is the
   * target and stops the event before it reaches here.
   */
  const hostDragOver = (e: React.DragEvent) => {
    if (!isEditMode || !builder?.pendingPaletteDrag.current) return;
    e.preventDefault();
  };
  const hostDrop = (e: React.DragEvent) => {
    const payload = builder?.pendingPaletteDrag.current || null;
    if (!isEditMode || !payload) return;
    e.preventDefault();
    builder!.pendingPaletteDrag.current = null;
    markUserMove();
    addLayoutItem(payload.type, payload.config as Partial<DashboardLayoutItem>);
  };

  return (
    <div
      ref={containerRef}
      // `dashboard-canvas-host-empty` only makes this a column flex container so
      // the empty state's `flex: 1` still has a parent to stretch against now
      // that it is nested here instead of replacing this element.
      className={`dashboard-canvas${isEmpty ? ' dashboard-canvas-host-empty' : ''} ${className}`}
      style={isEmpty ? undefined : { paddingBottom: 32 }}
      onDragOver={hostDragOver}
      onDrop={hostDrop}
    >
      {isEmpty && (
        <div className="dashboard-canvas-empty">
          <div className="dashboard-canvas-empty-icon"><i className="fas fa-th-large" /></div>
          <div className="dashboard-canvas-empty-title">
            {isEditMode ? 'Your dashboard is empty' : 'No content available'}
          </div>
          <div className="dashboard-canvas-empty-subtitle">
            {isEditMode
              ? 'Drag a chart from the Items list, or add one here'
              : 'This dashboard has no content to display'}
          </div>
          {isEditMode && (
            <button onClick={handleAddChart} style={addBtnStyle}>
              <i className="fas fa-plus" /> Add Chart
            </button>
          )}
        </div>
      )}

      {!isEmpty && mounted && width > 0 && (
        <GridLayout
          width={width}
          layout={placements}
          gridConfig={GRID_CONFIG}
          dragConfig={{ enabled: isEditMode, cancel: DRAG_CANCEL, threshold: DRAG_THRESHOLD }}
          resizeConfig={{ enabled: isEditMode, handles: RESIZE_HANDLES }}
          dropConfig={{
            enabled: isEditMode,
            defaultItem: DEFAULT_COMPONENT_DIMENSIONS.chart,
            onDragOver: handleDropDragOver,
          }}
          onLayoutChange={handleLayoutChange}
          onDragStart={markUserMove}
          onResizeStart={markUserMove}
          onDrop={handleDrop}
        >
          {rootItems.map((item) => {
            const isContainer = item.type === 'row' || item.type === 'column';
            return (
              // A tabs tile needs no overflow of its own: the grid it holds
              // lives inside `.dashboard-component-body`, which already
              // scrolls.
              <div
                key={item.i}
                className={`${styles.tile}${isContainer ? ` ${styles.tileContainer}` : ''}`}
              >
                {isEditMode && (
                  <div
                    className={`dashboard-drag-handle ${styles.grab}`}
                    title="Drag to move"
                  >
                    <i className="fas fa-grip-lines" />
                  </div>
                )}
                <DashboardItem item={item} isEditMode={isEditMode} />
              </div>
            );
          })}
        </GridLayout>
      )}

      {!isEmpty && isEditMode && (
        <div style={{ display: 'flex', justifyContent: 'center', padding: '12px 0 8px', gap: 10 }}>
          <button onClick={handleAddChart} style={addBtnStyle}>
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
  padding: '9px 22px', background: 'var(--accent)', color: '#fff', border: 'none',
  borderRadius: 8, cursor: 'pointer', fontSize: 14, fontWeight: 600,
  display: 'inline-flex', alignItems: 'center', gap: 8,
};

export default DashboardCanvas;
