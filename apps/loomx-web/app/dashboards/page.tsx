"use client";

import React, { useEffect, useState } from "react";
import { loginRequest, msalInstance } from "../../auth/msalConfig";

import { API_BASE } from "../../config";
import { LoadingOverlay } from "../../components/LoadingOverlay";
import { msalFetch } from "../../utils/msalFetch";
import { useAuth } from "../../auth/useAuth";
import { Button } from "../../components/Button";

interface DashboardSummary {
  id: string;
  name: string;
  description?: string | null;
  owner?: string | null;
  created_by?: string;
  modified_by?: string;
  created_at?: string | null;
  updated_at?: string | null;
  favorite?: boolean;
  is_published?: boolean;
  is_archived?: boolean;
}

interface UserInfo {
  id: string;
  email: string;
}

const DashboardsPage: React.FC = () => {
  const { isAuthenticated } = useAuth();

  const [dashboards, setDashboards] = useState<DashboardSummary[]>([]);
  // No longer needed: const [userMap, setUserMap] = useState<Record<string, string>>({});
  const [error, setError] = useState<string | null>(null);
  const [isLoading, setIsLoading] = useState(false);
  const [deletingId, setDeletingId] = useState<string | null>(null);

  // On first load, migrate localStorage favorites to backend and clear localStorage
  // Helper to load dashboards from backend
  const loadDashboards = async () => {
    setIsLoading(true);
    setError(null);
    try {
      const res = await msalFetch(`${API_BASE}/api/v1/dashboards/summary`);
      if (!res.ok) {
        throw new Error("Failed to load dashboards");
      }
      const data = await res.json();
      console.log("[loadDashboards] Received data:", data);
      const dashboardsData = Array.isArray(data.recent) ? data.recent : [];
      // Normalize favorite field (backend might return is_favorite or favorite)
      const normalizedData = dashboardsData.map((d: any) => {
        const favValue = d.favorite ?? d.is_favorite ?? false;
        console.log(`[loadDashboards] Dashboard ${d.id} (${d.name}): favorite=${d.favorite}, is_favorite=${d.is_favorite}, normalized=${favValue}`);
        return {
          ...d,
          favorite: favValue,
        };
      });
      console.log("[loadDashboards] Setting dashboards with favorites:", normalizedData.map((d: any) => ({ id: d.id, name: d.name, favorite: d.favorite })));
      setDashboards(normalizedData);
    } catch (err) {
      console.error("Error loading dashboards:", err);
      const message = err instanceof Error ? err.message : "Failed to load dashboards";
      setError(message);
    } finally {
      setIsLoading(false);
    }
  };

  // Migrate localStorage favorites to backend on first load
  useEffect(() => {
    if (!isAuthenticated) return;
    const migrateFavorites = async () => {
      const raw = window.localStorage.getItem("fabric_dashboard_favorites");
      if (raw) {
        try {
          const parsed = JSON.parse(raw) as Record<string, boolean>;
          await Promise.all(
            Object.entries(parsed).map(async ([id, value]) => {
              if (value) {
                await msalFetch(`${API_BASE}/api/v1/dashboards/${id}/favorite?is_favorite=true`, { method: "PUT" });
              }
            })
          );
          window.localStorage.removeItem("fabric_dashboard_favorites");
        } catch {}
      }
    };
    migrateFavorites().finally(() => {
      loadDashboards();
    });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [isAuthenticated]);

  // Refresh dashboards when page regains focus
  useEffect(() => {
    if (!isAuthenticated) return;
    const handleFocus = () => {
      loadDashboards();
    };
    window.addEventListener("focus", handleFocus);
    return () => {
      window.removeEventListener("focus", handleFocus);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [isAuthenticated]);

  const formatDateTime = (value?: string | null) => {
    if (!value) return "—";
    // Return raw SQL date without formatting
    return value;
  };


  const isFavorite = (id: number | string) => {
    const dash = dashboards.find((d) => d.id === id);
    return dash ? !!dash.favorite : false;
  };

  const toggleFavorite = async (id: number | string) => {
    const dash = dashboards.find((d) => d.id === id);
    if (!dash) return;
    const newFav = !dash.favorite;
    console.log(`[toggleFavorite] Dashboard ${id} (${dash.name}): current=${dash.favorite}, new=${newFav}`);
    // Optimistically update UI
    setDashboards((prev) => prev.map((d) =>
      d.id === id ? { ...d, favorite: newFav } : d
    ));
    // Update backend
    try {
      const response = await msalFetch(`${API_BASE}/api/v1/dashboards/${id}/favorite`, {
        method: "PUT",
        headers: {
          'Content-Type': 'application/json',
        },
        body: JSON.stringify({ is_favorite: newFav }),
      });
      if (!response.ok) {
        throw new Error(`Failed to update favorite status: ${response.status}`);
      }
      const result = await response.json();
      console.log(`[toggleFavorite] Backend response:`, result);
    } catch (err) {
      console.error('Error toggling favorite:', err);
      // On error, revert UI
      setDashboards((prev) => prev.map((d) =>
        d.id === id ? { ...d, favorite: dash.favorite } : d
      ));
    }
  };

  const handleDeleteDashboard = async (dashboard: DashboardSummary) => {
    const confirmed = window.confirm(`Delete dashboard "${dashboard.name}"? This cannot be undone.`);
    if (!confirmed) return;
    setDeletingId(dashboard.id);
    try {
      // Use msalFetch (includes Bearer token) to prevent CSRF on destructive operations
      const res = await msalFetch(`${API_BASE}/api/v1/dashboards/${dashboard.id}`, {
        method: "DELETE",
      });
      if (!res.ok && res.status !== 204) {
        throw new Error(`Failed to delete dashboard (${res.status})`);
      }
      setDashboards((prev) => prev.filter((d) => d.id !== dashboard.id));
    } catch (err) {
      console.error("Failed to delete dashboard", err);
    } finally {
      setDeletingId(null);
    }
  };

  const dashboardCount = dashboards.length;

  return (
    <div className="page-shell">
      <header className="page-header-with-actions">
        <div className="page-header-main">
          <h1 className="page-header-title">Dashboards</h1>
        </div>
        <Button
          onClick={() => {
            window.location.href = "/dashboards/new";
          }}
          style={{ flexShrink: 0 }}
        >
          <i className="fas fa-plus" /> New dashboard
        </Button>
      </header>

      {!isAuthenticated && <p className="muted">Sign in to explore your dashboards.</p>}

      {isAuthenticated && isLoading && <LoadingOverlay />}

      {isAuthenticated && !isLoading && error && (
        <div className="card page-empty-card">
          <p className="page-empty-title">Unable to load dashboards</p>
          <p className="page-empty-body">{error}</p>
        </div>
      )}


      {isAuthenticated && !isLoading && !error && dashboardCount === 0 && (
        <div className="card page-empty-card" style={{ marginTop: 12 }}>
          <p className="page-empty-title">No dashboards yet</p>
        </div>
      )}

      {/* Button is now in the header */}

      {isAuthenticated && !isLoading && !error && dashboardCount > 0 && (
        <div className="page-header-actions-row" style={{ display: 'none' }}>
          {/* Button moved to header */}
        </div>
      )}

      {isAuthenticated && !isLoading && !error && dashboardCount > 0 && (
        <div className="card" style={{ marginTop: 12 }}>
          <div className="results-table-container">
            <table className="results-table">
              <thead>
                <tr>
                  <th>
                    <span className="column-header-label">Name</span>
                  </th>
                  <th>
                    <span className="column-header-label">Owner</span>
                  </th>
                  <th>
                    <span className="column-header-label">Modified by</span>
                  </th>
                  <th>
                    <span className="column-header-label">Last modified</span>
                  </th>
                  <th>
                    <span className="column-header-label">Status</span>
                  </th>
                  <th>
                    <span className="column-header-label">Actions</span>
                  </th>
                </tr>
              </thead>
              <tbody>
                {dashboards.map((d) => (
                  <tr
                    key={d.id}
                    onClick={() => {
                      window.location.href = `/dashboards/${d.id}/view`;
                    }}
                    style={{ cursor: "pointer" }}
                  >
                    <td>
                      <strong>{d.name}</strong>
                      {d.description && (
                        <div className="muted" style={{ marginTop: 4, fontSize: 13 }}>
                          {d.description}
                        </div>
                      )}
                    </td>
                    <td className="muted" style={{ fontSize: 13 }}>
                      {d.owner || d.created_by || "—"}
                    </td>
                    <td className="muted" style={{ fontSize: 13 }}>
                      {d.modified_by ? d.modified_by : (d.owner || d.created_by || "—")}
                    </td>
                    <td className="muted" style={{ fontSize: 13 }}>
                      {formatDateTime(d.updated_at || d.created_at || null)}
                    </td>
                    <td className="muted" style={{ fontSize: 13 }}>
                      {d.is_archived ? "Archived" : d.is_published ? "Published" : "Draft"}
                    </td>
                    <td className="actions-cell">
                      <div className="row-actions">
                        <button
                          type="button"
                          className="action-icon-btn"
                          title={isFavorite(d.id) ? "Unfavorite" : "Mark as favorite"}
                          aria-label={isFavorite(d.id) ? "Unfavorite dashboard" : "Mark dashboard as favorite"}
                          onClick={(e) => {
                            e.stopPropagation();
                            toggleFavorite(d.id);
                          }}
                        >
                          <i
                            className={isFavorite(d.id) ? "fas fa-star" : "far fa-star"}
                            aria-hidden="true"
                            style={isFavorite(d.id) ? { color: "#f5c518" } : {}}
                          />
                        </button>
                        <button
                          type="button"
                          className="action-icon-btn"
                          title="Edit dashboard"
                          aria-label="Edit dashboard"
                          onClick={(e) => {
                            e.stopPropagation();
                            window.location.href = `/dashboards/${d.id}/edit`;
                          }}
                        >
                          <i className="fas fa-edit" aria-hidden="true" />
                        </button>
                        <button
                          type="button"
                          className="action-icon-btn"
                          title="Delete dashboard"
                          aria-label="Delete dashboard"
                          onClick={(e) => {
                            e.stopPropagation();
                            void handleDeleteDashboard(d);
                          }}
                          disabled={deletingId === d.id}
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
};

export default DashboardsPage;

