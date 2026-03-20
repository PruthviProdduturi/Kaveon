"use client";

import { loginRequest, msalInstance } from "../../auth/msalConfig";
import { useEffect, useMemo, useState } from "react";

import { API_BASE } from "../../config";
import { TEMPLATES } from "../../components/charts/ChartBuilderContext";
import { LoomXLoading } from "../../components/LoomXLoading";
import { msalFetch } from "../../utils/msalFetch";
import { useAuth } from "../../auth/useAuth";
import { useRouter } from "next/navigation";
import { Button } from "../../components/Button";

// using same-origin relative API calls

interface ChartSummary {
  id: number;
  name: string;
  chart_type: string;
  description?: string | null;
  owner: string | null;
  created_by?: string | null;
  modified_by?: string | null;
  created_at?: string | null;
  updated_at?: string | null;
  favorite?: boolean;
}

export default function ChartsPage() {
  const { isAuthenticated, account } = useAuth();
  const router = useRouter();
  const [charts, setCharts] = useState<ChartSummary[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [deletingId, setDeletingId] = useState<number | null>(null);

  // Favorite logic
  const handleToggleFavorite = async (chart: ChartSummary, e: React.MouseEvent) => {
    e.stopPropagation();
    if (!isAuthenticated) return;
    try {
      const userEmail = account?.email || account?.username || null;
      const res = await msalFetch(`${API_BASE}/api/v1/charts/${chart.id}/favorite?is_favorite=${!chart.favorite}`, {
        method: "PUT",
        headers: userEmail ? { 'x-user-email': userEmail } : undefined,
      });
      if (!res.ok) throw new Error("Failed to update favorite");
      setCharts(prev => prev.map(c => c.id === chart.id ? { ...c, favorite: !c.favorite } : c));
    } catch (err) {
      // Optionally show error
    }
  };

  useEffect(() => {
    if (!isAuthenticated) return;
    const load = async () => {
      setLoading(true);
      try {
        const userEmail = account?.email || account?.username || null;
        const res = await msalFetch(`${API_BASE}/api/v1/charts/summary`, {
          headers: userEmail ? { 'x-user-email': userEmail } : undefined,
        });
        if (!res.ok) {
          throw new Error(`Failed to load charts: ${res.status}`);
        }
        const data = await res.json();
        const chartsWithFavorite = (data.recent || []).map((c: ChartSummary) => ({
          ...c,
          favorite: c.favorite ?? false,
          owner: c.owner || "—",
          modified_by: c.modified_by || "—",
        }));
        setCharts(chartsWithFavorite);
      } catch (e: unknown) {
        const message = e instanceof Error ? e.message : "Unknown error";
        setError(message);
      } finally {
        setLoading(false);
      }
    };
    load();
  }, [isAuthenticated]);



  const chartsWithMeta = useMemo(
    () =>
      charts.map((c) => {
        const tpl = TEMPLATES.find((t) => t.id === c.chart_type);
        return {
          ...c,
          friendlyType: tpl && tpl.name ? tpl.name : (c.chart_type ? c.chart_type.replace(/_/g, ' ') : 'Unknown'),
          category: tpl && tpl.category ? tpl.category : "",
        };
      }),
    [charts],
  );

  // Show all charts, no owner filtering
  const allCharts = chartsWithMeta;
  const formatDateTime = (value?: string | null) => {
    if (!value) return "—";
    try {
      return new Date(value).toLocaleDateString(undefined, { year: "numeric", month: "short", day: "numeric" });
    } catch {
      return value;
    }
  };



  const handleDeleteChart = async (chart: ChartSummary) => {
    const confirmed = window.confirm(`Delete chart "${chart.name}"? This cannot be undone.`);
    if (!confirmed) return;

    setDeletingId(chart.id);
    try {
      const userEmail = account?.email || account?.username || null;
      const res = await msalFetch(`${API_BASE}/api/v1/charts/${chart.id}`, {
        method: "DELETE",
        headers: userEmail ? { 'x-user-email': userEmail } : undefined,
      });
      if (!res.ok && res.status !== 204) {
        throw new Error(`Failed to delete chart (${res.status})`);
      }
      setCharts((prev) => prev.filter((c) => c.id !== chart.id));
    } catch (err) {
      console.error("Failed to delete chart", err);
    } finally {
      setDeletingId(null);
    }
  };

  return (
    <div className="page-shell">
      <header className="page-header-with-actions">
        <div className="page-header-main">
          <h1 className="page-header-title">Charts</h1>
          <p className="page-header-subtitle">
            Browse and manage saved charts built on top of your datasets and Lab queries.
          </p>
        </div>
        <Button
          onClick={() => {
            void router.push("/charts/new");
          }}
          style={{ flexShrink: 0 }}
        >
          <i className="fas fa-plus" /> New chart
        </Button>
      </header>

      {!isAuthenticated && <p className="muted">Sign in to see your charts.</p>}

      {isAuthenticated && loading && <LoomXLoading message="Loading charts" />}

      {!loading && error && (
        <div className="card page-empty-card">
          <p className="page-empty-title">Problem loading charts</p>
          <p className="page-empty-body">{error}</p>
        </div>
      )}

      {isAuthenticated && !loading && !error && chartsWithMeta.length === 0 && (
        <div className="card page-empty-card" style={{ marginTop: 12 }}>
          <p className="page-empty-title">No charts yet</p>
          <p className="page-empty-body">
            Build a chart from any dataset or Lab query using the <strong>New chart</strong> button above.
          </p>
        </div>
      )}

      {/* Button is now in the header */}

      {isAuthenticated && !loading && !error && chartsWithMeta.length > 0 && (
        <div className="card" style={{ marginTop: 12 }}>
          <div className="results-table-container">
            <table className="results-table">
              <thead>
                <tr>
                  <th>
                    <span className="column-header-label">Name</span>
                  </th>
                  <th>
                    <span className="column-header-label">Type</span>
                  </th>
                  <th>
                    <span className="column-header-label">Owner</span>
                  </th>
                    <th>
                      <span className="column-header-label">Modified By</span>
                    </th>
                  <th>
                    <span className="column-header-label">Last modified</span>
                  </th>
                  <th>
                    <span className="column-header-label">Actions</span>
                  </th>
                </tr>
              </thead>
              <tbody>
                {/* Show all charts by default */}
                {allCharts.map((c) => (
                  <tr
                    key={c.id}
                    onClick={() => {
                      void router.push(`/charts/${c.id}`);
                    }}
                    style={{ cursor: "pointer" }}
                  >
                    <td>
                      <strong>{c.name}</strong>
                      {c.description && (
                        <div className="muted" style={{ marginTop: 4, fontSize: 13 }}>
                          {c.description}
                        </div>
                      )}
                    </td>
                    <td className="muted" style={{ fontSize: 13 }}>
                      {c.friendlyType}
                    </td>
                    <td className="muted" style={{ fontSize: 13 }}>
                      {c.owner || '—'}
                    </td>
                    <td className="muted" style={{ fontSize: 13 }}>
                      {c.modified_by || '—'}
                    </td>
                    <td className="muted" style={{ fontSize: 13 }}>
                      {formatDateTime(c.updated_at || c.created_at || null)}
                    </td>
                    <td className="actions-cell">
                      <div className="row-actions">
                        <button
                          type="button"
                          className="action-icon-btn"
                          title={c.favorite ? "Unfavorite chart" : "Favorite chart"}
                          aria-label={c.favorite ? "Unfavorite chart" : "Favorite chart"}
                          onClick={e => handleToggleFavorite(c, e)}
                          style={{ color: c.favorite ? "#f5c518" : undefined }}
                        >
                          <i className={c.favorite ? "fas fa-star" : "far fa-star"} aria-hidden="true" />
                        </button>
                        <button
                          type="button"
                          className="action-icon-btn"
                          title="Edit chart"
                          aria-label="Edit chart"
                          onClick={(e) => {
                            e.stopPropagation();
                            void router.push(`/charts/${c.id}`);
                          }}
                        >
                          <i className="fas fa-edit" aria-hidden="true" />
                        </button>
                        <button
                          type="button"
                          className="action-icon-btn"
                          title="Delete chart"
                          aria-label="Delete chart"
                          onClick={(e) => {
                            e.stopPropagation();
                            void handleDeleteChart(c);
                          }}
                          disabled={deletingId === c.id}
                        >
                          <i className="fas fa-trash" aria-hidden="true" />
                        </button>
                      </div>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </div>
      )}
    </div>
  );
}