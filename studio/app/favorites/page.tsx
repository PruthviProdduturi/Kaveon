"use client";

import React, { useEffect, useState } from "react";

import { API_BASE } from "../../config";
import { LoadingOverlay } from "../../components/LoadingOverlay";
import { msalFetch } from "../../utils/msalFetch";
import { useAuth } from "../../auth/useAuth";
import { useSetup } from "../../components/ClientLayout";
import { useTheme } from "../../contexts/ThemeContext";
import { UnknownPanel, reasonFromStatus, type ReadFailure } from "../../components/Reading";

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
  const { isSetupOk } = useSetup();
  const { primaryColor, gradientColors } = useTheme();
  const [favorites, setFavorites] = useState<FavoriteItem[]>([]);
  const [loading, setLoading] = useState(false);
  // Three separable states, which this page used to collapse into "No
  // favorites yet.": a list that answered and is empty, a list that could not
  // be read, and a list nobody has asked for yet because the setup check has
  // not come back. Only the first of those is an absence, and only the first
  // is now written as one.
  const [answered, setAnswered] = useState(false);
  const [failure, setFailure] = useState<ReadFailure | null>(null);

  useEffect(() => {
    if (!isAuthenticated) return;
    // `false` is an answer — this installation reports itself as not set up —
    // and `null` only means the check has not come back. Neither is an empty
    // list, and neither may leave the page waiting for ever.
    if (isSetupOk === false) {
      setFailure({ what: "The system of record", says: "which items you have marked", reason: "not_configured" });
      return;
    }
    if (isSetupOk !== true) return;
    setLoading(true);
    setFailure(null);
    msalFetch(`${API_BASE}/api/v1/favorites`)
      .then(r => r.ok ? r.json() : Promise.reject(r.status))
      .then((favs: FavoriteItem[]) => {
        // Deduplicate by kind+id — the backend can return the same item via
        // multiple favorite rows (e.g. row per dataset join per chart join)
        const seen = new Set<string>();
        const unique = (favs || []).filter(f => {
          const k = `${f.kind}-${f.id}`;
          if (seen.has(k)) return false;
          seen.add(k);
          return true;
        });
        setFavorites(unique);
        setAnswered(true);
      })
      .catch((status: unknown) => {
        setFailure({
          what: "Your favorites",
          says: "what you have marked",
          reason: typeof status === "number" ? reasonFromStatus(status) : null,
        });
      })
      .finally(() => setLoading(false));
  }, [isAuthenticated, isSetupOk]);

  return (
    <div className="page-shell animate-fade-in">
      <div style={{
        display: "flex", alignItems: "center", justifyContent: "space-between",
        flexWrap: "wrap", gap: "1rem",
        marginBottom: "1.25rem",
        padding: "1.25rem 1.5rem",
        background: "var(--bg-surface)",
        borderRadius: 12,
        border: "1px solid var(--border)",
        boxShadow: "var(--shadow-sm)",
      }}>
        <div style={{ display: "flex", alignItems: "center", gap: "1.25rem", flexWrap: "wrap", flex: 1, minWidth: 0 }}>
          <div style={{
            width: 48, height: 48, borderRadius: 12, flexShrink: 0,
            background: `linear-gradient(135deg, ${gradientColors.light}, ${gradientColors.dark})`,
            display: "flex", alignItems: "center", justifyContent: "center",
            boxShadow: `0 4px 14px ${primaryColor}35`,
          }}>
            <i className="fas fa-star" style={{ color: "white", fontSize: 20 }} />
          </div>
          <div style={{ minWidth: 0 }}>
            <h1 style={{ margin: 0, fontSize: "1.15rem", fontWeight: 700, color: "var(--text-primary)" }}>Favorites</h1>
            <p style={{ margin: "3px 0 0", fontSize: "0.85rem", color: "var(--text-muted)", lineHeight: 1.4 }}>
              Your bookmarked dashboards, charts, and datasets.
            </p>
          </div>
          {!loading && favorites.length > 0 && (
            <div style={{ display: "flex", gap: "0.5rem", flexWrap: "wrap" }}>
              <span style={{
                display: "inline-flex", alignItems: "center", gap: "0.35rem",
                padding: "0.3rem 0.75rem", borderRadius: 20,
                background: `${primaryColor}12`, border: `1px solid ${primaryColor}30`,
                fontSize: 12, fontWeight: 600, color: primaryColor, whiteSpace: "nowrap",
              }}>
                <i className="fas fa-star" style={{ fontSize: 10 }} />
                {favorites.length} Favorite{favorites.length !== 1 ? "s" : ""}
              </span>
            </div>
          )}
        </div>
      </div>

      {loading && <LoadingOverlay />}
      {!loading && failure && <UnknownPanel {...failure} />}

      {!loading && !failure && !answered && (
        <div className="card page-empty-card">
          <p className="page-empty-title">Checking</p>
          <p className="page-empty-body">Reading your favorites.</p>
        </div>
      )}

      {!loading && !failure && answered && favorites.length === 0 && (
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
                    href = `/charts/${fav.id}`;
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
