/**
 * Dashboard Context
 *
 * Provides centralized state management for the dashboard system, including:
 * - Layout management (add, update, remove, reorder components)
 * - Filter management (dashboard-level and component-level filters)
 * - Filter resolution (merging dashboard and component filters)
 * - Save/load operations
 * - Edit mode state
 */

import React, { createContext, useContext, useState, useCallback, useRef, useEffect } from 'react';
import type {
  DashboardLayoutItem,
  DashboardFilter,
  FilterConfig,
  FilterLogic,
  DashboardConfig,
  ChartId,
  ComponentType,
  FilterResolutionResult,
  DEFAULT_COMPONENT_DIMENSIONS,
  MIN_COMPONENT_DIMENSIONS,
} from '../../types/dashboard';
import {
  DEFAULT_COMPONENT_DIMENSIONS as DEFAULT_DIMS,
  MIN_COMPONENT_DIMENSIONS as MIN_DIMS,
  MAX_COMPONENT_DIMENSIONS as MAX_DIMS,
  GRID_COLUMNS,
  DASHBOARD_THEMES,
} from '../../types/dashboard';

/** Distribute GRID_COLUMNS units evenly across n children */
function distributeGridCols(n: number): number[] {
  const base = Math.floor(GRID_COLUMNS / n);
  const remainder = GRID_COLUMNS % n;
  return Array.from({ length: n }, (_, i) => (i < remainder ? base + 1 : base));
}

/**
 * A row's children share the twelve columns evenly, so any add or remove has to
 * re-split them. Factored out because three call sites need it and only one of
 * them had it: moving a child OUT of a row left the remaining children holding
 * the widths they had when the row was fuller, so a two-child row became one
 * half-width child beside a permanent gap.
 */
function redistributeRowChildren(
  container: DashboardLayoutItem,
  children: DashboardLayoutItem[]
): DashboardLayoutItem[] {
  if (container.type !== 'row' || children.length === 0) return children;
  const widths = distributeGridCols(children.length);
  return children.map((child, idx) => ({ ...child, w: widths[idx] }));
}

/**
 * The only two component types that hold children.
 *
 * There is no nesting-rule table in `types/dashboard.ts` — the rules were
 * written twice, inline, in `addItemToContainer` and `moveItemToContainer`, and
 * nothing in the UI could read them. They live here now, once, and both those
 * functions and the drag-and-drop surfaces consult the same predicate so the
 * highlight a user sees can never disagree with what the model will accept.
 */
export const isContainerType = (type?: ComponentType): boolean =>
  type === 'row' || type === 'column';

/**
 * Whether `childType` may be placed inside `containerType`.
 *
 * A row holds columns and leaves. A column holds leaves only. Neither holds a
 * row: a row is a full-width band, so nesting one inside a column (or inside
 * another row) has no coherent geometry, and the model has always refused it.
 * The one legal container-in-container move is therefore a COLUMN INTO A ROW,
 * which is how a row of columns is built in the first place.
 */
export const canContain = (containerType?: ComponentType, childType?: ComponentType): boolean => {
  if (!isContainerType(containerType) || !childType) return false;
  if (childType === 'row') return false;
  if (containerType === 'column' && childType === 'column') return false;
  return true;
};

/**
 * Recursively collect all chart IDs from a layout tree (including nested children).
 */
function collectChartIds(items: DashboardLayoutItem[]): ChartId[] {
  const ids: ChartId[] = [];
  for (const item of items) {
    if (item.type === 'chart' && item.chartId) ids.push(item.chartId);
    if (item.children) ids.push(...collectChartIds(item.children));
  }
  return ids;
}

/**
 * The canonical change-tracking snapshot of a dashboard.
 *
 * Every writer of `initialSnapshotRef` and the change-tracking effect build the
 * snapshot here so the two can never disagree. They did: the saved snapshot
 * carried the colour theme and the tracked one did not, so the comparison was
 * never equal again after a save and the Unsaved badge re-lit immediately.
 */
function buildChangeSnapshot(config: Partial<DashboardConfig>): string {
  const layout = Array.isArray(config.layout) ? config.layout : [];
  return JSON.stringify({
    id: config.id ?? null,
    name: config.name ?? '',
    description: config.description ?? '',
    theme: config.theme || 'default',
    layout,
    filters: config.filters ?? [],
    filterLogic: config.filterLogic ?? 'AND',
    chartIds: collectChartIds(layout),
  });
}

/**
 * Dashboard context state interface
 */
interface DashboardContextState {
  // Dashboard metadata
  dashboardId: number | null;
  name: string;
  description: string;

  // Layout state
  layout: DashboardLayoutItem[];
  isEditMode: boolean;

  // Filter state
  dashboardFilters: DashboardFilter[];
  filterLogic: FilterLogic;

  // Chart config cache for parallel loading
  chartConfigCache: Map<ChartId, any>;
  isPreloading: boolean;

  // UI state
  selectedItemId: string | null;
  isDragging: boolean;
  isSaving: boolean;
  saveError: string | null;

  // Change tracking
  hasUnsavedChanges: boolean;

  // Actions - Dashboard metadata
  setDashboardId: (id: number | null) => void;
  setName: (name: string) => void;
  setDescription: (description: string) => void;

  // Actions - Layout management
  setLayout: (layout: DashboardLayoutItem[]) => void;
  /**
   * Accept coordinates the grid normalised on its own (compaction, defaulted
   * widths) without counting them as an edit the user has to save.
   */
  adoptLayout: (layout: DashboardLayoutItem[]) => void;
  addLayoutItem: (type: ComponentType, config?: Partial<DashboardLayoutItem>) => void;
  updateLayoutItem: (itemId: string, updates: Partial<DashboardLayoutItem>) => void;
  removeLayoutItem: (itemId: string) => void;
  duplicateLayoutItem: (itemId: string) => void;

