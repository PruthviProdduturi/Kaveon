/**
 * Dashboard Type Definitions
 *
 * This file contains TypeScript type definitions for the dashboard system,
 * including layout items, filters, and component configurations.
 */

/**
 * Supported dashboard component types
 */
export type ComponentType = 'chart' | 'text' | 'header' | 'tabs' | 'divider' | 'row' | 'column';

/**
 * Filter operators supported by the filter system
 */
export type FilterOperator = '=' | '!=' | '>' | '<' | '>=' | '<=' | 'IN' | 'NOT IN' | 'LIKE' | 'NOT LIKE';

/**
 * Filter logic for combining multiple filters
 */
export type FilterLogic = 'AND' | 'OR';

/**
 * Configuration for a single filter
 */
export interface FilterConfig {
  /** Unique identifier for the filter */
  id: string;
  /** Column to filter on (format: table_name.column_name) */
  column: string;
  /** Friendly display name shown in the filter bar (defaults to the column) */
  label?: string;
  /** Filter operator */
  operator: FilterOperator;
  /** Filter value(s) - comma-separated for IN/NOT IN operators */
  value: string;
  /** Key value corresponding to the display value (for dimension key optimization) */
  valueKey?: string;
  /** Key column name (e.g., "table.ProductKey") used for direct fact table filtering */
  keyColumn?: string;
  /** Available options for this filter (populated from API) */
  options?: string[];
  /** Whether the filter is currently loading options */
  isLoading?: boolean;
  /** Whether the filter has pending changes */
  isPending?: boolean;
  /** Dataset ID this filter belongs to (used for fetching distinct values) */
  datasetId?: number;
  /** Filter UI type: 'value' (default) or 'date_range' for date pickers */
  filterType?: 'value' | 'date_range';
  /** Start date for date_range filters (ISO date string YYYY-MM-DD) */
  dateFrom?: string;
  /** End date for date_range filters (ISO date string YYYY-MM-DD) */
  dateTo?: string;
}

/**
 * Dashboard-level filter with application scope
 */
export interface DashboardFilter extends FilterConfig {
  /** Which charts this filter applies to */
  appliesTo: 'all' | number[];
  /** Whether this filter is currently enabled */
  enabled: boolean;
}

/**
 * Configuration for a tab in the tabs component
 */
export interface TabConfig {
  /** Unique identifier for the tab */
  id: string;
  /** Display name of the tab */
  name: string;
  /** Layout items contained within this tab */
  layout: DashboardLayoutItem[];
}

/**
 * Text alignment options
 */
export type TextAlignment = 'left' | 'center' | 'right';

/**
 * Header size options (maps to H1, H2, H3)
 */
export type HeaderSize = 'large' | 'medium' | 'small';

/**
 * Divider orientation
 */
export type DividerOrientation = 'horizontal' | 'vertical';

/**
 * Configuration for text components
 */
export interface TextComponentConfig {
  /** Text content (supports basic markdown) */
  content: string;
  /** Text alignment */
  alignment?: TextAlignment;
  /** Font size in pixels */
  fontSize?: number;
  /** Text color (hex) */
  color?: string;
}

/**
 * Configuration for header components
 */
export interface HeaderComponentConfig {
  /** Header text content */
  content: string;
  /** Header size */
  size: HeaderSize;
  /** Text alignment */
  alignment?: TextAlignment;
  /** Text color (hex) */
  color?: string;
}

/**
 * Configuration for divider components
 */
export interface DividerComponentConfig {
  /** Divider orientation */
  orientation: DividerOrientation;
  /** Line thickness in pixels */
  thickness?: number;
  /** Line color (hex) */
  color?: string;
  /** Line style */
  style?: 'solid' | 'dashed' | 'dotted';
}

/**
 * Configuration for row container components
 */
export interface RowContainerConfig {
  /** Horizontal alignment of children */
  alignment?: 'start' | 'center' | 'end' | 'stretch';
  /** Gap between children in pixels */
  gap?: number;
  /** Background color (hex or rgba) */
  backgroundColor?: string;
  /** Whether to show border */
  showBorder?: boolean;
  /** Padding in pixels */
  padding?: number;
}

