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

/**
 * Inner component that pre-loads charts using the dashboard context
 * Must be inside DashboardProvider to access useDashboard hook
 */
const DashboardViewContent: React.FC<{
  id: string;
  isFavorite: boolean;
  isAnimating: boolean;
  isPublished: boolean;
  publishing: boolean;
  sidebarCollapsed: boolean;
  initialConfig: DashboardConfig | undefined;
  onFavoriteClick: () => void;
  onPublish: () => void;
  onEdit: () => void;
  setSidebarCollapsed: (collapsed: boolean) => void;
}> = ({
  id,
  isFavorite,
  isAnimating,
  isPublished,
  publishing,
  sidebarCollapsed,
  initialConfig,
  onFavoriteClick,
  onPublish,
  onEdit,
  setSidebarCollapsed,
}) => {
  const { preloadAllCharts, isPreloading } = useDashboard();
  const hasPreloadedRef = useRef(false);

  // Pre-load all chart configs in parallel when dashboard loads
  useEffect(() => {
    if (hasPreloadedRef.current || !initialConfig) return;

    hasPreloadedRef.current = true;
    preloadAllCharts(API_BASE, msalFetch);
  }, [initialConfig, preloadAllCharts]);

  return (
    <div className="page-shell page-shell-wide">
      {/* Header */}
      <header className="page-header page-header-with-actions">
        <div className="page-header-main">
          <h1 className="page-header-title">{initialConfig?.name || "Dashboard"}</h1>
          {initialConfig?.description && (
            <p className="page-header-subtitle">{initialConfig.description}</p>
          )}
        </div>
        <div className="page-header-actions">
          {/* Favorite Icon */}
          <button
            type="button"
            className="chart-builder-fav-btn chart-builder-fav-btn-inline"
            aria-label={isFavorite ? "Unfavorite dashboard" : "Favorite dashboard"}
            onClick={onFavoriteClick}
            style={{
              background: 'transparent',
              border: 'none',
              padding: '8px 12px',
              cursor: 'pointer',
              transition: 'all 0.2s ease',
              borderRadius: '6px',
              transform: isAnimating ? 'scale(0.9)' : 'scale(1)',
            }}
            onMouseOver={(e) => {
              if (!isAnimating) {
                e.currentTarget.style.background = isFavorite ? '#FEF3C7' : '#F3F4F6';
                e.currentTarget.style.transform = 'scale(1.05)';
              }
            }}
            onMouseOut={(e) => {
              if (!isAnimating) {
                e.currentTarget.style.background = 'transparent';
                e.currentTarget.style.transform = 'scale(1)';
              }
            }}
          >
            <i
              className={isFavorite ? "fas fa-star" : "far fa-star"}
              aria-hidden="true"
              style={{
                fontSize: '18px',
                color: isFavorite ? '#F59E0B' : '#9CA3AF',
                transition: 'all 0.2s ease',
                filter: isFavorite ? 'drop-shadow(0 2px 4px rgba(245, 158, 11, 0.3))' : 'none',
                transform: isAnimating && isFavorite ? 'scale(1.3)' : 'scale(1)',
              }}
            />
          </button>

          {/* Publish Button - Only shown if not published */}
          {!isPublished && (
            <button
              onClick={onPublish}
              disabled={publishing}
              style={{
                padding: "8px 16px",
                background: publishing ? "#94a3b8" : "#10b981",
                color: "#ffffff",
                border: "none",
                borderRadius: 6,
                cursor: publishing ? "not-allowed" : "pointer",
                fontSize: 13,
                fontWeight: 600,
              }}
            >
              {publishing ? (
                <>
                  <i className="fas fa-spinner fa-spin" style={{ marginRight: 6 }} />
                  Publishing...
                </>
              ) : (
                <>
                  <i className="fas fa-check-circle" style={{ marginRight: 6 }} />
                  Publish
                </>
              )}
            </button>
          )}

          {/* Edit Button - Only shown if not published */}
          {!isPublished && (
            <button
              onClick={onEdit}
              style={{
                padding: "8px 16px",
                background: "#2563eb",
                color: "#ffffff",
                border: "none",
                borderRadius: 6,
                cursor: "pointer",
                fontSize: 13,
                fontWeight: 600,
              }}
            >
              <i className="fas fa-edit" style={{ marginRight: 6 }} />
              Edit
            </button>
          )}
        </div>
      </header>

      {/* Main content with sidebar */}
      <div
        style={{
          flex: 1,
          display: 'grid',
          gridTemplateColumns: sidebarCollapsed
            ? "auto minmax(0, 1fr)"
            : "minmax(0, 360px) minmax(0, 1fr)",
          overflow: 'hidden',
        }}
      >
        {/* Left sidebar - Filters */}
        <div
          style={{
            background: sidebarCollapsed ? 'transparent' : '#fff',
            borderRight: sidebarCollapsed ? 'none' : '1px solid #e2e8f0',
            display: 'flex',
            flexDirection: 'column',
            overflow: 'hidden',
            width: sidebarCollapsed ? 'auto' : undefined,
          }}
        >
          {sidebarCollapsed ? (
            <div style={{
              background: '#f8fafc',
              borderRight: '1px solid #e2e8f0',
              display: 'flex',
              flexDirection: 'column',
              alignItems: 'center',
              height: '100%',
            }}>
              <button
                onClick={() => setSidebarCollapsed(false)}
                style={{
                  background: 'transparent',
                  border: 'none',
                  padding: '12px 8px',
                  cursor: 'pointer',
                  color: '#475569',
                  fontSize: 18,
                  fontWeight: 600,
                }}
                aria-label="Expand filters"
              >
                ❯
              </button>
              <div style={{
                writingMode: 'vertical-rl',
                transform: 'rotate(180deg)',
                fontSize: 12,
                fontWeight: 600,
                color: '#64748b',
                letterSpacing: '0.5px',
                padding: '16px 0',
                textTransform: 'uppercase',
              }}>
                Filters
              </div>
            </div>
          ) : (
            <>
              <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', padding: '12px 16px', borderBottom: '1px solid #e2e8f0' }}>
                <span style={{ fontSize: 14, fontWeight: 600, color: '#1e293b' }}>Filters</span>
                <button
                  onClick={() => setSidebarCollapsed(true)}
                  style={{
                    background: 'transparent',
                    border: 'none',
                    padding: '4px 8px',
                    cursor: 'pointer',
                    color: '#64748b',
                    fontSize: 14,
                  }}
                  aria-label="Collapse filters"
                >
                  ❮
                </button>
              </div>
              <div style={{ flex: 1, overflow: 'auto', padding: '16px' }}>
                <DashboardFilterBarReadOnly />
              </div>
            </>
          )}
        </div>

        {/* Dashboard canvas */}
        <div style={{ background: '#f8fafc', padding: '12px', overflow: 'auto' }}>
          {isPreloading && (
            <div style={{ padding: 20, textAlign: 'center', color: '#64748b' }}>
              <i className="fas fa-spinner fa-spin" style={{ marginRight: 8 }} />
              Loading charts...
            </div>
          )}
          <DashboardCanvas />
        </div>
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
  const [sidebarCollapsed, setSidebarCollapsed] = useState(true);
  const [isFavorite, setIsFavorite] = useState(false);
  const [isAnimating, setIsAnimating] = useState(false);
  const [isPublished, setIsPublished] = useState(false);
  const [publishing, setPublishing] = useState(false);

  /**
   * Load dashboard data
   */
  useEffect(() => {
    if (!id) return;

    const loadDashboard = async () => {
      try {
        setLoading(true);
        const response = await msalFetch(`${API_BASE}/api/v1/dashboards/${id}`);

        if (!response.ok) {
          throw new Error(`Failed to load dashboard: ${response.status}`);
        }

        const dashboard = await response.json();

        // Parse JSON fields
        const config: DashboardConfig = {
          id: dashboard.id,
          name: dashboard.name,
          description: dashboard.description || "",
          layout: (() => { const p = JSON.parse(dashboard.layout || "[]"); return Array.isArray(p) ? p : []; })(),
          filters: JSON.parse(dashboard.filters || "[]"),
          filterLogic: "AND",
          chartIds: JSON.parse(dashboard.charts || "[]"),
        };

        setInitialConfig(config);
        setIsPublished(dashboard.is_published || false);
        setIsFavorite(dashboard.is_favorite || false);
        setError(null);
      } catch (err) {
        console.error("Error loading dashboard:", err);
        const errorMessage = err instanceof Error && err.message === "Failed to fetch"
          ? `Cannot connect to API server at ${API_BASE}. Please ensure the backend is running.`
          : err instanceof Error ? err.message : "Failed to load dashboard";
        setError(errorMessage);
      } finally {
        setLoading(false);
      }
    };

    loadDashboard();
  }, [id]);

  /**
   * Handle favorite toggle
   */
  const handleFavoriteClick = async () => {
    if (!id) return;

    setIsAnimating(true);
    const newFavoriteStatus = !isFavorite;
    setIsFavorite(newFavoriteStatus);

    try {
      const response = await msalFetch(`${API_BASE}/api/v1/dashboards/${id}/favorite`, {
        method: 'PUT',
        headers: {
          'Content-Type': 'application/json',
        },
        body: JSON.stringify({ is_favorite: newFavoriteStatus }),
      });

      if (!response.ok) {
        // Revert on failure
        setIsFavorite(!newFavoriteStatus);
        console.error('Failed to update favorite status');
      }
    } catch (error) {
      // Revert on error
      setIsFavorite(!newFavoriteStatus);
      console.error('Error updating favorite status:', error);
    } finally {
      setTimeout(() => setIsAnimating(false), 300);
    }
  };

  /**
   * Handle publish
   */
  const handlePublish = async () => {
    if (!id) return;

    try {
      setPublishing(true);
      const response = await msalFetch(`${API_BASE}/api/v1/dashboards/${id}`, {
        method: 'PUT',
        headers: {
          'Content-Type': 'application/json',
        },
        body: JSON.stringify({ is_published: true }),
      });

      if (!response.ok) {
        throw new Error(`Failed to publish dashboard: ${response.status}`);
      }

      setIsPublished(true);
    } catch (error) {
      console.error('Error publishing dashboard:', error);
      alert(error instanceof Error ? error.message : 'Failed to publish dashboard');
    } finally {
      setPublishing(false);
    }
  };

  // Show loading state
  if (loading) {
    return <LoadingOverlay />;
  }

  // Show error state
  if (error) {
    return (
      <div className="page-shell page-shell-wide">
        <div style={{ padding: 40, textAlign: "center" }}>
          <i className="fas fa-exclamation-circle" style={{ fontSize: 32, color: "#ef4444" }} />
          <div style={{ marginTop: 16, color: "#64748b" }}>{error}</div>
          <button
            onClick={() => router.push("/dashboards")}
            style={{
              marginTop: 16,
              padding: "8px 16px",
              background: "#2563eb",
              color: "#ffffff",
              border: "none",
              borderRadius: 6,
              cursor: "pointer",
            }}
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
        id={id as string}
        isFavorite={isFavorite}
        isAnimating={isAnimating}
        isPublished={isPublished}
        publishing={publishing}
        sidebarCollapsed={sidebarCollapsed}
        initialConfig={initialConfig}
        onFavoriteClick={handleFavoriteClick}
        onPublish={handlePublish}
        onEdit={() => router.push(`/dashboards/${id}/edit`)}
        setSidebarCollapsed={setSidebarCollapsed}
      />
    </DashboardProvider>
  );
};

export default DashboardViewPage;
