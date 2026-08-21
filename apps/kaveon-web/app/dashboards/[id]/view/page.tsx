"use client";

import React, { useEffect, useState, useRef } from "react";
import { useRouter, useParams } from "next/navigation";
import { DashboardProvider, DashboardConfig, useDashboard } from "../../../../components/dashboards/DashboardContext";
import DashboardCanvas from "../../../../components/dashboards/DashboardCanvas";
import DashboardFilterBarReadOnly from "../../../../components/dashboards/DashboardFilterBarReadOnly";
import { LoadingOverlay } from "../../../../components/LoadingOverlay";
import { KaveonLoading } from "../../../../components/KaveonLoading";
import { msalFetch } from "../../../../utils/msalFetch";
import { useRecents } from "../../../../hooks/useRecents";
import { resetQuerySemaphore, isQueryIdle } from "../../../../utils/querySemaphore";
import { API_BASE } from "../../../../config";
import { toJpeg } from "html-to-image";
import { useTheme } from "../../../../contexts/ThemeContext";

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
  onClose: () => void;
}> = ({ isFavorite, isAnimating, isPublished, initialConfig, publishing, onFavoriteClick, onPublish, onEdit, onClose }) => {
  const { preloadAllCharts, isPreloading, dashboardFilters, triggerGlobalRefresh } = useDashboard();
  const { theme } = useTheme();
  const isDark = theme === "dark";
  const hasPreloadedRef = useRef(false);
  const [chartsReady, setChartsReady] = useState(false);
  const canvasRef = useRef<HTMLDivElement>(null);
  const capturedRef = useRef(false);
  const viewParams = useParams();
  const dashId = viewParams?.id as string | undefined;

  // Capture a real full-dashboard thumbnail — but only AFTER the charts have
  // actually finished querying/rendering (else the thumbnail is just spinners),
  // and off the main thread (toJpeg is heavy and would block clicks mid-load).
  useEffect(() => {
    if (!chartsReady || capturedRef.current || !dashId || !canvasRef.current) return;
    let cancelled = false;
    let sawActivity = false;
    const started = Date.now();

    const capture = async () => {
      if (cancelled || capturedRef.current) return;
      capturedRef.current = true;
      try {
        const node = canvasRef.current;
        if (!node) return;
        const dataUrl = await toJpeg(node, {
          // skipFonts avoids html-to-image reading cssRules from cross-origin
          // stylesheets (FontAwesome/fonts CDN) which throws a SecurityError.
          // Match the active theme so a dark-mode dashboard doesn't get a light
          // thumbnail (and vice-versa). Re-captured on each view, so switching
          // theme and reopening refreshes the thumbnail to match.
          quality: 0.55, pixelRatio: 0.4, backgroundColor: isDark ? "#0b1220" : "#f8fafc", cacheBust: true, skipFonts: true,
        });
        if (!dataUrl || dataUrl.length > 3_500_000) {
          notifyParentDone();
          return;
        }
        // Save into the theme-specific slot so both a light and a dark preview
        // can coexist; the Library shows whichever matches the viewer's theme.
        await msalFetch(`${API_BASE}/api/v1/dashboards/${dashId}`, {
          method: "PUT",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify(isDark ? { thumbnail_dark: dataUrl } : { thumbnail: dataUrl }),
        });
      } catch { /* best-effort */ }
      finally { notifyParentDone(); }
    };

    // When rendered inside the thumbnail-refresh iframe (?capture=1), tell the
    // parent this dashboard+theme is done so it can advance to the next one.
    const notifyParentDone = () => {
      try {
        const sp = new URLSearchParams(window.location.search);
        if (sp.get("capture") === "1" && window.parent && window.parent !== window) {
          window.parent.postMessage(
            { type: "kaveon-thumb-done", id: dashId, theme: isDark ? "dark" : "light" }, "*",
          );
        }
      } catch { /* ignore */ }
    };

    // Poll: wait until queries have started AND then gone idle (charts rendered),
    // plus a short render buffer; hard cap at 12s. Run the capture when the main
    // thread is free so it doesn't block interaction.
    const tick = () => {
      if (cancelled || capturedRef.current) return;
      if (!isQueryIdle()) sawActivity = true;
      const elapsed = Date.now() - started;
      const ready = (sawActivity && isQueryIdle()) || elapsed > 12000;
      if (ready) {
        setTimeout(() => {
          const ric = (window as any).requestIdleCallback as undefined | ((cb: () => void) => void);
          if (ric) ric(() => capture()); else setTimeout(capture, 0);
        }, 900); // render buffer after queries drain
        return;
      }
      poll = window.setTimeout(tick, 500);
    };
    let poll = window.setTimeout(tick, 800);
    return () => { cancelled = true; clearTimeout(poll); };
  }, [chartsReady, dashId, isDark]);

  // If the user flips the theme while viewing, allow one fresh capture so the
  // stored thumbnail is re-shot to match the new theme.
  useEffect(() => { capturedRef.current = false; }, [isDark]);
  const [filtersOpen, setFiltersOpen] = useState(false);
  // Show the filter bar by default when the dashboard has filters (once, on load)
  // — filters that exist should be visible, not hidden behind the button. The user
  // can still collapse it.
  const filtersAutoOpenedRef = useRef(false);
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

  // react-grid-layout measures the canvas width on mount; after a save→redirect
  // that can land at half width before the layout settles. Nudge a remeasure
  // once charts are ready (rAF + a short delayed retry).
  useEffect(() => {
    if (!chartsReady) return;
    const fire = () => window.dispatchEvent(new Event("resize"));
    const raf = requestAnimationFrame(fire);
    const t = setTimeout(fire, 200);
    return () => { cancelAnimationFrame(raf); clearTimeout(t); };
  }, [chartsReady]);

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

  // Open the filter bar by default the first time filters are present (published only).
  useEffect(() => {
    if (hasFilters && isPublished && !filtersAutoOpenedRef.current) {
      filtersAutoOpenedRef.current = true;
      setFiltersOpen(true);
    }
  }, [hasFilters, isPublished]);

  // Shared action button style
  const btnBase: React.CSSProperties = {
    display: 'flex', alignItems: 'center', gap: 6, height: 34,
    padding: '0 12px', borderRadius: 8, fontSize: 13, fontWeight: 500,
    cursor: 'pointer', border: '1px solid var(--border)', background: 'var(--bg-surface)',
    color: 'var(--text-secondary)', transition: 'background 0.15s, border-color 0.15s, color 0.15s', whiteSpace: 'nowrap',
  };

  return (
    <div className="page-shell page-shell-wide">
      {/* ── Elegant dashboard header (rounded card, matches the chart page) ── */}
      <header style={{
        background: 'var(--bg-surface)', border: '1px solid var(--border)', borderRadius: 12,
        padding: '12px 18px', display: 'flex', alignItems: 'center',
        justifyContent: 'space-between', gap: 16, flexWrap: 'wrap', marginBottom: 16,
      }}>
        {/* Left: back + title + badge */}
        <div style={{ display: 'flex', alignItems: 'center', gap: 14, minWidth: 0 }}>
          <button
            type="button"
            onClick={onClose}
            title="Back to dashboards"
            aria-label="Back to dashboards"
            style={{
              flexShrink: 0, width: 36, height: 36, borderRadius: 9,
              display: 'flex', alignItems: 'center', justifyContent: 'center',
              border: '1px solid var(--border)', background: 'var(--bg-surface)',
              color: 'var(--text-secondary)', cursor: 'pointer',
              transition: 'background 0.15s, color 0.15s, border-color 0.15s',
            }}
            onMouseEnter={(e) => { e.currentTarget.style.background = 'var(--bg-hover)'; e.currentTarget.style.color = 'var(--text-primary)'; }}
            onMouseLeave={(e) => { e.currentTarget.style.background = 'var(--bg-surface)'; e.currentTarget.style.color = 'var(--text-secondary)'; }}
          >
            <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
              <line x1="19" y1="12" x2="5" y2="12" /><polyline points="12 19 5 12 12 5" />
            </svg>
          </button>
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
        </div>

        {/* Right: action toolbar */}
        <div style={{ display: 'flex', alignItems: 'center', gap: 6, flexShrink: 0, flexWrap: 'wrap' }}>

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
            ...btnBase, width: 34, padding: 0, justifyContent: 'center',
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

          {/* Edit — icon-only */}
          <button onClick={onEdit} title="Edit dashboard" aria-label="Edit dashboard"
            style={{ ...btnBase, width: 34, padding: 0, justifyContent: 'center', background: '#2563eb', borderColor: '#2563eb', color: 'white' }}
            onMouseEnter={e => { (e.currentTarget as HTMLButtonElement).style.background = '#1d4ed8'; }}
            onMouseLeave={e => { (e.currentTarget as HTMLButtonElement).style.background = '#2563eb'; }}
          >
            <i className="fas fa-edit" style={{ fontSize: 13 }} />
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
      {/* Page-shell already provides the outer padding — don't double-inset here. */}
      <div style={{ flex: 1, overflow: 'auto', background: 'var(--bg-primary)' }}>
        {!chartsReady ? (
          <KaveonLoading message="Loading dashboard" />
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
        onEdit={() => {
          // Drop queued dashboard-chart queries so the edit page isn't blocked
          // behind this view's in-flight/queued queries.
          resetQuerySemaphore();
          router.push(`/dashboards/${id}/edit`);
        }}
        onClose={() => {
          resetQuerySemaphore();
          router.push('/workspace?tab=dashboards');
        }}
      />
    </DashboardProvider>
  );
};

export default DashboardViewPage;