/**
 * Configuration for column container components
 */
export interface ColumnContainerConfig {
  /** Vertical alignment of children */
  verticalAlignment?: 'start' | 'center' | 'end' | 'stretch';
  /** Gap between children in pixels */
  gap?: number;
  /** Background color (hex or rgba) */
  backgroundColor?: string;
  /** Whether to show border */
  showBorder?: boolean;
  /** Padding in pixels */
  padding?: number;
}

/**
 * A single item in the dashboard grid layout
 *
 * This interface defines the position, size, and configuration of
 * a component in the dashboard grid. The grid uses a 12-column layout.
 */
export interface DashboardLayoutItem {
  /** Unique key for this layout item (required by react-grid-layout) */
  i: string;

  /** Grid column position (0-11) */
  x: number;

  /** Grid row position (0-based) */
  y: number;

  /** Width in grid units (1-12) */
  w: number;

  /** Height in grid units */
  h: number;

  /** Type of component to render */
  type: ComponentType;

  /** Chart ID (required for chart components) */
  chartId?: number;

  /** Configuration for text components */
  textConfig?: TextComponentConfig;

  /** Configuration for header components */
  headerConfig?: HeaderComponentConfig;

  /** Configuration for divider components */
  dividerConfig?: DividerComponentConfig;

  /** Configuration for tabs components */
  tabs?: TabConfig[];

  /** Configuration for row containers */
  rowConfig?: RowContainerConfig;

  /** Configuration for column containers */
  columnConfig?: ColumnContainerConfig;

  /** Children items for container components (row/column) */
  children?: DashboardLayoutItem[];

  /** Parent container ID (if this item is nested) */
  parentId?: string;

  /** Component-level filters (override or extend dashboard filters) */
  filters?: FilterConfig[];

  /** Dashboard filter IDs that this component should ignore */
  ignoreFilters?: string[];

  /** When true, this chart ignores ALL dashboard filters and cross-filters.
   *  Use for charts where a filter makes no sense (e.g. a "share by country"
   *  donut would collapse to 100% if filtered to a single country). */
  exemptFromFilters?: boolean;

  /** Minimum width constraint (in grid units) */
  minW?: number;

  /** Minimum height constraint (in grid units) */
  minH?: number;

  /** Maximum width constraint (in grid units) */
  maxW?: number;

  /** Maximum height constraint (in grid units) */
  maxH?: number;

  /** Whether this item is static (cannot be moved or resized) */
  static?: boolean;

  /** Whether this item is currently being dragged */
  isDragging?: boolean;

  /** Whether this item is currently being resized */
  isResizing?: boolean;
}

/**
 * Complete dashboard configuration
 */
export interface DashboardConfig {
  /** Dashboard unique identifier */
  id?: number;

  /** Dashboard name */
  name: string;

  /** Dashboard description */
  description?: string;

  /** Colour theme key (see DASHBOARD_THEMES) — recolours all charts */
  theme?: string;

  /** Layout items in the dashboard */
  layout: DashboardLayoutItem[];

  /** Dashboard-level filters */
  filters: DashboardFilter[];

  /** Filter logic (AND/OR) */
  filterLogic: FilterLogic;

  /** Chart IDs used in this dashboard (for quick reference) */
  chartIds: number[];

  /** Dashboard metadata */
  metadata?: {
    /** Created timestamp */
    createdAt?: string;
    /** Last updated timestamp */
    updatedAt?: string;
    /** Creator email */
    createdBy?: string;
    /** Last editor email */
    updatedBy?: string;
  };
}

/**
 * Props for dashboard component types
 */
export interface DashboardComponentProps {
  /** Layout item configuration */
  item: DashboardLayoutItem;

  /** Whether the dashboard is in edit mode */
  isEditMode: boolean;

  /** Effective filters applied to this component (dashboard + component filters merged) */
  effectiveFilters: FilterConfig[];

  /** Callback when component configuration changes */
  onConfigChange?: (itemId: string, updates: Partial<DashboardLayoutItem>) => void;

  /** Callback when component is removed */
  onRemove?: (itemId: string) => void;

  /** Callback when component is duplicated */
  onDuplicate?: (itemId: string) => void;
}

