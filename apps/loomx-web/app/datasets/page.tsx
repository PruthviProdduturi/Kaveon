"use client";

import React, { useEffect, useState } from "react";
import { API_BASE } from "../../config";
import { LoomXLoading } from "../../components/LoomXLoading";
import { msalFetch } from "../../utils/msalFetch";
import { useAuth } from "../../auth/useAuth";
import { useRouter } from "next/navigation";
import { Button } from "../../components/Button";

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

export default function DatasetsPage() {
  const { isAuthenticated, account } = useAuth();
  const router = useRouter();
  const [datasets, setDatasets] = useState<DatasetSummary[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [deletingId, setDeletingId] = useState<number | null>(null);

  useEffect(() => {
    if (!isAuthenticated) return;
    const load = async () => {
      setLoading(true);
      try {
        const userEmail = account?.email || account?.username || null;
        const res = await msalFetch(`${API_BASE}/api/v1/datasets/summary`, {
          headers: userEmail ? { "x-user-email": userEmail } : undefined,
        });
        if (!res.ok) {
          throw new Error(`Failed to load datasets: ${res.status}`);
        }
        const data = await res.json();
        setDatasets(Array.isArray(data.recent) ? data.recent : []);
      } catch (e: unknown) {
        const message = e instanceof Error ? e.message : "Unknown error";
        setError(message);
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
        {
          method: "PUT",
          headers: userEmail ? { "x-user-email": userEmail } : undefined,
        },
      );
      if (!res.ok) throw new Error("Failed to update favorite");
      setDatasets((prev) =>
        prev.map((d) => (d.id === dataset.id ? { ...d, favorite: !d.favorite } : d)),
      );
    } catch {
      // silently ignore — star will revert on next page load
    }
  };

  const handleDeleteDataset = async (dataset: DatasetSummary) => {
    if (!isAuthenticated) return;
    const confirmed = window.confirm(
      `Delete dataset "${dsName(dataset)}"? This cannot be undone.`,
    );
    if (!confirmed) return;

    setDeletingId(dataset.id);
    try {
      const userEmail = account?.email || account?.username || null;
      const res = await msalFetch(`${API_BASE}/api/v1/datasets/${dataset.id}`, {
        method: "DELETE",
        headers: userEmail ? { "x-user-email": userEmail } : undefined,
      });
      if (!res.ok && res.status !== 204) {
        throw new Error(`Failed to delete dataset (${res.status})`);
      }
      setDatasets((prev) => prev.filter((d) => d.id !== dataset.id));
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to delete dataset");
    } finally {
      setDeletingId(null);
    }
  };

  const formatDate = (value?: string | null) => {
    if (!value) return "—";
    try {
      return new Date(value).toLocaleDateString(undefined, { year: "numeric", month: "short", day: "numeric" });
    } catch {
      return value;
    }
  };

  return (
    <div className="page-shell">
      <header className="page-header-with-actions">
        <div className="page-header-main">
          <h1 className="page-header-title">Datasets</h1>
          <p className="page-header-subtitle">
            Define reusable tables and views that power charts and Lab queries.
          </p>
        </div>
        <Button
          onClick={() => {
            void router.push("/datasets/new");
          }}
          style={{ flexShrink: 0 }}
        >
          <i className="fas fa-plus" /> New dataset
        </Button>
      </header>

      {!isAuthenticated && <p className="muted">Sign in to see your datasets.</p>}

      {isAuthenticated && loading && <LoomXLoading message="Loading datasets" />}

      {!loading && error && (
        <div className="card page-empty-card">
          <p className="page-empty-title">Problem loading datasets</p>
          <p className="page-empty-body">{error}</p>
        </div>
      )}

      {isAuthenticated && !loading && !error && datasets.length === 0 && (
        <div className="card page-empty-card" style={{ marginTop: 12 }}>
          <p className="page-empty-title">No datasets yet</p>
          <p className="page-empty-body">
            Register a table or view to start building charts and running queries against it.
          </p>
        </div>
      )}

      {isAuthenticated && !loading && !error && datasets.length > 0 && (
        <div className="card" style={{ marginTop: 12 }}>
          <div className="results-table-container">
            <table className="results-table">
              <thead>
                <tr>
                  <th><span className="column-header-label">Name</span></th>
                  <th><span className="column-header-label">Owner</span></th>
                  <th><span className="column-header-label">Modified By</span></th>
                  <th><span className="column-header-label">Modified At</span></th>
                  <th><span className="column-header-label">Actions</span></th>
                </tr>
              </thead>
              <tbody>
                {datasets.map((d) => (
                  <tr
                    key={d.id}
                    onClick={() => {
                      void router.push(`/datasets/${d.id}`);
                    }}
                    style={{ cursor: "pointer" }}
                  >
                    <td>
                      <strong>{dsName(d)}</strong>
                      {d.description && (
                        <div className="muted" style={{ marginTop: 4, fontSize: 13 }}>
                          {d.description}
                        </div>
                      )}
                    </td>
                    <td className="muted" style={{ fontSize: 13 }}>
                      {d.created_by || "—"}
                    </td>
                    <td className="muted" style={{ fontSize: 13 }}>
                      {d.modified_by || "—"}
                    </td>
                    <td className="muted" style={{ fontSize: 13 }}>
                      {formatDate(d.modified_at)}
                    </td>
                    <td className="actions-cell">
                      <div className="row-actions">
                        <button
                          type="button"
                          className="action-icon-btn"
                          title={d.favorite ? "Unfavorite dataset" : "Favorite dataset"}
                          aria-label={d.favorite ? "Unfavorite dataset" : "Favorite dataset"}
                          onClick={(e) => handleToggleFavorite(d, e)}
                          style={{ color: d.favorite ? "#f5c518" : undefined }}
                        >
                          <i className={d.favorite ? "fas fa-star" : "far fa-star"} aria-hidden="true" />
                        </button>
                        <button
                          type="button"
                          className="action-icon-btn"
                          title="Edit dataset"
                          aria-label="Edit dataset"
                          onClick={(e) => {
                            e.stopPropagation();
                            void router.push(`/datasets/new?datasetId=${d.id}`);
                          }}
                        >
                          <i className="fas fa-edit" aria-hidden="true" />
                        </button>
                        <button
                          type="button"
                          className="action-icon-btn"
                          title="Delete dataset"
                          aria-label="Delete dataset"
                          onClick={(e) => {
                            e.stopPropagation();
                            void handleDeleteDataset(d);
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
}
