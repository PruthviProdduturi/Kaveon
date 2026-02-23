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
 */

import React, { useEffect, useState, useRef } from 'react';
import type { DashboardComponentProps } from '../../../types/dashboard';
import { msalFetch } from '../../../utils/msalFetch';
import { API_BASE } from '../../../config';
import { useDashboard } from '../DashboardContext';
import { ChartBuilderProvider } from '../../charts/ChartBuilderContext';
import ChartHydrator from '../../charts/ChartHydrator';
import ChartPreview from '../../charts/ChartPreview';

interface DashboardChartLoaderProps {
  chartId: number;
  filters: any[];
}

/**
 * Fetches (or retrieves from preload cache) the chart config, then renders
 * ChartHydrator + ChartPreview inside the shared ChartBuilderProvider.
 */
const DashboardChartLoader: React.FC<DashboardChartLoaderProps> = ({ chartId, filters }) => {
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
      <ChartHydrator chart={chart} externalFilters={filtersRef.current} />
      <ChartPreview />
    </ChartBuilderProvider>
  );
};

/**
 * Top-level dashboard chart component — thin wrapper that passes item props.
 */
export const DashboardChartComponent: React.FC<DashboardComponentProps> = ({ item, effectiveFilters }) => {
  return (
    <div className="dashboard-chart-component" style={{ height: '100%', width: '100%' }}>
      <DashboardChartLoader chartId={item.chartId!} filters={effectiveFilters} />
    </div>
  );
};

export default DashboardChartComponent;
