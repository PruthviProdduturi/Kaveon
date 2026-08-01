/**
 * BuilderContext
 *
 * Provides a bridge between the DashboardBuilder (which owns the chart picker
 * modal and the available-charts list) and the row/column components that need
 * to trigger "add chart to THIS container" from deep in the tree.
 *
 * This context is only provided inside DashboardBuilder – in view-only mode
 * useBuilderContext() returns null, which the components check before rendering
 * edit controls.
 */

import { createContext, useContext } from 'react';

export interface BuilderContextValue {
  /** Open the chart picker modal targeting a specific container, or null for root canvas */
  openChartPickerForContainer: (containerId: string | null) => void;
}

export const BuilderContext = createContext<BuilderContextValue | null>(null);

/**
 * Hook to access builder context.
 * Returns null when used outside of a DashboardBuilder (i.e. in view mode).
 */
export const useBuilderContext = (): BuilderContextValue | null =>
  useContext(BuilderContext);
