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
  id: string | number;
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
  const { name, setName, canSave, isSaving, saveError, saveChartAs, chartId } = useChartBuilder();
  const headerRouter = useRouter();
  const [isSaveModalOpen, setIsSaveModalOpen] = useState(false);
  const [isAnimating, setIsAnimating] = useState(false);
  const [isEditingName, setIsEditingName] = useState(false);
  const [showSaveAs, setShowSaveAs] = useState(false);
  const [saveAsName, setSaveAsName] = useState("");
  const [savingAs, setSavingAs] = useState(false);

  const openSaveAs = () => { setSaveAsName(name ? `${name} (Copy)` : "Untitled chart"); setShowSaveAs(true); };
  const confirmSaveAs = async () => {
    if (!saveAsName.trim()) return;
    setSavingAs(true);
    try {
      const newId = await saveChartAs(saveAsName.trim());
      setShowSaveAs(false);
      if (newId) headerRouter.push(`/charts/${newId}/edit`);
    } finally {
      setSavingAs(false);
    }
  };
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
            onClick={() => {
              // Return to wherever the user came from (their previous state);
              // fall back to the Library charts tab on a direct/first visit.
              if (typeof window !== "undefined" && window.history.length > 1) headerRouter.back();
              else headerRouter.push("/workspace?tab=charts");
            }}
            title="Back"
            aria-label="Back"
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
                // Plain text (matches the dashboard title). Click to rename; the
                // only hover affordance is a subtle underline — no boxy border/fill.
                style={{ margin: 0, cursor: "pointer", textDecoration: "none", textDecorationColor: "var(--text-muted)", textUnderlineOffset: 4, transition: "text-decoration-color 0.15s" }}
                onClick={() => setIsEditingName(true)}
                onMouseOver={e => { e.currentTarget.style.textDecoration = "underline"; e.currentTarget.style.textDecorationColor = "var(--text-muted)"; }}
                onMouseOut={e => { e.currentTarget.style.textDecoration = "none"; }}
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
          {chartId && (
            <button
              type="button"
              onClick={openSaveAs}
              disabled={isSaving}
              title="Save as a new chart (duplicate)"
              style={{
                padding: "8px 16px", fontWeight: 600, fontSize: 13,
                background: "transparent", color: "var(--text-secondary)",
                border: "1px solid var(--border)", borderRadius: 6, cursor: "pointer",
                display: "flex", alignItems: "center", gap: 6,
              }}
            >
              <i className="fas fa-copy" /> Save As
            </button>
          )}
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

      {showSaveAs && (
        <div
          style={{ position: "fixed", inset: 0, background: "rgba(15,23,42,0.5)", display: "flex", alignItems: "center", justifyContent: "center", zIndex: 1001, padding: 20 }}
          onClick={() => { if (!savingAs) setShowSaveAs(false); }}
        >
          <div
            onClick={(e) => e.stopPropagation()}
            style={{ width: "100%", maxWidth: 420, background: "var(--bg-surface)", border: "1px solid var(--border)", borderRadius: 14, boxShadow: "0 24px 48px rgba(0,0,0,0.28)", padding: 24 }}
          >
            <h3 style={{ margin: "0 0 4px", fontSize: 17, fontWeight: 700, color: "var(--text-primary)" }}>Save as new chart</h3>
            <p style={{ margin: "0 0 16px", fontSize: 13, color: "var(--text-muted)" }}>Creates an independent copy. The original stays unchanged.</p>
            <label className="chart-builder-label" htmlFor="chart-save-as-name">Name</label>
            <input
              id="chart-save-as-name"
              autoFocus
              className="chart-builder-input"
              value={saveAsName}
              onChange={(e) => setSaveAsName(e.target.value)}
              onKeyDown={(e) => { if (e.key === "Enter") confirmSaveAs(); }}
            />
            <div style={{ display: "flex", justifyContent: "flex-end", gap: 10, marginTop: 20 }}>
              <button type="button" onClick={() => setShowSaveAs(false)} disabled={savingAs}
                style={{ padding: "9px 18px", fontSize: 13, fontWeight: 600, cursor: "pointer", border: "1px solid var(--border)", borderRadius: 8, background: "var(--bg-surface)", color: "var(--text-secondary)" }}>
                Cancel
              </button>
              <button type="button" onClick={confirmSaveAs} disabled={savingAs || !saveAsName.trim()}
                style={{ padding: "9px 18px", fontSize: 13, fontWeight: 700, cursor: "pointer", border: "none", borderRadius: 8, background: "#2563eb", color: "#fff", minWidth: 96 }}>
                {savingAs ? "Saving…" : "Create copy"}
              </button>
            </div>
          </div>
        </div>
      )}
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
    if (!isAuthenticated || !chartId) return;
    // One load per chart id, held across a Strict Mode double mount.
    if (fetchedChartIdRef.current === chartId) return;
    fetchedChartIdRef.current = chartId;

    setIsLoading(true);
    setError(null);

    const userEmail = account?.email || account?.username || null;
    const identity = userEmail ? { 'x-user-email': userEmail } : undefined;

    // Three independent reads. The chart itself, the caller's favourites, and
    // the dataset list that decides whether this caller may see the chart's
    // dataset — none of them needs an answer from another, so none of them
    // waits for one. This used to be a three-deep chain whose first leg was
    // the whole chart library, thumbnails included, read only to recover one
    // boolean the favourites list answers in a fraction of the payload.
    const load = async () => {
      try {
        const [chartRes, favRes, datasetRes] = await Promise.all([
          msalFetch(`${API_BASE}/api/v1/charts/${chartId}`),
          msalFetch(`${API_BASE}/api/v1/favorites`, { headers: identity }),
          msalFetch(`${API_BASE}/api/v1/datasets/summary`),
        ]);

        // A chart this caller may not read is indistinguishable from one that
        // does not exist, and the API answers both the same way.
        if (chartRes.status === 404) throw new Error("Chart not found");
        if (!chartRes.ok) throw new Error("Failed to load chart details");
        const chartDetail = (await chartRes.json()) as ChartDetail;
        setChart(chartDetail);
        if (chartDetail?.name) {
          // Recents id is prefixed by type, matching delete cleanup.
          addRecent({ id: `chart-${chartId}`, label: chartDetail.name, href: `/charts/${chartId}`, type: "chart" });
        }

        if (favRes.ok) {
          const favorites = (await favRes.json()) as { kind?: string; id?: string | number }[];
          setIsFavorite(
            Array.isArray(favorites)
            && favorites.some(f => f.kind === "chart" && String(f.id) === String(chartId)),
          );
        }

        if (!chartDetail.dataset_id) throw new Error("This chart has no associated dataset.");
        if (!datasetRes.ok) throw new Error("Failed to load dataset details");
        const datasetList = (await datasetRes.json()) as { recent?: any[] };
        // The dataset list is visibility-scoped, so its silence is an answer:
        // this caller may not read the dataset the chart is built on.
        const found = (datasetList.recent || []).find((d: any) => String(d.id) === String(chartDetail.dataset_id));
        if (!found) {
          throw new Error(`Dataset (ID: ${chartDetail.dataset_id}) not found or you don't have access to it.`);
        }
        setDataset({ id: found.id, name: found.dataset_name, description: found.description || null });
      } catch (e) {
        setError(e instanceof Error ? e.message : "Failed to load chart");
      } finally {
        setIsLoading(false);
      }
    };

    void load();
  }, [isAuthenticated, chartId, account?.email, account?.username, addRecent]);

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

      // The star reverts to what the server still holds if the write did not land.
      if (!res.ok) setIsFavorite(!newFavoriteState);
    } catch {
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
