"use client";

import React, { useEffect, useState } from "react";

import { API_BASE } from "../../config";
import { LoadingOverlay } from "../../components/LoadingOverlay";
import { msalFetch } from "../../utils/msalFetch";
import { useAuth } from "../../auth/useAuth";
import { useTheme } from "../../contexts/ThemeContext";

interface WorkspaceActivityItem {
  id: string | number;
  name: string;
  kind: "dataset" | "chart" | "dashboard" | "saved_query";
  owner?: string | null;
  created_at?: string | null;
  updated_at?: string | null;
}

const WorkspaceActivityPage: React.FC = () => {
  const { isAuthenticated } = useAuth();
  const { primaryColor, gradientColors } = useTheme();
  const [activity, setActivity] = useState<WorkspaceActivityItem[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!isAuthenticated) return;
    setLoading(true);
    setError(null);
    Promise.all([
      msalFetch(`${API_BASE}/api/v1/datasets/summary`).then(r => r.ok ? r.json() : { recent: [] }),
      msalFetch(`${API_BASE}/api/v1/charts/summary`).then(r => r.ok ? r.json() : { recent: [] }),
      msalFetch(`${API_BASE}/api/v1/dashboards/summary`).then(r => r.ok ? r.json() : { recent: [] }),
      msalFetch(`${API_BASE}/api/v1/lab/saved-queries`).then(r => r.ok ? r.json() : []),
    ]).then(([datasetsRes, chartsRes, dashboardsRes, savedQueries]) => {
      const items: WorkspaceActivityItem[] = [];
      (datasetsRes.recent || []).forEach((d: any) => {
        items.push({
          id: d.id,
          name: d.name,
          kind: "dataset",
          owner: d.created_by || d.owner || d.user_email || d.owner_email || null,
          created_at: d.created_at,
          updated_at: d.updated_at,
        });
      });
      (chartsRes.recent || []).forEach((c: any) => {
        items.push({
          id: c.id,
          name: c.name,
          kind: "chart",
          owner: c.owner || c.created_by || c.user_email || c.owner_email || null,
          created_at: c.created_at,
          updated_at: c.updated_at,
        });
      });
      (dashboardsRes.recent || []).forEach((d: any) => {
        items.push({
          id: d.id,
          name: d.name,
          kind: "dashboard",
          owner: d.owner || d.created_by || d.user_email || d.owner_email || null,
          created_at: d.created_at,
          updated_at: d.updated_at,
        });
      });
      (savedQueries || []).forEach((q: any) => {
        items.push({
          id: q.id,
          name: q.tab_name || q.name || q.query_name || q.title || `Query ${q.id}`,
          kind: "saved_query",
          owner: q.created_by || q.owner || q.user_email || q.owner_email || null,
          created_at: q.created_at || q.saved_at || q.inserted_at || q.created_on,
          updated_at: q.updated_at || q.updated_on,
        });
      });
      // Sort by updated_at desc
      items.sort((a, b) => (b.updated_at || "").localeCompare(a.updated_at || ""));
      setActivity(items);
    }).catch(e => {
      setError("Failed to load workspace activity");
    }).finally(() => setLoading(false));
  }, [isAuthenticated]);

  return (
    <div className="page-shell animate-fade-in">
      <div style={{
        display: "flex", alignItems: "center", justifyContent: "space-between",
        flexWrap: "wrap", gap: "1rem",
        marginBottom: "1.25rem",
        padding: "1.25rem 1.5rem",
        background: "white",
        borderRadius: 12,
        border: "1px solid #e5e7eb",
        boxShadow: "0 1px 4px rgba(15,23,42,0.06)",
      }}>
        <div style={{ display: "flex", alignItems: "center", gap: "1.25rem", flexWrap: "wrap", flex: 1, minWidth: 0 }}>
          <div style={{
            width: 48, height: 48, borderRadius: 12, flexShrink: 0,
            background: `linear-gradient(135deg, ${gradientColors.light}, ${gradientColors.dark})`,
            display: "flex", alignItems: "center", justifyContent: "center",
            boxShadow: `0 4px 14px ${primaryColor}35`,
          }}>
            <i className="fas fa-history" style={{ color: "white", fontSize: 20 }} />
          </div>
          <div style={{ minWidth: 0 }}>
            <h1 style={{ margin: 0, fontSize: "1.15rem", fontWeight: 700, color: "#0f172a" }}>Workspace Activity</h1>
            <p style={{ margin: "3px 0 0", fontSize: "0.85rem", color: "#64748b", lineHeight: 1.4 }}>
              Recent changes across dashboards, charts, datasets and queries.
            </p>
          </div>
        </div>
      </div>

      {loading && <LoadingOverlay />}
      {error && (
        <div className="card page-empty-card">
          <p className="page-empty-title">Problem loading workspace activity</p>
          <p className="page-empty-body">{error}</p>
        </div>
      )}

      {!loading && activity.length === 0 && (
        <div className="card page-empty-card">
          <p className="page-empty-title">No recent changes in this workspace.</p>
        </div>
      )}

      {!loading && activity.length > 0 && (
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
                {activity.map((item) => {
                  let href = "#";
                  let typeLabel = "";
                  let lastModified = "—";
                  let created = "—";
                  if (item.kind === "dataset") {
                    href = `/datasets/${item.id}`;
                    typeLabel = "Dataset";
                    lastModified = item.updated_at || "—";
                    created = item.created_at || "—";
                  } else if (item.kind === "chart") {
                    href = `/charts/${item.id}`;
                    typeLabel = "Chart";
                    lastModified = item.updated_at || "—";
                    created = item.created_at || "—";
                  } else if (item.kind === "dashboard") {
                    href = `/dashboards/${item.id}/view`;
                    typeLabel = "Dashboard";
                    lastModified = item.updated_at || "—";
                    created = item.created_at || "—";
                  } else if (item.kind === "saved_query") {
                    href = `/lab?savedQueryId=${item.id}`;
                    typeLabel = "Saved query";
                    lastModified = item.updated_at || "—";
                    created = item.created_at || "—";
                  }
                  return (
                    <tr
                      key={`${item.kind}-${item.id}`}
                      onClick={() => { window.location.href = href; }}
                      style={{ cursor: "pointer" }}
                    >
                      <td>
                        <strong>{item.name}</strong>
                      </td>
                      <td className="muted" style={{ fontSize: 13 }}>{typeLabel}</td>
                      <td className="muted" style={{ fontSize: 13 }}>{item.owner || "—"}</td>
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

export default WorkspaceActivityPage;
