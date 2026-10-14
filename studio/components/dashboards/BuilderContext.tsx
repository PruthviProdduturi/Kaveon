/**
 * BuilderContext
 *
 * Provides a bridge between the DashboardBuilder (which owns the chart picker
 * modal, the available-charts list and the Items palette) and the canvas and
 * row/column components that need to reach back into it from deep in the tree.
 *
 * This context is only provided inside DashboardBuilder – in view-only mode
 * useBuilderContext() returns null, which the components check before rendering
 * edit controls.
 */

import { createContext, useContext } from 'react';
import type { ComponentType, DashboardLayoutItem } from '../../types/dashboard';

/** MIME type the Items palette stamps on a drag so a drop target can recognise it. */
export const DASHBOARD_DRAG_MIME = 'application/x-kaveon-dashboard-item';

/** What the Items palette is currently dragging, if anything. */
export interface PalettePayload {
  type: ComponentType;
  /** Type-specific fields the new item needs — `chartId`, `textConfig`, … */
  config?: Partial<DashboardLayoutItem>;
}

export interface BuilderContextValue {
  /** Open the chart picker modal targeting a specific container, or null for root canvas */
  openChartPickerForContainer: (containerId: string | null) => void;
  /**
   * The item being dragged out of the Items palette, or null.
   *
   * A ref rather than state on purpose. react-grid-layout asks for the dropped
   * item's dimensions on every `dragover`, and `DataTransfer.getData` is
   * deliberately unreadable until the `drop` event itself (the HTML5 protected
   * mode), so the payload has to live somewhere the canvas can read
   * synchronously — and re-rendering the whole canvas on each dragover while a
   * drag is in flight would stutter the placeholder.
   */
  pendingPaletteDrag: { current: PalettePayload | null };
}

export const BuilderContext = createContext<BuilderContextValue | null>(null);

/**
 * Hook to access builder context.
 * Returns null when used outside of a DashboardBuilder (i.e. in view mode).
 */
export const useBuilderContext = (): BuilderContextValue | null =>
  useContext(BuilderContext);