/**
 * Result of a filter resolution operation
 */
export interface FilterResolutionResult {
  /** Final effective filters for a component */
  effectiveFilters: FilterConfig[];

  /** Dashboard filters that were applied */
  appliedDashboardFilters: DashboardFilter[];

  /** Dashboard filters that were ignored */
  ignoredDashboardFilters: DashboardFilter[];

  /** Component-level filters that were added */
  componentFilters: FilterConfig[];
}

/**
 * Dashboard grid configuration
 */
export interface DashboardGridConfig {
  /** Number of columns in the grid */
  cols: number;

  /** Row height in pixels */
  rowHeight: number;

  /** Margin between items [horizontal, vertical] */
  margin: [number, number];

  /** Container padding [horizontal, vertical] */
  containerPadding: [number, number];

  /** Whether to compact the layout vertically */
  compactType: 'vertical' | 'horizontal' | null;

  /** Whether items can be dragged */
  isDraggable: boolean;

  /** Whether items can be resized */
  isResizable: boolean;
}

/**
 * Dashboard colour themes — a theme recolours every chart on the board at once.
 * The `default` theme has no colours (each chart keeps its own palette).
 */
export const DASHBOARD_THEMES: Record<string, { label: string; colors: string[] }> = {
  default: { label: "Default", colors: [] },
  vibrant: { label: "Vibrant", colors: ["#6366f1", "#ec4899", "#14b8a6", "#f59e0b", "#8b5cf6", "#06b6d4", "#ef4444", "#10b981", "#f97316", "#3b82f6"] },
  ocean:   { label: "Ocean",   colors: ["#0ea5e9", "#0369a1", "#06b6d4", "#3b82f6", "#6366f1", "#38bdf8", "#0891b2", "#22d3ee"] },
  sunset:  { label: "Sunset",  colors: ["#f59e0b", "#ef4444", "#ec4899", "#f97316", "#e11d48", "#fb923c", "#f43f5e", "#facc15"] },
  forest:  { label: "Forest",  colors: ["#10b981", "#14b8a6", "#84cc16", "#22c55e", "#059669", "#65a30d", "#4ade80", "#a3e635"] },
  slate:   { label: "Slate",   colors: ["#334155", "#0ea5e9", "#64748b", "#38bdf8", "#475569", "#93c5fd", "#94a3b8", "#0369a1"] },
};

/**
 * Number of horizontal grid columns in a row (12-column grid)
 */
export const GRID_COLUMNS = 12;

/**
 * Default grid configuration values
 */
export const DEFAULT_GRID_CONFIG: DashboardGridConfig = {
  cols: 12,
  rowHeight: 30,
  margin: [16, 16],
  containerPadding: [16, 16],
  compactType: 'vertical',
  isDraggable: true,
  isResizable: true,
};

/**
 * Default minimum dimensions for component types (in grid units)
 */
export const MIN_COMPONENT_DIMENSIONS: Record<ComponentType, { w: number; h: number }> = {
  chart: { w: 2, h: 3 },
  text: { w: 2, h: 2 },
  header: { w: 2, h: 1 },
  tabs: { w: 4, h: 8 },
  divider: { w: 1, h: 1 },
  row: { w: 12, h: 2 },
  column: { w: 2, h: 3 },
};

/**
 * Default dimensions for newly added components (in grid units)
 */
export const DEFAULT_COMPONENT_DIMENSIONS: Record<ComponentType, { w: number; h: number }> = {
  chart: { w: 6, h: 8 },
  text: { w: 4, h: 3 },
  header: { w: 12, h: 1 },
  tabs: { w: 8, h: 12 },
  divider: { w: 12, h: 1 },
  row: { w: 12, h: 4 },
  column: { w: 3, h: 8 },
};

/**
 * Maximum dimensions for component types (in grid units)
 * Used to enforce layout constraints (e.g., rows must be full width)
 */
export const MAX_COMPONENT_DIMENSIONS: Record<ComponentType, { w?: number; h?: number }> = {
  chart: {},
  text: {},
  header: {},
  tabs: {},
  divider: {},
  row: { w: 12 }, // Rows are always full width
  column: {},
};
