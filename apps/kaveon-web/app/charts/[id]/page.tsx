"use client";

import {
  ChartBuilderProvider,
  useChartBuilder,
} from "../../../components/charts/ChartBuilderContext";
import ChartHydrator from "../../../components/charts/ChartHydrator";
import React, { useEffect, useRef, useState } from "react";
import { loginRequest, getMsalInstance } from "../../../auth/msalConfig";

import { API_BASE } from "../../../config";
import CreateChartLayout from "../../../components/charts/CreateChartLayout";
import { LoadingOverlay } from "../../../components/LoadingOverlay";
import SaveChartModal from "../../../components/charts/SaveChartModal";
import { msalFetch } from "../../../utils/msalFetch";
import { useAuth } from "../../../auth/useAuth";
import { useRecents } from "../../../hooks/useRecents";
import { useRouter, useParams } from "next/navigation";

export const dynamic = 'force-dynamic';
export const dynamicParams = true;

// using same-origin relative API calls

interface ChartDetail {
  id: number;
  name: string;
  description?: string | null;
  chart_type: string;
  dataset_id: number;
  query_config?: any;
  viz_config?: any;
  sql_text?: string | null;
}

interface DatasetDetailSummary {
  id: number;
  name: string;
  description?: string | null;
}

interface ChartDetailBuilderViewProps {
  chart: ChartDetail;
  isFavorite: boolean;
  onToggleFavorite: () => void;
  dataset: DatasetDetailSummary;
}

const ChartDetailBuilderView: React.FC<ChartDetailBuilderViewProps> = ({
  chart,
  isFavorite,
  onToggleFavorite,
  dataset,
}) => {
  const { name, setName, canSave, isSaving, saveError } = useChartBuilder();
  const headerRouter = useRouter();
  const [isSaveModalOpen, setIsSaveModalOpen] = useState(false);
  const [isAnimating, setIsAnimating] = useState(false);
  const [isEditingName, setIsEditingName] = useState(false);
  const nameInputRef = useRef<HTMLInputElement>(null);

  const handleFavoriteClick = () => {
    setIsAnimating(true);
    onToggleFavorite();
    setTimeout(() => setIsAnimating(false), 300);
  };

  useEffect(() => {
    if (isEditingName) nameInputRef.current?.select();
  }, [isEditingName]);

  const displayName = name.trim() || chart.name || "Chart";

  return (
    <>
      <header className="page-header page-header-with-actions">
        <div style={{ display: "flex", alignItems: "center", gap: 12, minWidth: 0 }}>
          <button
            type="button"
            onClick={() => headerRouter.push("/workspace?tab=charts")}
            title="Back to charts"
            aria-label="Back to charts"
            style={{
              flexShrink: 0, width: 36, height: 36, borderRadius: 9,
              display: "flex", alignItems: "center", justifyContent: "center",
              border: "1px solid var(--border)", background: "var(--bg-surface)",
              color: "var(--text-secondary)", cursor: "pointer",
              transition: "background 0.15s, color 0.15s, border-color 0.15s",
            }}
            onMouseEnter={(e) => { e.currentTarget.style.background = "var(--bg-hover)"; e.currentTarget.style.color = "var(--text-primary)"; }}
            onMouseLeave={(e) => { e.currentTarget.style.background = "var(--bg-surface)"; e.currentTarget.style.color = "var(--text-secondary)"; }}
          >
            <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
              <line x1="19" y1="12" x2="5" y2="12" /><polyline points="12 19 5 12 12 5" />
            </svg>
          </button>
          <div className="page-header-main">
          <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
            {!isEditingName ? (
              <h1
                className="page-header-title"
                style={{ margin: 0, cursor: "pointer", borderRadius: 6, padding: "4px 8px", border: "2px solid transparent", transition: "background 0.15s" }}
                onClick={() => setIsEditingName(true)}
                onMouseOver={e => { e.currentTarget.style.background = "var(--bg-hover)"; }}
                onMouseOut={e => { e.currentTarget.style.background = "transparent"; }}
                title="Click to rename"
              >
                {displayName}
              </h1>
            ) : (
              <input
                ref={nameInputRef}
                type="text"
                value={name}
                onChange={e => setName(e.target.value)}
                onBlur={() => setIsEditingName(false)}
                onKeyDown={e => { if (e.key === "Enter" || e.key === "Escape") setIsEditingName(false); }}
                placeholder="Chart name"
                style={{
                  fontSize: 18, fontWeight: 600, color: "var(--text-primary)",
                  padding: "4px 8px", margin: 0,
                  border: "2px solid var(--accent)", borderRadius: 6,
                  outline: "none", background: "var(--bg-surface)",
                  fontFamily: "inherit", minWidth: 200,
                }}
              />
            )}
          </div>
          {chart.description && (
            <p className="page-header-subtitle">{chart.description}</p>
          )}
          </div>
        </div>
        <div className="page-header-actions">
          {saveError && <span className="page-header-error">{saveError}</span>}
          <button
            type="button"
            className="chart-builder-fav-btn chart-builder-fav-btn-inline"
            aria-label={isFavorite ? "Unfavorite chart" : "Favorite chart"}
            onClick={handleFavoriteClick}
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
                e.currentTarget.style.background = isFavorite ? 'color-mix(in srgb, #F59E0B 15%, var(--bg-surface))' : 'var(--bg-hover)';
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
                fontSize: '20px',
                color: isFavorite ? '#F59E0B' : 'var(--text-muted)',
                transition: 'all 0.2s ease',
                filter: isFavorite ? 'drop-shadow(0 2px 4px rgba(245, 158, 11, 0.3))' : 'none',
                transform: isAnimating && isFavorite ? 'scale(1.3)' : 'scale(1)',
              }}
            />
          </button>
          <button
            type="button"
            className="chart-builder-primary-btn"
            onClick={() => {
              if (canSave) {
                setIsSaveModalOpen(true);
              }
            }}
            disabled={!canSave || isSaving}
          >
            {isSaving ? "Saving..." : "Save chart"}
          </button>
        </div>
      </header>

      <ChartHydrator chart={chart} />
      <CreateChartLayout />

      <SaveChartModal
        isOpen={isSaveModalOpen}
        onClose={() => setIsSaveModalOpen(false)}
      />
    </>
  );
};

