"use client";

import React, { useEffect, useState } from "react";
import { API_BASE } from "../../config";
import { msalFetch } from "../../utils/msalFetch";
import { useAuth } from "../../auth/useAuth";
import { useRouter } from "next/navigation";
import { Button } from "../../components/Button";
import { ListPageShell } from "../../components/ListPageShell";
import { Pagination } from "../../components/Pagination";

interface DatasetSummary {
  id: number;
  name?: string | null;
  dataset_name?: string | null;
  description?: string | null;
  database_name?: string | null;
  schema_name?: string | null;
  created_by?: string | null;
  modified_by?: string | null;
  modified_at?: string | null;
  favorite?: boolean;
}

function dsName(d: DatasetSummary): string {
  return d.dataset_name || d.name || "—";
}

const PAGE_SIZE = 20;

export default function DatasetsPage() {
  const { isAuthenticated, account } = useAuth();
  const router = useRouter();
  const [datasets, setDatasets] = useState<DatasetSummary[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [deletingId, setDeletingId] = useState<number | null>(null);
  const [search, setSearch] = useState("");
  const [page, setPage] = useState(1);

  useEffect(() => {
    if (!isAuthenticated) return;
    const load = async () => {
      setLoading(true);
      try {
        const userEmail = account?.email || account?.username || null;
        const res = await msalFetch(`${API_BASE}/api/v1/datasets/summary`, {
          headers: userEmail ? { "x-user-email": userEmail } : undefined,
        });
        if (!res.ok) throw new Error(`Failed to load datasets: ${res.status}`);
        const data = await res.json();
        setDatasets(Array.isArray(data.recent) ? data.recent : []);
      } catch (e) {
        setError(e instanceof Error ? e.message : "Unknown error");
      } finally {
        setLoading(false);
      }
    };
    load();
  }, [isAuthenticated]);

  const handleToggleFavorite = async (dataset: DatasetSummary, e: React.MouseEvent) => {
    e.stopPropagation();
    if (!isAuthenticated) return;
    try {
      const userEmail = account?.email || account?.username || null;
      const res = await msalFetch(
        `${API_BASE}/api/v1/datasets/${dataset.id}/favorite?is_favorite=${!dataset.favorite}`,
        { method: "PUT", headers: userEmail ? { "x-user-email": userEmail } : undefined }
      );
      if (!res.ok) throw new Error();
      setDatasets(prev => prev.map(d => d.id === dataset.id ? { ...d, favorite: !d.favorite } : d));
    } catch {}
  };

  const handleDeleteDataset = async (dataset: DatasetSummary) => {
    if (!isAuthenticated) return;
    if (!window.confirm(`Delete dataset "${dsName(dataset)}"? This cannot be undone.`)) return;
    setDeletingId(dataset.id);
    try {
      const userEmail = account?.email || account?.username || null;
      const res = await msalFetch(`${API_BASE}/api/v1/datasets/${dataset.id}`, {
        method: "DELETE",
        headers: userEmail ? { "x-user-email": userEmail } : undefined,
      });
      if (!res.ok && res.status !== 204) throw new Error(`Failed (${res.status})`);
      setDatasets(prev => prev.filter(d => d.id !== dataset.id));
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to delete dataset");
    } finally {
      setDeletingId(null);
    }
  };

  const formatDate = (value?: string | null) => {
    if (!value) return "—";
    try { return new Date(value).toLocaleDateString(undefined, { year: "numeric", month: "short", day: "numeric" }); }
    catch { return value; }
  };

  const filtered = datasets.filter(d =>
    !search || dsName(d).toLowerCase().includes(search.toLowerCase()) ||
    (d.description ?? "").toLowerCase().includes(search.toLowerCase()) ||
    (d.database_name ?? "").toLowerCase().includes(search.toLowerCase())
  );

  const handleSearch = (q: string) => { setSearch(q); setPage(1); };
  const paged = filtered.slice((page - 1) * PAGE_SIZE, page * PAGE_SIZE);
  const userEmail = account?.email || account?.username || null;
  const myCount = userEmail ? datasets.filter(d => d.created_by === userEmail).length : 0;
  const othersCount = datasets.length - myCount;

  return (
    <ListPageShell
      icon="fa-database"
      title="Datasets"
      subtitle="Define reusable tables and views that power charts and Lab queries."
      pills={!loading && !error ? [
        { label: `${datasets.length} Total`, icon: "fa-database" },
        ...(myCount > 0 ? [{ label: `${myCount} Mine`, icon: "fa-user" }] : []),
        ...(othersCount > 0 ? [{ label: `${othersCount} Others`, icon: "fa-users", bg: "#f1f5f9", border: "#e2e8f0", color: "#64748b" }] : []),
      ] : []}
      action={
        <Button onClick={() => void router.push("/datasets/new")}>
          <i className="fas fa-plus" /> New dataset
        </Button>
      }
      loading={loading}
      loadingMessage="Loading datasets"
      error={error}
      empty={!loading && !error && datasets.length === 0}
      emptyTitle="Register your first dataset"
      emptyBody="A dataset is a table, view or query that Lens can chart and explore. Connect one to unlock charts and dashboards."
      emptySteps={[
        { icon: "fa-plug", title: "Connect a source", desc: "Postgres, MySQL, Fabric or Azure SQL — add a data source." },
        { icon: "fa-table", title: "Choose a table", desc: "Pick a table or view, or paste a SQL query as the source." },
        { icon: "fa-wand-magic-sparkles", title: "Define semantics", desc: "Mark dimensions and metrics so charts just work." },
      ]}
      emptyAction={<Button onClick={() => void router.push("/datasets/new")}><i className="fas fa-plus" /> New dataset</Button>}
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
                <th><span className="column-header-label">Modified at</span></th>
                <th><span className="column-header-label">Actions</span></th>
              </tr>
            </thead>
            <tbody>
              {paged.map(d => (
                <tr key={d.id} onClick={() => void router.push(`/datasets/${d.id}`)} style={{ cursor: "pointer" }}>
                  <td>
                    <strong>{dsName(d)}</strong>
                    {d.description && <div className="muted" style={{ marginTop: 4, fontSize: 13 }}>{d.description}</div>}
                  </td>
                  <td className="muted" style={{ fontSize: 13 }}>{d.created_by || "—"}</td>
                  <td className="muted" style={{ fontSize: 13 }}>{d.modified_by || "—"}</td>
                  <td className="muted" style={{ fontSize: 13 }}>{formatDate(d.modified_at)}</td>
                  <td className="actions-cell">
                    <div className="row-actions">
                      <button type="button" className="action-icon-btn" title={d.favorite ? "Unfavorite" : "Favorite"}
                        onClick={e => handleToggleFavorite(d, e)} style={{ color: d.favorite ? "#f5c518" : undefined }}>
                        <i className={d.favorite ? "fas fa-star" : "far fa-star"} />
                      </button>
                      <button type="button" className="action-icon-btn" title="Edit dataset"
                        onClick={e => { e.stopPropagation(); void router.push(`/datasets/new?datasetId=${d.id}`); }}>
                        <i className="fas fa-edit" />
                      </button>
                      <button type="button" className="action-icon-btn" title="Delete dataset"
                        onClick={e => { e.stopPropagation(); void handleDeleteDataset(d); }} disabled={deletingId === d.id}>
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
  );
}
