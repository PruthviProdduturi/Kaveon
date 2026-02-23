"use client";

import React, { useEffect, useState } from "react";

import { API_BASE } from "../../config";
import { LoadingOverlay } from "../../components/LoadingOverlay";
import { msalFetch } from "../../utils/msalFetch";
import { useAuth } from "../../auth/useAuth";

// API_BASE not required; using same-origin relative API calls

interface FavoriteItem {
  id: string | number;
  name: string;
  kind: "dataset" | "chart" | "dashboard";
  owner?: string | null;
  created_at?: string | null;
  updated_at?: string | null;
}

const FavoritesPage: React.FC = () => {
  const { isAuthenticated } = useAuth();
  const [favorites, setFavorites] = useState<FavoriteItem[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!isAuthenticated) return;
    setLoading(true);
    setError(null);
    msalFetch(`${API_BASE}/api/v1/favorites`)
      .then(r => r.ok ? r.json() : Promise.reject("Failed to load favorites"))
      .then((favs: FavoriteItem[]) => {
        setFavorites(favs || []);
      })
      .catch(e => {
        setError("Failed to load favorites");
      })
      .finally(() => setLoading(false));
  }, [isAuthenticated]);

  return (
    <div className="page-shell">
      <header className="page-header">
        <h1 className="page-header-title">Your Favorites</h1>
        <p className="page-header-subtitle">
          All dashboards, charts, and datasets you have marked as favorites.<br />
          <span className="muted" style={{fontSize:13}}>This table shows only <b>your</b> favorites.</span>
        </p>
      </header>

      {loading && <LoadingOverlay />}
      {error && (
        <div className="card page-empty-card">
          <p className="page-empty-title">Problem loading favorites</p>
          <p className="page-empty-body">{error}</p>
        </div>
      )}

      {!loading && favorites.length === 0 && (
        <div className="card page-empty-card">
          <p className="page-empty-title">No favorites yet.</p>
          <p className="page-empty-body">Mark dashboards, charts, or datasets as favorites to see them here.</p>
        </div>
      )}

      {!loading && favorites.length > 0 && (
        <div className="card" style={{ marginTop: 12 }}>
          <div className="results-table-container">
            <table className="results-table">
              <thead>
                <tr>
                  <th><span className="column-header-label">Name</span></th>
                  <th><span className="column-header-label">Type</span></th>
                  <th><span className="column-header-label">Owner</span></th>
                  <th><span className="column-header-label">Last Modified</span></th>
                  <th><span className="column-header-label">Created</span></th>
                </tr>
              </thead>
              <tbody>
                {favorites.map((fav) => {
                  let href = "#";
                  let typeLabel = "";
                  let lastModified = "—";
                  let created = "—";
                  if (fav.kind === "dataset") {
                    href = `/datasets/${fav.id}`;
                    typeLabel = "Dataset";
                    lastModified = fav.updated_at || "—";
                    created = fav.created_at || "—";
                  } else if (fav.kind === "chart") {
                    href = `/charts`;
                    typeLabel = "Chart";
                    lastModified = fav.updated_at || "—";
                    created = fav.created_at || "—";
                  } else if (fav.kind === "dashboard") {
                    href = `/dashboards/${fav.id}/view`;
                    typeLabel = "Dashboard";
                    lastModified = fav.updated_at || "—";
                    created = fav.created_at || "—";
                  }
                  return (
                    <tr
                      key={`${fav.kind}-${fav.id}`}
                      onClick={() => { window.location.href = href; }}
                      style={{ cursor: "pointer" }}
                    >
                      <td>
                        <strong>{fav.name || fav.name}</strong>
                      </td>
                      <td className="muted" style={{ fontSize: 13 }}>{typeLabel}</td>
                      <td className="muted" style={{ fontSize: 13 }}>{fav.owner || "—"}</td>
                      <td className="muted" style={{ fontSize: 13 }}>{lastModified}</td>
                      <td className="muted" style={{ fontSize: 13 }}>{created}</td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
        </div>
      )}
    </div>
  );
};

export default FavoritesPage;
