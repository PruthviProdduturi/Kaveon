"use client";

import React, { useEffect, useState, useRef } from "react";
import { useRouter, useParams } from "next/navigation";
import { DashboardProvider, DashboardConfig, useDashboard } from "../../../../components/dashboards/DashboardContext";
import DashboardCanvas from "../../../../components/dashboards/DashboardCanvas";
import DashboardFilterBarReadOnly from "../../../../components/dashboards/DashboardFilterBarReadOnly";
import { LoadingOverlay } from "../../../../components/LoadingOverlay";
import { msalFetch } from "../../../../utils/msalFetch";
import { API_BASE } from "../../../../config";

export const dynamic = 'force-dynamic';
export const dynamicParams = true;

const REFRESH_INTERVALS = [
  { label: 'Off', value: 0 },
  { label: '30s', value: 30 },
  { label: '1m', value: 60 },
  { label: '5m', value: 300 },
  { label: '10m', value: 600 },
  { label: '30m', value: 1800 },
];

const DashboardViewContent: React.FC<{
  isFavorite: boolean;
  isAnimating: boolean;
  isPublished: boolean;
  publishing: boolean;
  initialConfig: DashboardConfig | undefined;
  onFavoriteClick: () => void;
  onPublish: () => void;
  onEdit: () => void;
}> = ({ isFavorite, isAnimating, isPublished, initialConfig, publishing, onFavoriteClick, onPublish, onEdit }) => {
  const { preloadAllCharts, isPreloading, dashboardFilters, triggerGlobalRefresh } = useDashboard();
  const hasPreloadedRef = useRef(false);
  const [chartsReady, setChartsReady] = useState(false);
  const [filtersOpen, setFiltersOpen] = useState(false);
  const [refreshInterval, setRefreshInterval] = useState(0);
  const [lastRefreshed, setLastRefreshed] = useState<Date | null>(null);
  const refreshTimerRef = useRef<ReturnType<typeof setInterval> | null>(null);

  useEffect(() => {
    if (hasPreloadedRef.current || !initialConfig) return;
    hasPreloadedRef.current = true;
    preloadAllCharts(API_BASE, msalFetch)
      .then(() => setChartsReady(true))
      .catch(() => setChartsReady(true));
  }, [initialConfig, preloadAllCharts]);

  // Auto-refresh timer
  useEffect(() => {
    if (refreshTimerRef.current) clearInterval(refreshTimerRef.current);
    if (refreshInterval > 0) {
      refreshTimerRef.current = setInterval(() => {
        triggerGlobalRefresh();
        setLastRefreshed(new Date());
      }, refreshInterval * 1000);
    }
    return () => { if (refreshTimerRef.current) clearInterval(refreshTimerRef.current); };
  }, [refreshInterval, triggerGlobalRefresh]);

  const handleManualRefresh = () => {
    triggerGlobalRefresh();
    setLastRefreshed(new Date());
  };

  const hasFilters = dashboardFilters.length > 0;

  return (
    <div className="page-shell page-shell-wide">
      <header className="page-header page-header-with-actions">
        <div className="page-header-main">
          <h1 className="page-header-title">{initialConfig?.name || "Dashboard"}</h1>
          {initialConfig?.description && (
            <p className="page-header-subtitle">{initialConfig.description}</p>
          )}
        </div>
        <div className="page-header-actions">
          {/* Auto-refresh controls */}
          <div style={{ display: 'flex', alignItems: 'center', gap: 6 }}>
            <button
              onClick={handleManualRefresh}
              title="Refresh all charts"
              style={{
                padding: '7px 10px',
                background: 'transparent',
                border: '1px solid #e2e8f0',
                borderRadius: 6,
                cursor: 'pointer',
                color: '#475569',
              }}
            >
              <i className="fas fa-sync-alt" style={{ fontSize: 12 }} />
            </button>
            <select
              value={refreshInterval}
              onChange={(e) => setRefreshInterval(Number(e.target.value))}
              style={{
                padding: '7px 10px',
                background: refreshInterval > 0 ? '#f0fdf4' : 'transparent',
                border: refreshInterval > 0 ? '1px solid #86efac' : '1px solid #e2e8f0',
                borderRadius: 6,
                cursor: 'pointer',
                fontSize: 12,
                color: refreshInterval > 0 ? '#15803d' : '#475569',
              }}
            >
              {REFRESH_INTERVALS.map((ri) => (
                <option key={ri.value} value={ri.value}>{ri.label === 'Off' ? 'Auto-refresh' : ri.label}</option>
              ))}
            </select>
            {lastRefreshed && (
              <span style={{ fontSize: 11, color: '#94a3b8', whiteSpace: 'nowrap' }}>
                {lastRefreshed.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', second: '2-digit' })}
              </span>
            )}
          </div>

          {/* Filters toggle — only shown if dashboard has filters */}
          {hasFilters && (
            <button
              onClick={() => setFiltersOpen((v) => !v)}
              style={{
                padding: '8px 14px',
                background: filtersOpen ? '#eff6ff' : 'transparent',
                border: filtersOpen ? '1px solid #bfdbfe' : '1px solid #e2e8f0',
                borderRadius: 6,
                cursor: 'pointer',
                fontSize: 13,
                fontWeight: 500,
                color: filtersOpen ? '#2563eb' : '#475569',
                display: 'flex',
                alignItems: 'center',
                gap: 6,
              }}
            >
              <i className="fas fa-filter" style={{ fontSize: 11 }} />
              Filters
            </button>
          )}

          {/* Favorite */}
          <button
            type="button"
            aria-label={isFavorite ? "Unfavorite dashboard" : "Favorite dashboard"}
            onClick={onFavoriteClick}
            style={{
              background: 'transparent',
              border: 'none',
              padding: '8px 10px',
              cursor: 'pointer',
              borderRadius: 6,
              transform: isAnimating ? 'scale(0.9)' : 'scale(1)',
              transition: 'transform 0.2s ease',
            }}
            onMouseOver={(e) => { e.currentTarget.style.background = isFavorite ? '#FEF3C7' : '#F3F4F6'; }}
            onMouseOut={(e) => { e.currentTarget.style.background = 'transparent'; }}
          >
            <i
              className={isFavorite ? "fas fa-star" : "far fa-star"}
              style={{
                fontSize: 18,
                color: isFavorite ? '#F59E0B' : '#9CA3AF',
                transition: 'all 0.2s ease',
                filter: isFavorite ? 'drop-shadow(0 2px 4px rgba(245,158,11,0.3))' : 'none',
              }}
            />
          </button>

          {!isPublished && (
            <button
              onClick={onPublish}
              disabled={publishing}
              style={{
                padding: '8px 16px',
                background: publishing ? '#94a3b8' : '#10b981',
                color: '#fff',
                border: 'none',
                borderRadius: 6,
                cursor: publishing ? 'not-allowed' : 'pointer',
                fontSize: 13,
                fontWeight: 600,
              }}
            >
              {publishing
                ? <><i className="fas fa-spinner fa-spin" style={{ marginRight: 6 }} />Publishing…</>
                : <><i className="fas fa-check-circle" style={{ marginRight: 6 }} />Publish</>
              }
            </button>
          )}

          <button
            onClick={onEdit}
            style={{
              padding: '8px 16px',
              background: '#2563eb',
              color: '#fff',
              border: 'none',
              borderRadius: 6,
              cursor: 'pointer',
              fontSize: 13,
              fontWeight: 600,
            }}
          >
            <i className="fas fa-edit" style={{ marginRight: 6 }} />
            Edit
          </button>
        </div>
      </header>

      {/* Inline filter bar — shown only when toggled open */}
      {hasFilters && filtersOpen && (
        <div style={{
          background: '#fff',
          borderBottom: '1px solid #e2e8f0',
          padding: '12px 24px',
        }}>
          <DashboardFilterBarReadOnly />
        </div>
      )}

      {/* Canvas — rendered only after all chart configs are preloaded so each
          chart mounts once with its config already in cache and runs exactly
          one query instead of flashing through multiple loading states. */}
      <div style={{ flex: 1, overflow: 'auto', padding: '16px 24px', background: '#f8fafc' }}>
        {!chartsReady ? (
          <div style={{ display: 'flex', flexDirection: 'column', alignItems: 'center', justifyContent: 'center', padding: 60, gap: 12, color: '#94a3b8', fontSize: 13 }}>
            <i className="fas fa-spinner fa-spin" style={{ fontSize: 22 }} />
            <span>Loading dashboard…</span>
          </div>
        ) : (
          <DashboardCanvas />
        )}
      </div>
    </div>
  );
};

const DashboardViewPage: React.FC = () => {
  const router = useRouter();
  const params = useParams();
  const id = params?.id as string | undefined;
  const [initialConfig, setInitialConfig] = useState<DashboardConfig | undefined>(undefined);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [isFavorite, setIsFavorite] = useState(false);
  const [isAnimating, setIsAnimating] = useState(false);
  const [isPublished, setIsPublished] = useState(false);
  const [publishing, setPublishing] = useState(false);

  useEffect(() => {
    if (!id) return;
    const load = async () => {
      try {
        setLoading(true);
        const res = await msalFetch(`${API_BASE}/api/v1/dashboards/${id}`);
        if (!res.ok) throw new Error(`Failed to load dashboard: ${res.status}`);
        const d = await res.json();
        setInitialConfig({
          id: d.id,
          name: d.name,
          description: d.description || "",
          layout: (() => { const p = JSON.parse(d.layout || "[]"); return Array.isArray(p) ? p : []; })(),
          filters: JSON.parse(d.filters || "[]"),
          filterLogic: "AND",
          chartIds: JSON.parse(d.charts || "[]"),
        });
        setIsPublished(d.is_published || false);
        setIsFavorite(d.is_favorite || false);
      } catch (err) {
        setError(err instanceof Error ? err.message : "Failed to load dashboard");
      } finally {
        setLoading(false);
      }
    };
    load();
  }, [id]);

  const handleFavoriteClick = async () => {
    if (!id) return;
    setIsAnimating(true);
    const next = !isFavorite;
    setIsFavorite(next);
    try {
      const res = await msalFetch(`${API_BASE}/api/v1/dashboards/${id}/favorite`, {
        method: 'PUT',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ is_favorite: next }),
      });
      if (!res.ok) setIsFavorite(!next);
    } catch {
      setIsFavorite(!next);
    } finally {
      setTimeout(() => setIsAnimating(false), 300);
    }
  };

  const handlePublish = async () => {
    if (!id) return;
    try {
      setPublishing(true);
      const res = await msalFetch(`${API_BASE}/api/v1/dashboards/${id}`, {
        method: 'PUT',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ is_published: true }),
      });
      if (!res.ok) throw new Error(`Failed to publish: ${res.status}`);
      setIsPublished(true);
    } catch (err) {
      console.error(err);
    } finally {
      setPublishing(false);
    }
  };

  if (loading) return <LoadingOverlay />;

  if (error) {
    return (
      <div className="page-shell page-shell-wide">
        <div style={{ padding: 40, textAlign: "center" }}>
          <i className="fas fa-exclamation-circle" style={{ fontSize: 32, color: "#ef4444" }} />
          <div style={{ marginTop: 16, color: "#64748b" }}>{error}</div>
          <button
            onClick={() => router.push("/dashboards")}
            style={{ marginTop: 16, padding: "8px 16px", background: "#2563eb", color: "#fff", border: "none", borderRadius: 6, cursor: "pointer" }}
          >
            Back to Dashboards
          </button>
        </div>
      </div>
    );
  }

  return (
    <DashboardProvider initialConfig={initialConfig}>
      <DashboardViewContent
        isFavorite={isFavorite}
        isAnimating={isAnimating}
        isPublished={isPublished}
        publishing={publishing}
        initialConfig={initialConfig}
        onFavoriteClick={handleFavoriteClick}
        onPublish={handlePublish}
        onEdit={() => router.push(`/dashboards/${id}/edit`)}
      />
    </DashboardProvider>
  );
};

export default DashboardViewPage;
