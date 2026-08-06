"use client";

import React, { useEffect, useState, useRef } from "react";
import { useRouter, useParams } from "next/navigation";
import { DashboardProvider, DashboardConfig, useDashboard } from "../../../../components/dashboards/DashboardContext";
import DashboardCanvas from "../../../../components/dashboards/DashboardCanvas";
import DashboardFilterBarReadOnly from "../../../../components/dashboards/DashboardFilterBarReadOnly";
import { LoadingOverlay } from "../../../../components/LoadingOverlay";
import { msalFetch } from "../../../../utils/msalFetch";
import { useRecents } from "../../../../hooks/useRecents";
import { API_BASE } from "../../../../config";
import { toJpeg } from "html-to-image";

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
  const canvasRef = useRef<HTMLDivElement>(null);
  const capturedRef = useRef(false);
  const viewParams = useParams();
  const dashId = viewParams?.id as string | undefined;

  // Capture a real full-dashboard thumbnail a few seconds after it renders, then
  // save it so the dashboards list can show an actual preview (not a placeholder).
  useEffect(() => {
    if (!chartsReady || capturedRef.current || !dashId || !canvasRef.current) return;
    capturedRef.current = true;
    const t = setTimeout(async () => {
      try {
        const node = canvasRef.current;
        if (!node) return;
        const dataUrl = await toJpeg(node, {
          quality: 0.55, pixelRatio: 0.4, backgroundColor: "#f8fafc", cacheBust: true,
        });
        if (!dataUrl || dataUrl.length > 3_500_000) return;   // guard oversized
        await msalFetch(`${API_BASE}/api/v1/dashboards/${dashId}`, {
          method: "PUT",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({ thumbnail: dataUrl }),
        });
      } catch { /* thumbnail is best-effort */ }
    }, 3500);
    return () => clearTimeout(t);
  }, [chartsReady, dashId]);
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

  // Shared action button style
  const btnBase: React.CSSProperties = {
    display: 'flex', alignItems: 'center', gap: 6, height: 34,
    padding: '0 12px', borderRadius: 8, fontSize: 13, fontWeight: 500,
    cursor: 'pointer', border: '1px solid var(--border)', background: 'var(--bg-surface)',
    color: 'var(--text-secondary)', transition: 'background 0.15s, border-color 0.15s, color 0.15s', whiteSpace: 'nowrap',
  };

  return (
    <div className="page-shell page-shell-wide">
      {/* ── Elegant dashboard header ── */}
      <header style={{
        background: 'var(--bg-surface)', borderBottom: '1px solid var(--border)',
        padding: '12px 24px', display: 'flex', alignItems: 'center',
        justifyContent: 'space-between', gap: 16, flexWrap: 'wrap',
      }}>
        {/* Left: title + badge */}
        <div style={{ display: 'flex', flexDirection: 'column', gap: 2, minWidth: 0 }}>
          <div style={{ display: 'flex', alignItems: 'center', gap: 10 }}>
            <h1 style={{ margin: 0, fontSize: '1.35rem', fontWeight: 800, color: 'var(--text-primary)', lineHeight: 1.2, letterSpacing: '-0.3px' }}>
              {initialConfig?.name || 'Dashboard'}
            </h1>
            <span style={{
              fontSize: 11, fontWeight: 700, padding: '2px 8px', borderRadius: 20, lineHeight: 1.5,
              background: isPublished ? '#f0fdf4' : '#fef3c7',
              color: isPublished ? '#15803d' : '#92400e',
              border: `1px solid ${isPublished ? '#bbf7d0' : '#fde68a'}`,
            }}>
              {isPublished ? 'Published' : 'Draft'}
            </span>
          </div>
          {initialConfig?.description && (
            <p style={{ margin: 0, fontSize: 12.5, color: 'var(--text-muted)', lineHeight: 1.4, marginTop: 1 }}>
              {initialConfig.description}
            </p>
          )}
        </div>

        {/* Right: action toolbar */}
        <div style={{ display: 'flex', alignItems: 'center', gap: 6, flexShrink: 0 }}>

          {/* Refresh group */}
          <div style={{ display: 'flex', alignItems: 'center', gap: 4, background: 'var(--bg-hover)', border: '1px solid var(--border)', borderRadius: 8, padding: '2px 4px' }}>
            <button onClick={handleManualRefresh} title="Refresh all charts" style={{ ...btnBase, border: 'none', background: 'transparent', padding: '0 8px' }}>
              <i className="fas fa-sync-alt" style={{ fontSize: 11 }} />
            </button>
            <select
              value={refreshInterval}
              onChange={(e) => setRefreshInterval(Number(e.target.value))}
              style={{
                height: 28, padding: '0 6px', border: 'none', background: 'transparent',
                fontSize: 12, color: refreshInterval > 0 ? '#15803d' : '#475569',
                cursor: 'pointer', fontWeight: refreshInterval > 0 ? 600 : 400,
              }}
            >
              {REFRESH_INTERVALS.map((ri) => (
                <option key={ri.value} value={ri.value}>{ri.label === 'Off' ? 'Auto-refresh' : ri.label}</option>
              ))}
            </select>
            {lastRefreshed && (
              <span style={{ fontSize: 11, color: '#94a3b8', paddingRight: 6, whiteSpace: 'nowrap' }}>
                {lastRefreshed.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', second: '2-digit' })}
              </span>
            )}
          </div>

          {/* Divider */}
          <div style={{ width: 1, height: 22, background: '#e2e8f0', margin: '0 2px' }} />

          {/* Filters */}
          {hasFilters && (
            <button onClick={() => setFiltersOpen((v) => !v)} style={{
              ...btnBase,
              background: filtersOpen ? 'rgba(var(--accent-rgb), 0.06)' : 'var(--bg-surface)',
              borderColor: filtersOpen ? '#bfdbfe' : '#e2e8f0',
              color: filtersOpen ? '#2563eb' : '#475569',
            }}>
              <i className="fas fa-filter" style={{ fontSize: 11 }} />
              Filters
            </button>
          )}

          {/* Favorite */}
          <button type="button" onClick={onFavoriteClick} title={isFavorite ? 'Remove from favorites' : 'Add to favorites'} style={{
            ...btnBase, padding: '0 10px',
            background: isFavorite ? 'rgba(245, 158, 11, 0.08)' : 'var(--bg-surface)',
            borderColor: isFavorite ? '#fde68a' : '#e2e8f0',
            transform: isAnimating ? 'scale(0.88)' : 'scale(1)',
            transition: 'all 0.2s ease',
          }}>
            <i className={isFavorite ? 'fas fa-star' : 'far fa-star'} style={{
              fontSize: 14, color: isFavorite ? '#f59e0b' : '#9ca3af',
              filter: isFavorite ? 'drop-shadow(0 1px 3px rgba(245,158,11,0.4))' : 'none',
            }} />
          </button>

          {/* Divider */}
          <div style={{ width: 1, height: 22, background: '#e2e8f0', margin: '0 2px' }} />

          {/* Publish */}
          {!isPublished && (
            <button onClick={onPublish} disabled={publishing} style={{
              ...btnBase, background: publishing ? '#f1f5f9' : '#f0fdf4',
              borderColor: publishing ? '#e2e8f0' : '#86efac', color: publishing ? '#94a3b8' : '#15803d',
              fontWeight: 600, cursor: publishing ? 'not-allowed' : 'pointer',
            }}>
              <i className={`fas ${publishing ? 'fa-spinner fa-spin' : 'fa-check-circle'}`} style={{ fontSize: 12 }} />
              {publishing ? 'Publishing…' : 'Publish'}
            </button>
          )}

          {/* Edit */}
          <button onClick={onEdit} style={{ ...btnBase, background: '#2563eb', borderColor: '#2563eb', color: 'white', fontWeight: 600 }}
            onMouseEnter={e => { (e.currentTarget as HTMLButtonElement).style.background = '#1d4ed8'; }}
            onMouseLeave={e => { (e.currentTarget as HTMLButtonElement).style.background = '#2563eb'; }}
          >
            <i className="fas fa-edit" style={{ fontSize: 12 }} />
            Edit
          </button>
        </div>
      </header>

      {/* Inline filter bar — shown only when toggled open */}
      {hasFilters && filtersOpen && (
        <div style={{
          background: 'var(--bg-surface)',
          borderBottom: '1px solid var(--border)',
          padding: '12px 24px',
        }}>
          <DashboardFilterBarReadOnly />
        </div>
      )}

      {/* Canvas — rendered only after all chart configs are preloaded so each
          chart mounts once with its config already in cache and runs exactly
          one query instead of flashing through multiple loading states. */}
      <div style={{ flex: 1, overflow: 'auto', padding: '24px 32px', background: 'var(--bg-primary)' }}>
        {!chartsReady ? (
          <div style={{ display: 'flex', flexDirection: 'column', alignItems: 'center', justifyContent: 'center', padding: 60, gap: 12, color: 'var(--text-muted)', fontSize: 13 }}>
            <i className="fas fa-spinner fa-spin" style={{ fontSize: 22, color: 'var(--accent)' }} />
            <span>Loading dashboard…</span>
          </div>
        ) : (
          <div ref={canvasRef}>
            <DashboardCanvas />
          </div>
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
  const { addRecent } = useRecents();

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
          theme: d.theme || "default",
          layout: (() => { const p = JSON.parse(d.layout || "[]"); return Array.isArray(p) ? p : []; })(),
          filters: JSON.parse(d.filters || "[]"),
          filterLogic: "AND",
          chartIds: JSON.parse(d.charts || "[]"),
        });
        setIsPublished(d.is_published || false);
        setIsFavorite(d.is_favorite || false);
        addRecent({ id: `dashboard-${d.id}`, label: d.name || "Untitled Dashboard", href: `/dashboards/${d.id}/view`, type: "dashboard" });
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
