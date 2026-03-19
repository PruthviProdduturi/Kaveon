/**
 * Dashboard Chart Component
 *
 * Renders a chart inside a dashboard by reusing the exact same infrastructure
 * as the chart detail page. Fetches (or retrieves from the preload cache) the
 * chart config, then delegates hydration to ChartHydrator — the single shared
 * component used everywhere — so dashboard charts are always identical to
 * charts viewed on the chart page.
 *
 * Dashboard-specific concerns handled here:
 *   - Parallel preload cache lookup (avoids redundant network requests)
 *   - Loading / error states while the chart config is being fetched
 *   - Passing dashboard-level filters into ChartHydrator as externalFilters
 *   - Cross-filter click handling (click a bar/slice → filter other charts)
 */

import React, { useEffect, useState, useRef, useCallback } from 'react';
import type { DashboardComponentProps } from '../../../types/dashboard';
import { msalFetch } from '../../../utils/msalFetch';
import { API_BASE } from '../../../config';
import { useDashboard } from '../DashboardContext';
import { ChartBuilderProvider } from '../../charts/ChartBuilderContext';
import ChartHydrator from '../../charts/ChartHydrator';
import ChartPreview from '../../charts/ChartPreview';

interface DashboardChartLoaderProps {
  itemId: string;
  chartId: number;
  filters: any[];
  crossFilterFilters: Array<{ column: string | null; operator: string; value: string }>;
  onCrossFilter: (column: string | null, value: string) => void;
  isEditMode: boolean;
}

/**
 * Fetches (or retrieves from preload cache) the chart config, then renders
 * ChartHydrator + ChartPreview inside the shared ChartBuilderProvider.
 */
const DashboardChartLoader: React.FC<DashboardChartLoaderProps> = ({
  itemId,
  chartId,
  filters,
  crossFilterFilters,
  onCrossFilter,
  isEditMode,
}) => {
  const { getChartConfig, dashboardId } = useDashboard();

  const [chart, setChart] = useState<any>(null);
  const [isLoading, setIsLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const hasFetchedRef = useRef(false);

  // Keep latest filters in a ref to avoid re-triggering the fetch on filter changes.
  const filtersRef = useRef(filters);
  useEffect(() => { filtersRef.current = filters; }, [filters]);

  useEffect(() => {
    if (hasFetchedRef.current) return;
    hasFetchedRef.current = true;

    const load = async () => {
      try {
        // Try the preload cache first (populated by DashboardContext.preloadAllCharts).
        const cached = getChartConfig(chartId);
        if (cached) {
          setChart(cached);
        } else {
          const res = await msalFetch(`${API_BASE}/api/v1/charts/${chartId}`);
          if (!res.ok) throw new Error(`Failed to fetch chart ${chartId}: ${res.status}`);
          setChart(await res.json());
        }
      } catch (err) {
        setError(err instanceof Error ? err.message : 'Failed to load chart');
      } finally {
        setIsLoading(false);
      }
    };

    load();
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [chartId]);

  // Derive the primary dimension column from chart config for cross-filter
  const crossFilterColumn = chart?.query_config?.groupby?.[0] ?? null;

  // Inject the chart's primary dimension column before calling parent
  const handleCrossFilter = useCallback((value: string) => {
    onCrossFilter(crossFilterColumn, value);
  }, [onCrossFilter, crossFilterColumn]);

  if (isLoading) {
    return (
      <div style={{ padding: 20, textAlign: 'center', color: '#64748b' }}>
        <i className="fas fa-spinner fa-spin" style={{ marginRight: 8 }} />
        Loading chart...
      </div>
    );
  }

  if (error || !chart) {
    return (
      <div style={{ padding: 20, color: '#dc2626', fontSize: 13 }}>
        {error || 'Chart unavailable'}
      </div>
    );
  }

  // Encode the dashboard ID into runContext so the API can record it in
  // query_history.run_context for every query this chart executes.
  const runCtx = dashboardId ? `dashboard:${dashboardId}` : 'dashboard';

  // Cross-filter extras are kept separate from dashboard externalFilters so they
  // are NOT baked into context state during initial hydration (which would cause
  // dashboard filters to be applied twice when the cross-filter re-query fires).
  const crossExtras = crossFilterFilters.filter(cf => cf.column !== null);

  return (
    <ChartBuilderProvider runContext={runCtx}>
      {/* ChartHydrator renders null — it only populates context state */}
      <ChartHydrator
        chart={chart}
        externalFilters={filtersRef.current}
        crossFilterExtra={crossExtras}
      />
      <ChartPreview
        onCrossFilter={isEditMode ? undefined : handleCrossFilter}
      />
    </ChartBuilderProvider>
  );
};

/**
 * Top-level dashboard chart component — thin wrapper that passes item props.
 */
export const DashboardChartComponent: React.FC<DashboardComponentProps> = ({ item, effectiveFilters, isEditMode }) => {
  const { setCrossFilter, clearCrossFilter, getCrossFilterFilters, crossFilters } = useDashboard();

  const crossFilterFilters = getCrossFilterFilters(item.i);
  const activeCrossFilter = crossFilters[item.i];

  const handleCrossFilter = useCallback((column: string | null, value: string) => {
    // Toggle: clicking the same value again clears the filter
    if (activeCrossFilter?.value === value) {
      clearCrossFilter(item.i);
    } else {
      setCrossFilter(item.i, column, value);
    }
  }, [item.i, activeCrossFilter, setCrossFilter, clearCrossFilter]);

  return (
    <div className="dashboard-chart-component" style={{ height: '100%', width: '100%', position: 'relative' }}>
      {/* Active cross-filter badge */}
      {activeCrossFilter && !isEditMode && (
        <div style={{
          position: 'absolute',
          top: 6,
          left: 8,
          zIndex: 10,
          display: 'flex',
          alignItems: 'center',
          gap: 4,
          background: '#eff6ff',
          border: '1px solid #bfdbfe',
          borderRadius: 6,
          padding: '3px 8px',
          fontSize: 11,
          color: '#1d4ed8',
          fontWeight: 600,
        }}>
          <i className="fas fa-filter" style={{ fontSize: 9 }} />
          <span>{activeCrossFilter.value}</span>
          <button
            type="button"
            onClick={() => clearCrossFilter(item.i)}
            style={{ background: 'none', border: 'none', cursor: 'pointer', color: '#60a5fa', padding: '0 0 0 2px', fontSize: 11, lineHeight: 1 }}
            title="Clear cross-filter"
          >×</button>
        </div>
      )}
      <DashboardChartLoader
        itemId={item.i}
        chartId={item.chartId!}
        filters={effectiveFilters}
        crossFilterFilters={crossFilterFilters}
        onCrossFilter={handleCrossFilter}
        isEditMode={isEditMode}
      />
    </div>
  );
};

export default DashboardChartComponent;