const ChartDetailPage: React.FC = () => {
  const router = useRouter();
  const params = useParams();
  const { isAuthenticated, account } = useAuth();
  const { addRecent } = useRecents();
  const [accessToken, setAccessToken] = useState<string>("");

  const [chart, setChart] = useState<ChartDetail | null>(null);
  const [dataset, setDataset] = useState<DatasetDetailSummary | null>(null);
  const [isLoading, setIsLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [isFavorite, setIsFavorite] = useState(false);
  const fetchedChartIdRef = useRef<string | null>(null);

  const chartId = params?.id as string | undefined;

  // Acquire and cache access token once
  useEffect(() => {
    if (!isAuthenticated) return;
    const getToken = async () => {
      const accounts = getMsalInstance().getAllAccounts();
      if (accounts && accounts.length > 0) {
        const result = await getMsalInstance().acquireTokenSilent({ ...loginRequest, account: accounts[0] });
        setAccessToken(result.accessToken);
      }
    };
    getToken();
  }, [isAuthenticated]);

  useEffect(() => {
    console.log('[ChartPage] Fetch effect triggered:', {
      isAuthenticated,
      chartId,
      refValue: fetchedChartIdRef.current,
    });

    if (!isAuthenticated || !chartId) return;

    // Prevent duplicate fetches - use a flag that persists across React Strict Mode double-mounting
    if (fetchedChartIdRef.current === chartId) {
      console.log('[ChartPage] ⛔ Skipping duplicate fetch - already fetched chartId:', chartId);
      return;
    }

    console.log('[ChartPage] ✓ Proceeding with fetch for chartId:', chartId);

    // Set the flag IMMEDIATELY before any async operations to prevent race conditions
    fetchedChartIdRef.current = chartId;
    console.log('[ChartPage] Set ref to:', fetchedChartIdRef.current);

    setIsLoading(true);
    setError(null);

    const userEmail = account?.email || account?.username || null;

    // Step 1: Fetch chart summary to get dataset_id and favorite status
    msalFetch(`${API_BASE}/api/v1/charts/summary`, {
      headers: userEmail ? { 'x-user-email': userEmail } : undefined,
    })
      .then(res => res.ok ? res.json() : Promise.reject("Failed to load charts"))
      .then((data) => {
        const found = (data.recent || []).find((c: any) => String(c.id) === String(chartId));
        if (!found) throw new Error("Chart not found");
        setIsFavorite(!!found.favorite);
        // Step 2: Fetch full chart details
        return msalFetch(`${API_BASE}/api/v1/charts/${chartId}`)
          .then(res => res.ok ? res.json() : Promise.reject("Failed to load chart details"))
          .then((chartDetail) => {
            setChart(chartDetail);
            // Record in recents (id prefixed by type, matching delete cleanup).
            if (chartDetail?.name) {
              addRecent({ id: `chart-${chartId}`, label: chartDetail.name, href: `/charts/${chartId}`, type: "chart" });
            }
            // Step 3: Fetch dataset details if dataset_id exists
            if (chartDetail.dataset_id) {
              // Fetch all summaries and filter for the correct one
              return msalFetch(`${API_BASE}/api/v1/datasets/summary`)
                .then(res => res.ok ? res.json() : Promise.reject("Failed to load dataset details"))
                .then((data) => {
                  const found = (data.recent || []).find((d: any) => d.id === chartDetail.dataset_id);
                  if (found) {
                    setDataset({
                      id: found.id,
                      name: found.dataset_name,
                      description: found.description || null
                    });
                  } else {
                    // Dataset not found - show error instead of blank screen
                    throw new Error(`Dataset (ID: ${chartDetail.dataset_id}) not found or you don't have access to it.`);
                  }
                });
            } else {
              // Chart has no dataset - set error
              throw new Error("This chart has no associated dataset.");
            }
          });
      })
      .catch((e) => {
        setError(e.message || "Failed to load chart");
      })
      .finally(() => setIsLoading(false));
  }, [isAuthenticated, chartId, account?.email]);

  // Favorite toggle handler
  const handleToggleFavorite = async () => {
    if (!chartId) return;

    const newFavoriteState = !isFavorite;

    // Optimistically update UI
    setIsFavorite(newFavoriteState);

    try {
      const userEmail = account?.email || account?.username || null;
      const res = await msalFetch(
        `${API_BASE}/api/v1/charts/${chartId}/favorite?is_favorite=${newFavoriteState}`,
        {
          method: "PUT",
          headers: {
            "Content-Type": "application/json",
            ...(userEmail ? { 'x-user-email': userEmail } : {}),
          },
        }
      );

      if (!res.ok) {
        // Revert on error
        setIsFavorite(!newFavoriteState);
        throw new Error("Failed to update favorite status");
      }
    } catch (error) {
      console.error("Error toggling favorite:", error);
      // Revert on error
      setIsFavorite(!newFavoriteState);
    }
  };

  return (
    <div className="page-shell page-shell-wide">
      {!isAuthenticated && (
        <p className="muted">Sign in to view chart details.</p>
      )}

      {isAuthenticated && error && (
        <div className="card page-empty-card" style={{ marginTop: 12 }}>
          <p className="page-empty-title">Problem loading chart</p>
          <p className="page-empty-body">{error}</p>
        </div>
      )}

      {isAuthenticated && !error && isLoading && (
        <LoadingOverlay />
      )}

      {isAuthenticated && !error && !isLoading && chart && dataset && (
        <ChartBuilderProvider>
          <ChartDetailBuilderView
            chart={chart}
            isFavorite={isFavorite}
            onToggleFavorite={handleToggleFavorite}
            dataset={dataset}
          />
        </ChartBuilderProvider>
      )}
    </div>
  );
};

export default ChartDetailPage;
