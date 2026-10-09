/**
 * Dashboard grid geometry
 *
 * Dashboard tiles are authored on a fixed 12-column grid and persisted as
 * x/y/w/h on the dashboard record. Narrow canvases cannot show twelve legible
 * columns, so the canvas renders a *derived* layout rather than letting
 * react-grid-layout reflow for it: RGL's responsive mode drops the column count
 * and then clamps every tile that no longer fits onto the last column
 * (`correctBounds`), which pushes the tail of an authored row into a single
 * column and leaves the rest of that row as dead space.
 *
 * The derivation below keeps reading order, gives every row an explicit `y`
 * taken from the row above, and equalises heights within a row — so a row of
 * KPI tiles stays a row of equal tiles at every width, and a tile that moves
 * never leaves a hole behind it.
 */

/** Columns in the authored grid. Constant at every canvas width. */
export const DASHBOARD_GRID_COLS = 12;

/**
 * Padding the grid keeps between the canvas edge and the first/last tile.
 * Page chrome above the canvas (the dashboard header, the applied-filters card)
 * is inset by the same amount so every band shares one left and right edge.
 */
export const DASHBOARD_GRID_EDGE = 4;

/** Canvas width (not viewport width) below which tiles pair up two per row. */
export const DASHBOARD_PAIRED_WIDTH = 900;

/** Canvas width below which every tile takes a full row of its own. */
export const DASHBOARD_STACKED_WIDTH = 600;

/** The part of a layout item that places it on the grid. */
export interface GridPlacement {
  i: string;
  x: number;
  y: number;
  w: number;
  h: number;
  minW: number;
  minH: number;
}

/**
 * Tiles per row for a measured canvas width, or `null` when the canvas is wide
 * enough to render the authored twelve-column layout unchanged.
 */
export function tilesPerRow(width: number): number | null {
  if (width >= DASHBOARD_PAIRED_WIDTH) return null;
  return width >= DASHBOARD_STACKED_WIDTH ? 2 : 1;
}

/**
 * Rebuild an authored layout as full rows of at most `perRow` tiles, in reading
 * order. Every row is packed edge to edge and stacked directly under the one
 * above, so the result is gap-free for any tile count or size mixture.
 */
export function reflowPlacements(placements: GridPlacement[], perRow: number): GridPlacement[] {
  const ordered = [...placements].sort((a, b) => a.y - b.y || a.x - b.x);
  const reflowed: GridPlacement[] = [];
  let row: GridPlacement[] = [];
  let y = 0;

  // Lay the buffered row out edge to edge. A row left holding a single tile
  // spans the full width instead of leaving its remaining columns empty.
  const flushRow = () => {
    if (row.length === 0) return;
    const span = DASHBOARD_GRID_COLS / row.length;
    const height = Math.max(...row.map((placement) => Math.max(placement.h, placement.minH)));
    row.forEach((placement, index) => {
      reflowed.push({
        ...placement,
        x: index * span,
        y,
        w: span,
        h: height,
        minW: Math.min(placement.minW, span),
      });
    });
    y += height;
    row = [];
  };

  ordered.forEach((placement) => {
    // A tile authored wider than half the grid keeps a row to itself: pairing a
    // map or a wide table with a neighbour squeezes it past readability.
    if (perRow > 1 && placement.w * 2 > DASHBOARD_GRID_COLS) {
      flushRow();
      row = [placement];
      flushRow();
      return;
    }
    row.push(placement);
    if (row.length === perRow) flushRow();
  });
  flushRow();

  return reflowed;
}
