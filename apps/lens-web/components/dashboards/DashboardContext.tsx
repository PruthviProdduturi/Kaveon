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
} from '../../types/dashboard';

/** Distribute GRID_COLUMNS units evenly across n children */
function distributeGridCols(n: number): number[] {
  const base = Math.floor(GRID_COLUMNS / n);
  const remainder = GRID_COLUMNS % n;
  return Array.from({ length: n }, (_, i) => (i < remainder ? base + 1 : base));
}

/**
 * Recursively collect all chart IDs from a layout tree (including nested children).
 */
function collectChartIds(items: DashboardLayoutItem[]): number[] {
  const ids: number[] = [];
  for (const item of items) {
    if (item.type === 'chart' && item.chartId) ids.push(item.chartId);
    if (item.children) ids.push(...collectChartIds(item.children));
  }
  return ids;
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
  chartConfigCache: Map<number, any>;
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
  addLayoutItem: (type: ComponentType, config?: Partial<DashboardLayoutItem>) => void;
  updateLayoutItem: (itemId: string, updates: Partial<DashboardLayoutItem>) => void;
  removeLayoutItem: (itemId: string) => void;
  duplicateLayoutItem: (itemId: string) => void;

  // Actions - Nested container management
  findItem: (itemId: string) => DashboardLayoutItem | null;
  getItemDepth: (itemId: string) => number;
  addItemToContainer: (containerId: string, type: ComponentType, config?: Partial<DashboardLayoutItem>) => void;
  removeItemFromContainer: (containerId: string, itemId: string) => void;
  moveItemToContainer: (itemId: string, targetContainerId: string, position?: { x: number; y: number }) => void;
  moveItemToRoot: (itemId: string) => void;

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
  getChartConfig: (chartId: number) => any | null;

  // Global refresh (auto-refresh / manual refresh all charts)
  globalRefreshTick: number;
  triggerGlobalRefresh: () => void;

  // UI actions
  setIsEditMode: (isEditMode: boolean) => void;
  setSelectedItemId: (itemId: string | null) => void;
  setIsDragging: (isDragging: boolean) => void;

  // Save/load operations
  saveDashboard: () => Promise<void>;
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

  // Chart config cache for parallel loading
  const [chartConfigCache, setChartConfigCache] = useState<Map<number, any>>(new Map());
  const [isPreloading, setIsPreloading] = useState<boolean>(false);
  const [globalRefreshTick, setGlobalRefreshTick] = useState<number>(0);
  const triggerGlobalRefresh = useCallback(() => setGlobalRefreshTick((t) => t + 1), []);

  // Cross-filter state
  const [crossFilters, setCrossFiltersState] = useState<Record<string, { column: string | null; value: string }>>({});

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
   */
  const addItemToContainer = useCallback(
    (containerId: string, type: ComponentType, config?: Partial<DashboardLayoutItem>) => {
      const container = findItem(containerId);
      if (!container) {
        console.error(`Container ${containerId} not found`);
        return;
      }

      // Validate nesting rules
      if (container.type === 'row') {
        // Rows cannot contain other rows
        if (type === 'row') {
          console.error('Cannot add row to row');
          return;
        }
      } else if (container.type === 'column') {
        // Columns cannot contain rows or columns
        if (type === 'row' || type === 'column') {
          console.error('Cannot add row or column to column');
          return;
        }
      } else {
        console.error(`${container.type} is not a container`);
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
              const updatedChildren = [...(item.children || []), newItem];

              // For rows: redistribute 12 grid columns evenly among all children
              let finalChildren = updatedChildren;
              if (item.type === 'row') {
                const widths = distributeGridCols(updatedChildren.length);
                finalChildren = updatedChildren.map((child, idx) => ({
                  ...child,
                  w: widths[idx],
                }));
              }

              return { ...item, children: finalChildren };
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

            // For rows: redistribute 12 grid columns evenly among remaining children
            let finalChildren = remaining;
            if (item.type === 'row' && remaining.length > 0) {
              const widths = distributeGridCols(remaining.length);
              finalChildren = remaining.map((child, idx) => ({ ...child, w: widths[idx] }));
            }

            return { ...item, children: finalChildren };
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
   * Move an item from one location to a container
   * @param itemId ID of the item to move
   * @param targetContainerId ID of the target container
   * @param position Optional position within the target container
   */
  const moveItemToContainer = useCallback(
    (itemId: string, targetContainerId: string, position?: { x: number; y: number }) => {
      const item = findItem(itemId);
      const targetContainer = findItem(targetContainerId);

      if (!item || !targetContainer) {
        console.error('Item or container not found');
        return;
      }

      // Validate nesting rules
      if (targetContainer.type === 'row' && item.type === 'row') {
        console.error('Cannot move row into row');
        return;
      }
      if (targetContainer.type === 'column' && (item.type === 'row' || item.type === 'column')) {
        console.error('Cannot move row or column into column');
        return;
      }

      // Check depth limit (max 2 levels)
      const targetDepth = getItemDepth(targetContainerId);
      if (targetDepth >= 2) {
        console.error('Cannot nest deeper than 2 levels');
        return;
      }

      setLayout((prev) => {
        // Remove item from its current location
        const removeItem = (items: DashboardLayoutItem[]): DashboardLayoutItem[] => {
          const filtered = items.filter((i) => i.i !== itemId);
          return filtered.map((i) => {
            if (i.children) {
              return { ...i, children: removeItem(i.children) };
            }
            return i;
          });
        };

        // Add item to target container
        const addToContainer = (items: DashboardLayoutItem[]): DashboardLayoutItem[] => {
          return items.map((i) => {
            if (i.i === targetContainerId) {
              const updatedItem = {
                ...item,
                parentId: targetContainerId,
                x: position?.x ?? 0,
                y: position?.y ?? (i.children || []).length > 0
                  ? Math.max(...(i.children || []).map((c) => c.y + c.h))
                  : 0,
              };
              return {
                ...i,
                children: [...(i.children || []), updatedItem],
              };
            }
            if (i.children) {
              return { ...i, children: addToContainer(i.children) };
            }
            return i;
          });
        };

        let updated = removeItem(prev);
        updated = addToContainer(updated);
        return updated;
      });

      setHasUnsavedChanges(true);
    },
    [findItem, getItemDepth]
  );

  /**
   * Move an item from a container back to the root canvas
   * @param itemId ID of the item to move to root
   */
  const moveItemToRoot = useCallback(
    (itemId: string) => {
      const item = findItem(itemId);
      if (!item || !item.parentId) {
        console.error('Item not found or already at root');
        return;
      }

      setLayout((prev) => {
        // Remove from container
        const removeFromChildren = (items: DashboardLayoutItem[]): DashboardLayoutItem[] => {
          return items.map((i) => {
            if (i.children) {
              return {
                ...i,
                children: i.children.filter((child) => child.i !== itemId),
              };
            }
            return i;
          }).map((i) => {
            if (i.children) {
              return { ...i, children: removeFromChildren(i.children) };
            }
            return i;
          });
        };

        // Add to root
        const position = findNextPosition(item.w, item.h);
        const updatedItem = {
          ...item,
          parentId: undefined,
          x: position.x,
          y: position.y,
        };

        return [...removeFromChildren(prev), updatedItem];
      });

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
  const saveDashboard = useCallback(async () => {
    if (!onSave) {
      console.warn('No onSave callback provided to DashboardProvider');
      return;
    }

    setIsSaving(true);
    setSaveError(null);

    try {
      const config: DashboardConfig = {
        id: dashboardId || undefined,
        name,
        description,
        layout,
        filters: dashboardFilters,
        filterLogic,
        chartIds: collectChartIds(layout),
      };

      await onSave(config);

      // Update the initial snapshot after successful save
      initialSnapshotRef.current = JSON.stringify(config);
      setHasUnsavedChanges(false);
    } catch (error) {
      console.error('Error saving dashboard:', error);
      setSaveError(error instanceof Error ? error.message : 'Failed to save dashboard');
    } finally {
      setIsSaving(false);
    }
  }, [dashboardId, name, description, layout, dashboardFilters, filterLogic, onSave]);

  /**
   * Load a dashboard configuration
   * Replaces the current state with the provided configuration
   * @param config Dashboard configuration to load
   */
  const loadDashboard = useCallback((config: DashboardConfig) => {
    setDashboardId(config.id || null);
    setName(config.name);
    setDescription(config.description || '');
    setLayout(Array.isArray(config.layout) ? config.layout : []);
    setDashboardFilters(config.filters);
    setFilterLogic(config.filterLogic);
    setHasUnsavedChanges(false);

    // Store initial snapshot
    initialSnapshotRef.current = JSON.stringify(config);
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
    initialSnapshotRef.current = '';
  }, []);

  /**
   * Register the current state as the initial snapshot
   * Used to detect unsaved changes
   */
  const registerInitialSnapshot = useCallback(() => {
    const config: DashboardConfig = {
      id: dashboardId || undefined,
      name,
      description,
      layout,
      filters: dashboardFilters,
      filterLogic,
      chartIds: collectChartIds(layout),
    };
    initialSnapshotRef.current = JSON.stringify(config);
    setHasUnsavedChanges(false);
  }, [dashboardId, name, description, layout, dashboardFilters, filterLogic]);

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
      // Fetch all charts in parallel using Promise.all
      const fetchPromises = chartIds.map(async (chartId) => {
        try {
          const response = await msalFetch(`${apiBase}/api/v1/charts/${chartId}`);
          if (!response.ok) {
            console.error(`Failed to fetch chart ${chartId}: ${response.status}`);
            return { chartId, data: null };
          }
          const data = await response.json();
          return { chartId, data };
        } catch (error) {
          console.error(`Error fetching chart ${chartId}:`, error);
          return { chartId, data: null };
        }
      });

      const results = await Promise.all(fetchPromises);

      // Build the cache map
      const newCache = new Map<number, any>();
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
  const getChartConfig = useCallback((chartId: number): any | null => {
    return chartConfigCache.get(chartId) || null;
  }, [chartConfigCache]);

  /**
   * Track changes by comparing current state to initial snapshot
   */
  useEffect(() => {
    if (!initialSnapshotRef.current) return;

    const currentConfig: DashboardConfig = {
      id: dashboardId || undefined,
      name,
      description,
      layout,
      filters: dashboardFilters,
      filterLogic,
      chartIds: collectChartIds(layout),
    };

    const currentSnapshot = JSON.stringify(currentConfig);
    const hasChanges = currentSnapshot !== initialSnapshotRef.current;

    if (hasChanges !== hasUnsavedChanges) {
      setHasUnsavedChanges(hasChanges);
    }
  }, [dashboardId, name, description, layout, dashboardFilters, filterLogic, hasUnsavedChanges]);

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
    addLayoutItem,
    updateLayoutItem,
    removeLayoutItem,
    duplicateLayoutItem,

    // Actions - Nested container management
    findItem,
    getItemDepth,
    addItemToContainer,
    removeItemFromContainer,
    moveItemToContainer,
    moveItemToRoot,

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
    loadDashboard,
    resetDashboard,
    registerInitialSnapshot,
  };

  return <DashboardContext.Provider value={contextValue}>{children}</DashboardContext.Provider>;
};

// Re-export types for convenience
export type { DashboardConfig };
