"use client";

import React, { useEffect, useState } from "react";
import { API_BASE } from "../../config";
import { msalFetch } from "../../utils/msalFetch";
import { useAuth } from "../../auth/useAuth";
import { Button } from "../../components/Button";
import { ConfirmModal } from "../../components/ConfirmModal";
import { ListPageShell } from "../../components/ListPageShell";
import { Pagination } from "../../components/Pagination";

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

const PAGE_SIZE = 20;

const DashboardsPage: React.FC = () => {
  const { isAuthenticated } = useAuth();

  const [dashboards, setDashboards] = useState<DashboardSummary[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [isLoading, setIsLoading] = useState(false);
  const [deletingId, setDeletingId] = useState<string | null>(null);
  const [confirmDelete, setConfirmDelete] = useState<DashboardSummary | null>(null);
  const [search, setSearch] = useState("");
  const [page, setPage] = useState(1);

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

  const formatDateTime = (value?: string | null) => value ?? "—";

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

  return (
    <>
      <ListPageShell
        icon="fa-tachometer-alt"
        title="Dashboards"
        subtitle="Browse and manage your saved dashboards."
        pills={!isLoading && !error ? [
          { label: `${dashboards.length} Dashboard${dashboards.length !== 1 ? "s" : ""}`, icon: "fa-tachometer-alt" },
          ...(publishedCount > 0 ? [{ label: `${publishedCount} Published`, icon: "fa-globe", bg: "#d1fae5", border: "#6ee7b7", color: "#065f46" }] : []),
        ] : []}
        action={
          <Button onClick={() => { window.location.href = "/dashboards/new"; }}>
            <i className="fas fa-plus" /> New dashboard
          </Button>
        }
        loading={isLoading}
        loadingMessage="Loading dashboards"
        error={error}
        empty={!isLoading && !error && dashboards.length === 0}
        emptyTitle="No dashboards yet"
        emptyBody="Create your first dashboard to start visualising your data."
        emptyAction={<Button onClick={() => { window.location.href = "/dashboards/new"; }}><i className="fas fa-plus" /> New dashboard</Button>}
        search={search}
        onSearch={handleSearch}
        resultCount={search ? filtered.length : undefined}
      >
        <div className="card">
          <div className="results-table-container">
            <table className="results-table">
              <thead>
                <tr>
                  <th><span className="column-header-label">Name</span></th>
                  <th><span className="column-header-label">Owner</span></th>
                  <th><span className="column-header-label">Modified by</span></th>
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
                    <td className="muted" style={{ fontSize: 13 }}>{d.modified_by || d.owner || d.created_by || "—"}</td>
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
          <Pagination total={filtered.length} page={page} pageSize={PAGE_SIZE} onChange={setPage} />
        </div>
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
