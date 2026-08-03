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
import dynamic from 'next/dynamic';
import { ChartBuilderProvider } from '../../charts/ChartBuilderContext';

const ChartHydrator = dynamic(() => import('../../charts/ChartHydrator'), { ssr: false });
const ChartPreview  = dynamic(() => import('../../charts/ChartPreview'),  { ssr: false });
import ChartActionsOverlay from './ChartActionsOverlay';

interface DashboardChartLoaderProps {
  itemId: string;
  chartId: number;
  filters: any[];
  crossFilterFilters: Array<{ column: string | null; operator: string; value: string }>;
  onCrossFilter: (column: string | null, value: string) => void;
  isEditMode: boolean;
  onDuplicate?: () => void;
  onRemove?: () => void;
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
  onDuplicate,
  onRemove,
}) => {
  const { getChartConfig, dashboardId, globalRefreshTick } = useDashboard();

  // Initialise synchronously from preload cache when available so charts that
  // mount after preloading skip the fetch entirely and never show a loading flash.
  const [chart, setChart] = useState<any>(() => getChartConfig(chartId) || null);
  const [, setIsLoading] = useState<boolean>(() => !getChartConfig(chartId));
  const [error, setError] = useState<string | null>(null);
  const [refreshKey, setRefreshKey] = useState(0);
  const hasFetchedRef = useRef(false);
  const exportsRef = useRef<{ downloadPng: () => void; downloadCsv: () => void } | null>(null);

  // Keep latest filters in a ref to avoid re-triggering the fetch on filter changes.
  const filtersRef = useRef(filters);
  useEffect(() => { filtersRef.current = filters; }, [filters]);

  // Trigger a chart re-query whenever the global refresh tick increments.
  const prevTickRef = useRef(globalRefreshTick);
  useEffect(() => {
    if (globalRefreshTick !== prevTickRef.current) {
      prevTickRef.current = globalRefreshTick;
      setRefreshKey((k) => k + 1);
    }
  }, [globalRefreshTick]);

  useEffect(() => {
    if (hasFetchedRef.current) return;
    hasFetchedRef.current = true;

    // If useState already resolved from cache, nothing to do.
    if (chart) return;

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

  // Broadcast every cross-filter to all tiles. The backend gracefully skips a
  // filter whose column isn't in a chart's dataset, so clicking (e.g.) a country
  // on the map filters every COVID chart that has a `country` column and is a
  // no-op on the rest — no per-chart gating needed, and no spurious errors.
  const relevantCrossExtras = crossFilterFilters.filter(cf => !!cf.column);
  void normCol; // retained for potential future column-aware gating

  // Inject the resolved column before calling parent
  const handleCrossFilter = useCallback((value: string) => {
    onCrossFilter(crossFilterColumn, value);
  }, [onCrossFilter, crossFilterColumn]);

  if (error) {
    return (
      <div style={{ padding: 20, color: '#dc2626', fontSize: 13 }}>
        {error}
      </div>
    );
  }

  // Always render ChartBuilderProvider so ChartPreview owns all loading states.
  // When chart config is still being fetched (chart = null), ChartHydrator is a
  // no-op and ChartPreview shows its built-in empty/loading animation.
  // This means users see exactly ONE loading animation instead of the old
  // "Loading chart…" text → animated bars → "Querying…" sequence.
  const runCtx = dashboardId ? `dashboard:${dashboardId}` : 'dashboard';

  const tileTitle: string = chart?.name || '';
  const tileInfo: string = chart?.description && chart.description !== chart?.name ? chart.description : '';

  return (
    <ChartBuilderProvider runContext={runCtx}>
      {/* ChartHydrator renders null — it only populates context state */}
      <ChartHydrator
        key={refreshKey}
        chart={chart}
        externalFilters={filtersRef.current}
        crossFilterExtra={relevantCrossExtras}
      />
      {/* Tile title bar — gives every chart a clear name + optional info tooltip */}
      {tileTitle && (
        <div style={{
          display: 'flex', alignItems: 'center', gap: 6,
          padding: '7px 12px 3px', flexShrink: 0, minHeight: 0,
        }}>
          <span
            title={tileTitle}
            style={{
              fontSize: 12.5, fontWeight: 700, color: '#0f172a', letterSpacing: '-0.2px',
              whiteSpace: 'nowrap', overflow: 'hidden', textOverflow: 'ellipsis',
            }}
          >{tileTitle}</span>
          {tileInfo && (
            <span
              title={tileInfo}
              style={{ color: '#94a3b8', fontSize: 11, cursor: 'help', flexShrink: 0, lineHeight: 1 }}
            >
              <i className="fas fa-circle-info" />
            </span>
          )}
        </div>
      )}
      <div style={{ position: 'relative', flex: 1, minHeight: 0 }}>
        <ChartPreview
          onCrossFilter={isEditMode ? undefined : handleCrossFilter}
          onRegisterExports={(exp) => { exportsRef.current = exp; }}
        />
        <ChartActionsOverlay
          chartId={chartId}
          dashboardId={dashboardId}
          isEditMode={isEditMode}
          onRefresh={() => setRefreshKey(k => k + 1)}
          onDuplicate={onDuplicate}
          onRemove={onRemove}
          exportsRef={exportsRef}
        />
      </div>
    </ChartBuilderProvider>
  );
};

/**
 * Top-level dashboard chart component — thin wrapper that passes item props.
 */
export const DashboardChartComponent: React.FC<DashboardComponentProps> = ({ item, effectiveFilters, isEditMode, onRemove, onDuplicate }) => {
  const { setCrossFilter, clearCrossFilter, getCrossFilterFilters, crossFilters } = useDashboard();

  // Charts marked exempt ignore ALL dashboard + cross filters (e.g. a share-by-
  // country donut that would otherwise collapse to 100% when a country is picked).
  const isExempt = !!item.exemptFromFilters;
  const appliedFilters = isExempt ? [] : effectiveFilters;
  const crossFilterFilters = isExempt ? [] : getCrossFilterFilters(item.i);
  const activeCrossFilter = crossFilters[item.i];

  const handleCrossFilter = useCallback((column: string | null, value: string) => {
    // Ignore empty/blank axis clicks — they'd otherwise become a real `= ''` predicate.
    if (value === null || value === undefined || String(value).trim() === '') return;
    // Toggle: clicking the same column+value again clears the filter.
    if (activeCrossFilter && activeCrossFilter.value === value && activeCrossFilter.column === column) {
      clearCrossFilter(item.i);
    } else {
      setCrossFilter(item.i, column, value);
    }
  }, [item.i, activeCrossFilter, setCrossFilter, clearCrossFilter]);

  return (
    <div className="dashboard-chart-component" style={{ height: '100%', width: '100%', position: 'relative', display: 'flex', flexDirection: 'column' }}>
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
        filters={appliedFilters}
        crossFilterFilters={crossFilterFilters}
        onCrossFilter={handleCrossFilter}
        isEditMode={isEditMode}
        onDuplicate={onDuplicate ? () => onDuplicate(item.i) : undefined}
        onRemove={onRemove ? () => onRemove(item.i) : undefined}
      />
    </div>
  );
};

export default DashboardChartComponent;
