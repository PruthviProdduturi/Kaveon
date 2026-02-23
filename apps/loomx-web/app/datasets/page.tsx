"use client";

import React, { useEffect, useState } from "react";
import { loginRequest, msalInstance } from "../../auth/msalConfig";

import { API_BASE } from "../../config";
import { msalFetch } from "../../utils/msalFetch";
import { useAuth } from "../../auth/useAuth";
import { useRouter } from "next/navigation";
import { Button } from "../../components/Button";

// using same-origin relative API calls

// const API_BASE = process.env.NEXT_PUBLIC_API_BASE_URL || "http://localhost:8103";

interface DatasetSummary {
  id: number;
  dataset_name: string;
  description?: string | null;
  database_name?: string | null;
  schema_name?: string | null;
  created_by?: string | null;
  modified_by?: string | null;
  modified_at?: string | null;
  favorite?: boolean;
}

export default function DatasetsPage() {
  const { isAuthenticated, account } = useAuth();
  const router = useRouter();
  const [datasets, setDatasets] = useState<DatasetSummary[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [deletingId, setDeletingId] = useState<number | null>(null);
  // Remove accessToken state, use msalFetch for all API calls

  // No need to cache access token, msalFetch handles it

  useEffect(() => {
    if (!isAuthenticated) return;
    const load = async () => {
      try {
        const userEmail = account?.email || account?.username || null;
        const res = await msalFetch(`${API_BASE}/api/v1/datasets/summary`, {
          headers: {
            ...(userEmail ? { 'x-user-email': userEmail } : {}),
            'Cache-Control': 'no-cache, no-store, must-revalidate',
            'Pragma': 'no-cache',
            'Expires': '0'
          },
        });
        if (!res.ok) {
          throw new Error(`Failed to load datasets: ${res.status}`);
        }
        const data = await res.json();
        // The summary endpoint returns { count, recent: [...] }
        setDatasets(Array.isArray(data.recent) ? data.recent : []);
      } catch (e: unknown) {
        const message = e instanceof Error ? e.message : "Unknown error";
        setError(message);
      }
    };
    load();
  }, [isAuthenticated]);

  useEffect(() => {
    if (!isAuthenticated) return;
  }, [isAuthenticated]);


  // Favorite logic
  const handleToggleFavorite = async (dataset: DatasetSummary, e: React.MouseEvent) => {
    e.stopPropagation();
    if (!isAuthenticated) return;
    try {
      const userEmail = account?.email || account?.username || null;
      const res = await msalFetch(`${API_BASE}/api/v1/datasets/${dataset.id}/favorite?is_favorite=${!dataset.favorite}`, {
        method: "PUT",
        headers: userEmail ? { 'x-user-email': userEmail } : undefined,
      });
      if (!res.ok) throw new Error("Failed to update favorite");
      setDatasets(prev => prev.map(d => d.id === dataset.id ? { ...d, favorite: !d.favorite } : d));
    } catch (err) {
      // Optionally show error
    }
  };

  const handleDeleteDataset = async (dataset: DatasetSummary) => {
    if (!isAuthenticated) return;
    const confirmed = window.confirm(`Delete dataset "${dataset.dataset_name}"? This cannot be undone.`);
    if (!confirmed) return;

    setDeletingId(dataset.id);
    try {
      const userEmail = account?.email || account?.username || null;
      const res = await msalFetch(`${API_BASE}/api/v1/datasets/${dataset.id}`, {
        method: "DELETE",
        headers: userEmail ? { 'x-user-email': userEmail } : undefined,
      });
      if (!res.ok && res.status !== 204) {
        throw new Error(`Failed to delete dataset (${res.status})`);
      }
      setDatasets((prev) => prev.filter((d) => d.id !== dataset.id));
    } catch (err) {
      console.error("Failed to delete dataset", err);
    } finally {
      setDeletingId(null);
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

      {error && (
        <div className="card page-empty-card">
          <p className="page-empty-title">Problem loading datasets</p>
          <p className="page-empty-body">{error}</p>
        </div>
      )}

      {isAuthenticated && !error && datasets.length === 0 && (
        <div className="card page-empty-card" style={{ marginTop: 12 }}>
          <p className="page-empty-title">No datasets yet</p>
          <p className="page-empty-body">
            As you explore Fabric, you&apos;ll be able to register curated datasets here.
          </p>
          <Button
            style={{ marginTop: 12 }}
            onClick={() => {
              void router.push("/datasets/new");
            }}
          >
            <i className="fas fa-plus" /> Create your first dataset
          </Button>
        </div>
      )}

      {/* Button is now in the header */}

      {isAuthenticated && !error && datasets.length > 0 && (
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
                    <span className="column-header-label">Modified By</span>
                  </th> 
                  <th>
                    <span className="column-header-label">Modified At</span>
                  </th>
                  <th>
                    <span className="column-header-label">Actions</span>
                  </th>
                </tr>
              </thead>
              <tbody>
                {datasets.map((d, index) => (
                  <tr
                    key={`dataset-${d.id}-${index}`}
                    onClick={() => {
                      void router.push(`/datasets/${d.id}`);
                    }}
                    style={{ cursor: "pointer" }}
                  >
                    <td>
                      <strong>{ d.dataset_name}</strong>
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
                      {d.modified_at || "—"}
                    </td>
                    <td className="actions-cell">
                      <div className="row-actions">
                        <button
                          type="button"
                          className="action-icon-btn"
                          title={d.favorite ? "Unfavorite dataset" : "Favorite dataset"}
                          aria-label={d.favorite ? "Unfavorite dataset" : "Favorite dataset"}
                          onClick={e => handleToggleFavorite(d, e)}
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
                            handleDeleteDataset(d);
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