  // Actions - Nested container management
  findItem: (itemId: string) => DashboardLayoutItem | null;
  getItemDepth: (itemId: string) => number;
  /**
   * Add a new item to a container, at `index` among its children (appended
   * when omitted) — the position a drop indicator showed the user.
   */
  addItemToContainer: (containerId: string, type: ComponentType, config?: Partial<DashboardLayoutItem>, index?: number) => void;
  removeItemFromContainer: (containerId: string, itemId: string) => void;
  /** Whether a drop of `itemId` into `targetContainerId` would be accepted. */
  canMoveItemToContainer: (itemId: string, targetContainerId: string) => boolean;
  moveItemToContainer: (itemId: string, targetContainerId: string, index?: number) => void;
  moveItemToRoot: (itemId: string, position?: { x: number; y: number }) => void;

  // Colour theme
  theme: string;
  setTheme: (theme: string) => void;
  themePalette: string[] | null;

  // Actions - Filter management
  setDashboardFilters: (filters: DashboardFilter[]) => void;
  addDashboardFilter: (filter: Omit<DashboardFilter, 'id'>) => void;
  updateDashboardFilter: (filterId: string, updates: Partial<DashboardFilter>) => void;
  removeDashboardFilter: (filterId: string) => void;
  setFilterLogic: (logic: FilterLogic) => void;

  // Actions - Component filters
  setComponentFilters: (itemId: string, filters: FilterConfig[]) => void;
  addIgnoredFilter: (itemId: string, filterId: string) => void;
  removeIgnoredFilter: (itemId: string, filterId: string) => void;

  // Cross-filter state (click-based filtering across charts)
  crossFilters: Record<string, { column: string | null; value: string }>;
  setCrossFilter: (sourceItemId: string, column: string | null, value: string) => void;
  clearCrossFilter: (sourceItemId: string) => void;
  clearAllCrossFilters: () => void;
  getCrossFilterFilters: (itemId: string) => Array<{ column: string | null; operator: string; value: string }>;

  // Filter resolution
  getEffectiveFilters: (itemId: string) => FilterResolutionResult;

  // Chart preloading
  preloadAllCharts: (apiBase: string, msalFetch: any) => Promise<void>;
  getChartConfig: (chartId: ChartId) => any | null;

  // Global refresh (auto-refresh / manual refresh all charts)
  globalRefreshTick: number;
  triggerGlobalRefresh: () => void;

  // UI actions
  setIsEditMode: (isEditMode: boolean) => void;
  setSelectedItemId: (itemId: string | null) => void;
  setIsDragging: (isDragging: boolean) => void;

  // Save/load operations
  /** Persists the dashboard. Resolves true only when it was stored. */
  saveDashboard: (overrides?: Partial<DashboardConfig>) => Promise<boolean>;
  /** Save the current dashboard as a NEW copy; resolves to the new dashboard id (or null). */
  saveDashboardAs: (newName: string) => Promise<string | number | null>;
  loadDashboard: (config: DashboardConfig) => void;
  resetDashboard: () => void;
  registerInitialSnapshot: () => void;
}

/**
 * Create the dashboard context
 */
const DashboardContext = createContext<DashboardContextState | undefined>(undefined);

/**
 * Hook to access the dashboard context
 * @throws Error if used outside of DashboardProvider
 */
export const useDashboard = (): DashboardContextState => {
  const context = useContext(DashboardContext);
  if (!context) {
    throw new Error('useDashboard must be used within a DashboardProvider');
  }
  return context;
};

/**
 * Props for the DashboardProvider component
 */
interface DashboardProviderProps {
  children: React.ReactNode;
  /** Initial dashboard configuration (for editing existing dashboards) */
  initialConfig?: DashboardConfig;
  /** Callback when dashboard is saved */
  onSave?: (config: DashboardConfig) => Promise<void>;
  /** Callback to create a NEW dashboard from the given config; returns the new id. */
  onSaveAs?: (config: DashboardConfig) => Promise<string | number | null>;
}

/**
 * Dashboard Provider Component
 *
 * Wraps the dashboard UI and provides state management via React Context.
 * Handles layout changes, filter management, and persistence operations.
 */
