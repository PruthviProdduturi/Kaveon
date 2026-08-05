"use client";

import React, { useEffect, useState } from "react";
import { API_BASE } from "../../config";
import { msalFetch } from "../../utils/msalFetch";
import { useAuth } from "../../auth/useAuth";
import { Button } from "../../components/Button";
import { ConfirmModal } from "../../components/ConfirmModal";
import { ListPageShell } from "../../components/ListPageShell";
import { Pagination } from "../../components/Pagination";
import { relativeTime } from "../../utils/relativeTime";
import DashboardThumbnail from "../../components/dashboards/DashboardThumbnail";

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
  charts?: string;
  thumbnail?: string | null;
}

const PAGE_SIZE = 20;

const DashboardsPage: React.FC = () => {
  const { isAuthenticated, account } = useAuth();

  const [dashboards, setDashboards] = useState<DashboardSummary[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [isLoading, setIsLoading] = useState(false);
  const [deletingId, setDeletingId] = useState<string | null>(null);
  const [confirmDelete, setConfirmDelete] = useState<DashboardSummary | null>(null);
  const [search, setSearch] = useState("");
  const [page, setPage] = useState(1);
  const [viewMode, setViewMode] = useState<"grid" | "list">("list");
  const [hoveredId, setHoveredId] = useState<string | null>(null);

  useEffect(() => {
    const saved = typeof window !== "undefined" ? window.localStorage.getItem("lens_dash_view") : null;
    if (saved === "grid" || saved === "list") setViewMode(saved);
  }, []);
  const changeView = (m: "grid" | "list") => { setViewMode(m); try { window.localStorage.setItem("lens_dash_view", m); } catch {} };

  const loadDashboards = async () => {
    setIsLoading(true);
    setError(null);
    try {
      const res = await msalFetch(`${API_BASE}/api/v1/dashboards/summary`);
      if (!res.ok) throw new Error("Failed to load dashboards");
      const data = await res.json();
      const dashboardsData = Array.isArray(data.recent) ? data.recent : [];
      setDashboards(dashboardsData.map((d: any) => ({ ...d, favorite: d.favorite ?? d.is_favorite ?? false })));
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to load dashboards");
    } finally {
      setIsLoading(false);
    }
  };

  useEffect(() => {
    if (!isAuthenticated) return;
    const migrateFavorites = async () => {
      const raw = window.localStorage.getItem("fabric_dashboard_favorites");
      if (raw) {
        try {
          const parsed = JSON.parse(raw) as Record<string, boolean>;
          await Promise.all(
            Object.entries(parsed).map(async ([id, value]) => {
              if (value) await msalFetch(`${API_BASE}/api/v1/dashboards/${id}/favorite?is_favorite=true`, { method: "PUT" });
            })
          );
          window.localStorage.removeItem("fabric_dashboard_favorites");
        } catch {}
      }
    };
    migrateFavorites().finally(() => loadDashboards());
  }, [isAuthenticated]);

  useEffect(() => {
    if (!isAuthenticated) return;
    const handleFocus = () => loadDashboards();
    window.addEventListener("focus", handleFocus);
    return () => window.removeEventListener("focus", handleFocus);
  }, [isAuthenticated]);

  const formatDateTime = (value?: string | null) => relativeTime(value);

  const toggleFavorite = async (id: string) => {
    const dash = dashboards.find(d => d.id === id);
    if (!dash) return;
    const newFav = !dash.favorite;
    setDashboards(prev => prev.map(d => d.id === id ? { ...d, favorite: newFav } : d));
    try {
      const res = await msalFetch(`${API_BASE}/api/v1/dashboards/${id}/favorite`, {
        method: "PUT",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ is_favorite: newFav }),
      });
      if (!res.ok) throw new Error();
    } catch {
      setDashboards(prev => prev.map(d => d.id === id ? { ...d, favorite: dash.favorite } : d));
    }
  };

  const handleDeleteDashboard = async (dashboard: DashboardSummary) => {
    setConfirmDelete(null);
    setDeletingId(dashboard.id);
    try {
      const res = await msalFetch(`${API_BASE}/api/v1/dashboards/${dashboard.id}`, { method: "DELETE" });
      if (!res.ok && res.status !== 204) throw new Error(`Failed to delete dashboard (${res.status})`);
      setDashboards(prev => prev.filter(d => d.id !== dashboard.id));
    } catch (err) {
      console.error("Failed to delete dashboard", err);
    } finally {
      setDeletingId(null);
    }
  };

  const filtered = dashboards.filter(d =>
    !search || d.name.toLowerCase().includes(search.toLowerCase()) || (d.description ?? "").toLowerCase().includes(search.toLowerCase())
  );

  // Reset to page 1 when search changes
  const handleSearch = (q: string) => { setSearch(q); setPage(1); };

  const paged = filtered.slice((page - 1) * PAGE_SIZE, page * PAGE_SIZE);
  const publishedCount = dashboards.filter(d => d.is_published).length;
  const userEmail = account?.email || account?.username || null;
  const myCount = userEmail ? dashboards.filter(d => d.created_by === userEmail).length : 0;
  const othersCount = dashboards.length - myCount;

  return (
    <>
      <ListPageShell
        icon="fa-tachometer-alt"
        title="Dashboards"
        subtitle="Browse and manage your saved dashboards."
        pills={!isLoading && !error ? [
          { label: `${dashboards.length} Total`, icon: "fa-tachometer-alt" },
          ...(myCount > 0 ? [{ label: `${myCount} Mine`, icon: "fa-user" }] : []),
          ...(othersCount > 0 ? [{ label: `${othersCount} Others`, icon: "fa-users", bg: "#f1f5f9", border: "#e2e8f0", color: "#64748b" }] : []),
          ...(publishedCount > 0 ? [{ label: `${publishedCount} Published`, icon: "fa-globe", bg: "#d1fae5", border: "#6ee7b7", color: "#065f46" }] : []),
        ] : []}
        action={
          <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
            <div style={{ display: "flex", border: "1px solid #e2e8f0", borderRadius: 8, overflow: "hidden" }}>
              {(["grid", "list"] as const).map(m => (
                <button key={m} type="button" onClick={() => changeView(m)}
                  title={m === "grid" ? "Thumbnail view" : "List view"}
                  style={{
                    width: 34, height: 34, border: "none", cursor: "pointer",
                    background: viewMode === m ? "#eff6ff" : "#fff",
                    color: viewMode === m ? "#2563eb" : "#94a3b8",
                  }}>
                  <i className={m === "grid" ? "fas fa-table-cells-large" : "fas fa-list"} />
                </button>
              ))}
            </div>
            <Button onClick={() => { window.location.href = "/dashboards/new"; }}>
              <i className="fas fa-plus" /> New dashboard
            </Button>
          </div>
        }
        loading={isLoading}
        loadingMessage="Loading dashboards"
        error={error}
        empty={!isLoading && !error && dashboards.length === 0}
        emptyTitle="Build your first dashboard"
        emptyBody="Dashboards bring your charts together into a single, interactive view — with filters, cross-filtering and live data. Here's how to get there."
        emptySteps={[
          { icon: "fa-database", title: "Connect a dataset", desc: "Point Lens at a table or query — Postgres, MySQL, Fabric and more." },
          { icon: "fa-chart-column", title: "Create charts", desc: "Explore your data and save KPIs, time-series, maps and tables." },
          { icon: "fa-tachometer-alt", title: "Assemble the dashboard", desc: "Drag charts onto a grid, add filters, and publish to share." },
        ]}
        emptyAction={<Button onClick={() => { window.location.href = "/dashboards/new"; }}><i className="fas fa-plus" /> New dashboard</Button>}
        emptySecondaryAction={<a href="/charts" style={{ fontSize: 13, color: "#64748b", textDecoration: "none" }}>or browse your charts first →</a>}
        search={search}
        onSearch={handleSearch}
        resultCount={search ? filtered.length : undefined}
      >
        {viewMode === "grid" ? (
        <div style={{ display: "grid", gridTemplateColumns: "repeat(auto-fill, minmax(300px, 1fr))", gap: 16 }}>
          {paged.map(d => {
            const hero = (() => { try { return JSON.parse(d.charts || "[]")[0] ?? null; } catch { return null; } })();
            return (
              <div
                key={d.id}
                className="card"
                onClick={() => { window.location.href = `/dashboards/${d.id}/view`; }}
                style={{ padding: 0, overflow: "hidden", cursor: "pointer", display: "flex", flexDirection: "column", transition: "box-shadow 0.15s, transform 0.15s" }}
                onMouseEnter={e => { e.currentTarget.style.boxShadow = "0 8px 24px rgba(15,23,42,0.12)"; e.currentTarget.style.transform = "translateY(-2px)"; setHoveredId(d.id); }}
                onMouseLeave={e => { e.currentTarget.style.boxShadow = ""; e.currentTarget.style.transform = ""; setHoveredId(null); }}
              >
                {/* Full-dashboard snapshot if captured, else a live hero-chart preview */}
                <div style={{ height: 156, borderBottom: "1px solid #eef2f7", position: "relative", background: "#f8fafc" }}>
                  {d.thumbnail ? (
                    <img src={d.thumbnail} alt={d.name} style={{ width: "100%", height: "100%", objectFit: "cover", objectPosition: "top" }} />
                  ) : (
                    <DashboardThumbnail chartId={hero} />
                  )}
                  {/* Regenerate thumbnail (opens the dashboard, which re-snapshots it) */}
                  {hoveredId === d.id && (
                    <button
                      type="button"
                      title="Regenerate thumbnail (opens & re-captures the dashboard)"
                      onClick={e => { e.stopPropagation(); window.location.href = `/dashboards/${d.id}/view?recapture=1`; }}
                      style={{
                        position: "absolute", top: 8, left: 8, zIndex: 2,
                        display: "flex", alignItems: "center", gap: 5,
                        padding: "4px 9px", fontSize: 11, fontWeight: 600,
                        background: "rgba(255,255,255,0.94)", color: "#334155",
                        border: "1px solid #e2e8f0", borderRadius: 6, cursor: "pointer",
                        boxShadow: "0 2px 8px rgba(0,0,0,0.1)",
                      }}
                    >
                      <i className="fas fa-rotate" style={{ fontSize: 10 }} /> Regenerate
                    </button>
                  )}
                  <span style={{
                    position: "absolute", top: 8, right: 8,
                    padding: "0.15rem 0.5rem", borderRadius: 6, fontSize: "0.7rem", fontWeight: 600,
                    background: d.is_archived ? "#f3f4f6" : d.is_published ? "#d1fae5" : "#eff6ff",
                    color: d.is_archived ? "#6b7280" : d.is_published ? "#065f46" : "#1d4ed8",
                  }}>
                    {d.is_archived ? "Archived" : d.is_published ? "Published" : "Draft"}
                  </span>
                </div>
                <div style={{ padding: "12px 14px", flex: 1, display: "flex", flexDirection: "column", gap: 5, minWidth: 0 }}>
                  <strong style={{ fontSize: 14, color: "#0f172a", whiteSpace: "nowrap", overflow: "hidden", textOverflow: "ellipsis" }}>{d.name}</strong>
                  {d.description && (
                    <div className="muted" style={{ fontSize: 12.5, lineHeight: 1.4, display: "-webkit-box", WebkitLineClamp: 2, WebkitBoxOrient: "vertical", overflow: "hidden" }}>{d.description}</div>
                  )}
                  <div style={{ marginTop: "auto", display: "flex", alignItems: "center", justifyContent: "space-between", paddingTop: 6 }}>
                    <span className="muted" style={{ fontSize: 12 }}>{formatDateTime(d.updated_at || d.created_at)}</span>
                    <div className="row-actions" style={{ display: "flex", gap: 2 }}>
                      <button type="button" className="action-icon-btn" title={d.favorite ? "Unfavorite" : "Favorite"}
                        onClick={e => { e.stopPropagation(); toggleFavorite(d.id); }}>
                        <i className={d.favorite ? "fas fa-star" : "far fa-star"} style={d.favorite ? { color: "#f5c518" } : {}} />
                      </button>
                      <button type="button" className="action-icon-btn" title="Edit dashboard"
                        onClick={e => { e.stopPropagation(); window.location.href = `/dashboards/${d.id}/edit`; }}>
                        <i className="fas fa-edit" />
                      </button>
                      <button type="button" className="action-icon-btn" title="Delete dashboard"
                        onClick={e => { e.stopPropagation(); setConfirmDelete(d); }} disabled={deletingId === d.id}>
                        <i className={deletingId === d.id ? "fas fa-spinner fa-spin" : "fas fa-trash"} />
                      </button>
                    </div>
                  </div>
                </div>
              </div>
            );
          })}
        </div>
        ) : (
          <div className="card">
            <div className="results-table-container">
              <table className="results-table">
                <thead>
                  <tr>
                    <th><span className="column-header-label">Name</span></th>
                    <th><span className="column-header-label">Owner</span></th>
                    <th><span className="column-header-label">Last modified</span></th>
                    <th><span className="column-header-label">Status</span></th>
                    <th><span className="column-header-label">Actions</span></th>
                  </tr>
                </thead>
                <tbody>
                  {paged.map(d => (
                    <tr key={d.id} onClick={() => { window.location.href = `/dashboards/${d.id}/view`; }} style={{ cursor: "pointer" }}>
                      <td>
                        <strong>{d.name}</strong>
                        {d.description && <div className="muted" style={{ marginTop: 4, fontSize: 13 }}>{d.description}</div>}
                      </td>
                      <td className="muted" style={{ fontSize: 13 }}>{d.owner || d.created_by || "—"}</td>
                      <td className="muted" style={{ fontSize: 13 }}>{formatDateTime(d.updated_at || d.created_at)}</td>
                      <td>
                        <span style={{
                          padding: "0.2rem 0.6rem", borderRadius: 6, fontSize: "0.75rem", fontWeight: 600,
                          background: d.is_archived ? "#f3f4f6" : d.is_published ? "#d1fae5" : "#eff6ff",
                          color: d.is_archived ? "#6b7280" : d.is_published ? "#065f46" : "#1d4ed8",
                        }}>
                          {d.is_archived ? "Archived" : d.is_published ? "Published" : "Draft"}
                        </span>
                      </td>
                      <td className="actions-cell">
                        <div className="row-actions">
                          <button type="button" className="action-icon-btn" title={d.favorite ? "Unfavorite" : "Favorite"}
                            onClick={e => { e.stopPropagation(); toggleFavorite(d.id); }}>
                            <i className={d.favorite ? "fas fa-star" : "far fa-star"} style={d.favorite ? { color: "#f5c518" } : {}} />
                          </button>
                          <button type="button" className="action-icon-btn" title="Edit dashboard"
                            onClick={e => { e.stopPropagation(); window.location.href = `/dashboards/${d.id}/edit`; }}>
                            <i className="fas fa-edit" />
                          </button>
                          <button type="button" className="action-icon-btn" title="Delete dashboard"
                            onClick={e => { e.stopPropagation(); setConfirmDelete(d); }} disabled={deletingId === d.id}>
                            <i className={deletingId === d.id ? "fas fa-spinner fa-spin" : "fas fa-trash"} />
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
        <Pagination total={filtered.length} page={page} pageSize={PAGE_SIZE} onChange={setPage} />
      </ListPageShell>

      <ConfirmModal
        isOpen={!!confirmDelete}
        title="Delete dashboard"
        message={confirmDelete ? `"${confirmDelete.name}" will be permanently deleted. This cannot be undone.` : ""}
        confirmLabel="Delete"
        onConfirm={() => confirmDelete && void handleDeleteDashboard(confirmDelete)}
        onCancel={() => setConfirmDelete(null)}
      />
    </>
  );
};

export default DashboardsPage;
