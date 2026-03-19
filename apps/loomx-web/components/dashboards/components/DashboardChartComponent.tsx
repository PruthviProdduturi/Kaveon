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

  const TIME_SERIES_TYPES = new Set([
    'time_series_line', 'time_series_line_share',
    'time_series_area', 'time_series_area_share',
    'line_multi_series', 'area_stack',
  ]);
  const chartKind: string = chart?.chart_type || chart?.viz_config?.chartType || chart?.viz_config?.chart_type || '';
  const isTimeSeries = TIME_SERIES_TYPES.has(chartKind);
  const qc = chart?.query_config || {};

  // For time series, cross-filter by the time column (date click).
  // For other charts, cross-filter by the first dimension/groupby column.
  const crossFilterColumn: string | null = isTimeSeries
    ? (qc.time_column ?? null)
    : (qc.groupby?.[0] ?? null);

  // Helper to normalise a column name for loose matching
  // (strips brackets, qualifiers — matches "[dbo].[tbl].[Col]" to "col")
  const normCol = (s: string | null | undefined) =>
    s ? s.replace(/\[|\]/g, '').split('.').pop()!.toLowerCase() : '';

  // Only apply incoming cross-filters where the source column matches one of
  // THIS chart's columns (time column or any groupby). Prevents nonsensical
  // filters being applied to unrelated charts.
  const relevantCrossExtras = crossFilterFilters.filter(cf => {
    if (!cf.column) return false;
    const cfNorm = normCol(cf.column);
    if (qc.time_column && normCol(qc.time_column) === cfNorm) return true;
    if (Array.isArray(qc.groupby) && qc.groupby.some((g: string) => normCol(g) === cfNorm)) return true;
    return false;
  });

  // Inject the resolved column before calling parent
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

  return (
    <ChartBuilderProvider runContext={runCtx}>
      {/* ChartHydrator renders null — it only populates context state */}
      <ChartHydrator
        chart={chart}
        externalFilters={filtersRef.current}
        crossFilterExtra={relevantCrossExtras}
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