export const DashboardProvider: React.FC<DashboardProviderProps> = ({
  children,
  initialConfig,
  onSave,
  onSaveAs,
}) => {
  // Dashboard metadata state
  const [dashboardId, setDashboardId] = useState<number | null>(initialConfig?.id || null);
  const [name, setName] = useState<string>(initialConfig?.name || '');
  const [description, setDescription] = useState<string>(initialConfig?.description || '');

  // Layout state
  const [layout, setLayout] = useState<DashboardLayoutItem[]>(Array.isArray(initialConfig?.layout) ? initialConfig.layout : []);
  const [isEditMode, setIsEditMode] = useState<boolean>(false);

  // Filter state
  const [dashboardFilters, setDashboardFilters] = useState<DashboardFilter[]>(initialConfig?.filters || []);
  const [filterLogic, setFilterLogic] = useState<FilterLogic>(initialConfig?.filterLogic || 'AND');

  // Colour theme (recolours all charts). '' / 'default' = each chart keeps its own colours.
  const [theme, setThemeState] = useState<string>(initialConfig?.theme || 'default');
  const themePalette: string[] | null = (DASHBOARD_THEMES[theme]?.colors?.length ? DASHBOARD_THEMES[theme].colors : null);
  const setTheme = useCallback((t: string) => {
    setThemeState(t);
    setHasUnsavedChanges(true);
  }, []);

  // Chart config cache for parallel loading
  const [chartConfigCache, setChartConfigCache] = useState<Map<ChartId, any>>(new Map());
  const [isPreloading, setIsPreloading] = useState<boolean>(false);
  const [globalRefreshTick, setGlobalRefreshTick] = useState<number>(0);
  const triggerGlobalRefresh = useCallback(() => setGlobalRefreshTick((t) => t + 1), []);

  // Cross-filter state
  //
  // Kept per dashboard for the lifetime of the tab. A cross-filter is a
  // reading position -- "show me Kenya" -- and losing it on a reload or on a
  // trip to a chart and back meant finding Kenya again on every return.
  // sessionStorage rather than localStorage on purpose: a selection that
  // followed the reader into a new session would silently narrow a dashboard
  // they believed they had opened fresh.
  const [crossFilters, setCrossFiltersState] = useState<Record<string, { column: string | null; value: string }>>({});
  const crossFilterKey = dashboardId === null ? null : `kaveon:dashboard:${dashboardId}:crossFilters`;
  const crossFiltersRestoredFor = useRef<string | null>(null);

  // The dashboard id arrives after mount for a loaded dashboard, so the restore
  // waits for it and happens once per identity. Until it has happened the save
  // below holds off, or an empty initial state would erase what was stored.
  useEffect(() => {
    if (!crossFilterKey || crossFiltersRestoredFor.current === crossFilterKey) return;
    crossFiltersRestoredFor.current = crossFilterKey;
    try {
      const raw = sessionStorage.getItem(crossFilterKey);
      if (!raw) return;
      const parsed = JSON.parse(raw) as unknown;
      if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) return;
      // These values become query predicates, so anything that is not the
      // shape this code wrote is dropped rather than trusted.
      const restored: Record<string, { column: string | null; value: string }> = {};
      for (const [itemId, entry] of Object.entries(parsed as Record<string, unknown>)) {
        if (!entry || typeof entry !== 'object') continue;
        const { column, value } = entry as { column?: unknown; value?: unknown };
        if (typeof value !== 'string' || !value.trim()) continue;
        if (column !== null && typeof column !== 'string') continue;
        restored[itemId] = { column: (column as string | null) ?? null, value };
      }
      if (Object.keys(restored).length) setCrossFiltersState(restored);
    } catch {
      // A blocked or unparseable store is not a reason to fail to open a
      // dashboard -- it just opens unfiltered.
    }
  }, [crossFilterKey]);

  useEffect(() => {
    if (!crossFilterKey || crossFiltersRestoredFor.current !== crossFilterKey) return;
    try {
      if (Object.keys(crossFilters).length) {
        sessionStorage.setItem(crossFilterKey, JSON.stringify(crossFilters));
      } else {
        sessionStorage.removeItem(crossFilterKey);
      }
    } catch {
      // Same: persistence is a convenience, not a precondition.
    }
  }, [crossFilterKey, crossFilters]);

  const setCrossFilter = useCallback((sourceItemId: string, column: string | null, value: string) => {
    setCrossFiltersState((prev) => ({ ...prev, [sourceItemId]: { column, value } }));
  }, []);

  const clearCrossFilter = useCallback((sourceItemId: string) => {
    setCrossFiltersState((prev) => {
      const next = { ...prev };
      delete next[sourceItemId];
      return next;
    });
  }, []);

  const clearAllCrossFilters = useCallback(() => {
    setCrossFiltersState({});
  }, []);

  /** Returns cross-filter filters from OTHER charts that this itemId should apply */
  const getCrossFilterFilters = useCallback(
    (itemId: string): Array<{ column: string | null; operator: string; value: string }> => {
      return Object.entries(crossFilters)
        .filter(([sourceId]) => sourceId !== itemId)
        .map(([, cf]) => ({ column: cf.column, operator: '=', value: cf.value }));
    },
    [crossFilters]
  );

  // UI state
  const [selectedItemId, setSelectedItemId] = useState<string | null>(null);
  const [isDragging, setIsDragging] = useState<boolean>(false);
  const [isSaving, setIsSaving] = useState<boolean>(false);
  const [saveError, setSaveError] = useState<string | null>(null);

  // Change tracking
  const [hasUnsavedChanges, setHasUnsavedChanges] = useState<boolean>(false);
  const initialSnapshotRef = useRef<string>('');
  if (!initialSnapshotRef.current) {
    // Baseline for change tracking, mirroring the state initialisers above so
    // the tracker further down compares against the dashboard as it was handed
    // to the provider. Set on the first render rather than in an effect because
    // the grid emits its normalised coordinates from a child effect, which runs
    // before any effect here. Without a baseline the tracker never ran at all
    // and "unsaved" was a one-way flag: undoing every edit left the badge lit.
    initialSnapshotRef.current = buildChangeSnapshot({
      id: initialConfig?.id,
      name: initialConfig?.name ?? '',
      description: initialConfig?.description ?? '',
      theme: initialConfig?.theme ?? 'default',
      layout: Array.isArray(initialConfig?.layout) ? initialConfig.layout : [],
      filters: initialConfig?.filters ?? [],
      filterLogic: initialConfig?.filterLogic ?? 'AND',
    });
  }

  /**
   * Generate a unique ID for layout items
   */
  const generateItemId = useCallback((): string => {
    return `item-${Date.now()}-${Math.random().toString(36).substr(2, 9)}`;
  }, []);

  /**
   * Find the next available position in the grid for a new item
   * @param width Width of the new item in grid units
   * @param height Height of the new item in grid units
   * @returns Position { x, y } for the new item
   */
  const findNextPosition = useCallback(
    (width: number, height: number): { x: number; y: number } => {
      if (layout.length === 0) {
        return { x: 0, y: 0 };
      }

      // Find the maximum y position in the current layout
      const maxY = Math.max(...layout.map((item) => item.y + item.h));

      // Place the new item at the start of the next row
      return { x: 0, y: maxY };
    },
    [layout]
  );

  /**
   * Add a new layout item to the dashboard
   * @param type Type of component to add
   * @param config Optional configuration overrides
   */
  const addLayoutItem = useCallback(
    (type: ComponentType, config?: Partial<DashboardLayoutItem>) => {
      const defaultDims = DEFAULT_DIMS[type];
      const minDims = MIN_DIMS[type];
      const maxDims = MAX_DIMS[type];
      const itemId = generateItemId();
      const position = findNextPosition(defaultDims.w, defaultDims.h);

      const newItem: DashboardLayoutItem = {
        i: itemId,
        x: position.x,
        y: position.y,
        w: defaultDims.w,
        h: defaultDims.h,
        type,
        minW: minDims.w,
        minH: minDims.h,
        maxW: maxDims.w,
        maxH: maxDims.h,
        ...config,
      };

      setLayout((prev) => [...prev, newItem]);
      setSelectedItemId(itemId);
      setHasUnsavedChanges(true);
    },
    [generateItemId, findNextPosition]
  );

  /**
   * Update an existing layout item
   * @param itemId ID of the item to update
   * @param updates Partial updates to apply
   */
  const updateLayoutItem = useCallback((itemId: string, updates: Partial<DashboardLayoutItem>) => {
    setLayout((prev) => {
      const updateRecursive = (items: DashboardLayoutItem[]): DashboardLayoutItem[] =>
        items.map((item) => {
          if (item.i === itemId) return { ...item, ...updates };
          if (item.children) return { ...item, children: updateRecursive(item.children) };
          return item;
        });
      return updateRecursive(prev);
    });
    setHasUnsavedChanges(true);
  }, []);

  /**
   * Remove a layout item from the dashboard
   * @param itemId ID of the item to remove
   */
  const removeLayoutItem = useCallback(
    (itemId: string) => {
      setLayout((prev) => prev.filter((item) => item.i !== itemId));
      if (selectedItemId === itemId) {
        setSelectedItemId(null);
      }
      setHasUnsavedChanges(true);
    },
    [selectedItemId]
  );

  /**
   * Duplicate an existing layout item
   * @param itemId ID of the item to duplicate
   */
  const duplicateLayoutItem = useCallback(
    (itemId: string) => {
      const itemToDuplicate = layout.find((item) => item.i === itemId);
      if (!itemToDuplicate) return;

      const newItemId = generateItemId();
      const position = findNextPosition(itemToDuplicate.w, itemToDuplicate.h);

      const duplicatedItem: DashboardLayoutItem = {
        ...itemToDuplicate,
        i: newItemId,
        x: position.x,
        y: position.y,
      };

      setLayout((prev) => [...prev, duplicatedItem]);
      setSelectedItemId(newItemId);
      setHasUnsavedChanges(true);
    },
    [layout, generateItemId, findNextPosition]
  );

  /**
   * Recursively find an item by ID in the layout hierarchy
   * @param itemId ID of the item to find
   * @returns The item if found, null otherwise
   */
  const findItem = useCallback(
    (itemId: string): DashboardLayoutItem | null => {
      const searchInItems = (items: DashboardLayoutItem[]): DashboardLayoutItem | null => {
        for (const item of items) {
          if (item.i === itemId) return item;
          if (item.children) {
            const found = searchInItems(item.children);
            if (found) return found;
          }
        }
        return null;
      };

      return searchInItems(layout);
    },
    [layout]
  );

  /**
   * Calculate the nesting depth of an item (0 = root, 1 = in row, 2 = in column)
   * @param itemId ID of the item
   * @returns The depth level
   */
  const getItemDepth = useCallback(
    (itemId: string): number => {
      const getDepth = (items: DashboardLayoutItem[], currentDepth: number): number => {
        for (const item of items) {
          if (item.i === itemId) return currentDepth;
          if (item.children) {
            const foundDepth = getDepth(item.children, currentDepth + 1);
            if (foundDepth > -1) return foundDepth;
          }
        }
        return -1;
      };

      return getDepth(layout, 0);
    },
    [layout]
  );

  /**
   * Add a new item to a container's children array
   * @param containerId ID of the container (row/column)
   * @param type Type of component to add
   * @param config Optional configuration overrides
   * @param index Optional insertion index among the container's children;
   *              appended when omitted. The drop surfaces resolve this from the
   *              pointer, so a palette entry can be dropped BETWEEN two
   *              existing children (D-37) rather than only at the end.
   */
  const addItemToContainer = useCallback(
    (containerId: string, type: ComponentType, config?: Partial<DashboardLayoutItem>, index?: number) => {
      const container = findItem(containerId);
      if (!container) {
        console.error(`Container ${containerId} not found`);
        return;
      }

      // One rule table, shared with moveItemToContainer and with the drag-and-drop
      // surfaces, so what the UI offers is exactly what this will accept.
      if (!canContain(container.type, type)) {
        console.error(`Cannot add ${type} to ${container.type}`);
        return;
      }

      const defaultDims = DEFAULT_DIMS[type];
      const minDims = MIN_DIMS[type];
      const maxDims = MAX_DIMS[type];
      const itemId = generateItemId();

      // Find position within container
      const containerChildren = container.children || [];
      const maxY = containerChildren.length > 0
        ? Math.max(...containerChildren.map((item) => item.y + item.h))
        : 0;

      const newItem: DashboardLayoutItem = {
        i: itemId,
        x: 0,
        y: maxY,
        w: defaultDims.w,
        h: defaultDims.h,
        type,
        minW: minDims.w,
        minH: minDims.h,
        maxW: maxDims.w,
        maxH: maxDims.h,
        parentId: containerId,
        ...config,
      };

      // Update the container with the new child
      setLayout((prev) => {
        const updateContainerChildren = (items: DashboardLayoutItem[]): DashboardLayoutItem[] => {
          return items.map((item) => {
            if (item.i === containerId) {
              const existing = item.children || [];
              const at = index === undefined
                ? existing.length
                : Math.max(0, Math.min(existing.length, index));
              const updatedChildren = [
                ...existing.slice(0, at),
                newItem,
                ...existing.slice(at),
              ];
              return {
                ...item,
                children: redistributeRowChildren(item, updatedChildren),
              };
            }
            if (item.children) {
              return {
                ...item,
                children: updateContainerChildren(item.children),
              };
            }
            return item;
          });
        };

        return updateContainerChildren(prev);
      });

      setSelectedItemId(itemId);
      setHasUnsavedChanges(true);
    },
    [findItem, generateItemId]
  );

  /**
   * Remove an item from its parent container
   * @param containerId ID of the parent container
   * @param itemId ID of the item to remove
   */
  const removeItemFromContainer = useCallback((containerId: string, itemId: string) => {
    setLayout((prev) => {
      const removeFromChildren = (items: DashboardLayoutItem[]): DashboardLayoutItem[] => {
        return items.map((item) => {
          if (item.i === containerId) {
            const remaining = (item.children || []).filter((child) => child.i !== itemId);
            return { ...item, children: redistributeRowChildren(item, remaining) };
          }
          if (item.children) {
            return {
              ...item,
              children: removeFromChildren(item.children),
            };
          }
          return item;
        });
      };

      return removeFromChildren(prev);
    });

    if (selectedItemId === itemId) {
      setSelectedItemId(null);
    }
    setHasUnsavedChanges(true);
  }, [selectedItemId]);

  /**
   * Whether `itemId` may be moved into `targetContainerId` right now.
   *
   * Exposed because the drag-and-drop surfaces have to decide whether to
   * highlight a container BEFORE the drop, and a highlight that promises
   * something `moveItemToContainer` then refuses is worse than no highlight.
   * Both read this, so they cannot disagree.
   */
  const canMoveItemToContainer = useCallback(
    (itemId: string, targetContainerId: string): boolean => {
      if (itemId === targetContainerId) return false;
      const item = findItem(itemId);
      const target = findItem(targetContainerId);
      if (!item || !target) return false;
      if (!canContain(target.type, item.type)) return false;
      // A container cannot be moved inside its own subtree.
      const contains = (candidate: DashboardLayoutItem): boolean =>
        (candidate.children || []).some((c) => c.i === targetContainerId || contains(c));
      if (contains(item)) return false;
      // Two levels of nesting is the limit: row > column > leaf.
      return getItemDepth(targetContainerId) < 2;
    },
    [findItem, getItemDepth]
  );

  /**
   * Move an existing item into a container, from the root canvas or from
   * another container.
   *
   * @param itemId ID of the item to move
   * @param targetContainerId ID of the target container
   * @param index Optional insertion index among the container's children;
   *              appended when omitted. Applied AFTER the item has been lifted
   *              out, which matters only when it is already a child of the
   *              target: a caller reordering within one container has to drop
   *              the index by one for any boundary below the item's current
   *              place. `useContainerDropTarget` does exactly that.
   */
  const moveItemToContainer = useCallback(
    (itemId: string, targetContainerId: string, index?: number) => {
      const item = findItem(itemId);
      if (!item || !canMoveItemToContainer(itemId, targetContainerId)) {
        console.error(`Cannot move ${itemId} into ${targetContainerId}`);
        return;
      }

      setLayout((prev) => {
        // Lift the item out of wherever it is, re-splitting the widths of any
        // row it leaves behind. Without that a row keeps the column widths it
        // had while the item was still in it.
        const removeItem = (items: DashboardLayoutItem[]): DashboardLayoutItem[] =>
          items
            .filter((i) => i.i !== itemId)
            .map((i) => {
              if (!i.children) return i;
              const remaining = removeItem(i.children);
              return { ...i, children: redistributeRowChildren(i, remaining) };
            });

        const addToContainer = (items: DashboardLayoutItem[]): DashboardLayoutItem[] =>
          items.map((i) => {
            if (i.i === targetContainerId) {
              const existing = i.children || [];
              // A child is positioned by its container, not by the canvas grid:
              // a row reads `w` as its share of twelve columns and a column
              // stacks by order, so `x` is always 0 and `y` follows the stack.
              // This used to read
              //   `y: position?.y ?? (children||[]).length > 0 ? max : 0`
              // which parses as `(position?.y ?? (length > 0)) ? max : 0` — so a
              // caller's position was discarded and `y: 0` collapsed to 0 by a
              // different route. See OPEN-DEFECTS D-33. Replaced with an
              // explicit insertion index, which is what the callers actually
              // have and what a container actually honours.
              const stackedY = existing.length
                ? Math.max(...existing.map((c) => c.y + c.h))
                : 0;
              const moved: DashboardLayoutItem = {
                ...item,
                parentId: targetContainerId,
                x: 0,
                y: stackedY,
              };
              const at = index === undefined
                ? existing.length
                : Math.max(0, Math.min(existing.length, index));
              const next = [...existing.slice(0, at), moved, ...existing.slice(at)];
              return { ...i, children: redistributeRowChildren(i, next) };
            }
            if (i.children) return { ...i, children: addToContainer(i.children) };
            return i;
          });

        return addToContainer(removeItem(prev));
      });

      setSelectedItemId(itemId);
      setHasUnsavedChanges(true);
    },
    [findItem, canMoveItemToContainer]
  );

  /**
   * Move an item from a container back to the root canvas.
   *
   * @param itemId ID of the item to move to root
   * @param position Optional grid cell to land on — the cell the pointer was
   *                 over when a drag was released. Omitted for the "move to
   *                 root" button, which appends below everything.
   */
  const moveItemToRoot = useCallback(
    (itemId: string, position?: { x: number; y: number }) => {
      const item = findItem(itemId);
      if (!item || !item.parentId) {
        console.error('Item not found or already at root');
        return;
      }

      setLayout((prev) => {
        // Remove from whatever container holds it, re-splitting that row's
        // widths. The previous version did not: taking one child out of a
        // two-child row left the survivor at six of twelve columns beside a
        // permanent gap.
        const removeFromChildren = (items: DashboardLayoutItem[]): DashboardLayoutItem[] =>
          items.map((i) => {
            if (!i.children) return i;
            const remaining = removeFromChildren(
              i.children.filter((child) => child.i !== itemId)
            );
            return { ...i, children: redistributeRowChildren(i, remaining) };
          });

        const landing = position ?? findNextPosition(item.w, item.h);
        const updatedItem: DashboardLayoutItem = {
          ...item,
          parentId: undefined,
          x: Math.max(0, Math.min(GRID_COLUMNS - (item.w || 1), landing.x)),
          y: Math.max(0, landing.y),
        };

        return [...removeFromChildren(prev), updatedItem];
      });

      setSelectedItemId(itemId);
      setHasUnsavedChanges(true);
    },
    [findItem, findNextPosition]
  );

  /**
   * Add a new dashboard-level filter
   * @param filter Filter configuration (without ID)
   */
  const addDashboardFilter = useCallback((filter: Omit<DashboardFilter, 'id'>) => {
    const newFilter: DashboardFilter = {
      ...filter,
      id: `filter-${Date.now()}-${Math.random().toString(36).substr(2, 9)}`,
    };
    setDashboardFilters((prev) => [...prev, newFilter]);
    setHasUnsavedChanges(true);
  }, []);

  /**
   * Update an existing dashboard filter
   * @param filterId ID of the filter to update
   * @param updates Partial updates to apply
   */
  const updateDashboardFilter = useCallback((filterId: string, updates: Partial<DashboardFilter>) => {
    setDashboardFilters((prev) =>
      prev.map((filter) => (filter.id === filterId ? { ...filter, ...updates } : filter))
    );
    setHasUnsavedChanges(true);
  }, []);

  /**
   * Remove a dashboard filter
   * @param filterId ID of the filter to remove
   */
  const removeDashboardFilter = useCallback((filterId: string) => {
    setDashboardFilters((prev) => prev.filter((filter) => filter.id !== filterId));
    setHasUnsavedChanges(true);
  }, []);

  /**
   * Set component-level filters for a specific layout item
   * @param itemId ID of the layout item
   * @param filters Array of filters to apply
   */
  const setComponentFilters = useCallback((itemId: string, filters: FilterConfig[]) => {
    updateLayoutItem(itemId, { filters });
  }, [updateLayoutItem]);

  /**
   * Add a dashboard filter to the ignore list for a specific component
   * @param itemId ID of the layout item
   * @param filterId ID of the dashboard filter to ignore
   */
  const addIgnoredFilter = useCallback(
    (itemId: string, filterId: string) => {
      const item = layout.find((i) => i.i === itemId);
      if (!item) return;

      const currentIgnored = item.ignoreFilters || [];
      if (currentIgnored.includes(filterId)) return;

      updateLayoutItem(itemId, {
        ignoreFilters: [...currentIgnored, filterId],
      });
    },
    [layout, updateLayoutItem]
  );

  /**
   * Remove a dashboard filter from the ignore list for a specific component
   * @param itemId ID of the layout item
   * @param filterId ID of the dashboard filter to stop ignoring
   */
  const removeIgnoredFilter = useCallback(
    (itemId: string, filterId: string) => {
      const item = layout.find((i) => i.i === itemId);
      if (!item) return;

      const currentIgnored = item.ignoreFilters || [];
      updateLayoutItem(itemId, {
        ignoreFilters: currentIgnored.filter((id) => id !== filterId),
      });
    },
    [layout, updateLayoutItem]
  );

  /**
   * Calculate the effective filters for a specific component
   *
   * Merges dashboard-level filters with component-level filters,
   * respecting the component's ignoreFilters list.
   *
   * @param itemId ID of the layout item
   * @returns Filter resolution result with effective filters
   */
  const getEffectiveFilters = useCallback(
    (itemId: string): FilterResolutionResult => {
      // Search the full tree (handles nested items inside row/column children)
      const item = findItem(itemId);
      if (!item) {
        return {
          effectiveFilters: [],
          appliedDashboardFilters: [],
          ignoredDashboardFilters: [],
          componentFilters: [],
        };
      }

      const ignoreList = item.ignoreFilters || [];
      const componentFilters = item.filters || [];

      // Filter dashboard filters based on appliesTo and ignoreList
      const applicableDashboardFilters = dashboardFilters.filter((filter) => {
        // Skip if not enabled
        if (!filter.enabled) return false;

        // Skip enabled-but-empty value filters so they don't wipe out all data
        // (e.g. an unset "Country" filter would otherwise become country = '').
        if (filter.filterType !== 'date_range' && !String(filter.value ?? '').trim()) return false;

        // Skip if in ignore list
        if (ignoreList.includes(filter.id)) return false;

        // Check if applies to this component
        if (filter.appliesTo === 'all') return true;
        if (item.chartId && filter.appliesTo.includes(item.chartId)) return true;

        return false;
      });

      const ignoredDashboardFilters = dashboardFilters.filter((filter) => {
        return filter.enabled && ignoreList.includes(filter.id);
      });

      // Convert DashboardFilter to FilterConfig for merging.
      // date_range filters expand into two filters (>= and <=) so downstream
      // chart execution doesn't need any special handling.
      const dashboardFilterConfigs: FilterConfig[] = [];
      for (const f of applicableDashboardFilters) {
        if (f.filterType === 'date_range') {
          if (f.dateFrom) {
            dashboardFilterConfigs.push({
              id: `${f.id}_from`,
              column: f.column,
              operator: '>=',
              value: f.dateFrom,
            });
          }
          if (f.dateTo) {
            dashboardFilterConfigs.push({
              id: `${f.id}_to`,
              column: f.column,
              operator: '<=',
              value: f.dateTo,
            });
          }
        } else {
          dashboardFilterConfigs.push({
            id: f.id,
            column: f.column,
            operator: f.operator,
            value: f.value,
            options: f.options,
            isLoading: f.isLoading,
            isPending: f.isPending,
          });
        }
      }

      // Merge dashboard and component filters
      // Component filters with the same column override dashboard filters
      const effectiveFilters: FilterConfig[] = [...dashboardFilterConfigs];

      componentFilters.forEach((compFilter) => {
        const existingIndex = effectiveFilters.findIndex((f) => f.column === compFilter.column);
        if (existingIndex >= 0) {
          // Override existing filter
          effectiveFilters[existingIndex] = compFilter;
        } else {
          // Add new filter
          effectiveFilters.push(compFilter);
        }
      });

      return {
        effectiveFilters,
        appliedDashboardFilters: applicableDashboardFilters,
        ignoredDashboardFilters,
        componentFilters,
      };
    },
    [findItem, dashboardFilters]
  );

  /**
   * Save the current dashboard state
   * Calls the onSave callback with the complete dashboard configuration
   */
  const saveDashboard = useCallback(async (overrides?: Partial<DashboardConfig>): Promise<boolean> => {
    if (!onSave) {
      console.warn('No onSave callback provided to DashboardProvider');
      return false;
    }

    setIsSaving(true);
    setSaveError(null);

    try {
      // Overrides win over current state — lets the save modal pass the just-typed
      // name/description directly instead of relying on a setState having flushed.
      // (Previously the modal did setDescription() then saveDashboard() in a
      // setTimeout, but that closure captured the pre-update description, so
      // description edits were silently dropped.)
      const config: DashboardConfig = {
        id: dashboardId || undefined,
        name,
        description,
        theme,
        layout,
        filters: dashboardFilters,
        filterLogic,
        chartIds: collectChartIds(layout),
        ...overrides,
      };

      await onSave(config);

      // Reflect overrides in context state so the UI matches what was persisted.
      if (overrides?.name !== undefined) setName(overrides.name);
      if (overrides?.description !== undefined) setDescription(overrides.description);

      // Update the initial snapshot after successful save
      initialSnapshotRef.current = buildChangeSnapshot(config);
      setHasUnsavedChanges(false);
      return true;
    } catch (error) {
      console.error('Error saving dashboard:', error);
      setSaveError(error instanceof Error ? error.message : 'Failed to save dashboard');
      return false;
    } finally {
      setIsSaving(false);
    }
  }, [dashboardId, name, description, theme, layout, dashboardFilters, filterLogic, onSave]);

  const saveDashboardAs = useCallback(async (newName: string): Promise<string | number | null> => {
    if (!onSaveAs) {
      console.warn('No onSaveAs callback provided to DashboardProvider');
      return null;
    }
    setIsSaving(true);
    setSaveError(null);
    try {
      // New copy: no id, new name, same everything else.
      const config: DashboardConfig = {
        id: undefined,
        name: newName,
        description,
        theme,
        layout,
        filters: dashboardFilters,
        filterLogic,
        chartIds: collectChartIds(layout),
      };
      const newId = await onSaveAs(config);
      return newId;
    } catch (error) {
      console.error('Error saving dashboard copy:', error);
      setSaveError(error instanceof Error ? error.message : 'Failed to save a copy');
      return null;
    } finally {
      setIsSaving(false);
    }
  }, [description, theme, layout, dashboardFilters, filterLogic, onSaveAs]);

  /**
   * Load a dashboard configuration
   * Replaces the current state with the provided configuration
   * @param config Dashboard configuration to load
   */
  const loadDashboard = useCallback((config: DashboardConfig) => {
    setDashboardId(config.id || null);
    setName(config.name);
    setDescription(config.description || '');
    setThemeState(config.theme || 'default');
    setLayout(Array.isArray(config.layout) ? config.layout : []);
    setDashboardFilters(config.filters);
    setFilterLogic(config.filterLogic);
    setHasUnsavedChanges(false);

    // Store initial snapshot
    initialSnapshotRef.current = buildChangeSnapshot(config);
  }, []);

  /**
   * Reset the dashboard to an empty state
   */
  const resetDashboard = useCallback(() => {
    setDashboardId(null);
    setName('');
    setDescription('');
    setLayout([]);
    setDashboardFilters([]);
    setFilterLogic('AND');
    setSelectedItemId(null);
    setHasUnsavedChanges(false);
    // Baseline the empty dashboard the reset just produced; an empty string
    // would switch change tracking off entirely.
    initialSnapshotRef.current = buildChangeSnapshot({ theme });
  }, [theme]);

  /**
   * Register the current state as the initial snapshot
   * Used to detect unsaved changes
   */
  const registerInitialSnapshot = useCallback(() => {
    initialSnapshotRef.current = buildChangeSnapshot({
      id: dashboardId || undefined,
      name,
      description,
      theme,
      layout,
      filters: dashboardFilters,
      filterLogic,
    });
    setHasUnsavedChanges(false);
  }, [dashboardId, name, description, theme, layout, dashboardFilters, filterLogic]);

  /**
   * Accept coordinates the grid worked out for itself.
   *
   * react-grid-layout normalises the authored layout on mount — vertical
   * compaction, defaulted widths — and reports the result. Those coordinates do
   * belong in state, but they are not an edit the user made, and writing them
   * through setLayout lit the Unsaved badge on dashboards nobody had touched.
   * The baseline moves with them, so the badge and the leave-prompt stay quiet;
   * it only moves while there is nothing else pending, so a normalisation
   * arriving mid-edit can never swallow a real change.
   */
  const adoptLayout = useCallback((newLayout: DashboardLayoutItem[]) => {
    setLayout(newLayout);
    if (!hasUnsavedChanges) {
      initialSnapshotRef.current = buildChangeSnapshot({
        id: dashboardId || undefined,
        name,
        description,
        theme,
        layout: newLayout,
        filters: dashboardFilters,
        filterLogic,
      });
    }
  }, [hasUnsavedChanges, dashboardId, name, description, theme, dashboardFilters, filterLogic]);

  /**
   * Pre-fetch all chart configurations in parallel
   * This enables all charts to load simultaneously instead of sequentially
   * @param apiBase The API base URL
   * @param msalFetch The authenticated fetch function
   */
  const preloadAllCharts = useCallback(async (apiBase: string, msalFetch: any) => {
    // Extract all unique chart IDs from the full layout tree (including nested)
    const chartIds = [...new Set(collectChartIds(layout))];

    if (chartIds.length === 0) {
      return;
    }

    setIsPreloading(true);

    try {
      // The list endpoint returns the same chart records in one request. A
      // dashboard can contain dozens of charts; issuing one proxy round trip
      // per chart made Vercel queue requests even when Engine execution was
      // already complete. Keep the per-chart path as a compatibility fallback.
      let results: { chartId: ChartId; data: any | null }[] = [];
      try {
        const response = await msalFetch(`${apiBase}/api/v1/charts`);
        if (response.ok) {
          const payload = await response.json();
          const all = Array.isArray(payload) ? payload : payload.charts || payload.data || [];
          const byId = new Map(all.map((item: any) => [String(item.id), item]));
          results = chartIds.map((chartId) => ({ chartId, data: byId.get(String(chartId)) || null }));
        }
      } catch (error) {
        console.warn("Bulk chart preload failed; falling back to individual requests", error);
      }
      if (results.length !== chartIds.length || results.some((item) => !item.data)) {
        const fallback = await Promise.all(chartIds.map(async (chartId) => {
          try {
            const response = await msalFetch(`${apiBase}/api/v1/charts/${chartId}`);
            return { chartId, data: response.ok ? await response.json() : null };
          } catch { return { chartId, data: null }; }
        }));
        const byId = new Map(results.map((item) => [String(item.chartId), item]));
        for (const item of fallback) if (!byId.get(String(item.chartId))?.data) byId.set(String(item.chartId), item);
        results = chartIds.map((chartId) => byId.get(String(chartId)) || { chartId, data: null });
      }

      // Build the cache map
      const newCache = new Map<ChartId, any>();
      results.forEach(({ chartId, data }) => {
        if (data) {
          newCache.set(chartId, data);
        }
      });

      setChartConfigCache(newCache);
    } catch (error) {
      console.error('[DashboardContext] Error pre-loading charts:', error);
    } finally {
      setIsPreloading(false);
    }
  }, [layout]);

  /**
   * Get a pre-loaded chart configuration from cache
   * @param chartId The chart ID to retrieve
   * @returns The chart config if cached, null otherwise
   */
  const getChartConfig = useCallback((chartId: ChartId): any | null => {
    return chartConfigCache.get(chartId) || null;
  }, [chartConfigCache]);

  /**
   * Track changes by comparing current state to initial snapshot
   */
  useEffect(() => {
    if (!initialSnapshotRef.current) return;

    const currentSnapshot = buildChangeSnapshot({
      id: dashboardId || undefined,
      name,
      description,
      theme,
      layout,
      filters: dashboardFilters,
      filterLogic,
    });
    const hasChanges = currentSnapshot !== initialSnapshotRef.current;

    if (hasChanges !== hasUnsavedChanges) {
      setHasUnsavedChanges(hasChanges);
    }
  }, [dashboardId, name, description, theme, layout, dashboardFilters, filterLogic, hasUnsavedChanges]);

  /**
   * Context value with all state and actions
   */
  const contextValue: DashboardContextState = {
    // Dashboard metadata
    dashboardId,
    name,
    description,

    // Layout state
    layout,
    isEditMode,

    // Filter state
    dashboardFilters,
    filterLogic,

    // Chart config cache
    chartConfigCache,
    isPreloading,
    globalRefreshTick,
    triggerGlobalRefresh,

    // UI state
    selectedItemId,
    isDragging,
    isSaving,
    saveError,
    hasUnsavedChanges,

    // Actions - Dashboard metadata
    setDashboardId,
    setName,
    setDescription,

    // Actions - Layout management
    setLayout: (newLayout: DashboardLayoutItem[]) => { setLayout(newLayout); setHasUnsavedChanges(true); },
    adoptLayout,
    addLayoutItem,
    updateLayoutItem,
    removeLayoutItem,
    duplicateLayoutItem,

    // Actions - Nested container management
    findItem,
    getItemDepth,
    addItemToContainer,
    removeItemFromContainer,
    canMoveItemToContainer,
    moveItemToContainer,
    moveItemToRoot,

    // Colour theme
    theme,
    setTheme,
    themePalette,

    // Actions - Filter management
    setDashboardFilters,
    addDashboardFilter,
    updateDashboardFilter,
    removeDashboardFilter,
    setFilterLogic,

    // Actions - Component filters
    setComponentFilters,
    addIgnoredFilter,
    removeIgnoredFilter,

    // Cross-filter
    crossFilters,
    setCrossFilter,
    clearCrossFilter,
    clearAllCrossFilters,
    getCrossFilterFilters,

    // Filter resolution
    getEffectiveFilters,

    // Chart preloading
    preloadAllCharts,
    getChartConfig,

    // UI actions
    setIsEditMode,
    setSelectedItemId,
    setIsDragging,

    // Save/load operations
    saveDashboard,
    saveDashboardAs,
    loadDashboard,
    resetDashboard,
    registerInitialSnapshot,
  };

  return <DashboardContext.Provider value={contextValue}>{children}</DashboardContext.Provider>;
};

// Re-export types for convenience
export type { DashboardConfig };

/**
 * Pure filter resolver — mirrors the in-component getEffectiveFilters logic but takes
 * the item and the filter set explicitly. This lets callers (e.g. the full-screen
 * sandbox) resolve a chart's filters against a LOCAL copy of the filters without
 * mutating global dashboard state. Keep in sync with getEffectiveFilters above.
 */
export function resolveEffectiveFilters(
  item: { chartId?: ChartId | null; ignoreFilters?: string[]; filters?: FilterConfig[] } | null | undefined,
  dashboardFilters: DashboardFilter[],
): FilterResolutionResult {
  if (!item) {
    return { effectiveFilters: [], appliedDashboardFilters: [], ignoredDashboardFilters: [], componentFilters: [] };
  }

  const ignoreList = item.ignoreFilters || [];
  const componentFilters = item.filters || [];

  const applicableDashboardFilters = dashboardFilters.filter((filter) => {
    if (!filter.enabled) return false;
    if (filter.filterType !== 'date_range' && !String(filter.value ?? '').trim()) return false;
    if (ignoreList.includes(filter.id)) return false;
    if (filter.appliesTo === 'all') return true;
    if (item.chartId && Array.isArray(filter.appliesTo) && filter.appliesTo.includes(item.chartId)) return true;
    return false;
  });

  const ignoredDashboardFilters = dashboardFilters.filter(
    (filter) => filter.enabled && ignoreList.includes(filter.id),
  );

  const dashboardFilterConfigs: FilterConfig[] = [];
  for (const f of applicableDashboardFilters) {
    if (f.filterType === 'date_range') {
      if (f.dateFrom) dashboardFilterConfigs.push({ id: `${f.id}_from`, column: f.column, operator: '>=', value: f.dateFrom });
      if (f.dateTo) dashboardFilterConfigs.push({ id: `${f.id}_to`, column: f.column, operator: '<=', value: f.dateTo });
    } else {
      dashboardFilterConfigs.push({
        id: f.id, column: f.column, operator: f.operator, value: f.value,
        options: f.options, isLoading: f.isLoading, isPending: f.isPending,
      });
    }
  }

  const effectiveFilters: FilterConfig[] = [...dashboardFilterConfigs];
  componentFilters.forEach((compFilter) => {
    const existingIndex = effectiveFilters.findIndex((f) => f.column === compFilter.column);
    if (existingIndex >= 0) effectiveFilters[existingIndex] = compFilter;
    else effectiveFilters.push(compFilter);
  });

  return { effectiveFilters, appliedDashboardFilters: applicableDashboardFilters, ignoredDashboardFilters, componentFilters };
}
